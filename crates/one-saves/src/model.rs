//! The bundle data model: what a `.1saves` file says.
//!
//! Fields are public, because a bundle is data rather than a machine with an invariant to
//! protect. What invariants there are hold *across* fields — no two parts sharing an address, a
//! nested bundle's hash matching what it carries — so they are checked by
//! [`Bundle::validate`](crate::Bundle::validate) when a bundle is read or written, not by every
//! setter along the way.
//!
//! Every struct here is `#[non_exhaustive]`, because a later version of the specification adds
//! fields to them. Build one by chaining its setters, which take the field's name. A setter for an
//! optional field that is not text takes the value or an `Option` of it, so a value a caller may
//! not have chains the same as one it does:
//!
//! ```
//! use one_saves::{Bundle, Game, Header, Part, PartKind, Shape};
//!
//! let save = Bundle::new(Header::new(Shape::Save), vec![Part::new(0, *b"SAVE")]);
//! let part = Part::new(0, save.to_vec()?)
//!     .kind(PartKind::Bundle)
//!     .path("BASLUS-00594")
//!     .game(Game::default().serial("SLUS-00594").title("Metal Gear Solid"));
//! assert_eq!(part.game.as_ref().and_then(|game| game.serial.as_deref()), Some("SLUS-00594"));
//! # Ok::<(), one_saves::Error>(())
//! ```

use std::collections::BTreeMap;

use dcbor::CBOR;

use crate::extension::{Extension, Extensions};
use crate::hash::HashValue;
use crate::name::{Name, ReverseDnsName, Slug};
use crate::shape::Shape;

/// Integer keys this version does not define, preserved verbatim.
///
/// The only thing such a key can be is a field a later minor version assigned, so a decoder
/// ignores it and round-trips it unchanged rather than refusing the bundle.
pub type UnknownKeys = BTreeMap<i64, CBOR>;

/// A complete bundle: what it is, and the bytes.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct Bundle {
    /// What this bundle says about itself. Every field is optional; an empty header describes a
    /// bundle whose parts are all a consumer knows about it.
    pub header: Header,
    /// The bytes, as typed parts in ascending `id` order. Never empty.
    pub parts: Vec<Part>,
}

impl Bundle {
    /// A bundle of this header and these parts.
    #[must_use]
    pub fn new(header: Header, parts: Vec<Part>) -> Self {
        Bundle { header, parts }
    }
}

/// Everything the bundle says about itself.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
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
    /// A header naming its shape and nothing else.
    #[must_use]
    pub fn new(shape: Shape) -> Self {
        Header { shape, ..Header::default() }
    }

    /// Whether this header identifies a game at all.
    #[must_use]
    pub fn is_identified(&self) -> bool {
        self.game.is_some()
    }

    /// Sets when the bundle was assembled, in whole epoch seconds.
    #[must_use]
    pub fn created_at(mut self, seconds: impl Into<Option<i64>>) -> Self {
        self.created_at = seconds.into();
        self
    }

    /// Sets the system these bytes are a save for.
    #[must_use]
    pub fn system(mut self, system: impl Into<Option<Slug>>) -> Self {
        self.system = system.into();
        self
    }

    /// Sets the clues to which game this is.
    #[must_use]
    pub fn game(mut self, game: impl Into<Option<Game>>) -> Self {
        self.game = game.into();
        self
    }

    /// Sets who assembled the bundle.
    #[must_use]
    pub fn source(mut self, source: impl Into<Option<Source>>) -> Self {
        self.source = source.into();
        self
    }

    /// Sets the card this bundle is.
    #[must_use]
    pub fn card(mut self, card: impl Into<Option<Card>>) -> Self {
        self.card = card.into();
        self
    }

    /// Sets the human-readable note.
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Puts an extension value under its key.
    #[must_use]
    pub fn extension<T: Extension>(mut self, value: T) -> Self {
        self.extensions.insert(value);
        self
    }

    /// Replaces every extension key with these.
    #[must_use]
    pub fn extensions(mut self, extensions: Extensions) -> Self {
        self.extensions = extensions;
        self
    }
}

