//! A bundle's content hash.

use crate::error::Error;
use crate::hash::HashValue;
use crate::model::Bundle;

impl Bundle {
    /// The SHA-256 of this bundle's bytes: the bundle tag and the array, as encoded.
    ///
    /// A bundle has one encoding, so this is a pure function of what it says: two producers that
    /// build the same bundle, part for part and `id` for `id`, agree on it. It is what an integrity
    /// check compares, the identifier a content-addressable store keys on, and the value a
    /// [`bundle`](crate::PartKind::Bundle) part's `sha256` holds.
    ///
    /// # Errors
    ///
    /// Whatever [`to_vec`](Bundle::to_vec) refuses: a bundle that breaks a rule has no encoding.
    pub fn content_hash(&self) -> Result<HashValue, Error> {
        Ok(HashValue::sha256_of(&self.to_vec()?))
    }
}

#[cfg(test)]
mod tests {
    use crate::hash::HashValue;
    use crate::model::{Bundle, Header, Part};

    #[test]
    fn the_content_hash_is_the_hash_of_the_file() {
        let bundle = Bundle::new(Header::default(), vec![Part::new(0, [7u8; 4096])]);
        let bytes = bundle.to_vec().unwrap();
        assert_eq!(bundle.content_hash().unwrap(), HashValue::sha256_of(&bytes));
    }
}
