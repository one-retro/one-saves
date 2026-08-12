//! Hash values: a raw digest in a CBOR byte string, under a tag naming the algorithm.
//!
//! The value is self-describing, so nothing around it has to say which algorithm this is and no
//! hex text is involved. Two of the four tags are IANA's bare-hash registrations, where the tag
//! for COSE algorithm *N* is `18556 + N`; the other two are this specification's own, because
//! COSE registers no CRC-32 and no MD5 and never will.

use core::fmt;

/// A digest algorithm these specifications can carry.
///
/// The list is short on purpose: these four are what retro databases actually key on, and adding
/// another would cost every implementer a dependency and buy nobody a lookup they can perform
/// today.
///
/// `crc32`, `md5` and `sha1` are all broken as cryptography, and none of them is used here for
/// integrity. They identify a known file by matching it against a catalog, which is a job a weak
/// hash still does. Where a digest has to be trusted — a part's `sha256`, a bundle's content
/// hash — the format names SHA-256 directly and offers no choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HashAlgorithm {
    /// SHA-256, CBOR tag 18540. Carried by newer No-Intro sets, and the one to prefer.
    Sha256,
    /// SHA-1, CBOR tag 18542. The most common ROM identifier in public databases.
    Sha1,
    /// CRC-32 (ISO-HDLC, as zlib and every DAT file use it), CBOR tag 46010.
    Crc32,
    /// MD5, CBOR tag 46011.
    Md5,
}

impl HashAlgorithm {
    /// Every algorithm, in ascending tag order — which is the order a `rom_hashes` array holds.
    pub const ALL: [HashAlgorithm; 4] =
        [HashAlgorithm::Sha256, HashAlgorithm::Sha1, HashAlgorithm::Crc32, HashAlgorithm::Md5];

    /// The CBOR tag that names this algorithm.
    #[must_use]
    pub const fn tag(self) -> u64 {
        match self {
            HashAlgorithm::Sha256 => 18540,
            HashAlgorithm::Sha1 => 18542,
            HashAlgorithm::Crc32 => 46010,
            HashAlgorithm::Md5 => 46011,
        }
    }

    /// The algorithm a tag names, if it names one.
    #[must_use]
    pub const fn from_tag(tag: u64) -> Option<Self> {
        match tag {
            18540 => Some(HashAlgorithm::Sha256),
            18542 => Some(HashAlgorithm::Sha1),
            46010 => Some(HashAlgorithm::Crc32),
            46011 => Some(HashAlgorithm::Md5),
            _ => None,
        }
    }

    /// How many bytes this algorithm's digest runs to.
    #[must_use]
    pub const fn digest_len(self) -> usize {
        match self {
            HashAlgorithm::Sha256 => 32,
            HashAlgorithm::Sha1 => 20,
            HashAlgorithm::Crc32 => 4,
            HashAlgorithm::Md5 => 16,
        }
    }

    /// The algorithm's name, as the specifications write it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            HashAlgorithm::Sha256 => "sha256",
            HashAlgorithm::Sha1 => "sha1",
            HashAlgorithm::Crc32 => "crc32",
            HashAlgorithm::Md5 => "md5",
        }
    }
}

impl fmt::Display for HashAlgorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Why a tagged byte string is not a hash value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashError {
    /// The tag names no algorithm in the table.
    UnknownTag(u64),
    /// The digest length does not match what the algorithm implies.
    WrongLength {
        /// The algorithm the tag named.
        algorithm: HashAlgorithm,
        /// How many bytes the digest actually ran to.
        found: usize,
    },
}

impl fmt::Display for HashError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HashError::UnknownTag(tag) => write!(f, "tag {tag} names no hash algorithm"),
            HashError::WrongLength { algorithm, found } => {
                write!(f, "{algorithm} digests are {} bytes, found {found}", algorithm.digest_len())
            }
        }
    }
}

impl core::error::Error for HashError {}

/// A digest, with the algorithm that produced it.
///
/// Two hash values are equal when their tags are equal and their digest bytes are equal. Values
/// with different tags never compare, **even when both identify the same file** — which is why
/// [`PartialEq`] here is derived over both fields rather than written to be clever.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HashValue {
    algorithm: HashAlgorithm,
    digest: Vec<u8>,
}

