//! What can be wrong with a bundle, and where.

use core::fmt;

use crate::hash::{HashAlgorithm, HashError};
use crate::name::NameError;

/// A bundle that could not be read, or could not be written.
///
/// The [`path`](Error::path) locates the fault the way the specification's worked examples name
/// things — `parts[2].sha256`, `header.game.rom_hashes` — because a bundle is a nest of maps and
/// "expected a byte string" on its own does not say which one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    path: String,
    kind: ErrorKind,
}

impl Error {
    /// Builds an error at a location.
    pub(crate) fn at(path: impl Into<String>, kind: ErrorKind) -> Self {
        Error { path: path.into(), kind }
    }

    /// Where in the bundle the fault is.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// What the fault is.
    #[must_use]
    pub fn kind(&self) -> &ErrorKind {
        &self.kind
    }

    /// Prefixes the location, so an inner bundle's errors read from the outside in.
    pub(crate) fn within(mut self, outer: &str) -> Self {
        self.path = if self.path.is_empty() { outer.to_owned() } else { format!("{outer}.{}", self.path) };
        self
    }

    /// The fault, without where it is.
    pub(crate) fn into_kind(self) -> ErrorKind {
        self.kind
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            write!(f, "{}", self.kind)
        } else {
            write!(f, "{}: {}", self.path, self.kind)
        }
    }
}

impl core::error::Error for Error {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match &self.kind {
            ErrorKind::Name(source) => Some(source),
            ErrorKind::Hash(source) => Some(source),
            _ => None,
        }
    }
}

