//! The bundle data model: what a `.1saves` file says.
//!
//! Fields are public, because a bundle is data rather than a machine with an invariant to
//! protect. What invariants there are hold *across* fields — no two parts sharing an address, a
//! nested bundle's hash matching what it carries — so they are checked by
//! [`Bundle::validate`](crate::Bundle::validate) when a bundle is read or written, not by every
//! setter along the way.

use std::collections::BTreeMap;

use dcbor::CBOR;

use crate::hash::HashValue;
use crate::name::{Name, ReverseDnsName, Slug};
use crate::shape::Shape;

/// Extension keys and their values, keyed by a reverse-DNS name.
///
/// What sits under a key is entirely the producer's: any CBOR value of any shape. This crate
/// never looks inside one and never drops one it does not recognise.
pub type Extensions = BTreeMap<ReverseDnsName, CBOR>;

/// Integer keys this version does not define, preserved verbatim.
///
/// The only thing such a key can be is a field a later minor version assigned, so a decoder
/// ignores it and round-trips it unchanged rather than refusing the bundle.
pub type UnknownKeys = BTreeMap<i64, CBOR>;

/// A complete bundle: what it is, and the bytes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Bundle {
    /// What this bundle says about itself. Every field is optional; an empty header describes a
    /// bundle whose parts are all a consumer knows about it.
    pub header: Header,
    /// The bytes, as typed parts in ascending `id` order. Never empty.
    pub parts: Vec<Part>,
}

/// Everything the bundle says about itself.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Header {
    /// What this bundle is. Required, and the first key on the wire.
    ///
    /// Read rather than derived: since 0.2 the bundle names its own shape, and what its parts mean
    /// is that shape's document. A shape this version does not define is
    /// [`Unknown`](Shape::Unknown) and round-trips; see there before acting on one.
    pub shape: Shape,
    /// When the bundle was assembled, in whole epoch seconds.
    pub created_at: Option<i64>,
    /// The system **these bytes are a save for**: the one that reads them natively, whatever
    /// happened to write them.
    ///
    /// Read it as "who will load these bytes", never as "who wrote them". An N64 Transfer Pak
    /// lets a game write a Game Boy cartridge's SRAM, and that is a `gb` save whatever produced
    /// it.
    pub system: Option<Slug>,
    /// Clues to which game this is. Absent means the bundle is unidentified.
    pub game: Option<Game>,
    /// Who assembled the bundle, and the default source for every part.
    pub source: Option<Source>,
    /// Present when the bundle *is* a memory card.
    pub card: Option<Card>,
    /// A free-form human-readable note.
    pub description: Option<String>,
    /// Extension keys, flat alongside the integer ones.
    pub extensions: Extensions,
    /// Integer keys from a later minor version.
    pub unknown: UnknownKeys,
}

impl Header {
    /// Whether this header identifies a game at all.
    #[must_use]
    pub fn is_identified(&self) -> bool {
        self.game.is_some()
    }
}

/// Clues to which game a bundle or a part holds.
///
/// A producer fills in as many as it has; a consumer matches with whatever its catalog can
/// resolve. There is no resolution order here on purpose: the hints answer different questions
/// rather than one question with different confidence. A `sha256` identifies one dump, a
/// `serial` identifies one release that many dumps share, and a filename narrows the field
/// without settling it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Game {
    /// This game's id in one or more catalogs, keyed by resolver name. Never empty when present.
    pub game_id: BTreeMap<ReverseDnsName, GameId>,
    /// Known ROM hashes, at most one per algorithm, ascending by tag. Never empty when present.
    pub rom_hashes: Vec<HashValue>,
    /// The original ROM filename or stem.
    pub rom_filename: Option<String>,
    /// A per-system serial code out of the ROM header, such as `SLUS-00404`.
    pub serial: Option<String>,
    /// The best-known game title, as a free string.
    pub title: Option<String>,
    /// Which system's release these hints identify.
    ///
    /// Distinct from the header's [`system`](Header::system), which says what will load the bytes.
    /// The two answer different questions even where they agree.
    pub system: Option<Slug>,
    /// Integer keys from a later minor version.
    pub unknown: UnknownKeys,
}

impl Game {
    /// Whether every field is absent, which is a shape the format forbids on the wire.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.game_id.is_empty()
            && self.rom_hashes.is_empty()
            && self.rom_filename.is_none()
            && self.serial.is_none()
            && self.title.is_none()
            && self.system.is_none()
            && self.unknown.is_empty()
    }
}

/// A game's id in one catalog, in whichever form that catalog publishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameId {
    /// An integer id.
    Uint(u64),
    /// A string id.
    Text(String),
}

