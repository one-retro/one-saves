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
    /// Decompressing a payload that does not inflate to the length and digest it claims.
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
}

impl Part {
    /// Rewrites this part's payload into the embedded uncompressed form.
    fn normalize(&mut self, path: &str) -> Result<(), Error> {
        match &self.payload {
            Payload::Embedded(_) => Ok(()),
            Payload::Compressed { bytes, size } => {
                let plain = inflate(bytes, *size, path)?;
                self.payload = Payload::Embedded(plain);
                Ok(())
            }
        }
    }

    /// This part's bytes, inflating them if they are compressed.
    ///
    pub fn bytes(&self) -> Result<std::borrow::Cow<'_, [u8]>, Error> {
        match &self.payload {
            Payload::Embedded(bytes) => Ok(std::borrow::Cow::Borrowed(bytes)),
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
/// `size` is what a producer says the payload will come to, not a promise it will, so it is
/// neither trusted nor merely checked afterwards: the decompression is **abandoned** the moment it
/// outgrows the claim. Inflating first and comparing second would let a part claiming a hundred
/// bytes expand to gigabytes before anything noticed.
///
/// That still leaves the claim itself as an attacker-controlled bound. A consumer unwilling to
/// handle a payload of a given size should decline before calling: [`Payload::len`] reports what
/// the part claims without touching the compressed bytes.
#[cfg(feature = "zstd")]
fn inflate(bytes: &[u8], size: u64, path: &str) -> Result<Vec<u8>, Error> {
    use std::io::Read as _;

    let capacity = usize::try_from(size)
        .map_err(|_| Error::at(path, ErrorKind::ZstdInvalid("stated size does not fit".into())))?;
    let invalid = |message: String| Error::at(path, ErrorKind::ZstdInvalid(message));

    let decoder = zstd::stream::read::Decoder::new(bytes).map_err(|e| invalid(e.to_string()))?;
    // One byte past the claim is all it takes to know the payload outgrew it, and it caps what a
    // bomb can make this allocate at the stated length rather than at whatever it inflates to.
    let mut plain = Vec::new();
    decoder.take(size.saturating_add(1)).read_to_end(&mut plain).map_err(|e| invalid(e.to_string()))?;

    if plain.len() != capacity {
        return Err(invalid(format!("inflated past the stated {capacity} bytes")));
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

    #[cfg(feature = "zstd")]
    #[test]
    fn a_decompression_is_abandoned_when_it_outgrows_what_the_part_claimed() {
        // `size` is a claim, not a promise. A part understating it is how a small file asks a
        // consumer to allocate a large one, so the inflate stops at the claim rather than running
        // to completion and comparing afterwards.
        let bomb = zstd::stream::encode_all(&vec![0u8; 4 << 20][..], 3).unwrap();
        assert!(bomb.len() < 4096, "the point of the case is that the compressed form is small");

        let mut bundle = Bundle { header: Header::default(), parts: vec![Part::new(0, *b"SAVE")] };
        bundle.parts[0].payload = Payload::Compressed { bytes: bomb, size: 64 };

        let error = bundle.parts[0].bytes().unwrap_err();
        let ErrorKind::ZstdInvalid(message) = error.kind() else {
            panic!("a payload that outgrows its claim is invalid, not silently large: {error}");
        };
        // The evidence that it stopped rather than finished: having abandoned the decompression,
        // it cannot say how big the payload really was. Reporting the true 4194304 would mean it
        // had inflated the whole thing first, which is the behaviour this guards against.
        assert!(message.contains("64"), "names the claim it broke: {message}");
        assert!(
            !message.contains(&(4usize << 20).to_string()),
            "must not know the true length, which would mean it inflated everything: {message}"
        );
    }
}
