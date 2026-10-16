//! Extension keys, and the typed values under them.
//!
//! An extension key carries whatever the core format does not define, under a reverse-DNS name
//! that says whose it is. On the wire the value is any CBOR at all, and a decoder keeps every one
//! it reads and writes it back unchanged, recognised or not. That is the whole of what makes the
//! format extensible, and [`Extensions`] holds to it: a value read off the wire is kept verbatim
//! and is the thing written back out.
//!
//! What a caller works with is a typed value. A type that implements [`Extension`] names its key
//! and how it converts to and from CBOR, and [`Extensions::get`] hands back that type for a key
//! that holds it. The keys these specifications define under `x.1sav` have their types here —
//! [`Rtc`], [`RtcMbc3`], [`RtcS3511a`], [`Label`], [`Icon`], [`Dirent`] and [`DirentGcMc`] — and so
//! does 1retro's [`Forge`]. Any other producer's key is a type of its own implementing the same
//! trait.
//!
//! ```
//! use one_saves::{Extensions, extension::Label};
//!
//! let mut extensions = Extensions::new();
//! extensions.insert(Label::new("FINAL FANTASY VII").detail("Cloud  Lv 7"));
//! assert_eq!(extensions.get::<Label>().unwrap().title, "FINAL FANTASY VII");
//! ```
//!
//! A value that does not match its key's schema is still kept and still round-trips; it simply
//! has no typed reading, so [`get`](Extensions::get) returns `None` for it and
//! [`cbor`](Extensions::cbor) still has the bytes. Shown, it is CBOR in diagnostic notation.

use std::any::Any;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::OnceLock;

use dcbor::{CBOR, CBORDecodable, CBOREncodable};

use crate::name::ReverseDnsName;

mod dirent;
mod forge;
mod gc_mc;
mod icon;
mod label;
mod rtc;
mod time;

pub use dirent::{Dirent, EntryTime};
pub use forge::Forge;
pub use gc_mc::DirentGcMc;
pub use icon::{Icon, IconFrame};
pub use label::Label;
pub use rtc::{Rtc, RtcMbc3, RtcS3511a};

/// A typed value for one extension key.
///
/// The conversions are `dcbor`'s own: `From<Self> for CBOR` to write the value, and
/// `TryFrom<CBOR, Error = dcbor::Error>` to read it, which is everything the two blanket traits
/// [`CBOREncodable`] and [`CBORDecodable`] ask for. Reading should refuse a value that does not
/// match the key's schema rather than half-read it: a refused value stays in the bundle as its
/// raw CBOR, and a half-read one would put something the producer did not write in front of a
/// caller.
///
/// [`Display`](fmt::Display) is what a listing such as `1saves inspect` shows for the value, so it
/// should be one short line. [`Debug`](fmt::Debug) is what `{:?}` on a bundle shows for it, and is
/// the place for everything a person debugging the value would want that the one line leaves out.
///
/// A value read off the wire is shown as its type once something has read it as that type — this
/// crate does that for its own keys, and [`Extensions::get`] does it for a producer's — and as
/// its raw CBOR until then, since nothing else says which type a key has.
///
/// The types in this module implement it exactly as a producer's own type would, with nothing
/// else to lean on, so they are the examples: [`Label`] is the smallest, and [`Dirent`] shows a
/// value that nests one of its own.
pub trait Extension:
    CBOREncodable + CBORDecodable + fmt::Debug + fmt::Display + PartialEq + MaybeSendSync + 'static
{
    /// The key this value is carried under.
    const KEY: ReverseDnsName;
}

/// `Send + Sync` when the `multithread` feature is on, and nothing otherwise.
///
/// An [`Extension`] lives inside a [`Bundle`](crate::Bundle), so it has to be as thread-safe as
/// the feature promises a bundle is. Without the feature nothing in a bundle is `Send`, and a
/// value is free to hold a `dcbor::CBOR` or an `Rc` of its own.
#[cfg(feature = "multithread")]
pub trait MaybeSendSync: Send + Sync {}
#[cfg(feature = "multithread")]
impl<T: Send + Sync> MaybeSendSync for T {}

/// `Send + Sync` when the `multithread` feature is on, and nothing otherwise.
///
/// An [`Extension`] lives inside a [`Bundle`](crate::Bundle), so it has to be as thread-safe as
/// the feature promises a bundle is. Without the feature nothing in a bundle is `Send`, and a
/// value is free to hold a `dcbor::CBOR` or an `Rc` of its own.
#[cfg(not(feature = "multithread"))]
pub trait MaybeSendSync {}
#[cfg(not(feature = "multithread"))]
impl<T> MaybeSendSync for T {}