impl HashValue {
    /// Builds a hash value, checking the digest against the length the algorithm implies.
    pub fn new(algorithm: HashAlgorithm, digest: impl Into<Vec<u8>>) -> Result<Self, HashError> {
        let digest = digest.into();
        if digest.len() != algorithm.digest_len() {
            return Err(HashError::WrongLength { algorithm, found: digest.len() });
        }
        Ok(HashValue { algorithm, digest })
    }

    /// Builds a hash value from a CBOR tag number and digest bytes.
    ///
    /// Both halves are checked. A consumer must reject a value whose tag is not in the table and
    /// one whose digest length does not match the algorithm; neither is recoverable by guessing.
    pub fn from_tagged(tag: u64, digest: impl Into<Vec<u8>>) -> Result<Self, HashError> {
        let algorithm = HashAlgorithm::from_tag(tag).ok_or(HashError::UnknownTag(tag))?;
        HashValue::new(algorithm, digest)
    }

    /// The algorithm that produced this digest.
    #[must_use]
    pub const fn algorithm(&self) -> HashAlgorithm {
        self.algorithm
    }

    /// The raw digest bytes.
    #[must_use]
    pub fn digest(&self) -> &[u8] {
        &self.digest
    }

    /// The CBOR tag naming this value's algorithm.
    #[must_use]
    pub const fn tag(&self) -> u64 {
        self.algorithm.tag()
    }

    /// Computes the SHA-256 of `bytes`.
    ///
    /// This is the one algorithm the format computes rather than merely carries: it is a part's
    /// `sha256`, a bundle's content hash and its file hash.
    #[must_use]
    pub fn sha256_of(bytes: &[u8]) -> Self {
        use sha2::{Digest, Sha256};
        HashValue { algorithm: HashAlgorithm::Sha256, digest: Sha256::digest(bytes).to_vec() }
    }
}

impl fmt::Display for HashValue {
    /// Writes `algorithm:hex`, which is how the CLI shows a digest.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:", self.algorithm)?;
        for byte in &self.digest {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// Orders hash values the way a `rom_hashes` array holds them: ascending by tag number.
///
/// Ascending tag puts the two standard tags before the two local ones, which is arbitrary but
/// stable, and a deterministic encoding needs an order more than it needs a meaningful one.
impl Ord for HashValue {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.tag().cmp(&other.tag()).then_with(|| self.digest.cmp(&other.digest))
    }
}

impl PartialOrd for HashValue {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_match_the_cose_derivation() {
        // The tag for COSE algorithm N is 18556 + N; SHA-256 is -16 and SHA-1 is -14.
        assert_eq!(HashAlgorithm::Sha256.tag(), 18556 - 16);
        assert_eq!(HashAlgorithm::Sha1.tag(), 18556 - 14);
        // The other two are this specification's own First-Come-First-Served registrations.
        assert_eq!(HashAlgorithm::Crc32.tag(), 46010);
        assert_eq!(HashAlgorithm::Md5.tag(), 46011);
    }

    #[test]
    fn rejects_an_unknown_tag_and_a_wrong_length() {
        assert_eq!(HashValue::from_tagged(18541, vec![0; 32]), Err(HashError::UnknownTag(18541)));
        assert_eq!(
            HashValue::from_tagged(18540, vec![0; 16]),
            Err(HashError::WrongLength { algorithm: HashAlgorithm::Sha256, found: 16 })
        );
    }

    #[test]
    fn all_is_in_ascending_tag_order() {
        let tags: Vec<u64> = HashAlgorithm::ALL.iter().map(|a| a.tag()).collect();
        let mut sorted = tags.clone();
        sorted.sort_unstable();
        assert_eq!(tags, sorted);
    }

    #[test]
    fn values_of_different_algorithms_never_compare_equal() {
        // Both digests are all-zero, but the algorithms differ, so the values differ.
        let a = HashValue::new(HashAlgorithm::Crc32, vec![0; 4]).unwrap();
        let b = HashValue::new(HashAlgorithm::Md5, vec![0; 16]).unwrap();
        assert_ne!(a, b);
        assert!(a < b, "crc32 (46010) sorts before md5 (46011)");
    }

    #[test]
    fn sha256_of_empty_is_the_known_digest() {
        assert_eq!(
            HashValue::sha256_of(b"").to_string(),
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
