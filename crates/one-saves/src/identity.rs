//! A bundle's two hashes, and the normalized form one of them is taken over.

use crate::error::{Error, ErrorKind};
use crate::hash::HashValue;
use crate::model::{Bundle, Part, Payload};

impl Bundle {
    /// The SHA-256 of this bundle's bytes on disk.
    ///
    /// It identifies this exact file, and it is what an integrity check compares. Two producers
    /// that build the same logical bundle can disagree on it, because neither compression nor an
    /// external reference is pinned by the format.
    pub fn file_hash(&self) -> Result<HashValue, Error> {
        Ok(HashValue::sha256_of(&self.to_vec()?))
    }

    /// The SHA-256 of this bundle's normalized encoding.
    ///
    /// A pure function of what the bundle says rather than of how it was written, so two
    /// producers that build the same logical bundle agree on it. This is the identifier a
    /// content-addressable store keys on, and the value a [`bundle`](crate::PartKind::Bundle)
    /// part's `sha256` holds.
    ///
    /// It equals the [file hash](Bundle::file_hash) for a bundle that is self-contained and
    /// uncompressed, which is every bundle nested inside another.
    ///
    /// # Errors
    ///
    /// A thin bundle cannot compute its own content hash: normalizing it means embedding every
    /// referenced payload, which means resolving every reference against a store first. That is
    /// [`ErrorKind::Thin`].
    pub fn content_hash(&self) -> Result<HashValue, Error> {
        Ok(HashValue::sha256_of(&self.normalized()?.to_vec()?))
    }

    /// This bundle with every part uncompressed and every payload embedded.
    ///
    /// The same logical bundle, said the one way the content hash is defined over.
    pub fn normalized(&self) -> Result<Bundle, Error> {
        let mut normalized = self.clone();
        for (index, part) in normalized.parts.iter_mut().enumerate() {
            part.normalize(&format!("parts[{index}]"))?;
        }
        Ok(normalized)
    }

    /// Whether every part carries its bytes inline.
    ///
    /// A self-contained bundle works on its own; a thin one needs an external
    /// content-addressable store to resolve the referenced digests.
    #[must_use]
    pub fn is_self_contained(&self) -> bool {
        self.parts.iter().all(|part| part.payload.is_embedded())
    }
}

impl Part {
    /// Rewrites this part's payload into the embedded uncompressed form.
    fn normalize(&mut self, path: &str) -> Result<(), Error> {
        match &self.payload {
            Payload::Embedded(_) => Ok(()),
            Payload::External { .. } => Err(Error::at(path, ErrorKind::Thin)),
            Payload::Compressed { bytes, size } => {
                let plain = inflate(bytes, *size, path)?;
                self.payload = Payload::Embedded(plain);
                Ok(())
            }
        }
    }

    /// This part's bytes, inflating them if they are compressed.
    ///
    /// Returns [`ErrorKind::Thin`] for a referenced payload, whose bytes are not here.
    pub fn bytes(&self) -> Result<std::borrow::Cow<'_, [u8]>, Error> {
        match &self.payload {
            Payload::Embedded(bytes) => Ok(std::borrow::Cow::Borrowed(bytes)),
            Payload::External { .. } => Err(Error::at("", ErrorKind::Thin)),
            Payload::Compressed { bytes, size } => Ok(std::borrow::Cow::Owned(inflate(bytes, *size, "")?)),
        }
    }

    /// Whether this part's `sha256` matches the bytes it carries.
    pub fn verify(&self) -> Result<bool, Error> {
        Ok(HashValue::sha256_of(&self.bytes()?) == self.sha256)
    }
}

/// Inflates a zstd payload, checking it against the length the part stated.
///
/// The stated size is checked rather than trusted: it is an attacker-controlled number that
/// would otherwise size an allocation.
#[cfg(feature = "zstd")]
fn inflate(bytes: &[u8], size: u64, path: &str) -> Result<Vec<u8>, Error> {
    let capacity = usize::try_from(size)
        .map_err(|_| Error::at(path, ErrorKind::ZstdInvalid("stated size does not fit".into())))?;
    let plain = zstd::stream::decode_all(bytes)
        .map_err(|e| Error::at(path, ErrorKind::ZstdInvalid(e.to_string())))?;
    if plain.len() != capacity {
        return Err(Error::at(
            path,
            ErrorKind::ZstdInvalid(format!("inflated to {} bytes, not the stated {capacity}", plain.len())),
        ));
    }
    Ok(plain)
}

#[cfg(not(feature = "zstd"))]
fn inflate(_bytes: &[u8], _size: u64, path: &str) -> Result<Vec<u8>, Error> {
    Err(Error::at(path, ErrorKind::ZstdUnavailable))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Header;

    #[test]
    fn a_self_contained_uncompressed_bundle_hashes_the_same_both_ways() {
        // The two hashes coincide exactly when there is nothing to normalize, which is every
        // bundle nested inside another.
        let bundle = Bundle { header: Header::default(), parts: vec![Part::new(0, *b"SAVE")] };
        assert!(bundle.is_self_contained());
        assert_eq!(bundle.file_hash().unwrap(), bundle.content_hash().unwrap());
    }

    #[cfg(feature = "zstd")]
    #[test]
    fn compression_changes_the_file_hash_but_not_the_content_hash() {
        let plain = Bundle { header: Header::default(), parts: vec![Part::new(0, vec![7u8; 4096])] };

        let mut compressed = plain.clone();
        let bytes = zstd::stream::encode_all(&[7u8; 4096][..], 3).unwrap();
        compressed.parts[0].payload = Payload::Compressed { bytes, size: 4096 };

        assert_ne!(plain.file_hash().unwrap(), compressed.file_hash().unwrap());
        assert_eq!(plain.content_hash().unwrap(), compressed.content_hash().unwrap());
    }

    #[test]
    fn a_thin_bundle_cannot_compute_its_own_content_hash() {
        use crate::model::ExternalRef;
        let mut bundle = Bundle { header: Header::default(), parts: vec![Part::new(0, *b"SAVE")] };
        let hash = bundle.parts[0].sha256.clone();
        bundle.parts[0].payload = Payload::External { reference: ExternalRef { hash, uri: None }, size: 4 };
        assert!(!bundle.is_self_contained());
        assert_eq!(bundle.content_hash().unwrap_err().kind(), &ErrorKind::Thin);
    }
}