/// An [`Extension`] with its type erased, so that one map can hold values of several types.
trait Erased: Any + fmt::Debug + fmt::Display + MaybeSendSync {
    fn to_cbor(&self) -> CBOR;
    fn clone_box(&self) -> Box<dyn Erased>;
}

impl<T: Extension> Erased for T {
    fn to_cbor(&self) -> CBOR {
        CBOREncodable::to_cbor(self)
    }

    fn clone_box(&self) -> Box<dyn Erased> {
        Box::new(self.clone())
    }
}

/// Reads a value as `T`, refusing one that does not match `T`'s schema.
fn decode<T: Extension>(value: &CBOR) -> Option<Box<dyn Erased>> {
    T::try_from_cbor(value).ok().map(|typed| Box::new(typed) as Box<dyn Erased>)
}

type Decoder = fn(&CBOR) -> Option<Box<dyn Erased>>;

/// The type this crate has for a key, so a value read off the wire can be shown without a caller
/// naming its type.
fn known(key: &ReverseDnsName) -> Option<Decoder> {
    let known: [(ReverseDnsName, Decoder); 8] = [
        (Rtc::KEY, decode::<Rtc>),
        (RtcMbc3::KEY, decode::<RtcMbc3>),
        (RtcS3511a::KEY, decode::<RtcS3511a>),
        (Label::KEY, decode::<Label>),
        (Icon::KEY, decode::<Icon>),
        (Dirent::KEY, decode::<Dirent>),
        (DirentGcMc::KEY, decode::<DirentGcMc>),
        (Forge::KEY, decode::<Forge>),
    ];
    known.into_iter().find(|(name, _)| name == key).map(|(_, decoder)| decoder)
}

/// One key's value.
enum Entry {
    /// Put there by a producer as a typed value, and encoded when the bundle is written.
    Typed(Box<dyn Erased>),
    /// Read off the wire. The CBOR is what is written back, so the round trip cannot depend on
    /// how well a type reads it; the typed reading is decoded the first time someone asks.
    Raw { value: CBOR, typed: OnceLock<Option<Box<dyn Erased>>> },
}

impl Entry {
    fn raw(value: CBOR) -> Self {
        Entry::Raw { value, typed: OnceLock::new() }
    }

    fn to_cbor(&self) -> CBOR {
        match self {
            Entry::Typed(typed) => typed.to_cbor(),
            Entry::Raw { value, .. } => value.clone(),
        }
    }

    /// The typed value, decoding a raw one with `decoder` if nobody has yet.
    ///
    /// The first reading sticks. A key has one type, so a second caller asking with another type
    /// for the same key is asking a question this map has already answered.
    fn typed(&self, decoder: impl FnOnce(&CBOR) -> Option<Box<dyn Erased>>) -> Option<&dyn Erased> {
        match self {
            Entry::Typed(typed) => Some(&**typed),
            Entry::Raw { value, typed } => typed.get_or_init(|| decoder(value)).as_deref(),
        }
    }

    /// The typed value for a key this crate has a type for, or whatever reading it already has.
    ///
    /// Not knowing a key's type is not a reading, so it is not cached: a producer's own key read
    /// later through [`Extensions::get`] still gets its type.
    fn known(&self, key: &ReverseDnsName) -> Option<&dyn Erased> {
        match (self, known(key)) {
            (Entry::Typed(typed), _) => Some(&**typed),
            (Entry::Raw { .. }, Some(decoder)) => self.typed(decoder),
            (Entry::Raw { typed, .. }, None) => typed.get()?.as_deref(),
        }
    }
}

impl Clone for Entry {
    fn clone(&self) -> Self {
        match self {
            Entry::Typed(typed) => Entry::Typed(typed.clone_box()),
            // The reading is a cache, and cheap to redo.
            Entry::Raw { value, .. } => Entry::raw(value.clone()),
        }
    }
}

impl Entry {
    /// The value as a listing shows it.
    fn shown(&self, key: &ReverseDnsName) -> Shown<'_> {
        match self {
            Entry::Typed(typed) => Shown::Typed(&**typed),
            Entry::Raw { value, .. } => self.known(key).map_or(Shown::Raw(value), Shown::Typed),
        }
    }
}

