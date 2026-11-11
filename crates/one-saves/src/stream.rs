//! Reading a bundle as it arrives, without holding it.
//!
//! [`Bundle::from_slice`](crate::Bundle::from_slice) wants the whole file in memory, and then builds
//! a CBOR tree of it, a [`Bundle`](crate::Bundle) out of that and a copy of every payload: several
//! times the file, before it has judged a byte of the header. A [`Reader`] reads the same file a
//! piece at a time, as it comes off a socket or a disk, and hands each payload on as it goes by
//! instead of keeping it. What it keeps is bounded by [`Limits`] the caller sets, whatever the
//! input.
//!
//! It is a push parser that does no I/O of its own: hand it whatever bytes have arrived, take the
//! events it has for them, and hand it more. Nothing in it blocks or awaits, so it runs the same
//! under an async runtime as off one, and this crate depends on no runtime.
//! [`Reader::read_from`] drives it from a [`std::io::Read`] for a caller that has one.
//!
//! # Why the format allows it
//!
//! A bundle is `tag(1SAV) [header, parts]`, so the header is whole before the first part begins.
//! Inside a part's map the keys are sorted bytewise on their encodings, which puts every unsigned
//! integer key ahead of `payload` at `-1`: `id` at 0 through `binding` at 13, and any a later
//! minor version assigns. So a part's `kind`, `encoding`, `size` and `sha256` are all in hand
//! before the first byte of its payload. What can follow the payload is a negative key a later
//! version assigns and the producer's own text keys. That is why a part's extension keys arrive in
//! [`PartEnd`](Event::PartEnd) rather than in [`PartStart`](Event::PartStart). The reader relies on
//! that ordering, which deterministic encoding fixes, and on nothing else about where a key sits:
//! in particular it does not take the payload to be a part's last key, which the specification
//! says nothing may depend on.
//!
//! # Driving it
//!
//! [`Reader::read`] takes the bytes that have arrived and returns a [`Step`]: how many of them it
//! took, and an event if it has one. Call it again with what it did not take until it returns no
//! event, which means it has taken everything and wants more. When the input ends, call
//! [`Reader::finish`], which is where the verdict on the whole bundle comes from.
//!
//! ```
//! use one_saves::stream::{Event, Limits, Reader};
//! use one_saves::{Bundle, Header, Part};
//!
//! let bytes = Bundle::new(Header::default(), vec![Part::new(0, *b"SAVE")]).to_vec()?;
//!
//! let mut reader = Reader::new(Limits::NONE.part_size(16 << 20));
//! // However the input arrives: here, three bytes at a time.
//! for chunk in bytes.chunks(3) {
//!     let mut input = chunk;
//!     loop {
//!         let step = reader.read(input)?;
//!         input = &input[step.consumed..];
//!         match step.event {
//!             Some(Event::Header { header, parts }) => println!("a {} of {parts} parts", header.shape),
//!             Some(Event::PartStart(head)) => println!("part {}: {} bytes", head.id, head.size),
//!             Some(Event::Data(bytes)) => { /* pass them on */ }
//!             Some(Event::PartEnd(_)) => { /* that payload checked out */ }
//!             Some(_) => {}
//!             None => break,
//!         }
//!     }
//! }
//! let done = reader.finish()?;
//! println!("content hash: {}", done.content_hash);
//! # Ok::<(), one_saves::Error>(())
//! ```
//!
//! # Events
//!
//! For a bundle of `n` parts, always in this order:
//!
//! 1. [`Header`](Event::Header), once, carrying the [`Header`] and `n`.
//! 2. For each part, in the order the bundle holds them:
//!    1. [`PartStart`](Event::PartStart), carrying a [`PartHead`]: every key of the part ahead of its
//!       payload, which is all of them but the extension keys. Its
//!       [`size`](PartHead::size) is the payload's uncompressed length and its
//!       [`sha256`](PartHead::sha256) the digest that length has to hash to.
//!    2. [`Data`](Event::Data), as many times as it takes: the payload, uncompressed, in order. The
//!       slices add up to exactly [`size`](PartHead::size), and an empty payload has none. Where
//!       the part is stored uncompressed each slice borrows the input it came in, so nothing is
//!       copied; where it is `zstd`, each is inflated into a buffer the reader owns.
//!    3. [`PartEnd`](Event::PartEnd), carrying a [`PartTail`]: the extension keys and any later
//!       version's negative keys, which follow the payload. By then the payload has been hashed and
//!       matched its `sha256`.
//! 3. Then no further event, and [`finish`](Reader::finish) returns [`Done`], with the bundle's file
//!    hash and content hash.
//!
//! An error ends it at any point: the reader returns it from every call after, and takes nothing
//! more.
//!
//! # Nothing counts until `Done`
//!
//! An event says what has been checked so far, not that the bundle is good. A [`Header`] that came
//! out fine, and a payload whose digest matched, still belong to a bundle that can be refused at
//! any later byte, and the specification is plain that a malformed bundle is refused whole: no part
//! of it is kept. So treat everything an event hands over as provisional, and file nothing until
//! [`finish`](Reader::finish) has returned [`Done`].
//!
//! # What is checked when
//!
//! Every check runs as soon as everything it needs has arrived, and a bundle is refused at the
//! first byte that shows it malformed.
//!
//! | When | What |
//! | --- | --- |
//! | each byte | Deterministic encoding, as `dcbor` holds [`from_slice`](crate::Bundle::from_slice) to it: shortest heads, definite lengths, the simple values and floats it admits, sorted keys and no duplicates. The bundle tag, a two-element envelope. Nesting no deeper than [`from_slice`](crate::Bundle::from_slice) reads ([`TooDeep`](ErrorKind::TooDeep)), counted on an explicit stack, never by recursion. |
//! | the header's last byte | The header decoded, and everything [`validate`](crate::Bundle::validate) says about a header on its own: what its shape admits, `game`, `taken_with`, where its extension keys sit, a card's `system_area`. The header is not handed over until the parts array's head follows it, a byte or so later. |
//! | the parts array's head | It holds at least one part, no more than [`Limits::parts`], and no more than one in a screenshot. |
//! | a payload's head | The part's keys decoded, and every rule about one part that does not need its bytes: ascending `id`s, what the shape admits, no two parts at one address, a `system` file after its directory, a card's `dirent` lengths and save names, which keys a `state`, a `screenshot` or a `system` part carries, `game`. Its `size`, and its length on the wire, against [`Limits::part_size`]. A part these refuse is refused here where nothing follows the payload in its map, and at the map's end where something does (see below). |
//! | each payload byte | A `zstd` payload abandoned the moment it inflates past its `size`. A nested bundle read as it arrives, by a reader of its own (see below). |
//! | the payload's last byte | Its SHA-256 against `sha256`. A `zstd` payload that stopped short of `size`, or did not end its frame. A nested bundle complete, and its hash. |
//! | the part map's last byte | The extension keys after the payload, and where they sit. |
//! | the parts array's last byte | A state holds a `state` part. Every rule has now run. |
//! | [`finish`](Reader::finish) | Nothing follows the bundle, and nothing of it is missing. |
//!
//! ## Nested bundles
//!
//! A card's saves, and a device's or a collection's entries, are bundles carried as the payload of
//! a [`bundle`](PartKind::Bundle) part. [`from_slice`](crate::Bundle::from_slice) decodes each one
//! and holds it to the rules of the bundle around it, and so does this reader. It reads each as it
//! arrives, with a reader of its own whose events it keeps to itself, so a nested bundle costs a
//! header's worth of memory rather than its length. Its bytes are handed over in
//! [`Data`](Event::Data) like any other payload's. To read an entry's own header and parts, keep
//! its bytes and hand them to [`from_slice`](crate::Bundle::from_slice), which will accept them:
//! the reader has already checked them the same way.
//!
//! # The verdict
//!
//! The reader accepts a bundle exactly when [`from_slice`](crate::Bundle::from_slice) does and
//! every part's [`verify`](crate::Part::verify) returns `true`, at the same
//! [`Strictness`]. It refuses everything else, with the same [`Error`],
//! [`ErrorKind`] and path. The [`Header`] and the parts' keys it hands over are the ones
//! [`from_slice`](crate::Bundle::from_slice) would have built: they come out of the same code.
//!
//! The second half is the one difference, and it is what reading every payload buys. A part's
//! `sha256` is over its uncompressed payload, and [`from_slice`](crate::Bundle::from_slice) leaves
//! that to whoever reads the payload, since nothing obliges a consumer to read a part; it checks
//! the digest only on a nested bundle, whose identity it is. This reader hands every payload over,
//! so it checks every digest: a mismatch is
//! [`PayloadHashMismatch`](ErrorKind::PayloadHashMismatch), and a `zstd` payload that does not
//! inflate to its `size` is [`ZstdInvalid`](ErrorKind::ZstdInvalid), both at `parts[i]`. In a build
//! without the `zstd` feature a compressed part is
//! [`ZstdUnavailable`](ErrorKind::ZstdUnavailable) as soon as its head is read, since its bytes
//! cannot be handed over.
//!
//! A [limit](Limits) refuses more again, with [`OverLimit`](ErrorKind::OverLimit). With
//! [`Limits::NONE`], the reader's verdict is exactly the one above.
//!
//! ## Which error, when there are several
//!
//! A bundle with one thing wrong with it is refused with the error
//! [`from_slice`](crate::Bundle::from_slice) gives. One with several can be refused with a
//! different one of them: the reader reports the first fault in the order the bytes arrive, which
//! is what lets it refuse a header before its parts have arrived, while
//! [`from_slice`](crate::Bundle::from_slice) checks the whole file's encoding first and its rules
//! second. Both refuse it.
//!
//! Holding to that costs one wait. A part's keys are judged at its payload's head because every
//! unsigned key sorts ahead of the payload, but a key out of order can follow it, and then what the
//! part seems to lack at the head is only late. That is one fault, out of order, and the error
//! [`from_slice`](crate::Bundle::from_slice) names. So a part refused at its payload's head whose
//! map goes on past the payload is not refused until the map's end: its payload is passed over,
//! neither handed on nor kept, and the rest of the map read. Most parts carry nothing after the
//! payload, and those are refused at its head.
//!
//! One count can differ too. Bytes after the end of a bundle are
//! [`Encoding`](ErrorKind::Encoding) with dcbor's `UnusedData`, refused the moment they arrive. The
//! reader cannot wait to count all of them without reading the rest of the input, so it counts
//! those in the call that brought the first. That is all of them when the input arrives in one
//! piece.
//!
//! # Limits
//!
//! Four of the [`Limits`] bound what an attacker can make the reader hold, and one bounds what a
//! `zstd` payload can make it allocate. Each length or count is judged when it is read, before any
//! of what it announces: a payload whose head declares more than [`part_size`](Limits::part_size)
//! is refused there, without a byte of it having arrived, and so is a `zstd` part whose `size`
//! does. A string in the header that would take the header past
//! [`header_bytes`](Limits::header_bytes) is refused at its head, and so is a map or an array that
//! declares more items than the bytes left could hold.
//!
//! A limit is the caller's, not the specification's, so a bundle refused for one may still be well
//! formed; [`OverLimit`](ErrorKind::OverLimit) says which limit, and the path says where.
//!
//! # Memory
//!
//! With limits set, what the reader holds at any moment is at most:
//!
//! - the header, until [`Header`](Event::Header) hands it over: [`header_bytes`](Limits::header_bytes);
//! - the current part's keys, until [`PartStart`](Event::PartStart) and [`PartEnd`](Event::PartEnd)
//!   hand them over: a small multiple of [`part_metadata_bytes`](Limits::part_metadata_bytes),
//!   since their bytes are kept beside their decoding until the payload's head;
//! - one entry per part gone by, for the rules that compare parts: an `id`, an address (`role`,
//!   `path` of at most 512 bytes, `slot`), and a `system` part's path, so a few hundred bytes times
//!   [`parts`](Limits::parts);
//! - for a `zstd` part, the decoder's window, which the frame sets and
//!   [`zstd_window`](Limits::zstd_window) caps, and a 128 KiB buffer for what it inflates;
//! - for a nested bundle, the same again for the reader that reads it. Nesting is bounded by the
//!   shapes rather than by a counter: a card holds saves, a device holds cards, a collection holds
//!   devices, and a save holds no bundle, so there are at most three of them.
//!
//! An uncompressed payload is never held at all.
//!
//! # Threads
//!
//! A reader holds the header and keys it has decoded, so like a [`Bundle`](crate::Bundle) it is
//! [`Send`] only with the `multithread` feature on. Turn it on to hold a reader across an `.await`
//! in a future that has to be `Send`.
//!
//! # A writer
//!
//! There is none yet, and the events are shaped so that one can consume them. A deterministic
//! encoding is definite-length, so a writer has to know a count before it writes what it counts:
//! [`Header`](Event::Header) carries the number of parts, and [`PartHead`] carries everything ahead
//! of the payload, the payload's length and
//! [`keys_after_payload`](PartHead::keys_after_payload), which is what a writer needs to write a
//! part's map head before it has the keys that follow the payload. A part's keys and a header are
//! the same types [`from_slice`](crate::Bundle::from_slice) builds, and
//! [`PartHead::into_part`] puts a part back together.