/// Clues to which game a bundle or a part holds.
///
/// A producer fills in as many as it has; a consumer matches with whatever its catalog can
/// resolve. There is no resolution order here on purpose: the hints answer different questions
/// rather than one question with different confidence. A `sha256` identifies one dump, a
/// `serial` identifies one release that many dumps share, and a filename narrows the field
/// without settling it.
///
/// Start from [`Game::default`] and set what you have.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
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
    /// Extension keys, flat alongside the integer ones: a clue none of the fields above names,
    /// such as the revision byte a ROM header carries beside its serial.
    ///
    /// A key here describes the game and says nothing about this save, so two saves of one game
    /// still carry the same hints. A clock reading stays on the header. A consumer matching
    /// against a catalog skips a key it does not recognise, as it skips an unknown resolver in
    /// [`game_id`](Game::game_id).
    pub extensions: Extensions,
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
            && self.extensions.is_empty()
            && self.unknown.is_empty()
    }

    /// Adds this game's id in one catalog, replacing any the same resolver had.
    #[must_use]
    pub fn game_id(mut self, resolver: ReverseDnsName, id: impl Into<GameId>) -> Self {
        self.game_id.insert(resolver, id.into());
        self
    }

    /// Adds a ROM hash, replacing any of the same algorithm.
    ///
    /// The list stays ascending by tag, which is the order the format writes it in, so hashes can
    /// be added in whatever order a producer computed them.
    #[must_use]
    pub fn rom_hash(mut self, hash: HashValue) -> Self {
        self.rom_hashes.retain(|existing| existing.algorithm() != hash.algorithm());
        let at = self.rom_hashes.partition_point(|existing| existing.tag() < hash.tag());
        self.rom_hashes.insert(at, hash);
        self
    }

    /// Sets the original ROM filename or stem.
    #[must_use]
    pub fn rom_filename(mut self, rom_filename: impl Into<String>) -> Self {
        self.rom_filename = Some(rom_filename.into());
        self
    }

    /// Sets the serial out of the ROM header.
    #[must_use]
    pub fn serial(mut self, serial: impl Into<String>) -> Self {
        self.serial = Some(serial.into());
        self
    }

    /// Sets the best-known title.
    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Sets which system's release these hints identify.
    #[must_use]
    pub fn system(mut self, system: impl Into<Option<Slug>>) -> Self {
        self.system = system.into();
        self
    }

    /// Puts an extension value under its key: a clue none of the fields names.
    #[must_use]
    pub fn extension<T: Extension>(mut self, value: T) -> Self {
        self.extensions.insert(value);
        self
    }

    /// Replaces every extension key with these.
    #[must_use]
    pub fn extensions(mut self, extensions: Extensions) -> Self {
        self.extensions = extensions;
        self
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

impl From<u64> for GameId {
    fn from(id: u64) -> Self {
        GameId::Uint(id)
    }
}

impl From<String> for GameId {
    fn from(id: String) -> Self {
        GameId::Text(id)
    }
}

impl From<&str> for GameId {
    fn from(id: &str) -> Self {
        GameId::Text(id.to_owned())
    }
}

/// Who produced some bytes.
///
/// The same map attaches in two places: on the header it names the producer that assembled the
/// bundle and serves as the default for every part, and on a part it names that part's own
/// producer. Provenance belongs to each save, not just to the file around it — a bundle can hold
/// one save read from a real cartridge and another exported by an emulator.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
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

    /// Sets what kind of producer read the bytes.
    #[must_use]
    pub fn device_kind(mut self, device_kind: impl Into<Option<Slug>>) -> Self {
        self.device_kind = device_kind.into();
        self
    }

    /// Sets the opaque identifier for the device or install.
    #[must_use]
    pub fn fingerprint(mut self, fingerprint: impl Into<String>) -> Self {
        self.fingerprint = Some(fingerprint.into());
        self
    }

    /// Sets the producing software or device.
    #[must_use]
    pub fn app(mut self, app: impl Into<Option<Name>>) -> Self {
        self.app = app.into();
        self
    }

    /// Sets the version of `app`.
    #[must_use]
    pub fn app_version(mut self, app_version: impl Into<String>) -> Self {
        self.app_version = Some(app_version.into());
        self
    }
}