impl fmt::Debug for Entry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Entry::Typed(typed) => typed.fmt(f),
            Entry::Raw { value, .. } => write_shown(f, &diagnostic(value)),
        }
    }
}

/// How many characters a value is shown in before it is cut short.
///
/// A backstop rather than a target. What runs long is nearly always a byte string, and those are
/// shortened inside the value — see [`SHOWN_BYTES`] — so what reaches this is long text or a very
/// large map, which a listing or a log line still has no use for all of. The whole value is
/// always [`cbor`](Extensions::cbor) away.
pub const SHOWN_CHARS: usize = 256;

/// How many bytes of a byte string are shown before the rest is left out.
///
/// A picture or a producer's blob is kilobytes of hex, and a map holding one should still show
/// the keys after it. Past this, a byte string is shown as its first bytes and its length:
/// `h'89504e470d0a1a0a0000000d49484452…' (4096 bytes)`.
pub const SHOWN_BYTES: usize = 16;

/// A value in CBOR diagnostic notation on one line, with every long byte string shortened.
///
/// Anything that holds no byte string past [`SHOWN_BYTES`] comes out exactly as `dcbor` writes
/// it, since every leaf is written by `dcbor` and only the containers around a leaf are walked
/// here.
fn diagnostic(value: &CBOR) -> String {
    let mut text = String::new();
    write_diagnostic(&mut text, value).expect("a String takes any write");
    text
}

fn write_diagnostic(out: &mut impl fmt::Write, value: &CBOR) -> fmt::Result {
    match value.as_case() {
        dcbor::CBORCase::ByteString(bytes) if bytes.len() > SHOWN_BYTES => {
            out.write_str("h'")?;
            bytes.iter().take(SHOWN_BYTES).try_for_each(|byte| write!(out, "{byte:02x}"))?;
            write!(out, "…' ({} bytes)", bytes.len())
        }
        dcbor::CBORCase::Array(items) => {
            out.write_char('[')?;
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.write_str(", ")?;
                }
                write_diagnostic(out, item)?;
            }
            out.write_char(']')
        }
        dcbor::CBORCase::Map(map) => {
            out.write_char('{')?;
            for (index, (key, item)) in map.iter().enumerate() {
                if index > 0 {
                    out.write_str(", ")?;
                }
                write_diagnostic(out, key)?;
                out.write_str(": ")?;
                write_diagnostic(out, item)?;
            }
            out.write_char('}')
        }
        dcbor::CBORCase::Tagged(tag, inner) => {
            write!(out, "{}(", tag.value())?;
            write_diagnostic(out, inner)?;
            out.write_char(')')
        }
        _ => out.write_str(&value.diagnostic_flat()),
    }
}

/// Writes `text`, cut to [`SHOWN_CHARS`] characters with the last one an ellipsis where it ran on.
fn write_shown(f: &mut fmt::Formatter<'_>, text: &str) -> fmt::Result {
    if text.chars().nth(SHOWN_CHARS).is_none() {
        return f.write_str(text);
    }
    let (cut, _) = text.char_indices().nth(SHOWN_CHARS - 1).expect("it runs longer than that");
    f.write_str(&text[..cut])?;
    f.write_str("…")
}

/// A value on one line: its type's own line where it has one, its CBOR in diagnostic notation
/// where it does not.
enum Shown<'a> {
    Typed(&'a dyn Erased),
    Raw(&'a CBOR),
}

impl fmt::Display for Shown<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Shown::Typed(typed) => typed.to_string(),
            Shown::Raw(value) => diagnostic(value),
        };
        write_shown(f, &text)
    }
}

/// The extension keys on one map, flat alongside its integer keys.
///
/// A header, a part and a `game` map each carry one. Keys are unique and kept in order, which is
/// the order they are written in.
#[derive(Clone, Default)]
pub struct Extensions {
    entries: BTreeMap<ReverseDnsName, Entry>,
}

impl Extensions {
    /// An empty set of keys.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many keys there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The value under `T`'s key, as a `T`.
    ///
    /// `None` when the key is absent, and also when the value there does not match `T`'s schema:
    /// that value is still kept, and [`cbor`](Extensions::cbor) still has it.
    #[must_use]
    pub fn get<T: Extension>(&self) -> Option<&T> {
        let typed = self.entries.get(&T::KEY)?.typed(decode::<T>)?;
        (typed as &dyn Any).downcast_ref()
    }