use std::io;
use std::ops::Range;

use dcbor::prelude::*;
use sha2::{Digest, Sha256};

use crate::codec::{self, Fields, MAX_NESTING, PartKeys, Strictness};
use crate::error::{Error, ErrorKind};
use crate::extension::Extensions;
use crate::hash::HashValue;
use crate::model::{Game, Header, Part, PartKind, Payload, Source, UnknownKeys};
use crate::name::Slug;
use crate::shape::Shape;
use crate::validate::{self, Checker};

// ---- limits ------------------------------------------------------------------------------------

/// What a [`Reader`] will take, each as a maximum.
///
/// Start from [`Limits::NONE`] and set the ones you want:
///
/// ```
/// use one_saves::stream::Limits;
///
/// let limits = Limits::NONE
///     .header_bytes(64 << 10)
///     .parts(64)
///     .part_size(64 << 20)
///     .part_metadata_bytes(64 << 10)
///     .zstd_window(8 << 20);
/// ```
///
/// Every limit applies at every depth: a nested bundle is held to the same ones as the bundle
/// around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// The most bytes the header may take, from its map's head to its last byte.
    pub header_bytes: u64,
    /// The most parts a bundle may hold.
    pub parts: u64,
    /// The most bytes a part's payload may come to: its uncompressed `size`, and its length on the
    /// wire as well where it is compressed.
    pub part_size: u64,
    /// The most bytes a part's map may take besides its payload's bytes: its keys, their values,
    /// and the payload's head.
    pub part_metadata_bytes: u64,
    /// The largest window a `zstd` frame may make the reader's decoder keep, in bytes.
    ///
    /// A frame states its own window, up to 128 MiB by zstd's default and a frame that wants more
    /// is refused by [`Part::bytes`](crate::Part::bytes) as by this reader, so that is
    /// the most this can be: a larger value is taken as 128 MiB. Below it, a frame that wants
    /// more is refused with [`OverLimit`](ErrorKind::OverLimit). Every frame zstd writes at level
    /// 19 or below, at any size, wants 8 MiB or less, unless it was told to match over long
    /// distances.
    pub zstd_window: u64,
}

/// zstd's own cap on a frame's window, which is what [`Part::bytes`](crate::Part::bytes) holds a
/// frame to.
const ZSTD_DEFAULT_WINDOW: u64 = 1 << 27;

impl Limits {
    /// No limits but zstd's own on a window, so the reader refuses exactly what
    /// [`from_slice`](crate::Bundle::from_slice) and [`Part::verify`](crate::Part::verify) refuse.
    ///
    /// Without limits the reader holds as much as the input makes it, as
    /// [`from_slice`](crate::Bundle::from_slice) does. Set them for input you do not trust.
    pub const NONE: Limits = Limits {
        header_bytes: u64::MAX,
        parts: u64::MAX,
        part_size: u64::MAX,
        part_metadata_bytes: u64::MAX,
        zstd_window: ZSTD_DEFAULT_WINDOW,
    };

    /// Sets the most bytes the header may take.
    #[must_use]
    pub const fn header_bytes(mut self, max: u64) -> Self {
        self.header_bytes = max;
        self
    }

    /// Sets the most parts a bundle may hold.
    #[must_use]
    pub const fn parts(mut self, max: u64) -> Self {
        self.parts = max;
        self
    }

    /// Sets the most bytes a part's payload may come to.
    #[must_use]
    pub const fn part_size(mut self, max: u64) -> Self {
        self.part_size = max;
        self
    }

    /// Sets the most bytes a part's map may take besides its payload's bytes.
    #[must_use]
    pub const fn part_metadata_bytes(mut self, max: u64) -> Self {
        self.part_metadata_bytes = max;
        self
    }

    /// Sets the largest window a `zstd` frame may make the decoder keep.
    #[must_use]
    pub const fn zstd_window(mut self, max: u64) -> Self {
        self.zstd_window = max;
        self
    }
}

/// Which of the [`Limits`] a bundle went past.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Limit {
    /// [`Limits::header_bytes`].
    HeaderBytes,
    /// [`Limits::parts`].
    Parts,
    /// [`Limits::part_size`].
    PartSize,
    /// [`Limits::part_metadata_bytes`].
    PartMetadataBytes,
    /// [`Limits::zstd_window`].
    ZstdWindow,
}

impl Limit {
    /// What going past this limit, set to `max`, reads as.
    pub(crate) fn describe(self, max: u64) -> String {
        match self {
            Limit::HeaderBytes => format!("runs past the {max} bytes the reader takes for a header"),
            Limit::Parts => format!("holds more than the {max} parts the reader takes"),
            Limit::PartSize => format!("runs past the {max} bytes the reader takes for a payload"),
            Limit::PartMetadataBytes => {
                format!("runs past the {max} bytes the reader takes for a part's keys")
            }
            Limit::ZstdWindow => format!("needs a zstd window past the {max} bytes the reader allows"),
        }
    }
}

impl std::fmt::Display for Limit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Limit::HeaderBytes => "header bytes",
            Limit::Parts => "parts",
            Limit::PartSize => "part size",
            Limit::PartMetadataBytes => "part metadata bytes",
            Limit::ZstdWindow => "zstd window",
        })
    }
}

// ---- events ------------------------------------------------------------------------------------