/// The kinds of fault a bundle can have.
///
/// Each corresponds to a rule in the specification rather than to a step in this implementation,
/// so a message names something a reader can go and look up.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The bytes are not deterministically encoded CBOR.
    ///
    /// Definite lengths, preferred heads, sorted and unique map keys, and NFC text are all
    /// required; a bundle that breaks one of them would hash differently for no change in what
    /// it says.
    Encoding(String),

    /// The CBOR nests arrays, maps and tags deeper than this decoder reads.
    ///
    /// Not a rule of the specification, which sets no depth, but a limit of this implementation:
    /// decoding recurses, so without one a file of a few kilobytes made of nothing but array heads
    /// takes the stack with it, which is an abort rather than an error. The limit is far past
    /// anything a bundle needs.
    TooDeep {
        /// How many levels this decoder reads.
        limit: usize,
    },

    /// The file is not wrapped in the bundle tag.
    NotABundle {
        /// The tag that was there instead, if the value was tagged at all.
        found: Option<u64>,
    },

    /// A value had the wrong CBOR type.
    Type {
        /// What the specification says belongs here.
        expected: &'static str,
    },

    /// A required key was absent.
    MissingKey(&'static str),

    /// A shape this version does not define.
    ///
    /// Only [`Strictness::Schema`](crate::Strictness::Schema) raises this. A shipped decoder
    /// parses and round-trips such a bundle, because the only thing an unrecognised shape can be
    /// is one a later minor version assigned — the same asymmetry as
    /// [`UnknownIntegerKey`](ErrorKind::UnknownIntegerKey). It must still decline any operation
    /// that depends on knowing what the shape means.
    UnknownShape(String),

    /// A value this version does not list, in a vocabulary the specification owns: a card
    /// `format`, a `binding`, a `device_kind`, an `origin`, or a part's `kind` where a slug.
    ///
    /// Only [`Strictness::Schema`](crate::Strictness::Schema) raises this, for the same reason as
    /// [`UnknownShape`](ErrorKind::UnknownShape): a shipped decoder keeps such a value, since the
    /// only thing it can be is one a later version added to the list.
    UnknownValue {
        /// The field the value is in.
        field: &'static str,
        /// The value.
        value: String,
    },

    /// A key the bundle's shape does not admit.
    ///
    /// Each shape takes a different header: a card is a card because it carries the `card` map, a
    /// save never carries one, a device names a `system` and no `game`, and a collection has
    /// nothing of its own to say. The shape is read first, so this names what contradicted it.
    KeyNotInShape {
        /// The shape the header named.
        shape: &'static str,
        /// The field that shape does not admit.
        field: &'static str,
    },

    /// A part whose kind the bundle's shape does not admit, or too many of one kind.
    ///
    /// A save holds no nested bundle: a component that is a card of its own makes the bundle a
    /// device rather than a save. A state holds exactly one `state` part and at most one
    /// `screenshot`, a screenshot holds its image and nothing else, and neither kind sits in any
    /// other shape.
    PartNotInShape {
        /// The shape the header named.
        shape: &'static str,
        /// What was wrong with the part.
        reason: &'static str,
    },

    /// A key a `state` or a `screenshot` part does not take.
    ///
    /// A state part is the core's own data: it takes `slot`, for the number the emulator gave
    /// it, and nothing else of a part's addressing, and no `content_type`, since the data has no
    /// media type. A screenshot takes no addressing at all.
    KeyNotOnPart {
        /// The part's kind.
        kind: &'static str,
        /// The field that kind does not take.
        field: &'static str,
    },

    /// A state's `source` named neither a `core` nor an `app`.
    ///
    /// A state loads only into the core that took it, so its `source` is required to say which:
    /// the core, or the app where nothing runs one.
    StateLoadsIntoNothing,

    /// A screenshot's `content_type` was neither `image/png` nor `image/jpeg`.
    ///
    /// Those are the two any image library reads. A producer encodes anything else as PNG.
    ScreenshotType(String),

    /// A `taken_with` language was not a BCP 47 tag in its canonical case.
    ///
    /// The language is in lower case, a script in title case and a region in upper case, and the
    /// tag stops at the region, so one setting has one spelling.
    LanguageNotCanonical(String),

    /// An integer key this version does not define.
    ///
    /// Only [`Strictness::Schema`](crate::Strictness::Schema) raises this. A shipped decoder
    /// ignores and round-trips such a key, because the only thing it can be is a field a later
    /// minor version assigned after that decoder shipped.
    UnknownIntegerKey(i64),

    /// A name was neither a well-formed slug nor a well-formed reverse-DNS name.
    Name(NameError),

    /// A hash value had an unknown tag or a digest of the wrong length.
    Hash(HashError),

    /// An optional map, array, text string or byte string was present but empty, where absence
    /// is the only spelling of empty.
    ///
    /// An empty `game`, `source`, `taken_with`, `description` or `dirent` is malformed: nothing is
    /// absence, so spelling it out would give one bundle two encodings. A `parts` array is never
    /// empty either. A payload is not optional, so a payload of no bytes is a payload.
    EmptyContainer,

    /// A default was written out, where absence is the only spelling of the default.
    ///
    /// A part's `kind` means `save` when absent, its `role` means `primary`, and its `path` means
    /// the role is the whole address. A `utc_offset` of zero is UTC, which is what its absence
    /// says.
    DefaultSpelledOut(&'static str),

    /// A part's `game` or `source` equal to the header's, which a part without one takes already.
    ///
    /// On a card of one game, that makes an index copy on every save's part a second spelling of
    /// the same card.
    RepeatsTheHeader(&'static str),

    /// A `path` ran past 512 bytes.
    PathTooLong(usize),

    /// A `path` began or ended with `/`, or held an empty, `.` or `..` segment.
    ///
    /// Each is a second spelling of an address, or one that climbs out of the container it names.
    PathForbidden,

    /// A part carried key 7 or 8, `encoding` or `size`, which 0.9 retired with compression.
    ///
    /// A retired key is never free again, so it is malformed rather than a later version's.
    RetiredKey(i64),

    /// An epoch field carried a float, which this format forbids.
    ///
    /// Tag 1 admits one; left open, one instant would have two conforming encodings.
    FloatEpoch,

    /// Parts were not in ascending `id` order.
    PartsOutOfOrder,

    /// Two parts shared an `id`.
    DuplicatePartId(u64),

    /// Two parts of one kind agreed on `role`, `path` and `slot` together.
    ///
    /// Those three are how a consumer names a part to a user and matches it against a target, so
    /// two parts a consumer cannot tell apart is a bundle it cannot act on. Parts of different
    /// kinds never clash.
    PartsShareAnAddress {
        /// The `id` of the earlier part.
        first: u64,
        /// The `id` of the part that collided with it.
        second: u64,
    },

    /// A card's `system` part sits in a directory no `system` part before it names.
    ///
    /// A writer lays a card's entries down in part order, so the directory has to come first or
    /// there is nowhere to put the file.
    SystemEntryBeforeItsDirectory {
        /// The directory the entry's `path` names.
        directory: String,
    },

    /// A `dirent` was not the length its card's format makes one.
    ///
    /// Every format the memory cards specification lists fixes its entry's length, and a writer
    /// rebuilding the card takes an entry of that length and no other.
    DirentWrongLength {
        /// The card's format.
        format: String,
        /// How long that format's entry is.
        expected: usize,
        /// How long this one was.
        found: usize,
    },

    /// A card's `system_area` was not the length its format makes it.
    ///
    /// Only a Neo Geo card fixes one, at its whole 64-byte header block.
    SystemAreaWrongLength {
        /// The card's format.
        format: String,
        /// How long that format's system area is.
        expected: usize,
        /// How long this one was.
        found: usize,
    },

    /// A save's `path` on a card was not spelled the way its format spells one.
    ///
    /// A Neo Geo card and an N64 pak hold something other than text for a save's name, so the
    /// specification spells it for them, and two producers name one save alike.
    PathMisspelled {
        /// The card's format.
        format: String,
        /// How that format spells a save's name.
        spelling: &'static str,
    },

    /// `rom_hashes` was not in ascending tag order.
    RomHashesOutOfOrder,

    /// `rom_hashes` held two entries for one algorithm.
    ///
    /// Two digests of one algorithm name two different files, and nothing says which the bundle
    /// means.
    RomHashesDuplicateAlgorithm(HashAlgorithm),

    /// A part's `sha256` did not equal the digest of its payload.
    ///
    /// A decoder handing that payload out would hand out something corrupt.
    PayloadHashMismatch,

    /// `bound_to` without `binding`, whose target it names.
    BoundToWithoutBinding,

    /// `bound_to` named an identifier its `binding` does not bind a payload with: an account on a
    /// payload bound to a console, say.
    BoundToNamesAnotherTarget {
        /// The binding.
        binding: String,
        /// The `bound_to` key it does not give.
        key: u64,
    },

    /// Two parts of one device share a `role`.
    ///
    /// A device's components are told apart by `role` and nothing else, so this is stricter than
    /// the rule on addresses: two components differing in `path` still clash.
    RoleShared {
        /// The `id` of the earlier part.
        first: u64,
        /// The `id` of the part that shares its role.
        second: u64,
    },

    /// The parts of one card did not all name the same `role`, or all name none.
    ///
    /// A card sat in one slot when it was read.
    CardRolesDisagree,

    /// A part of a save or a state named a memory card's slot as its `role`: `memcard-<n>`,
    /// `controller-pak-<n>` or `neogeo-card`.
    ///
    /// A save on a card is never a part of another save. A Controller Pak is a card, and one game's
    /// note on it is a save on that card, which goes beside the cartridge's save in a device.
    CardSlotOnASavePart(String),

    /// An extension key placed somewhere a decoder refuses it.
    ///
    /// Only `x.1sav.n64` is placed by a decoder: every other key is round-tripped wherever it
    /// sits, since the rules on its page bind producers only.
    KeyOutOfPlace {
        /// The key.
        key: String,
        /// Where it does belong.
        allowed: &'static str,
    },

    /// The `cartridge` save part of a bundle naming `n64` did not carry `x.1sav.n64`.
    ///
    /// Neither the bytes nor their length say which chip they came from, so the part has to.
    N64ChipMissing,

    /// `x.1sav.n64` was not a map whose key 0 names a chip its page lists.
    ///
    /// A consumer could not tell how long a save for any other chip is, so a chip a later version
    /// adds is a breaking change rather than a value to round-trip.
    N64ChipUnknown(String),

    /// An N64 cartridge's payload was not as long as its chip.
    N64PayloadWrongLength {
        /// The chip `x.1sav.n64` names.
        chip: &'static str,
        /// How long that chip is.
        expected: usize,
        /// How long the payload was.
        found: usize,
    },

    /// A bundle naming `n64` held a second part with the role `cartridge`.
    ///
    /// A cartridge has one save chip.
    SecondCartridge {
        /// The `id` of the first.
        first: u64,
        /// The `id` of the second.
        second: u64,
    },

    /// A 64DD disk's save held its RAM area other than in one `save` part with no role.
    N64ddArea(&'static str),

    /// A 64DD disk's RAM area was not as long as any disk type makes it.
    ///
    /// Its length is how a consumer tells the disk's type back, so a save of part of the area
    /// cannot be told from a disk of another type.
    N64ddWrongLength(usize),

    /// A DS game card's save was not as long as any of the card's chips.
    ///
    /// A DS save names no chip, so its length is the chip: a save of any other length is padded,
    /// cut, or a NAND card's, which this version does not carry as a save.
    NdsWrongLength(usize),

    /// A `bundle` part's payload did not decode as a bundle.
    NestedPayloadNotABundle(Box<Error>),

    /// A `bundle` part's `sha256` did not equal the content hash of the bundle it carries.
    ///
    /// That equality is what makes extracting a save a byte copy rather than a re-encode.
    NestedHashMismatch,

    /// Past a limit the caller set on a [streaming reader](crate::stream::Reader).
    ///
    /// Not a rule of the specification: the bundle may be well formed, and
    /// [`from_slice`](crate::Bundle::from_slice), which takes no limits, may accept it. It is
    /// raised as soon as a length or a count that breaks the limit is read, before any of the bytes
    /// it announces.
    OverLimit {
        /// Which limit.
        limit: crate::stream::Limit,
        /// What the caller set it to.
        max: u64,
    },
}

impl fmt::Display for ErrorKind {
    // One arm a kind, and each kind is a rule of the specification, so the length is the spec's.
    #[allow(clippy::too_many_lines)]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErrorKind::Encoding(why) => write!(f, "not deterministically encoded CBOR: {why}"),
            ErrorKind::TooDeep { limit } => {
                write!(f, "nests deeper than the {limit} levels this decoder reads")
            }
            ErrorKind::NotABundle { found: Some(tag) } => {
                write!(f, "tag {tag} is not the bundle tag {}", crate::BUNDLE_TAG)
            }
            ErrorKind::NotABundle { found: None } => f.write_str("not wrapped in the bundle tag"),
            ErrorKind::Type { expected } => write!(f, "expected {expected}"),
            ErrorKind::MissingKey(name) => write!(f, "required key `{name}` is absent"),
            ErrorKind::UnknownShape(shape) => {
                write!(f, "shape `{shape}` is not defined by version {}", crate::SPEC_VERSION)
            }
            ErrorKind::UnknownValue { field, value } => {
                write!(f, "`{value}` is not a `{field}` version {} lists", crate::SPEC_VERSION)
            }
            ErrorKind::KeyNotInShape { shape, field } => {
                write!(f, "a `{shape}` bundle does not carry `{field}`")
            }
            ErrorKind::PartNotInShape { shape, reason } => {
                write!(f, "a `{shape}` bundle {reason}")
            }
            ErrorKind::KeyNotOnPart { kind, field } => write!(f, "a `{kind}` part does not carry `{field}`"),
            ErrorKind::StateLoadsIntoNothing => {
                f.write_str("names neither a `core` nor an `app`, so nothing says what the state loads into")
            }
            ErrorKind::ScreenshotType(found) => {
                write!(f, "`{found}` is not a type a screenshot takes; it is `image/png` or `image/jpeg`")
            }
            ErrorKind::LanguageNotCanonical(tag) => write!(
                f,
                "`{tag}` is not a BCP 47 tag in its canonical case: a lower-case language, then a \
                 title-case script, then an upper-case region"
            ),
            ErrorKind::UnknownIntegerKey(key) => {
                write!(f, "integer key {key} is not defined by version {}", crate::SPEC_VERSION)
            }
            ErrorKind::Name(source) => write!(f, "{source}"),
            ErrorKind::Hash(source) => write!(f, "{source}"),
            ErrorKind::EmptyContainer => f.write_str("is empty, and absence is the only empty"),
            ErrorKind::DefaultSpelledOut(default) => {
                write!(f, "spells out its default `{default}`, which is written by leaving the key out")
            }
            ErrorKind::PathTooLong(len) => write!(f, "runs {len} bytes, past the 512-byte cap"),
            ErrorKind::RepeatsTheHeader(field) => {
                write!(f, "repeats the header's `{field}`, which a part without one takes already")
            }
            ErrorKind::PathForbidden => {
                f.write_str("begins or ends with `/`, or holds an empty, `.` or `..` segment")
            }
            ErrorKind::RetiredKey(key) => {
                write!(f, "carries key {key}, which 0.9 retired with compression and no version reuses")
            }
            ErrorKind::FloatEpoch => f.write_str("is a float; every epoch here is whole seconds"),
            ErrorKind::PartsOutOfOrder => f.write_str("parts are not in ascending `id` order"),
            ErrorKind::DuplicatePartId(id) => write!(f, "two parts share the id {id}"),
            ErrorKind::PartsShareAnAddress { first, second } => write!(
                f,
                "parts {first} and {second} are of one kind and agree on `role`, `path` and `slot` \
                 together, so nothing can tell them apart"
            ),
            ErrorKind::SystemEntryBeforeItsDirectory { directory } => write!(
                f,
                "sits in `{directory}`, and no `system` part before it is that directory, so a writer \
                 has nowhere to put it"
            ),
            ErrorKind::DirentWrongLength { format, expected, found } => {
                write!(f, "runs {found} bytes, and a `{format}` card's entry is {expected}")
            }
            ErrorKind::SystemAreaWrongLength { format, expected, found } => {
                write!(f, "runs {found} bytes, and a `{format}` card's system area is {expected}")
            }
            ErrorKind::PathMisspelled { format, spelling } => {
                write!(f, "is not how a `{format}` card's save is spelled, which is {spelling}")
            }
            ErrorKind::RomHashesOutOfOrder => f.write_str("is not in ascending tag order"),
            ErrorKind::RomHashesDuplicateAlgorithm(algorithm) => {
                write!(f, "holds two {algorithm} digests, which name two different files")
            }
            ErrorKind::PayloadHashMismatch => {
                f.write_str("`sha256` does not equal the digest of the payload")
            }
            ErrorKind::BoundToWithoutBinding => f.write_str("carries `bound_to` without `binding`"),
            ErrorKind::BoundToNamesAnotherTarget { binding, key } => {
                write!(f, "`bound_to` key {key} names a target a `{binding}` binding does not bind to")
            }
            ErrorKind::RoleShared { first, second } => write!(
                f,
                "parts {first} and {second} share a role, which is all that tells a device's components apart"
            ),
            ErrorKind::CardRolesDisagree => f.write_str(
                "names a different `role` from the card's other parts; a card was read from one slot",
            ),
            ErrorKind::CardSlotOnASavePart(role) => write!(
                f,
                "names `{role}`, a memory card's slot; a save on a card is a save of its own, never a part of another"
            ),
            ErrorKind::KeyOutOfPlace { key, allowed } => {
                write!(f, "`{key}` belongs on {allowed} and nowhere else")
            }
            ErrorKind::N64ChipMissing => f.write_str(
                "is an N64 cartridge's save memory and does not carry `x.1sav.n64` to say which chip it is",
            ),
            ErrorKind::N64ChipUnknown(found) => {
                write!(f, "names no chip the `x.1sav.n64` page lists: {found}")
            }
            ErrorKind::N64PayloadWrongLength { chip, expected, found } => {
                write!(f, "runs {found} bytes, and the chip `{chip}` is {expected}")
            }
            ErrorKind::SecondCartridge { first, second } => write!(
                f,
                "parts {first} and {second} both have the role `cartridge`, and a cartridge has one save chip"
            ),
            ErrorKind::N64ddArea(why) => write!(f, "a 64DD disk's save {why}"),
            ErrorKind::N64ddWrongLength(len) => {
                write!(f, "runs {len} bytes, which is no 64DD disk type's RAM area")
            }
            ErrorKind::NdsWrongLength(len) => {
                write!(f, "runs {len} bytes, which is no DS game card's save chip")
            }
            ErrorKind::NestedPayloadNotABundle(source) => {
                write!(f, "carries kind `bundle` but its payload is not one: {source}")
            }
            ErrorKind::NestedHashMismatch => {
                f.write_str("`sha256` does not equal the content hash of the bundle it carries")
            }
            ErrorKind::OverLimit { limit, max } => write!(f, "{}", limit.describe(*max)),
        }
    }
}

impl From<NameError> for ErrorKind {
    fn from(source: NameError) -> Self {
        ErrorKind::Name(source)
    }
}

impl From<HashError> for ErrorKind {
    fn from(source: HashError) -> Self {
        ErrorKind::Hash(source)
    }
}