    /// Puts a value under its key, replacing whatever was there.
    pub fn insert<T: Extension>(&mut self, value: T) {
        self.entries.insert(T::KEY, Entry::Typed(Box::new(value)));
    }

    /// Puts raw CBOR under a key, replacing whatever was there.
    ///
    /// For a key with no [`Extension`] type, or for a value carried exactly as some other producer
    /// wrote it. A key this crate has a type for still gets its typed reading.
    pub fn insert_cbor(&mut self, key: ReverseDnsName, value: CBOR) {
        self.entries.insert(key, Entry::raw(value));
    }

    /// Takes a key out, returning what was under it.
    pub fn remove(&mut self, key: &ReverseDnsName) -> Option<CBOR> {
        self.entries.remove(key).map(|entry| entry.to_cbor())
    }

    /// Whether a key is present.
    #[must_use]
    pub fn contains_key(&self, key: &ReverseDnsName) -> bool {
        self.entries.contains_key(key)
    }

    /// The value under a key, as the CBOR it is written as.
    #[must_use]
    pub fn cbor(&self, key: &ReverseDnsName) -> Option<CBOR> {
        self.entries.get(key).map(Entry::to_cbor)
    }

    /// The value under a key, ready to show on one line.
    ///
    /// A typed value is shown by its type's [`Display`](fmt::Display): one a producer put here as
    /// a typed value, or one read off the wire whose key has a type and which matches it. Anything
    /// else is shown as its CBOR in diagnostic notation, since it is the producer's to define and a
    /// guess at its meaning would be worse, with each byte string past [`SHOWN_BYTES`] shortened.
    /// Either way the line stops at [`SHOWN_CHARS`].
    ///
    /// `None` only when the key is absent.
    #[must_use]
    pub fn display(&self, key: &ReverseDnsName) -> Option<impl fmt::Display + '_> {
        Some(self.entries.get(key)?.shown(key))
    }

    /// The keys, in the order they are written.
    pub fn keys(&self) -> impl Iterator<Item = &ReverseDnsName> {
        self.entries.keys()
    }

    /// Every key with its value as CBOR, in the order they are written.
    pub fn iter(&self) -> impl Iterator<Item = (&ReverseDnsName, CBOR)> {
        self.entries.iter().map(|(key, entry)| (key, entry.to_cbor()))
    }
}

impl fmt::Debug for Extensions {
    /// Each value as its type where it has a typed reading, which says more than its CBOR does,
    /// and otherwise as its CBOR in diagnostic notation, byte strings shortened and the whole cut
    /// to [`SHOWN_CHARS`]. A typed value's
    /// `Debug` is its type's own, and its type's to keep short.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut map = f.debug_map();
        for (key, entry) in &self.entries {
            match entry.known(key) {
                Some(typed) => map.entry(key, &typed),
                None => map.entry(key, entry),
            };
        }
        map.finish()
    }
}

/// Two sets of keys are equal when they would be written the same, however each value is held.
impl PartialEq for Extensions {
    fn eq(&self, other: &Self) -> bool {
        self.entries.len() == other.entries.len()
            && self
                .entries
                .iter()
                .zip(&other.entries)
                .all(|((a, x), (b, y))| a == b && x.to_cbor() == y.to_cbor())
    }
}

impl<T: Extension> Extend<T> for Extensions {
    fn extend<I: IntoIterator<Item = T>>(&mut self, values: I) {
        for value in values {
            self.insert(value);
        }
    }
}

/// Reads a map out of a value, or fails as the wrong type.
fn want_map(value: &CBOR) -> dcbor::Result<&dcbor::Map> {
    value.as_map().ok_or(dcbor::Error::WrongType)
}

/// Reads an unsigned integer that fits `T`.
fn want_uint<T: TryFrom<u64>>(value: &CBOR) -> dcbor::Result<T> {
    match value.as_case() {
        dcbor::CBORCase::Unsigned(n) => T::try_from(*n).map_err(|_| dcbor::Error::OutOfRange),
        _ => Err(dcbor::Error::WrongType),
    }
}