/// What a [`Reader`] has found in the bytes it was handed.
///
/// See the [module documentation](self) for the order they come in and what has been checked by
/// each.
#[derive(Debug)]
#[non_exhaustive]
pub enum Event<'a> {
    /// The header, decoded and checked on its own, and how many parts follow it.
    Header {
        /// The header, exactly as [`from_slice`](crate::Bundle::from_slice) would have read it.
        header: Header,
        /// How many parts the bundle holds, which is how many
        /// [`PartStart`](Event::PartStart)s follow. Never zero.
        parts: u64,
    },
    /// A part's keys ahead of its payload, checked against the header and the parts before it.
    PartStart(PartHead),
    /// The next bytes of the current part's payload, uncompressed.
    ///
    /// They borrow the input where the part is stored uncompressed, and the reader's own buffer
    /// where it is `zstd`; either way they are good until the next call.
    Data(&'a [u8]),
    /// The keys after the current part's payload, which has been hashed and matched its `sha256`.
    PartEnd(PartTail),
}

/// Everything a part says ahead of its payload.
///
/// That is every key but the extension keys and any negative key a later version assigns, which
/// follow the payload and arrive in [`PartTail`]. The fields are [`Part`]'s, read by the same code.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct PartHead {
    /// Where the part sits in the bundle, counting from zero: `parts[index]` in an error's path.
    pub index: u64,
    /// See [`Part::id`].
    pub id: u64,
    /// See [`Part::kind`].
    pub kind: PartKind,
    /// See [`Part::role`].
    pub role: Option<Slug>,
    /// See [`Part::path`].
    pub path: Option<String>,
    /// See [`Part::slot`].
    pub slot: Option<u64>,
    /// See [`Part::dirent`].
    pub dirent: Option<Vec<u8>>,
    /// See [`Part::content_type`].
    pub content_type: Option<String>,
    /// SHA-256 over the uncompressed payload, which the [`Data`](Event::Data) that follow are
    /// checked against before [`PartEnd`](Event::PartEnd).
    pub sha256: HashValue,
    /// See [`Part::source`].
    pub source: Option<Source>,
    /// See [`Part::game`].
    pub game: Option<Game>,
    /// See [`Part::system`].
    pub system: Option<Slug>,
    /// See [`Part::binding`].
    pub binding: Option<Slug>,
    /// How many bytes the payload comes to uncompressed, which is what the
    /// [`Data`](Event::Data) that follow add up to.
    pub size: u64,
    /// How many bytes the payload takes on the wire, where it is compressed with `zstd`; `None`
    /// where it is stored as it is, and takes [`size`](PartHead::size).
    pub compressed: Option<u64>,
    /// Integer keys from a later minor version that sort ahead of the payload: the unsigned ones.
    pub unknown: UnknownKeys,
    /// How many keys follow the payload in the part's map, which is how many
    /// [`PartTail`] will hold between its extensions and unknown keys.
    pub keys_after_payload: u64,
}

impl PartHead {
    /// A part's keys, with its payload's length and what follows it still to be filled in.
    fn of(index: u64, part: Part) -> Self {
        let Part {
            id,
            kind,
            role,
            path,
            slot,
            dirent,
            content_type,
            sha256,
            source,
            game,
            system,
            binding,
            ..
        } = part;
        PartHead {
            index,
            id,
            kind,
            role,
            path,
            slot,
            dirent,
            content_type,
            sha256,
            source,
            game,
            system,
            binding,
            size: 0,
            compressed: None,
            unknown: UnknownKeys::new(),
            keys_after_payload: 0,
        }
    }

    /// The whole part, given what followed its payload and the payload itself.
    #[must_use]
    pub fn into_part(self, tail: PartTail, payload: Payload) -> Part {
        let mut unknown = self.unknown;
        unknown.extend(tail.unknown);
        Part {
            id: self.id,
            kind: self.kind,
            role: self.role,
            path: self.path,
            slot: self.slot,
            dirent: self.dirent,
            content_type: self.content_type,
            sha256: self.sha256,
            source: self.source,
            game: self.game,
            system: self.system,
            binding: self.binding,
            payload,
            extensions: tail.extensions,
            unknown,
        }
    }
}

/// What follows a part's payload in its map.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct PartTail {
    /// See [`Part::extensions`].
    pub extensions: Extensions,
    /// Integer keys from a later minor version that sort after the payload: the negative ones.
    pub unknown: UnknownKeys,
}

/// A bundle read to its end and accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Done {
    /// The SHA-256 of the bytes read, which is [`Bundle::file_hash`](crate::Bundle::file_hash).
    pub file_hash: HashValue,
    /// [`Bundle::content_hash`](crate::Bundle::content_hash): the SHA-256 of the bundle with every
    /// part uncompressed, taken as it went by. The same as `file_hash` where no part is
    /// compressed.
    pub content_hash: HashValue,
}

/// What one call to [`Reader::read`] did.
#[derive(Debug)]
#[non_exhaustive]
pub struct Step<'a> {
    /// How many bytes from the front of the input the reader took. Hand it the rest next time.
    pub consumed: usize,
    /// What it found, or `None` when it has taken the whole input and wants more.
    pub event: Option<Event<'a>>,
}

// ---- the reader --------------------------------------------------------------------------------

/// A bundle read as it arrives.
///
/// See the [module documentation](self).
pub struct Reader {
    limits: Limits,
    strictness: Strictness,
    /// Where this reader sits: inside a `bundle` part, or at the top of a file when `None`.
    nested: Option<Nested>,
    stage: Stage,
    /// A structural head read across calls.
    head: HeadBuf,
    /// How many bytes the reader has taken.
    position: u64,
    /// Holds the parts to the header's shape. `None` until the header is read, and for a nested
    /// bundle that is not held to its rules.
    checker: Option<Checker>,
    /// The header, between its map's end and the parts array's head.
    header: Option<Header>,
    /// How many parts the bundle declares.
    parts: u64,
    /// The part being read.
    index: u64,
    part: Option<Box<PartState>>,
    /// The file hash and the content hash, at the top only.
    hashes: Option<Hashes>,
    /// What refused the bundle.
    failed: Option<Error>,
    /// What [`finish`](Reader::finish) returned.
    done: Option<Done>,
}

impl std::fmt::Debug for Reader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reader")
            .field("limits", &self.limits)
            .field("strictness", &self.strictness)
            .field("position", &self.position)
            .field("index", &self.index)
            .field("parts", &self.parts)
            .field("failed", &self.failed)
            .finish_non_exhaustive()
    }
}

/// What a reader inside a `bundle` part knows of the bundle around it.
struct Nested {
    /// The outer bundle's shape, which decides whether this one is held to its rules.
    outer: Shape,
    /// The outer card's format, which this bundle's entries are held to as well.
    card_format: Option<String>,
    /// The length of the payload this bundle is, which says how much follows it if it ends early.
    total: u64,
}

/// Where in a bundle a reader is.
enum Stage {
    /// The bundle tag's head.
    Tag,
    /// The envelope array's head.
    Envelope,
    /// The header's map, buffered whole.
    Header { item: Item, budget: Budget },
    /// The parts array's head.
    Parts,
    /// A part's map head.
    PartMap,
    /// A key in a part's map.
    Key(Item),
    /// A value in a part's map, other than the payload.
    Value(Item),
    /// The payload's head.
    PayloadHead,
    /// The payload's bytes.
    Payload,
    /// The payload's bytes, passed over: the part's keys were refused, and the rest of its map is
    /// still to be read.
    Skip { left: u64 },
    /// A `zstd` payload's last output, after its last input.
    #[cfg(feature = "zstd")]
    Drain,
    /// The payload's checks, once it has all gone by.
    PayloadEnd,
    /// A part's map, read to its end.
    PartEnd,
    /// The bundle, read to its end.
    Trailing,
    /// Between steps, or after a failure.
    Taken,
}

/// One part, while it is being read.
struct PartState {
    /// `parts[index]`.
    path: String,
    /// How many entries the map declares.
    entries: u64,
    /// How many of them have been read whole.
    read: u64,
    /// The keys read so far, as encoded, which the next has to sort after.
    keys: Vec<Vec<u8>>,
    /// Keys ahead of the payload, or every key where there is none.
    fields: Fields,
    /// Keys after the payload.
    trailing: Fields,
    /// The key whose value is being read.
    key: Option<CBOR>,
    budget: Budget,
    /// The map's bytes ahead of the payload, held back from the hashes until it is known whether the
    /// content hash takes them as they are. At the top only.
    pre: Vec<u8>,
    /// How long the map's head is, at the front of `pre`.
    map_head: usize,
    /// Where the current entry starts in `pre`.
    entry_start: usize,
    /// The entries in `pre` the content hash leaves out of a compressed part: `encoding` and `size`.
    skip: Vec<Range<usize>>,
    /// Set from the payload's head on.
    payload: Option<PayloadState>,
    /// What the part's keys were refused for at its payload's head, while the rest of its map is
    /// read to see whether a key out of order is the fault instead.
    settling: Option<Fault>,
}

/// A payload, while it goes by.
struct PayloadState {
    kind: PartKind,
    sha256: HashValue,
    /// Bytes still to come on the wire.
    left: u64,
    /// The digest being taken of the uncompressed bytes, where one is checked.
    hasher: Option<Sha256>,
    /// Whether a mismatch is a nested bundle's, which `validate` names differently.
    nested_bundle: bool,
    /// The reader reading the bundle this payload is.
    child: Option<Box<Reader>>,
    #[cfg(feature = "zstd")]
    inflater: Option<inflate::Inflater>,
}