/// Who produced some bytes.
///
/// The same map attaches in two places: on the header it names the producer that assembled the
/// bundle and serves as the default for every part, and on a part it names that part's own
/// producer. Provenance belongs to each save, not just to the file around it — a bundle can hold
/// one save read from a real cartridge and another exported by an emulator.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Source {
    /// What kind of producer read the bytes: `console`, `emulator`, `cartridge-reader`,
    /// `flashcart` or `service`. Spec-owned; a producer may not mint one.
    pub device_kind: Option<Slug>,
    /// An opaque stable identifier for the specific device or install. Should carry no personal
    /// information.
    pub fingerprint: Option<String>,
    /// The producing software or device.
    ///
    /// A core listed in the Emulator Cores registry **must** write its slug rather than a
    /// reverse-DNS name, even when it holds a domain, so that one core has one spelling. mGBA is
    /// `mgba`, never `io.mgba`. A producer with no entry mints a reverse-DNS name.
    pub app: Option<Name>,
    /// The version of `app`, in whatever scheme that app uses.
    pub app_version: Option<String>,
    /// Integer keys from a later minor version.
    pub unknown: UnknownKeys,
}

impl Source {
    /// Whether every field is absent.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.device_kind.is_none()
            && self.fingerprint.is_none()
            && self.app.is_none()
            && self.app_version.is_none()
            && self.unknown.is_empty()
    }
}

/// The card a bundle is, when it is one.
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    /// The card's layout: `ps1-mc`, `ps2-mc`, `n64-cpak`, `gc-mc`, `neogeo-mc`, `vmu` or
    /// `saturn-bup`. Spec-owned; a producer may not mint one, because a consumer that does not
    /// know a format cannot rebuild the card and must not try.
    pub format: Slug,
    /// The card's **data** capacity in bytes: what saves can occupy.
    ///
    /// Never the length of a dump. A PS2 card holds 8388608 bytes of data and dumps to 8650752,
    /// because every 512-byte page carries 16 further bytes of spare area; the two are supposed
    /// to disagree.
    pub capacity: u64,
    /// The card-level bytes belonging to no save, verbatim and opaque.
    ///
    /// A controller pak's label, a Neo Geo card's cardholder name, a VMU's colour and icon. None
    /// of it belongs to any save, so splitting a card would otherwise drop it.
    pub system_area: Option<Vec<u8>>,
    /// Integer keys from a later minor version.
    pub unknown: UnknownKeys,
}

/// What role a part's bytes play.
///
/// Absent on the wire means [`Save`](PartKind::Save), which is the vast majority of parts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum PartKind {
    /// Save data. The default, and never written out.
    #[default]
    Save,
    /// A complete memory-card image, kept whole instead of split into its saves.
    CardImage,
    /// The payload is itself a complete bundle. This is how a card holds its saves.
    Bundle,
    /// Anything else binary. `content_type` should be set.
    Aux,
    /// A kind this crate does not know.
    ///
    /// Handled as [`Aux`](PartKind::Aux) — never drop a part because you do not know what it is.
    Other(Name),
}

impl PartKind {
    /// The kind a name denotes, mapping the three the spec defines and keeping anything else.
    #[must_use]
    pub fn from_name(name: Name) -> Self {
        match name.as_str() {
            "card-image" => PartKind::CardImage,
            "bundle" => PartKind::Bundle,
            "aux" => PartKind::Aux,
            _ => PartKind::Other(name),
        }
    }

    /// The name this kind is written as, or `None` for the default.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            PartKind::Save => None,
            PartKind::CardImage => Some("card-image"),
            PartKind::Bundle => Some("bundle"),
            PartKind::Aux => Some("aux"),
            PartKind::Other(name) => Some(name.as_str()),
        }
    }

    /// Whether a consumer should treat this part as auxiliary data.
    ///
    /// True for [`Aux`](PartKind::Aux) and for any kind this crate does not know, which is the
    /// rule that keeps an unfamiliar kind from being dropped.
    #[must_use]
    pub fn is_aux(&self) -> bool {
        matches!(self, PartKind::Aux | PartKind::Other(_))
    }
}

