//! The Universal Saves Format (`.1saves`, `1SAV`): a portable, self-describing CBOR container
//! for retro console save files and everything attached to them.
//!
//! A retro save is a bare binary dump with no agreed way to say what game it belongs to, what
//! wrote it, or what else belongs with it. This crate reads and writes the container that fixes
//! that: typed binary parts plus whatever identifying context a producer has, in one file.
//!
//! The specification lives at
//! <https://docs.1retro.com/specifications/universal-saves-format/>. This crate implements
//! version 0.2, which is **not yet stabilized** — until 1.0, breaking changes happen in place
//! under the same bundle tag.
//!
//! # Determinism
//!
//! A bundle's [content hash](crate::Bundle::content_hash) is the identifier a content-addressable
//! store keys on, so it has to be a function of what the bundle says and not of how a producer
//! chose to say it. Two things get it there, and this crate enforces both:
//!
//! - the encoding is deterministic, per RFC 8949 section 4.2, so a given value has one spelling;
//! - the data model offers one way to say each thing, so absence is the only empty and the only
//!   default, and every epoch is whole seconds.
//!
//! Breaking either is malformed rather than merely unusual, because a consumer that accepted
//! both spellings would compute two content hashes for one save.
//!
//! # Reading a bundle
//!
//! ```no_run
//! use one_saves::Bundle;
//!
//! let bytes = std::fs::read("card.1saves")?;
//! let bundle = Bundle::from_slice(&bytes)?;
//!
//! if let Some(system) = &bundle.header.system {
//!     println!("system: {system}");
//! }
//! for part in &bundle.parts {
//!     println!("part {} is {} bytes", part.id, part.payload.len());
//! }
//! println!("content hash: {}", bundle.content_hash()?);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # What this crate will not do for you
//!
//! It never looks inside a payload. Whether a 64 KB dump and its 32 KB half are the same save
//! takes per-system parsing that this container deliberately keeps out; see
//! [`one-saves-convert`] for that half. A content hash answers "are these the same bytes", which
//! is what a store needs to deduplicate safely.
//!
//! [`one-saves-convert`]: https://docs.rs/one-saves-convert

mod codec;
mod error;
pub mod hash;
mod identity;
mod model;
pub mod name;
mod validate;

pub use codec::Strictness;
pub use error::{Error, ErrorKind};
pub use hash::{HashAlgorithm, HashError, HashValue};
pub use model::{
    Bundle, Card, Extensions, ExternalRef, Game, GameId, Header, Part, PartKind, Payload, Source, UnknownKeys,
};
pub use name::{Name, NameError, ReverseDnsName, Slug};

/// Re-exported so callers can read and build the values under extension keys.
pub use dcbor;

/// The CBOR tag wrapping every bundle, whose 5-byte encoding is the file magic.
///
/// `0xDA 0x31 0x53 0x41 0x56` reads `1SAV` from byte 1 of the file. Byte 0 is the tag's head
/// byte. The major version lives in the tag number, so a future breaking format takes a
/// different tag and an old decoder rejects a new-major file on its own, with no separate
/// version field to check.
pub const BUNDLE_TAG: u64 = 827_539_798;

/// The file extension a bundle takes.
pub const EXTENSION: &str = "1saves";

/// The media type a bundle is served as.
pub const MEDIA_TYPE: &str = "application/vnd.1saves+cbor";

/// The version of the specification this crate implements.
pub const SPEC_VERSION: &str = "0.2";

/// How deep a `bundle` part may nest.
///
/// A bundle's own parts are depth 0, a nested bundle's are depth 1, and one nested inside that is
/// depth 2. Put the other way: a `bundle` part may sit at depth 0 or depth 1 and never at depth
/// 2. A save, a card and a collection of cards fill all three tiers; they are the deepest shapes,
/// not the only ones.
pub const MAX_NESTING_DEPTH: usize = 2;

/// How long a part's `path` may run, in bytes.
///
/// Bytes rather than characters, since that is what a consumer sizing the head region has to
/// budget for.
pub const MAX_PATH_LEN: usize = 512;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundle_tag_encodes_to_the_file_magic() {
        // The tag number is chosen so that its own CBOR encoding is the magic: a reader that
        // knows nothing about the format still sees "1SAV" at byte 1 of the file.
        let mut magic = vec![0xda];
        magic.extend_from_slice(&u32::try_from(BUNDLE_TAG).unwrap().to_be_bytes());
        assert_eq!(magic, b"\xda1SAV");
    }
}