/// The file hash and the content hash, as they are taken.
struct Hashes {
    file: Sha256,
    /// Split off the file hash at the first compressed part, which the two see differently.
    content: Option<Sha256>,
}

impl Hashes {
    fn update(&mut self, bytes: &[u8]) {
        self.file.update(bytes);
        if let Some(content) = &mut self.content {
            content.update(bytes);
        }
    }

    /// Takes in a part's map up to and including its payload's head, which is where it is first
    /// known whether the content hash sees them as they are. `size` is the uncompressed length of a
    /// compressed payload.
    fn part_head(&mut self, part: &mut PartState, raw: &[u8], size: Option<u64>) {
        if let Some(size) = size {
            // The content hash is over the bundle with this part uncompressed: its map without
            // `encoding` and `size`, and its payload at its uncompressed length. It parts from the
            // file hash at the first such part, and the bytes before it are the two's alike.
            let content = self.content.get_or_insert_with(|| self.file.clone());
            let (map_head, len) = encode_head(5, part.entries - 2);
            content.update(&map_head[..len]);
            let mut from = part.map_head;
            for skip in &part.skip {
                content.update(&part.pre[from..skip.start]);
                from = skip.end;
            }
            content.update(&part.pre[from..]);
            let (payload_head, len) = encode_head(2, size);
            content.update(&payload_head[..len]);
        } else if let Some(content) = &mut self.content {
            content.update(&part.pre);
            content.update(raw);
        }
        self.file.update(&part.pre);
        self.file.update(raw);
        part.pre = Vec::new();
    }
}

/// What one step found, held without borrowing the input so a failure can be recorded.
enum Progress {
    NeedMore,
    Header {
        header: Header,
        parts: u64,
    },
    PartStart(PartHead),
    PartEnd(PartTail),
    /// Payload bytes, at this range of the input.
    Data(Range<usize>),
    /// Inflated payload bytes, at the front of the inflater's buffer.
    #[cfg(feature = "zstd")]
    Inflated(usize),
}

impl Reader {
    /// A reader for one bundle, as a shipped decoder reads it: [`Strictness::Decoder`].
    #[must_use]
    pub fn new(limits: Limits) -> Self {
        Reader::with_strictness(limits, Strictness::Decoder)
    }

    /// A reader for one bundle, at a chosen [`Strictness`].
    #[must_use]
    pub fn with_strictness(limits: Limits, strictness: Strictness) -> Self {
        Reader {
            limits,
            strictness,
            nested: None,
            stage: Stage::Tag,
            head: HeadBuf::default(),
            position: 0,
            checker: None,
            header: None,
            parts: 0,
            index: 0,
            part: None,
            hashes: Some(Hashes { file: Sha256::new(), content: None }),
            failed: None,
            done: None,
        }
    }

    /// A reader for the bundle a `bundle` part's payload is, as `validate` reads it.
    fn nested(limits: Limits, nested: Nested) -> Self {
        Reader { nested: Some(nested), hashes: None, ..Reader::with_strictness(limits, Strictness::Decoder) }
    }