/// The card a bundle is, when it is one.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
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

impl Card {
    /// A card of this layout and data capacity.
    #[must_use]
    pub fn new(format: Slug, capacity: u64) -> Self {
        Card { format, capacity, system_area: None, unknown: UnknownKeys::new() }
    }

    /// Sets the card-level bytes belonging to no save.
    #[must_use]
    pub fn system_area(mut self, system_area: impl Into<Vec<u8>>) -> Self {
        self.system_area = Some(system_area.into());
        self
    }
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
#[non_exhaustive]
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

    /// Sets what role these bytes play.
    #[must_use]
    pub fn kind(mut self, kind: PartKind) -> Self {
        self.kind = kind;
        self
    }

    /// Sets which socket these bytes came out of.
    #[must_use]
    pub fn role(mut self, role: impl Into<Option<Slug>>) -> Self {
        self.role = role.into();
        self
    }

    /// Sets where these bytes sat in the container they came from.
    #[must_use]
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Sets the index this part occupied in its container.
    #[must_use]
    pub fn slot(mut self, slot: impl Into<Option<u64>>) -> Self {
        self.slot = slot.into();
        self
    }

    /// Sets the container's own directory entry for this part, verbatim.
    #[must_use]
    pub fn dirent(mut self, dirent: impl Into<Vec<u8>>) -> Self {
        self.dirent = Some(dirent.into());
        self
    }

    /// Sets the media type of the payload.
    #[must_use]
    pub fn content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = Some(content_type.into());
        self
    }

    /// Sets who produced this part's bytes.
    #[must_use]
    pub fn source(mut self, source: impl Into<Option<Source>>) -> Self {
        self.source = source.into();
        self
    }

    /// Sets which game this part's bytes belong to.
    #[must_use]
    pub fn game(mut self, game: impl Into<Option<Game>>) -> Self {
        self.game = game.into();
        self
    }

    /// Sets the index copy of a nested bundle's `system`.
    #[must_use]
    pub fn system(mut self, system: impl Into<Option<Slug>>) -> Self {
        self.system = system.into();
        self
    }

    /// Sets what the console that wrote this payload bound it to.
    #[must_use]
    pub fn binding(mut self, binding: impl Into<Option<Slug>>) -> Self {
        self.binding = binding.into();
        self
    }

    /// Puts an extension value under its key.
    #[must_use]
    pub fn extension<T: Extension>(mut self, value: T) -> Self {
        self.extensions.insert(value);
        self
    }

    /// Replaces every extension key with these.
    #[must_use]
    pub fn extensions(mut self, extensions: Extensions) -> Self {
        self.extensions = extensions;
        self
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
    fn a_game_holding_only_an_extension_key_is_not_empty() {
        // An empty `game` is malformed, and a map with a key in it is not empty, whichever
        // namespace that key is in.
        let mut game = Game::default();
        assert!(game.is_empty());
        game.extensions.insert_cbor(ReverseDnsName::parse("x.example.revision").unwrap(), CBOR::from(1u64));
        assert!(!game.is_empty());
    }

    #[test]
    fn a_new_part_hashes_its_own_payload() {
        let part = Part::new(0, *b"SAVE");
        assert_eq!(part.sha256, HashValue::sha256_of(b"SAVE"));
        assert_eq!(part.payload.len(), 4);
    }
}