/// Reads whole epoch seconds under tag 1, the one shape these keys give an instant.
fn want_epoch(value: &CBOR) -> dcbor::Result<i64> {
    let dcbor::CBORCase::Tagged(tag, inner) = value.as_case() else {
        return Err(dcbor::Error::WrongType);
    };
    if tag.value() != 1 {
        return Err(dcbor::Error::WrongType);
    }
    match inner.as_case() {
        dcbor::CBORCase::Unsigned(n) => i64::try_from(*n).map_err(|_| dcbor::Error::OutOfRange),
        dcbor::CBORCase::Negative(n) => {
            i64::try_from(*n).map(|magnitude| -1 - magnitude).map_err(|_| dcbor::Error::OutOfRange)
        }
        _ => Err(dcbor::Error::WrongType),
    }
}

/// Writes whole epoch seconds under tag 1.
fn epoch(seconds: i64) -> CBOR {
    dcbor::CBORCase::Tagged(dcbor::Tag::new(1u64, "epoch"), seconds.into()).into()
}

/// Refuses a map holding an integer key the schema does not list.
///
/// A key a later version adds would be a different schema, so a reading that ignored it would be
/// a reading of something else.
fn only_keys(map: &dcbor::Map, allowed: &[u64]) -> dcbor::Result<()> {
    for (key, _) in map.iter() {
        let known = match key.as_case() {
            dcbor::CBORCase::Unsigned(n) => allowed.contains(n),
            _ => false,
        };
        if !known {
            return Err(dcbor::Error::msg("a key this schema does not list"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typed_value_reads_back_as_its_type() {
        let mut extensions = Extensions::new();
        extensions.insert(Label::new("FINAL FANTASY VII"));
        assert_eq!(extensions.get::<Label>().map(|label| label.title.as_str()), Some("FINAL FANTASY VII"));
        assert!(extensions.get::<Rtc>().is_none(), "a key that is absent has no reading");
    }

    #[test]
    fn a_raw_value_under_a_known_key_reads_as_its_type() {
        let mut extensions = Extensions::new();
        extensions.insert_cbor(Label::KEY, Label::new("ZELDA").into());
        assert_eq!(extensions.get::<Label>().map(|label| label.title.as_str()), Some("ZELDA"));
        assert_eq!(extensions.display(&Label::KEY).map(|shown| shown.to_string()).as_deref(), Some("ZELDA"));
    }

    #[test]
    fn a_value_off_its_schema_is_kept_but_not_read() {
        // A label is a map; a bare string under its key is somebody's mistake, and it still
        // round-trips because the CBOR is what gets written.
        let mut extensions = Extensions::new();
        extensions.insert_cbor(Label::KEY, CBOR::from("not a map"));
        assert!(extensions.get::<Label>().is_none());
        assert_eq!(
            extensions.display(&Label::KEY).map(|shown| shown.to_string()).as_deref(),
            Some(r#""not a map""#)
        );
        assert_eq!(extensions.cbor(&Label::KEY), Some(CBOR::from("not a map")));
    }

    #[test]
    fn a_key_this_crate_has_no_type_for_is_kept_as_written() {
        let key = ReverseDnsName::parse("io.mgba.rewind").unwrap();
        let mut extensions = Extensions::new();
        extensions.insert_cbor(key.clone(), CBOR::from(7u64));
        assert_eq!(extensions.display(&key).map(|shown| shown.to_string()).as_deref(), Some("7"));
        assert_eq!(extensions.iter().collect::<Vec<_>>(), [(&key, CBOR::from(7u64))]);
    }

    #[test]
    fn equality_is_by_what_would_be_written() {
        let label = Label::new("ZELDA").detail("Link");
        let mut typed = Extensions::new();
        typed.insert(label.clone());
        let mut raw = Extensions::new();
        raw.insert_cbor(Label::KEY, label.into());
        assert_eq!(typed, raw);
    }

    #[test]
    fn a_third_party_type_works_like_a_spec_one() {
        #[derive(Debug, Clone, PartialEq)]
        struct Rewind(u64);
        impl fmt::Display for Rewind {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{} frames", self.0)
            }
        }
        impl From<Rewind> for CBOR {
            fn from(value: Rewind) -> Self {
                value.0.into()
            }
        }
        impl TryFrom<CBOR> for Rewind {
            type Error = dcbor::Error;
            fn try_from(value: CBOR) -> dcbor::Result<Self> {
                want_uint(&value).map(Rewind)
            }
        }
        impl Extension for Rewind {
            const KEY: ReverseDnsName = ReverseDnsName::from_static("io.mgba.rewind");
        }

        // Read off the wire, where nothing knew the type until the caller named it.
        let mut extensions = Extensions::new();
        extensions.insert_cbor(Rewind::KEY, CBOR::from(600u64));
        assert_eq!(
            format!("{extensions:?}"),
            r#"{ReverseDnsName("io.mgba.rewind"): 600}"#,
            "nothing has said which type this is"
        );
        assert_eq!(extensions.display(&Rewind::KEY).map(|shown| shown.to_string()).as_deref(), Some("600"));
        // Neither of those is a reading, so neither stops the caller getting one.
        assert_eq!(extensions.get::<Rewind>(), Some(&Rewind(600)));
        // And once it has been read as one, it shows as one, in both forms.
        assert_eq!(
            extensions.display(&Rewind::KEY).map(|shown| shown.to_string()).as_deref(),
            Some("600 frames")
        );
        assert_eq!(format!("{extensions:?}"), r#"{ReverseDnsName("io.mgba.rewind"): Rewind(600)}"#);
    }

    #[test]
    fn a_long_byte_string_is_shortened_and_what_follows_it_still_shows() {
        let key = ReverseDnsName::parse("io.mgba.rewind").unwrap();
        let mut map = dcbor::Map::new();
        map.insert(0u64, CBOR::to_byte_string(vec![0xab; 4096]));
        map.insert(1u64, CBOR::to_byte_string([0xcd; SHOWN_BYTES]));
        map.insert(2u64, "after");
        let mut extensions = Extensions::new();
        extensions.insert_cbor(key.clone(), map.into());

        let shown = format!("h'{}…' (4096 bytes)", "ab".repeat(SHOWN_BYTES));
        let whole = format!(r#"{{0: {shown}, 1: h'{}', 2: "after"}}"#, "cd".repeat(SHOWN_BYTES));
        assert_eq!(extensions.display(&key).unwrap().to_string(), whole);
        assert_eq!(format!("{extensions:?}"), format!(r#"{{ReverseDnsName("io.mgba.rewind"): {whole}}}"#));
    }

    #[test]
    fn short_values_come_out_exactly_as_dcbor_writes_them() {
        let mut map = dcbor::Map::new();
        map.insert(0u64, dcbor::CBORCase::Tagged(dcbor::Tag::new(1u64, "epoch"), 1_700_000_000u64.into()));
        map.insert(1u64, vec![CBOR::from(1u64), CBOR::from(-2i64), CBOR::from(true), CBOR::null()]);
        map.insert("text", r#"a "quoted" café"#);
        map.insert(-1i64, CBOR::to_byte_string([1, 2, 3]));
        let value: CBOR = map.into();
        assert_eq!(diagnostic(&value), value.diagnostic_flat());
    }

    #[test]
    fn a_long_line_is_cut_short_wherever_it_is_shown() {
        let key = ReverseDnsName::parse("io.mgba.rewind").unwrap();
        let mut extensions = Extensions::new();
        extensions.insert_cbor(key.clone(), CBOR::from("a".repeat(4096)));

        let shown = extensions.display(&key).unwrap().to_string();
        assert_eq!(shown.chars().count(), SHOWN_CHARS);
        assert!(shown.starts_with(r#""aaa"#) && shown.ends_with('…'), "{shown}");
        let debug = format!("{extensions:?}");
        assert!(debug.ends_with("…}"), "{debug}");
        assert!(debug.len() < SHOWN_CHARS + 64, "{debug}");

        // A line that fits is left alone, and the cut never lands inside a character.
        extensions.insert_cbor(key.clone(), CBOR::from("é".repeat(SHOWN_CHARS - 2)));
        assert_eq!(extensions.display(&key).unwrap().to_string().chars().count(), SHOWN_CHARS);
        assert!(!extensions.display(&key).unwrap().to_string().ends_with('…'));
        extensions.insert_cbor(key.clone(), CBOR::from("é".repeat(SHOWN_CHARS)));
        assert!(extensions.display(&key).unwrap().to_string().ends_with("éé…"));
    }

    #[test]
    fn a_spec_key_read_off_the_wire_debugs_as_its_type() {
        let mut extensions = Extensions::new();
        extensions.insert_cbor(Label::KEY, Label::new("ZELDA").into());
        assert_eq!(
            format!("{extensions:?}"),
            r#"{ReverseDnsName("x.1sav.label"): Label { title: "ZELDA", detail: None }}"#
        );
    }
}