    /// Takes bytes from the front of `input`, and returns how many it took and what it found.
    ///
    /// Call it again with the rest of `input` until it returns no event, which means it has taken
    /// all of it; then call it with the next input that arrives, or call
    /// [`finish`](Reader::finish) if there is none. It can return an event without taking a byte,
    /// so an empty `input` is worth handing it while it still has events.
    ///
    /// # Errors
    ///
    /// When the bytes show the bundle malformed, or over a [limit](Limits). Every call after
    /// returns the same error.
    pub fn read<'a>(&'a mut self, input: &'a [u8]) -> Result<Step<'a>, Error> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        let (consumed, progress) = match self.advance(input) {
            Ok(advanced) => advanced,
            Err(fault) => {
                self.failed = Some(fault.error.clone());
                return Err(fault.error);
            }
        };
        let event = match progress {
            Progress::NeedMore => None,
            Progress::Header { header, parts } => Some(Event::Header { header, parts }),
            Progress::PartStart(head) => Some(Event::PartStart(head)),
            Progress::PartEnd(tail) => Some(Event::PartEnd(tail)),
            Progress::Data(range) => Some(Event::Data(&input[range])),
            #[cfg(feature = "zstd")]
            Progress::Inflated(len) => Some(Event::Data(self.inflated(len))),
        };
        Ok(Step { consumed, event })
    }

    /// Says the input has ended, and returns the verdict on the whole bundle.
    ///
    /// Call it once [`read`](Reader::read) has taken the last of the input and returned no event.
    ///
    /// # Errors
    ///
    /// When the bundle was refused already, or when the input ended before it did.
    pub fn finish(&mut self) -> Result<Done, Error> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        if let Some(done) = &self.done {
            return Ok(done.clone());
        }
        if !matches!(self.stage, Stage::Trailing) {
            let error = Error::at("", ErrorKind::Encoding(dcbor::Error::Underrun.to_string()));
            self.failed = Some(error.clone());
            return Err(error);
        }
        let hashes = self.hashes.take().unwrap_or_else(|| Hashes { file: Sha256::new(), content: None });
        let content = hashes.content.unwrap_or_else(|| hashes.file.clone());
        let done = Done {
            file_hash: HashValue::sha256_from(hashes.file),
            content_hash: HashValue::sha256_from(content),
        };
        self.done = Some(done.clone());
        Ok(done)
    }

    /// Reads a whole bundle from `source`, handing each event to `each`.
    ///
    /// The convenience for a caller with a [`std::io::Read`]: it reads 64 KiB at a time and drives
    /// [`read`](Reader::read) and [`finish`](Reader::finish) as the module documentation shows.
    /// `E` is the caller's error type, which has to take both an I/O error and a bundle's, as
    /// `Box<dyn std::error::Error>` and `anyhow::Error` do.
    ///
    /// # Errors
    ///
    /// Whatever reading `source` fails with, whatever refuses the bundle, and whatever `each`
    /// returns, which stops the reading there.
    pub fn read_from<R, E>(
        mut self,
        mut source: R,
        mut each: impl FnMut(Event<'_>) -> Result<(), E>,
    ) -> Result<Done, E>
    where
        R: io::Read,
        E: From<Error> + From<io::Error>,
    {
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let filled = match source.read(&mut buffer) {
                Ok(0) => break,
                Ok(filled) => filled,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.into()),
            };
            let mut input = &buffer[..filled];
            loop {
                let step = self.read(input)?;
                input = &input[step.consumed..];
                match step.event {
                    Some(event) => each(event)?,
                    None => break,
                }
            }
        }
        Ok(self.finish()?)
    }

    #[cfg(feature = "zstd")]
    fn inflated(&self, len: usize) -> &[u8] {
        self.part
            .as_ref()
            .and_then(|part| part.payload.as_ref())
            .and_then(|payload| payload.inflater.as_ref())
            .map_or(&[][..], |inflater| &inflater.out()[..len])
    }

    /// Runs the reader over `input` until it has something to say or wants more.
    fn advance(&mut self, input: &[u8]) -> Result<(usize, Progress), Fault> {
        let mut at = 0;
        let result = self.run(input, &mut at);
        self.position += at as u64;
        result.map(|progress| (at, progress))
    }

    fn run(&mut self, input: &[u8], at: &mut usize) -> Result<Progress, Fault> {
        loop {
            let stage = std::mem::replace(&mut self.stage, Stage::Taken);
            let (next, progress) = self.on(stage, input, at)?;
            self.stage = next;
            if let Some(progress) = progress {
                return Ok(progress);
            }
        }
    }

    /// One step from one stage: the stage after it, and what it found, if anything.
    #[allow(clippy::too_many_lines)]
    fn on(&mut self, stage: Stage, input: &[u8], at: &mut usize) -> Result<(Stage, Option<Progress>), Fault> {
        match stage {
            Stage::Tag => {
                let Some((head, raw)) = self.head.read(input, at)? else { return want(Stage::Tag) };
                if head.major != 6 {
                    return Err(Fault::decode(Error::at("", ErrorKind::NotABundle { found: None })));
                }
                if head.argument != crate::BUNDLE_TAG {
                    return Err(Fault::decode(Error::at(
                        "",
                        ErrorKind::NotABundle { found: Some(head.argument) },
                    )));
                }
                self.digest(raw.as_slice());
                Ok((Stage::Envelope, None))
            }

            Stage::Envelope => {
                let Some((head, raw)) = self.head.read(input, at)? else { return want(Stage::Envelope) };
                if head.major != 4 || head.argument != 2 {
                    return Err(Fault::decode(Error::at(
                        "",
                        ErrorKind::Type { expected: "a two-element array" },
                    )));
                }
                self.digest(raw.as_slice());
                let item =
                    Item::new(2, Some((5, Error::at("header", ErrorKind::Type { expected: "a map" }))));
                let budget = Budget::new(self.limits.header_bytes, Limit::HeaderBytes, "header".to_owned());
                Ok((Stage::Header { item, budget }, None))
            }

            Stage::Header { mut item, mut budget } => {
                if !item.feed(input, at, &mut budget)? {
                    return want(Stage::Header { item, budget });
                }
                self.digest(&item.bytes);
                let cbor = codec::decode_cbor(&item.bytes).map_err(Fault::cbor)?;
                let header = codec::decode_header(&cbor, self.strictness).map_err(Fault::decode)?;
                match &self.nested {
                    // `validate` runs on every bundle read from a file.
                    None => self.checker = Some(Checker::new(&header).map_err(Fault::validate)?),
                    // And on a nested one only where both shapes are this version's; otherwise its
                    // rules are in a document this decoder has not seen.
                    Some(nested) => {
                        if nested.outer.is_known() && header.shape.is_known() {
                            validate::check_holds(&nested.outer, &header.shape)
                                .map_err(|kind| Fault::holder(Error::at("", kind)))?;
                            self.checker = Some(Checker::new(&header).map_err(Fault::validate)?);
                        }
                    }
                }
                self.header = Some(header);
                Ok((Stage::Parts, None))
            }

            Stage::Parts => {
                let Some((head, raw)) = self.head.read(input, at)? else { return want(Stage::Parts) };
                if head.major != 4 {
                    return Err(Fault::decode(Error::at("parts", ErrorKind::Type { expected: "an array" })));
                }
                if head.argument == 0 {
                    return Err(Fault::decode(Error::at("parts", ErrorKind::EmptyContainer)));
                }
                self.digest(raw.as_slice());
                if head.argument > self.limits.parts {
                    return Err(Fault::limit("parts", Limit::Parts, self.limits.parts));
                }
                if let Some(checker) = &self.checker {
                    checker.parts(head.argument).map_err(Fault::validate)?;
                }
                self.parts = head.argument;
                let header = self.header.take();
                match header {
                    Some(header) if self.nested.is_none() => {
                        Ok((Stage::PartMap, Some(Progress::Header { header, parts: head.argument })))
                    }
                    _ => Ok((Stage::PartMap, None)),
                }
            }

            Stage::PartMap => {
                let Some((head, raw)) = self.head.read(input, at)? else { return want(Stage::PartMap) };
                let path = format!("parts[{}]", self.index);
                if head.major != 5 {
                    return Err(Fault::decode(Error::at(&path, ErrorKind::Type { expected: "a map" })));
                }
                let mut budget =
                    Budget::new(self.limits.part_metadata_bytes, Limit::PartMetadataBytes, path.clone());
                budget.spend(raw.len as u64)?;
                budget.room_for(head.argument.saturating_mul(2))?;
                let mut pre = Vec::new();
                if self.hashes.is_some() {
                    pre.extend_from_slice(raw.as_slice());
                }
                self.part = Some(Box::new(PartState {
                    path,
                    entries: head.argument,
                    read: 0,
                    keys: Vec::new(),
                    fields: Fields::default(),
                    trailing: Fields::default(),
                    key: None,
                    budget,
                    pre,
                    map_head: raw.len,
                    entry_start: 0,
                    skip: Vec::new(),
                    payload: None,
                    settling: None,
                }));
                let next = if head.argument == 0 { Stage::PartEnd } else { Stage::Key(Item::new(4, None)) };
                Ok((next, None))
            }

            Stage::Key(mut item) => {
                let Some(part) = self.part.as_deref_mut() else { return Err(Fault::lost()) };
                if !item.feed(input, at, &mut part.budget)? {
                    return want(Stage::Key(item));
                }
                let key = codec::decode_cbor(&item.bytes).map_err(Fault::cbor)?;
                check_key_order(&part.keys, &item.bytes)?;
                let payload_key = part.payload.is_none() && matches!(key.as_case(), CBORCase::Negative(0));
                if part.payload.is_none() {
                    if self.hashes.is_some() {
                        part.entry_start = part.pre.len();
                        part.pre.extend_from_slice(&item.bytes);
                    }
                } else if let Some(hashes) = &mut self.hashes {
                    hashes.update(&item.bytes);
                }
                part.keys.push(item.bytes);
                if payload_key {
                    return Ok((Stage::PayloadHead, None));
                }
                part.key = Some(key);
                Ok((Stage::Value(Item::new(4, None)), None))
            }

            Stage::Value(mut item) => {
                let Some(part) = self.part.as_deref_mut() else { return Err(Fault::lost()) };
                if !item.feed(input, at, &mut part.budget)? {
                    return want(Stage::Value(item));
                }
                let value = codec::decode_cbor(&item.bytes).map_err(Fault::cbor)?;
                let Some(key) = part.key.take() else { return Err(Fault::lost()) };
                if part.payload.is_none() {
                    if self.hashes.is_some() {
                        part.pre.extend_from_slice(&item.bytes);
                        if matches!(key.as_case(), CBORCase::Unsigned(7 | 8)) {
                            part.skip.push(part.entry_start..part.pre.len());
                        }
                    }
                    part.fields.push(&key, value, &part.path).map_err(Fault::decode)?;
                } else {
                    if let Some(hashes) = &mut self.hashes {
                        hashes.update(&item.bytes);
                    }
                    part.trailing.push(&key, value, &part.path).map_err(Fault::decode)?;
                }
                part.read += 1;
                let next =
                    if part.read == part.entries { Stage::PartEnd } else { Stage::Key(Item::new(4, None)) };
                Ok((next, None))
            }

            Stage::PayloadHead => {
                let Some((head, raw)) = self.head.read(input, at)? else { return want(Stage::PayloadHead) };
                if let Some(part_head) = self.payload_head(head, raw.as_slice())? {
                    return Ok((Stage::Payload, Some(Progress::PartStart(part_head))));
                }
                // Refused, and settling which error to give: pass over the payload, unread.
                let Some(part) = self.part.as_deref_mut() else { return Err(Fault::lost()) };
                if head.major == 2 {
                    return Ok((Stage::Skip { left: head.argument }, None));
                }
                // A payload that is not a byte string is a value like any other, read whole.
                let mut item = Item::new(4, None);
                let mut unlimited = Budget::new(u64::MAX, Limit::PartMetadataBytes, String::new());
                item.feed(raw.as_slice(), &mut 0, &mut unlimited)?;
                part.key = Some(CBOR::from(-1));
                Ok((Stage::Value(item), None))
            }

            Stage::Payload => self.payload(input, at),

            Stage::Skip { left } => {
                let take = usize::try_from(left).unwrap_or(usize::MAX).min(input.len() - *at);
                *at += take;
                let left = left - take as u64;
                if left > 0 {
                    return want(Stage::Skip { left });
                }
                let Some(part) = self.part.as_deref_mut() else { return Err(Fault::lost()) };
                // Its bytes are nothing to the error; that it is a byte string is.
                part.fields
                    .push(&CBOR::from(-1), CBOR::to_byte_string([]), &part.path)
                    .map_err(Fault::decode)?;
                part.read += 1;
                let next =
                    if part.read == part.entries { Stage::PartEnd } else { Stage::Key(Item::new(4, None)) };
                Ok((next, None))
            }

            #[cfg(feature = "zstd")]
            Stage::Drain => {
                let Some(part) = self.part.as_deref_mut() else { return Err(Fault::lost()) };
                let Some(payload) = part.payload.as_mut() else { return Err(Fault::lost()) };
                let Some(inflater) = payload.inflater.as_mut() else { return Err(Fault::lost()) };
                let made = inflater.drain().map_err(|kind| Fault::validate(Error::at(&part.path, kind)))?;
                if made > 0 {
                    let out = &inflater.out()[..made];
                    if let Some(hasher) = &mut payload.hasher {
                        hasher.update(out);
                    }
                    if let Some(content) = self.hashes.as_mut().and_then(|hashes| hashes.content.as_mut()) {
                        content.update(out);
                    }
                    return Ok((Stage::Drain, Some(Progress::Inflated(made))));
                }
                inflater.finish().map_err(|kind| Fault::validate(Error::at(&part.path, kind)))?;
                Ok((Stage::PayloadEnd, None))
            }
            Stage::PayloadEnd => {
                let Some(part) = self.part.as_deref_mut() else { return Err(Fault::lost()) };
                let Some(payload) = part.payload.as_mut() else { return Err(Fault::lost()) };
                if let Some(mut child) = payload.child.take() {
                    child.end().map_err(|fault| fault.nested_in(&part.path))?;
                }
                if let Some(hasher) = payload.hasher.take()
                    && HashValue::sha256_from(hasher) != payload.sha256
                {
                    let kind = if payload.nested_bundle {
                        ErrorKind::NestedHashMismatch
                    } else {
                        ErrorKind::PayloadHashMismatch
                    };
                    return Err(Fault::validate(Error::at(&part.path, kind)));
                }
                part.read += 1;
                let next =
                    if part.read == part.entries { Stage::PartEnd } else { Stage::Key(Item::new(4, None)) };
                Ok((next, None))
            }

            Stage::PartEnd => {
                let Some(part) = self.part.take() else { return Err(Fault::lost()) };
                let PartState { path, fields, trailing, payload, settling, .. } = *part;
                let Some(payload) = payload else {
                    // No payload handed over: whatever `from_slice` says about the part's keys,
                    // which is at the latest that it has no payload; or, where its keys were refused
                    // at the payload's head and none after it was out of order, what they were
                    // refused for.
                    codec::decode_part_fields(fields, &path, self.strictness).map_err(Fault::decode)?;
                    return Err(settling.unwrap_or_else(|| {
                        Fault::decode(Error::at(&path, ErrorKind::MissingKey("payload")))
                    }));
                };
                let extensions = trailing.extensions(&path).map_err(Fault::decode)?;
                let unknown = trailing.unknown(&path, self.strictness).map_err(Fault::decode)?;
                if self.checker.is_some() {
                    Checker::part_extensions(self.index, &payload.kind, &extensions)
                        .map_err(Fault::validate)?;
                }
                self.index += 1;
                let next = if self.index == self.parts {
                    if let Some(checker) = &self.checker {
                        checker.finish().map_err(Fault::validate)?;
                    }
                    Stage::Trailing
                } else {
                    Stage::PartMap
                };
                let progress =
                    self.nested.is_none().then_some(Progress::PartEnd(PartTail { extensions, unknown }));
                Ok((next, progress))
            }

            Stage::Trailing => {
                if *at == input.len() {
                    return want(Stage::Trailing);
                }
                let extra = match &self.nested {
                    Some(nested) => nested.total.saturating_sub(self.position + *at as u64),
                    None => (input.len() - *at) as u64,
                };
                let extra = usize::try_from(extra).unwrap_or(usize::MAX);
                Err(Fault::dcbor(&dcbor::Error::UnusedData(extra)))
            }

            Stage::Taken => Err(Fault::lost()),
        }
    }

    /// Judges a part at its payload's head, and readies what reads and checks the payload; or,
    /// where the part is refused and its map goes on past the payload, says so with `None`.
    ///
    /// Every unsigned key sorts ahead of the payload, so its head is where a part's keys are all in
    /// hand, unless one is out of order and comes after it. That is an encoding fault, and the one
    /// `from_slice` names, since it reads a file's encoding before anything it says. So a part
    /// refused here for its keys, whose map has more after the payload, is held in
    /// [`settling`](PartState::settling) until the map's end shows whether a key out of order is
    /// what it lacked; its payload is passed over rather than handed on. Where nothing follows the
    /// payload, nothing can be out of order after it, and the part is refused here.
    fn payload_head(&mut self, head: Head, raw: &[u8]) -> Result<Option<PartHead>, Fault> {
        let fault = match self.judge_part(head, raw) {
            Ok(part_head) => return Ok(Some(part_head)),
            Err(fault) if matches!(fault.phase, Phase::Decode | Phase::Validate) => fault,
            Err(fault) => return Err(fault),
        };
        let Some(part) = self.part.as_deref_mut() else { return Err(Fault::lost()) };
        if part.entries - part.read == 1 {
            return Err(fault);
        }
        // Passed over or not, a payload past the limit is not read.
        if head.major == 2 && head.argument > self.limits.part_size {
            return Err(Fault::limit(&part.path, Limit::PartSize, self.limits.part_size));
        }
        part.settling = Some(fault);
        Ok(None)
    }

    /// Judges a part at its payload's head, which is the last moment before its bytes: decodes its
    /// keys, checks them, and readies what reads and checks the payload.
    fn judge_part(&mut self, head: Head, raw: &[u8]) -> Result<PartHead, Fault> {
        let Some(part) = self.part.as_deref_mut() else { return Err(Fault::lost()) };
        part.budget.spend(raw.len() as u64)?;
        let path = part.path.clone();
        // On a copy, so a part refused here still has every key for its map's end.
        let mut fields = part.fields.clone();

        if head.major != 2 {
            // What `from_slice` says about a payload that is not a byte string, after what it
            // checks first.
            codec::decode_part_keys(&mut fields, &path, self.strictness).map_err(Fault::decode)?;
            return Err(Fault::decode(Error::at(
                format!("{path}.payload"),
                ErrorKind::Type { expected: "a byte string" },
            )));
        }
        let wire = head.argument;
        let PartKeys { part: shell, encoding, size } =
            codec::decode_part_keys(&mut fields, &path, self.strictness).map_err(Fault::decode)?;
        let compressed = codec::payload_form(encoding.as_deref(), size, &path).map_err(Fault::decode)?;
        let unknown = fields.unknown(&path, self.strictness).map_err(Fault::decode)?;

        let size = compressed.unwrap_or(wire);
        if size > self.limits.part_size || wire > self.limits.part_size {
            return Err(Fault::limit(&path, Limit::PartSize, self.limits.part_size));
        }
        if let Some(nested) = &self.nested {
            // Every part of a nested bundle is stored as it is, so its file hash and its content
            // hash are one value.
            if compressed.is_some() {
                return Err(Fault::validate(Error::at(&path, ErrorKind::NestedPartNotNormalized)));
            }
            if self.checker.is_some()
                && let Some(format) = &nested.card_format
            {
                validate::check_dirent(&shell, format, &path).map_err(Fault::validate)?;
            }
        }
        if let Some(checker) = &mut self.checker {
            checker.part(self.index, &shell).map_err(Fault::validate)?;
        }

        let top = self.nested.is_none();
        let mut payload = PayloadState {
            kind: shell.kind.clone(),
            sha256: shell.sha256.clone(),
            left: wire,
            // At the top every digest is checked, since every payload is handed over.
            hasher: top.then(Sha256::new),
            nested_bundle: false,
            child: None,
            #[cfg(feature = "zstd")]
            inflater: None,
        };
        if compressed.is_some() && top {
            #[cfg(feature = "zstd")]
            {
                payload.inflater = Some(
                    inflate::Inflater::new(size, self.limits.zstd_window)
                        .map_err(|kind| Fault::validate(Error::at(&path, kind)))?,
                );
            }
            #[cfg(not(feature = "zstd"))]
            return Err(Fault::validate(Error::at(&path, ErrorKind::ZstdUnavailable)));
        }
        // `validate` reads a stored `bundle` part's payload as a bundle and checks its digest, and
        // leaves a compressed one to whoever inflates it.
        if shell.kind == PartKind::Bundle
            && compressed.is_none()
            && let Some(checker) = &self.checker
        {
            payload.child = Some(Box::new(Reader::nested(
                self.limits,
                Nested {
                    outer: checker.shape().clone(),
                    card_format: checker.card_format().map(str::to_owned),
                    total: wire,
                },
            )));
            payload.hasher = Some(Sha256::new());
            payload.nested_bundle = true;
        }

        if let Some(hashes) = &mut self.hashes {
            hashes.part_head(part, raw, compressed.is_some().then_some(size));
        }

        let keys_after_payload = part.entries - part.read - 1;
        part.fields = Fields::default();
        part.payload = Some(payload);
        Ok(PartHead {
            size,
            compressed: compressed.map(|_| wire),
            unknown,
            keys_after_payload,
            ..PartHead::of(self.index, shell)
        })
    }

    /// Hands on the next payload bytes.
    fn payload(&mut self, input: &[u8], at: &mut usize) -> Result<(Stage, Option<Progress>), Fault> {
        let Some(part) = self.part.as_deref_mut() else { return Err(Fault::lost()) };
        let Some(payload) = part.payload.as_mut() else { return Err(Fault::lost()) };

        #[cfg(feature = "zstd")]
        if let Some(inflater) = payload.inflater.as_mut() {
            loop {
                if payload.left == 0 {
                    return Ok((Stage::Drain, None));
                }
                if *at == input.len() {
                    return Ok((Stage::Payload, Some(Progress::NeedMore)));
                }
                let take = usize::try_from(payload.left).unwrap_or(usize::MAX).min(input.len() - *at);
                let chunk = &input[*at..*at + take];
                let (used, made) =
                    inflater.step(chunk).map_err(|kind| Fault::validate(Error::at(&part.path, kind)))?;
                if let Some(hashes) = &mut self.hashes {
                    hashes.file.update(&chunk[..used]);
                }
                *at += used;
                payload.left -= used as u64;
                if made > 0 {
                    let out = &inflater.out()[..made];
                    if let Some(hasher) = &mut payload.hasher {
                        hasher.update(out);
                    }
                    if let Some(content) = self.hashes.as_mut().and_then(|hashes| hashes.content.as_mut()) {
                        content.update(out);
                    }
                    return Ok((Stage::Payload, Some(Progress::Inflated(made))));
                }
            }
        }

        if payload.left == 0 {
            return Ok((Stage::PayloadEnd, None));
        }
        if *at == input.len() {
            return Ok((Stage::Payload, Some(Progress::NeedMore)));
        }
        let take = usize::try_from(payload.left).unwrap_or(usize::MAX).min(input.len() - *at);
        let range = *at..*at + take;
        let chunk = &input[range.clone()];
        *at += take;
        payload.left -= take as u64;
        if let Some(hashes) = &mut self.hashes {
            hashes.update(chunk);
        }
        if let Some(hasher) = &mut payload.hasher {
            hasher.update(chunk);
        }
        if let Some(child) = &mut payload.child {
            child.feed(chunk).map_err(|fault| fault.nested_in(&part.path))?;
        }
        if self.nested.is_some() {
            // A nested reader's events are its own, so it carries on without handing them over.
            return Ok((Stage::Payload, None));
        }
        Ok((Stage::Payload, Some(Progress::Data(range))))
    }

    /// Feeds a nested reader the whole of `chunk`, keeping its events to itself.
    fn feed(&mut self, mut chunk: &[u8]) -> Result<(), Fault> {
        while !chunk.is_empty() {
            let (used, _) = self.advance(chunk)?;
            chunk = &chunk[used..];
        }
        Ok(())
    }

    /// Says a nested reader's payload has ended: it has to have read a whole bundle.
    fn end(&mut self) -> Result<(), Fault> {
        while !matches!(self.advance(&[])?, (_, Progress::NeedMore)) {}
        if matches!(self.stage, Stage::Trailing) {
            Ok(())
        } else {
            Err(Fault::dcbor(&dcbor::Error::Underrun))
        }
    }

    fn digest(&mut self, bytes: &[u8]) {
        if let Some(hashes) = &mut self.hashes {
            hashes.update(bytes);
        }
    }
}

