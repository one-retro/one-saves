//! The Universal Saves Format (`.1saves`, `1SAV`): a portable, self-describing CBOR container
//! for retro console save files and everything attached to them.
//!
//! A retro save is a bare binary dump with no agreed way to say what game it belongs to, what
//! wrote it, or what else belongs with it. This crate reads and writes the container that fixes
//! that: typed binary parts plus whatever identifying context a producer has, in one file.
//!
//! The specification lives at
//! <https://docs.1retro.com/specifications/universal-saves-format/>. This crate implements
//! version 0.1, which is **not yet stabilized** — until 1.0, breaking changes happen in place
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
//! ## Decoding and re-encoding gives the bytes back
//!
//! It follows that `Bundle::from_slice(&bytes)?.to_vec()? == bytes`, for every byte string
//! `from_slice` accepts. Determinism is enforced on the way *in* as well as on the way out — a
//! non-preferred integer head, a misordered map key, a duplicate key and an indefinite length are
//! all decode errors — so an accepted file was already written the one way this crate writes it,
//! and nothing a decoder does not recognise is dropped along the way.
//!
//! A store may lean on this: a bundle it decoded, inspected and re-encoded is byte-identical to
//! the one it was handed, so the [file hash](crate::Bundle::file_hash) survives the trip and the
//! original bytes need not be kept beside it. What the guarantee does *not* cover is a bundle you
//! have modified, or one you built rather than decoded: neither compression nor an external
//! reference is pinned by the format, so two producers can write one logical bundle as different
//! bytes. That is what the [content hash](crate::Bundle::content_hash) is for.
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
//! # Nesting: cards, collections and the saves inside them
//!
//! A [`bundle`](crate::PartKind::Bundle) part's payload is itself a complete `.1saves` file, which
//! is how a card holds its saves and how a collection holds its entries. Ask
//! [`shape`](crate::Bundle::shape) what you have been handed — a bundle names its own since 0.2 —
//! then step in with the same [`from_slice`](crate::Bundle::from_slice) you used on the outside.
//! Check [`is_known`](crate::Shape::is_known) first: a shape a later version defines must be
//! round-tripped rather than walked.
//!
//! ```no_run
//! use one_saves::{Bundle, Part, PartKind, Shape};
//!
//! let card = Bundle::from_slice(&std::fs::read("card.1saves")?)?;
//! assert_eq!(card.shape(), &Shape::Card);
//!
//! for part in &card.parts {
//!     if part.kind == PartKind::Bundle {
//!         // One save, standing on its own: the inner bundle repeats the system and the game,
//!         // because nothing is inherited across the nesting boundary.
//!         let save = Bundle::from_slice(&part.bytes()?)?;
//!         std::fs::write(format!("save-{}.1saves", part.id), save.to_vec()?)?;
//!     }
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! Going the other way is [`Part::new`], which computes the digest a `bundle` part's `sha256`
//! has to hold, followed by setting `kind`:
//!
//! ```
//! use one_saves::{Bundle, Header, Part, PartKind};
//!
//! let save = Bundle { header: Header::default(), parts: vec![Part::new(0, *b"SAVE")] };
//! let mut entry = Part::new(0, save.to_vec()?);
//! entry.kind = PartKind::Bundle;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! Because that payload is a whole file, slicing a save out of a card is a byte copy rather than a
//! re-encode, and the outer part's `sha256` is already the inner bundle's content hash.
//!
//! The cost is that decoding a nested bundle twice is the expected pattern, not a mistake to
//! design around. [`validate`](crate::Bundle::validate) — which
//! [`from_slice`](crate::Bundle::from_slice) runs for you — decodes every `bundle` part to check
//! it and then discards the result, since holding it would mean either a second representation of
//! a bundle or paying for the inner decode whether or not a caller wants it. So the loop above
//! decodes each save a second time. On a 16 MiB GameCube card that is the work twice; if it
//! matters, [`from_cbor`](crate::Bundle::from_cbor) skips validation and lets you decode the
//! outer bundle once without walking into its payloads.
//!
//! # Threads
//!
//! Every type here is a plain owned value except for the CBOR under an extension key, which
//! `dcbor` reference-counts. That makes [`Bundle`] and everything reachable from it `Send` and
//! `Sync` only when those refcounts are atomic, which is what the off-by-default `multithread`
//! feature switches on:
//!
//! ```toml
//! one-saves = { version = "0.2", features = ["multithread"] }
//! ```
//!
//! Turn it on to hold a bundle across an `.await` in a future that has to be `Send`, or to move
//! one between threads at all. Without it the reference counts are non-atomic and cheaper, which
//! is the right trade for a caller that stays on one thread. It changes no behaviour and no
//! encoding — a bundle written by either build is the same bytes.
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
mod shape;
mod validate;

pub use codec::Strictness;
pub use error::{Error, ErrorKind};
pub use hash::{HashAlgorithm, HashError, HashValue};
pub use model::{
    Bundle, Card, Extensions, Game, GameId, Header, Part, PartKind, Payload, Source, UnknownKeys,
};
pub use name::{Name, NameError, ReverseDnsName, Slug};
pub use shape::Shape;

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

/// How long a part's `path` may run, in bytes.
///
/// Bytes rather than characters, since that is what a consumer sizing the head region has to
/// budget for.
pub const MAX_PATH_LEN: usize = 512;

/// What the `multithread` feature promises, checked at compile time.
///
/// The bound lives on `Bundle` because it reaches every other type in the crate: a header, its
/// parts, their payloads, and the `dcbor::CBOR` under an extension key, which is the only thing
/// here that is reference-counted and so the only thing the feature changes. `Error` is checked
/// separately because it is what crosses the boundary on the failure path and shares none of
/// `Bundle`'s fields.
#[cfg(feature = "multithread")]
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Bundle>();
    assert_send_sync::<Error>();
};

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
