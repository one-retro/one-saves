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

    /// A container was present but empty, where absence is the only spelling of empty.
    ///
    /// An empty `game`, `game_id`, `rom_hashes` or `parts` is malformed: nothing is absence, so
    /// spelling it out would give one bundle two encodings.
    EmptyContainer,

    /// A default was written out, where absence is the only spelling of the default.
    ///
    /// A part's `kind` means `save` when absent, its `role` means `primary`, its `path` means the
    /// role is the whole address, and its `encoding` means `"none"`.
    DefaultSpelledOut(&'static str),

    /// A `path` ran past 512 bytes.
    PathTooLong(usize),

    /// A `path` was empty, started with `/`, or carried a `..` segment.
    PathForbidden,

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

    /// `rom_hashes` was not in ascending tag order.
    RomHashesOutOfOrder,

    /// `rom_hashes` held two entries for one algorithm.
    ///
    /// Two digests of one algorithm name two different files, and nothing says which the bundle
    /// means.
    RomHashesDuplicateAlgorithm(HashAlgorithm),

    /// `size` was present on an embedded uncompressed payload, which states its own length.
    SizeOnUncompressedPart,

    /// `size` was absent from a compressed or referenced payload, where it cannot be derived.
    MissingSize,

    /// `encoding` named something other than `"zstd"`.
    UnknownEncoding(String),

    /// A part's `sha256` did not equal the digest of its payload.
    PayloadHashMismatch,

    /// An extension key placed somewhere its own specification does not allow.
    ///
    /// Each key names every placement it admits, and names it by shape, because a header and a
    /// part mean different things in each. The clock keys are a save's or a state's bundle header
    /// and nowhere else: a nested bundle inherits nothing from the header around it, so a reading
    /// put on a part would be dropped by the byte copy that extracting a save is. mGBA's cheat
    /// list is a state's `state` part and nowhere else, since it loads with the core's data.
    KeyOutOfPlace {
        /// The key.
        key: String,
        /// Where its specification does allow it.
        allowed: &'static str,
    },

    /// A part inside a nested bundle was compressed or referenced rather than embedded plain.
    ///
    /// Every inner part is uncompressed with its payload embedded, so an inner bundle's file
    /// hash and content hash are the same value.
    NestedPartNotNormalized,

    /// A `bundle` part's payload did not decode as a bundle.
    NestedPayloadNotABundle(Box<Error>),

    /// A `bundle` part's `sha256` did not equal the content hash of the bundle it carries.
    ///
    /// That equality is what makes extracting a save a byte copy rather than a re-encode.
    NestedHashMismatch,

    /// A payload was `zstd` and this build cannot inflate it.
    ZstdUnavailable,

    /// A `zstd` payload did not inflate, or inflated to something other than its stated `size`.
    ZstdInvalid(String),
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErrorKind::Encoding(why) => write!(f, "not deterministically encoded CBOR: {why}"),
            ErrorKind::NotABundle { found: Some(tag) } => {
                write!(f, "tag {tag} is not the bundle tag {}", crate::BUNDLE_TAG)
            }
            ErrorKind::NotABundle { found: None } => f.write_str("not wrapped in the bundle tag"),
            ErrorKind::Type { expected } => write!(f, "expected {expected}"),
            ErrorKind::MissingKey(name) => write!(f, "required key `{name}` is absent"),
            ErrorKind::UnknownShape(shape) => {
                write!(f, "shape `{shape}` is not defined by version {}", crate::SPEC_VERSION)
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
            ErrorKind::PathForbidden => f.write_str("is empty, starts with `/`, or carries a `..` segment"),
            ErrorKind::FloatEpoch => f.write_str("is a float; every epoch here is whole seconds"),
            ErrorKind::PartsOutOfOrder => f.write_str("parts are not in ascending `id` order"),
            ErrorKind::DuplicatePartId(id) => write!(f, "two parts share the id {id}"),
            ErrorKind::PartsShareAnAddress { first, second } => write!(
                f,
                "parts {first} and {second} are of one kind and agree on `role`, `path` and `slot` \
                 together, so nothing can tell them apart"
            ),
            ErrorKind::RomHashesOutOfOrder => f.write_str("is not in ascending tag order"),
            ErrorKind::RomHashesDuplicateAlgorithm(algorithm) => {
                write!(f, "holds two {algorithm} digests, which name two different files")
            }
            ErrorKind::SizeOnUncompressedPart => {
                f.write_str("carries `size`, which an embedded uncompressed payload already states")
            }
            ErrorKind::MissingSize => {
                f.write_str("needs `size`, which cannot be derived from a compressed or referenced payload")
            }
            ErrorKind::UnknownEncoding(found) => {
                write!(f, "`{found}` is not an encoding; only `zstd` is ever written")
            }
            ErrorKind::PayloadHashMismatch => {
                f.write_str("`sha256` does not equal the digest of the payload")
            }
            ErrorKind::KeyOutOfPlace { key, allowed } => {
                write!(f, "`{key}` belongs on {allowed} and nowhere else")
            }
            ErrorKind::NestedPartNotNormalized => {
                f.write_str("is inside a nested bundle and is not embedded uncompressed")
            }
            ErrorKind::NestedPayloadNotABundle(source) => {
                write!(f, "carries kind `bundle` but its payload is not one: {source}")
            }
            ErrorKind::NestedHashMismatch => {
                f.write_str("`sha256` does not equal the content hash of the bundle it carries")
            }
            ErrorKind::ZstdUnavailable => {
                f.write_str("is `zstd` and this build of one-saves has the `zstd` feature off")
            }
            ErrorKind::ZstdInvalid(why) => write!(f, "`zstd` payload is not readable: {why}"),
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