/// What a step returns when it wants more input to go on.
#[allow(clippy::unnecessary_wraps)]
fn want(stage: Stage) -> Result<(Stage, Option<Progress>), Fault> {
    Ok((stage, Some(Progress::NeedMore)))
}

/// Holds a key of a part's map to dcbor's order: after every key before it, and none of them twice.
///
/// dcbor checks for a duplicate first, against every key, and for the order second.
fn check_key_order(keys: &[Vec<u8>], key: &[u8]) -> Result<(), Fault> {
    let Some(last) = keys.last() else { return Ok(()) };
    if keys.binary_search_by(|earlier| earlier.as_slice().cmp(key)).is_ok() {
        return Err(Fault::dcbor(&dcbor::Error::DuplicateMapKey));
    }
    if last.as_slice() >= key {
        return Err(Fault::dcbor(&dcbor::Error::MisorderedMapKey));
    }
    Ok(())
}

/// The head of a value of `major` type, holding `value`, in its shortest form.
fn encode_head(major: u8, value: u64) -> ([u8; 9], usize) {
    let mut head = [0u8; 9];
    let bytes = value.to_be_bytes();
    let (additional, len) = match value {
        0..=23 => (bytes[7], 0),
        24..=0xff => (24, 1),
        0x100..=0xffff => (25, 2),
        0x1_0000..=0xffff_ffff => (26, 4),
        _ => (27, 8),
    };
    head[0] = (major << 5) | additional;
    head[1..=len].copy_from_slice(&bytes[8 - len..]);
    (head, len + 1)
}