/// One typed blob, and everything said about it.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    /// A stable identifier within the bundle. Parts are stored in ascending `id` order, always.
    ///
    /// Ids are never reused: a part removed from a bundle takes its `id` with it, so a stored
    /// reference dangles rather than silently pointing at something else.
    pub id: u64,
    /// What role these bytes play.
    pub kind: PartKind,
    /// Which socket these bytes came out of, not what medium they are on.
    ///
    /// Absent means `primary`. A consumer meeting a role it does not recognise **must not** guess
    /// which socket is meant: restoring a controller pak's save into a cartridge slot is worse
    /// than declining to restore it.
    pub role: Option<Slug>,
    /// Where these bytes sat in the container they came from, as a relative path.
    ///
    /// On a card's `bundle` part this is the name the card's directory holds for that save,
    /// which is what tells two saves of one game apart.
    pub path: Option<String>,
    /// The index this part occupied in whatever container it came out of.
    ///
    /// Display order, not an address. A writer that packs the parts into different slots has not
    /// done anything wrong.
    pub slot: Option<u64>,
    /// That container's own directory entry for this part, verbatim and fully opaque.
    ///
    /// This specification never says what a byte inside it means, which is what keeps a
    /// card-format parser out of the container.
    pub dirent: Option<Vec<u8>>,
    /// The media type of the payload.
    pub content_type: Option<String>,
    /// SHA-256 over the **uncompressed** payload.
    pub sha256: HashValue,
    /// Who produced this part's bytes, when that differs from the bundle's source.
    pub source: Option<Source>,
    /// Which game this part's bytes belong to, when that differs from the bundle's.
    pub game: Option<Game>,
    /// An index copy of a nested bundle's `system`. Meaningless on a part that is not a bundle.
    pub system: Option<Slug>,
    /// Present when the console that wrote this payload bound it to itself.
    ///
    /// Its presence is what a consumer acts on; the value only names what the payload is bound
    /// to, so a consumer that has never seen the value still knows not to treat it as portable.
    pub binding: Option<Slug>,
    /// The bytes, or a reference to them.
    pub payload: Payload,
    /// Extension keys, flat alongside the integer ones.
    pub extensions: Extensions,
    /// Integer keys from a later minor version.
    pub unknown: UnknownKeys,
}

impl Part {
    /// Builds an ordinary save part around some bytes, computing its digest.
    #[must_use]
    pub fn new(id: u64, payload: impl Into<Vec<u8>>) -> Self {
        let bytes = payload.into();
        Part {
            id,
            kind: PartKind::Save,
            role: None,
            path: None,
            slot: None,
            dirent: None,
            content_type: None,
            sha256: HashValue::sha256_of(&bytes),
            source: None,
            game: None,
            system: None,
            binding: None,
            payload: Payload::Embedded(bytes),
            extensions: Extensions::new(),
            unknown: UnknownKeys::new(),
        }
    }

    /// The address a consumer names this part by: its role, path and slot together.
    ///
    /// No two parts in one bundle may share it, because those three are how a part is named to a
    /// user and matched against a target.
    #[must_use]
    pub fn address(&self) -> (Option<&Slug>, Option<&str>, Option<u64>) {
        (self.role.as_ref(), self.path.as_deref(), self.slot)
    }
}

/// A part's bytes, in one of the three shapes the format allows.
///
/// Which form a part is in decides whether `size` appears on the wire, so the two cannot disagree.
/// Since 0.2 `size` rides on compression and nothing else:
///
/// | Form         | `size`  | `encoding` | payload     |
/// | ------------ | ------- | ---------- | ----------- |
/// | Uncompressed | absent  | absent     | byte string |
/// | Compressed   | present | `"zstd"`   | byte string |
#[derive(Debug, Clone, PartialEq)]
pub enum Payload {
    /// The bytes, inline and uncompressed. The payload states its own length, so no `size`.
    Embedded(Vec<u8>),
    /// The bytes, inline and compressed with zstd.
    Compressed {
        /// The compressed bytes as they sit in the file.
        bytes: Vec<u8>,
        /// The uncompressed byte length, which the compressed form does not state.
        size: u64,
    },
}

impl Default for Payload {
    fn default() -> Self {
        Payload::Embedded(Vec::new())
    }
}

impl Payload {
    /// The uncompressed byte length, whichever form this is.
    #[must_use]
    pub fn len(&self) -> u64 {
        match self {
            Payload::Embedded(bytes) => bytes.len() as u64,
            Payload::Compressed { size, .. } => *size,
        }
    }

    /// Whether the payload is zero bytes long, which the format allows.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_kind_is_handled_as_aux() {
        // Never drop a part because you do not know what it is.
        let minted = PartKind::from_name(Name::parse("io.mgba.rewind").unwrap());
        assert!(minted.is_aux());
        assert_eq!(minted.as_str(), Some("io.mgba.rewind"));
    }

    #[test]
    fn the_default_kind_is_never_written_out() {
        assert_eq!(PartKind::default(), PartKind::Save);
        assert_eq!(PartKind::Save.as_str(), None);
    }

    #[test]
    fn a_spec_kind_round_trips_through_its_name() {
        for text in ["card-image", "bundle", "aux"] {
            let kind = PartKind::from_name(Name::parse(text).unwrap());
            assert_eq!(kind.as_str(), Some(text));
            assert!(!matches!(kind, PartKind::Other(_)), "{text} should be a known kind");
        }
    }

    #[test]
    fn a_new_part_hashes_its_own_payload() {
        let part = Part::new(0, *b"SAVE");
        assert_eq!(part.sha256, HashValue::sha256_of(b"SAVE"));
        assert_eq!(part.payload.len(), 4);
    }
}