// ---- faults ------------------------------------------------------------------------------------

/// Which step of [`from_slice`](crate::Bundle::from_slice) a fault belongs to, which decides how a
/// nested bundle's fault reads from the bundle around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// The encoding, which `decode_cbor` judges.
    Cbor,
    /// The data model, which `from_cbor` judges.
    Decode,
    /// What the outer bundle may hold, which is named at the outer part.
    Holder,
    /// The rules, which `validate` judges.
    Validate,
    /// A caller's limit.
    Limit,
}

struct Fault {
    phase: Phase,
    error: Error,
}

impl Fault {
    fn cbor(kind: ErrorKind) -> Self {
        Fault { phase: Phase::Cbor, error: Error::at("", kind) }
    }

    fn dcbor(error: &dcbor::Error) -> Self {
        Fault::cbor(ErrorKind::Encoding(error.to_string()))
    }

    fn decode(error: Error) -> Self {
        Fault { phase: Phase::Decode, error }
    }

    fn holder(error: Error) -> Self {
        Fault { phase: Phase::Holder, error }
    }

    fn validate(error: Error) -> Self {
        Fault { phase: Phase::Validate, error }
    }

    fn limit(path: &str, limit: Limit, max: u64) -> Self {
        Fault { phase: Phase::Limit, error: Error::at(path, ErrorKind::OverLimit { limit, max }) }
    }

    /// A state the reader cannot reach, refused rather than panicked on.
    fn lost() -> Self {
        Fault::cbor(ErrorKind::Encoding("the streaming reader lost its place".to_owned()))
    }

    /// A nested bundle's fault, as `validate` names it from the part at `path` that carries it.
    fn nested_in(self, path: &str) -> Self {
        let error = match self.phase {
            Phase::Cbor | Phase::Holder => Error::at(path, self.error.into_kind()),
            Phase::Decode => Error::at(path, ErrorKind::NestedPayloadNotABundle(Box::new(self.error))),
            Phase::Validate | Phase::Limit => self.error.within(&format!("{path}.payload")),
        };
        Fault { phase: if self.phase == Phase::Limit { Phase::Limit } else { Phase::Validate }, error }
    }
}

// ---- heads and items ---------------------------------------------------------------------------

/// One CBOR head.
#[derive(Debug, Clone, Copy)]
struct Head {
    major: u8,
    argument: u64,
}

/// How long a head is, from its first byte, refusing the lengths dcbor refuses: the reserved ones
/// and an indefinite length.
fn head_len(initial: u8) -> Result<usize, Fault> {
    match initial & 0x1f {
        0..=23 => Ok(1),
        24 => Ok(2),
        25 => Ok(3),
        26 => Ok(5),
        27 => Ok(9),
        reserved => Err(Fault::dcbor(&dcbor::Error::UnsupportedHeaderValue(reserved))),
    }
}

/// Reads a whole head, holding it to dcbor's rules for one.
///
/// Its argument in the shortest form that holds it, except on a float, which is judged as a float.
/// A head of major type 7 is a whole value on its own, a simple value or a float, so it is handed
/// to dcbor to judge, which keeps the two from disagreeing on what a canonical float is.
fn parse_head(bytes: &[u8]) -> Result<Head, Fault> {
    let initial = bytes[0];
    let argument = if bytes.len() == 1 {
        u64::from(initial & 0x1f)
    } else {
        let value = bytes[1..].iter().fold(0u64, |value, &byte| (value << 8) | u64::from(byte));
        let shortest = match bytes.len() {
            2 => value >= 24,
            3 => value > 0xff || initial == 0xf9,
            5 => value > 0xffff || initial == 0xfa,
            _ => value > 0xffff_ffff || initial == 0xfb,
        };
        if !shortest {
            return Err(Fault::dcbor(&dcbor::Error::NonCanonicalNumeric));
        }
        value
    };
    let major = initial >> 5;
    if major == 7 {
        CBOR::try_from_data(bytes).map_err(|error| Fault::dcbor(&error))?;
    }
    Ok(Head { major, argument })
}

/// A head's bytes, as read.
#[derive(Clone, Copy)]
struct RawHead {
    bytes: [u8; 9],
    len: usize,
}

impl RawHead {
    fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

/// A structural head, read a byte at a time if that is how it arrives.
#[derive(Default)]
struct HeadBuf {
    bytes: [u8; 9],
    len: usize,
}

impl HeadBuf {
    /// Takes bytes from `input` until a whole head is in hand, and returns it once it is.
    fn read(&mut self, input: &[u8], at: &mut usize) -> Result<Option<(Head, RawHead)>, Fault> {
        if self.len == 0 {
            let Some(&initial) = input.get(*at) else { return Ok(None) };
            self.bytes[0] = initial;
            self.len = 1;
            *at += 1;
        }
        let need = head_len(self.bytes[0])?;
        let take = (need - self.len).min(input.len() - *at);
        self.bytes[self.len..self.len + take].copy_from_slice(&input[*at..*at + take]);
        self.len += take;
        *at += take;
        if self.len < need {
            return Ok(None);
        }
        let raw = RawHead { bytes: self.bytes, len: need };
        self.len = 0;
        Ok(Some((parse_head(raw.as_slice())?, raw)))
    }
}

/// How many bytes are left of a limit.
struct Budget {
    left: u64,
    max: u64,
    limit: Limit,
    path: String,
}

impl Budget {
    fn new(max: u64, limit: Limit, path: String) -> Self {
        Budget { left: max, max, limit, path }
    }

    /// Takes `bytes` off what is left.
    fn spend(&mut self, bytes: u64) -> Result<(), Fault> {
        self.room_for(bytes)?;
        self.left -= bytes;
        Ok(())
    }

    /// Refuses at once what cannot fit in what is left: `items` values take a byte each at least.
    fn room_for(&self, items: u64) -> Result<(), Fault> {
        if items > self.left { Err(Fault::limit(&self.path, self.limit, self.max)) } else { Ok(()) }
    }
}

/// One CBOR value, buffered whole: a header, or a key or a value in a part's map.
///
/// Its heads are held to dcbor's rules as they arrive and its depth is counted on a stack, so a
/// value is refused at the first head that is wrong rather than once it is whole; what dcbor judges
/// only of a whole value, text and map order, it judges once the value is.
struct Item {
    bytes: Vec<u8>,
    /// For each container open in the value, how many items it still holds, beneath one slot for
    /// the value itself. The same count `check_nesting` keeps.
    open: Vec<u64>,
    /// Where in `bytes` the head being read starts; `bytes.len()` when none is.
    head_at: usize,
    /// How much of a string's content is still to come.
    string: u64,
    /// How many containers enclose the value in the file.
    depth: usize,
    /// The major type the value itself must be, and the error if it is not.
    expect: Option<(u8, Error)>,
}

impl Item {
    fn new(depth: usize, expect: Option<(u8, Error)>) -> Self {
        Item { bytes: Vec::new(), open: vec![1], head_at: 0, string: 0, depth, expect }
    }

    /// Takes bytes from `input` until the value is whole, and says whether it is.
    fn feed(&mut self, input: &[u8], at: &mut usize, budget: &mut Budget) -> Result<bool, Fault> {
        loop {
            if self.string > 0 {
                let take = usize::try_from(self.string).unwrap_or(usize::MAX).min(input.len() - *at);
                self.bytes.extend_from_slice(&input[*at..*at + take]);
                *at += take;
                self.string -= take as u64;
                if self.string > 0 {
                    return Ok(false);
                }
                self.head_at = self.bytes.len();
            }
            while self.open.last() == Some(&0) {
                self.open.pop();
            }
            if self.open.is_empty() {
                return Ok(true);
            }

            if self.head_at == self.bytes.len() {
                let Some(&initial) = input.get(*at) else { return Ok(false) };
                budget.spend(head_len(initial)? as u64)?;
                self.bytes.push(initial);
                *at += 1;
            }
            let need = head_len(self.bytes[self.head_at])?;
            let have = self.bytes.len() - self.head_at;
            let take = (need - have).min(input.len() - *at);
            self.bytes.extend_from_slice(&input[*at..*at + take]);
            *at += take;
            if have + take < need {
                return Ok(false);
            }
            let head = parse_head(&self.bytes[self.head_at..])?;
            self.head_at = self.bytes.len();
            if let Some((major, error)) = self.expect.take()
                && head.major != major
            {
                return Err(Fault::decode(error));
            }

            // The head is one of its container's items.
            if let Some(remaining) = self.open.last_mut() {
                *remaining -= 1;
            }
            match head.major {
                2 | 3 => {
                    budget.spend(head.argument)?;
                    self.string = head.argument;
                }
                4..=6 => {
                    let holds = match head.major {
                        4 => head.argument,
                        5 => head.argument.saturating_mul(2),
                        _ => 1,
                    };
                    if holds > 0 {
                        // Its ancestors are the containers around the value and those open in it.
                        if self.depth + self.open.len() > MAX_NESTING {
                            return Err(Fault::cbor(ErrorKind::TooDeep { limit: MAX_NESTING }));
                        }
                        budget.room_for(holds)?;
                        self.open.push(holds);
                    }
                }
                _ => {}
            }
        }
    }
}

// ---- zstd --------------------------------------------------------------------------------------

/// Inflating a `zstd` payload a piece at a time, exactly as [`Part::bytes`] inflates it whole.
///
/// [`Part::bytes`] reads `zstd`'s own streaming decoder through `take(size + 1)`. This drives the
/// same decoder the same way, pushing input rather than pulling it: the same frame handling (a
/// frame may follow a frame; a frame left unfinished is refused), the same cap on output one byte
/// past `size`, and the same errors in the same words.
#[cfg(feature = "zstd")]
mod inflate {
    use zstd::stream::raw::{Decoder, InBuffer, Operation, OutBuffer};

    use super::{Limit, ZSTD_DEFAULT_WINDOW};
    use crate::error::ErrorKind;

    /// What zstd says of a frame that wants a larger window than the decoder was allowed.
    const WINDOW_TOO_LARGE: &str = "Frame requires too much memory for decoding";

    pub(super) struct Inflater {
        decoder: Decoder<'static>,
        out: Box<[u8]>,
        /// The stated `size`.
        size: u64,
        /// The same, as [`Part::bytes`](crate::Part::bytes) names it in an error.
        capacity: usize,
        produced: u64,
        finished_frame: bool,
        /// The window the decoder was held to, where it was held below zstd's own cap.
        window: Option<u64>,
    }

    impl Inflater {
        pub(super) fn new(size: u64, window: u64) -> Result<Self, ErrorKind> {
            let capacity = usize::try_from(size)
                .map_err(|_| ErrorKind::ZstdInvalid("stated size does not fit".into()))?;
            let mut decoder = Decoder::new().map_err(|e| ErrorKind::ZstdInvalid(e.to_string()))?;
            let window = (window < ZSTD_DEFAULT_WINDOW).then_some(window);
            if let Some(window) = window {
                // zstd takes the window as a power of two, from 2^10 up.
                let log = window.max(1 << 10).ilog2();
                decoder
                    .set_parameter(zstd::stream::raw::DParameter::WindowLogMax(log))
                    .map_err(|e| ErrorKind::ZstdInvalid(e.to_string()))?;
            }
            Ok(Inflater {
                decoder,
                out: vec![0; zstd::zstd_safe::DCtx::out_size()].into_boxed_slice(),
                size,
                capacity,
                produced: 0,
                finished_frame: false,
                window,
            })
        }

        /// What the last step inflated, at the front.
        pub(super) fn out(&self) -> &[u8] {
            &self.out
        }

        /// Inflates from `input` until there is output or the input is spent: how much input it
        /// took, and how much output is at the front of [`out`](Inflater::out).
        pub(super) fn step(&mut self, input: &[u8]) -> Result<(usize, usize), ErrorKind> {
            let mut used = 0;
            loop {
                let rest = &input[used..];
                // A frame may follow a frame, and the decoder is reset for it only once there is
                // some of it, as zstd's reader does.
                if self.finished_frame && !rest.is_empty() {
                    self.decoder.reinit().map_err(|e| self.invalid(&e))?;
                    self.finished_frame = false;
                }
                let (taken, made) = self.run(rest)?;
                used += taken;
                if made > 0 || used == input.len() {
                    return Ok((used, made));
                }
                if taken == 0 {
                    // zstd always takes input or makes output when it has room for both.
                    return Err(ErrorKind::ZstdInvalid("the decoder made no progress".into()));
                }
            }
        }

        /// After the last of the input: whatever the decoder still holds.
        pub(super) fn drain(&mut self) -> Result<usize, ErrorKind> {
            self.run(&[]).map(|(_, made)| made)
        }

        /// After the last of the output: a frame has to have ended, at `size`.
        pub(super) fn finish(&mut self) -> Result<(), ErrorKind> {
            let mut nowhere = [0u8; 0];
            self.decoder
                .finish(&mut OutBuffer::around(&mut nowhere[..]), self.finished_frame)
                .map_err(|e| ErrorKind::ZstdInvalid(e.to_string()))?;
            if self.produced < self.size {
                return Err(ErrorKind::ZstdInvalid(format!(
                    "inflated to {} of the stated {} bytes",
                    self.produced, self.capacity
                )));
            }
            Ok(())
        }

        fn run(&mut self, input: &[u8]) -> Result<(usize, usize), ErrorKind> {
            // One byte past the claim is all it takes to know the payload outgrew it.
            let room = self.size.saturating_add(1) - self.produced;
            let cap = usize::try_from(room).unwrap_or(usize::MAX).min(self.out.len());
            let mut src = InBuffer::around(input);
            let mut dst = OutBuffer::around(&mut self.out[..cap]);
            let result = self.decoder.run(&mut src, &mut dst);
            let (taken, made) = (src.pos(), dst.pos());
            let hint = result.map_err(|e| self.invalid(&e))?;
            if hint == 0 {
                self.finished_frame = true;
            }
            self.produced += made as u64;
            if self.produced > self.size {
                return Err(ErrorKind::ZstdInvalid(format!(
                    "inflated past the stated {} bytes",
                    self.capacity
                )));
            }
            Ok((taken, made))
        }

        fn invalid(&self, error: &std::io::Error) -> ErrorKind {
            let message = error.to_string();
            match self.window {
                Some(max) if message == WINDOW_TOO_LARGE => {
                    ErrorKind::OverLimit { limit: Limit::ZstdWindow, max }
                }
                _ => ErrorKind::ZstdInvalid(message),
            }
        }
    }
}

/// What the `multithread` feature promises of a reader, checked at compile time.
#[cfg(feature = "multithread")]
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<Reader>();
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_head_writes_the_shortest_form() {
        for value in
            [0, 23, 24, 255, 256, 65_535, 65_536, u64::from(u32::MAX), u64::from(u32::MAX) + 1, u64::MAX]
        {
            let (head, len) = encode_head(5, value);
            let expected = CBOR::from(value).to_cbor_data();
            assert_eq!(head[0] & 0x1f, expected[0] & 0x1f, "{value}");
            assert_eq!(head[0] >> 5, 5);
            assert_eq!(&head[1..len], &expected[1..], "{value}");
        }
    }

    #[test]
    fn every_initial_byte_is_judged_as_dcbor_judges_it() {
        // A head on its own is a whole value for the scalar majors, so dcbor can be asked directly.
        for initial in 0u8..=255 {
            let major = initial >> 5;
            if !matches!(major, 0 | 1 | 7) {
                continue;
            }
            let len = head_len(initial).ok();
            let mut bytes = vec![initial];
            if let Some(len) = len {
                bytes.resize(len, 0);
                // Large enough to be shortest for an integer, a plausible float otherwise.
                if len > 1 {
                    bytes[1] = 0x7f;
                }
            }
            let ours = len.map(|_| parse_head(&bytes).map(|_| ()).map_err(|fault| fault.error));
            let theirs = CBOR::try_from_data(&bytes).map(|_| ()).map_err(|e| e.to_string());
            match ours {
                None => assert!(theirs.is_err(), "{initial:#04x}"),
                Some(ours) => assert_eq!(
                    ours.map_err(|e| e.to_string()),
                    theirs.map_err(|e| format!("not deterministically encoded CBOR: {e}")),
                    "{initial:#04x}"
                ),
            }
        }
    }
}
