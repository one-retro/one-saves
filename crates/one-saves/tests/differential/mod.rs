//! The streaming reader read beside `from_slice`, and inputs with exactly one thing wrong.
//!
//! Shared by `tests/stream.rs` and the fuzz target in `fuzz/fuzz_targets/stream.rs`, which
//! includes this file by path, so the two hold the reader to one contract:
//!
//! 1. **The verdict.** For every input, the reader accepts exactly when `from_slice` accepts and
//!    every part's `verify` passes. Said in `from_slice`'s terms: where `from_slice` refuses, the
//!    reader refuses; where it accepts, the reader accepts unless a part's payload does not hash to
//!    its `sha256` or does not inflate to its `size`, which `from_slice` leaves to whoever reads
//!    the payload and the reader reads them all. [`agree`] panics on anything else.
//! 2. **The error, where only one thing is wrong.** [`single_faults`] breaks a good bundle in
//!    exactly one place, at every place it can, and there the reader has to give the error
//!    `from_slice` gives. With several things wrong it may name another of them: it reports the
//!    first in the order the bytes arrive, and `from_slice` reports encoding before meaning.
//! 3. **What it hands over.** Where both accept, the reader's header, part keys and payloads are
//!    `from_slice`'s, and [`Done::content_hash`] is [`Bundle::content_hash`] for the same bytes;
//!    see [`assert_same`].
//!
//! Each input is read whole and cut, and the readings have to agree: a push parser whose answer
//! depends on where its input was cut is the bug this shape of API invites.

#![allow(dead_code)]

use std::ops::Range;

use one_saves::stream::{Done, Event, Limits, PartHead, PartTail, Reader};
use one_saves::{Bundle, Error, ErrorKind, HashValue, Header, PartKind, Payload, Strictness};

/// An error as a comparison sees it: where, and what.
pub type Fault = (String, ErrorKind);

pub fn fault(error: &Error) -> Fault {
    (error.path().to_owned(), error.kind().clone())
}

/// The verdict the reader is held to: `from_slice`'s, then `verify` on every part in order.
pub fn reference(bytes: &[u8], strictness: Strictness) -> Result<Bundle, Fault> {
    let bundle = Bundle::from_slice_with(bytes, strictness).map_err(|e| fault(&e))?;
    for (index, part) in bundle.parts.iter().enumerate() {
        let path = format!("parts[{index}]");
        match part.verify() {
            Ok(true) => {}
            Ok(false) => return Err((path, ErrorKind::PayloadHashMismatch)),
            Err(e) => return Err((path, e.kind().clone())),
        }
    }
    Ok(bundle)
}

/// Everything a reader handed over for one bundle.
#[derive(Debug, PartialEq)]
pub struct Streamed {
    pub header: Header,
    pub parts: Vec<(PartHead, Vec<u8>, PartTail)>,
    pub done: Done,
}

pub const WHOLE: &[usize] = &[usize::MAX];
pub const BYTES: &[usize] = &[1];
pub const UNEVEN: &[usize] = &[1, 2, 3, 5, 8, 13, 4096, 7];

/// Reads `bytes` through a reader, handing it the input in pieces of these sizes in turn, and
/// holds the events to the order the module documentation promises.
pub fn stream(
    bytes: &[u8],
    strictness: Strictness,
    limits: Limits,
    pieces: &[usize],
) -> Result<Streamed, Fault> {
    let mut reader = Reader::with_strictness(limits, strictness);
    let mut header: Option<(Header, u64)> = None;
    let mut parts: Vec<(PartHead, Vec<u8>, PartTail)> = Vec::new();
    let mut current: Option<(PartHead, Vec<u8>)> = None;
    let mut sizes = pieces.iter().cycle();
    let mut rest = bytes;
    while !rest.is_empty() {
        let size = (*sizes.next().unwrap()).clamp(1, rest.len());
        let (piece, after) = rest.split_at(size);
        rest = after;
        let mut input = piece;
        loop {
            let step = reader.read(input).map_err(|e| fault(&e))?;
            assert!(step.consumed <= input.len());
            input = &input[step.consumed..];
            match step.event {
                None => {
                    assert!(input.is_empty(), "no event, yet {} bytes left untaken", input.len());
                    break;
                }
                Some(Event::Header { header: read, parts: count }) => {
                    assert!(header.is_none(), "a second header");
                    assert!(count > 0);
                    header = Some((read, count));
                }
                Some(Event::PartStart(head)) => {
                    assert!(header.is_some(), "a part before the header");
                    assert!(current.is_none(), "a part inside a part");
                    assert_eq!(head.index, parts.len() as u64, "parts out of turn");
                    current = Some((head, Vec::new()));
                }
                Some(Event::Data(data)) => {
                    let (head, payload) = current.as_mut().expect("data outside a part");
                    assert!(!data.is_empty(), "an empty data event");
                    payload.extend_from_slice(data);
                    assert!(payload.len() as u64 <= head.size, "more data than the part's size");
                }
                Some(Event::PartEnd(tail)) => {
                    let (head, payload) = current.take().expect("a part's end without its start");
                    assert_eq!(payload.len() as u64, head.size, "data short of the part's size");
                    assert_eq!(
                        (tail.extensions.len() + tail.unknown.len()) as u64,
                        head.keys_after_payload,
                        "keys after the payload, against what the head said"
                    );
                    parts.push((head, payload, tail));
                }
                Some(other) => panic!("an event this test does not know: {other:?}"),
            }
        }
    }
    let done = reader.finish().map_err(|e| fault(&e))?;
    let (header, count) = header.expect("done without a header");
    assert!(current.is_none(), "done inside a part");
    assert_eq!(parts.len() as u64, count, "done short of the parts the header announced");
    // Asking again gives the same answer.
    assert_eq!(reader.finish().map_err(|e| fault(&e))?, done);
    Ok(Streamed { header, parts, done })
}

/// The reader handed over what `from_slice` read, and hashed it as `Bundle` hashes it.
pub fn assert_same(name: &str, bundle: &Bundle, streamed: &Streamed, bytes: &[u8]) {
    assert_eq!(streamed.header, bundle.header, "{name}: header");
    assert_eq!(streamed.parts.len(), bundle.parts.len(), "{name}: part count");
    for (part, (head, data, tail)) in bundle.parts.iter().zip(&streamed.parts) {
        let rebuilt = head.clone().into_part(tail.clone(), part.payload.clone());
        assert_eq!(&rebuilt, part, "{name}: part {}", part.id);
        assert_eq!(head.size, part.payload.len(), "{name}: part {} size", part.id);
        assert_eq!(data.as_slice(), part.bytes().unwrap().as_ref(), "{name}: part {} bytes", part.id);
    }
    assert_eq!(streamed.done.file_hash, HashValue::sha256_of(bytes), "{name}: file hash");
    assert_eq!(streamed.done.file_hash, bundle.file_hash().unwrap(), "{name}: file hash");
    assert_eq!(streamed.done.content_hash, bundle.content_hash().unwrap(), "{name}: content hash");
}

/// Trailing bytes are counted as far as the call that brought the first, which depends on how the
/// input was cut; nothing else does.
pub fn uncounted(fault: &Fault) -> Fault {
    match &fault.1 {
        ErrorKind::Encoding(why) if why.contains("extra bytes at the end") && fault.0.is_empty() => {
            (fault.0.clone(), ErrorKind::Encoding("extra bytes".into()))
        }
        _ => fault.clone(),
    }
}

/// How a reading of some bytes compares with `from_slice`'s.
#[derive(Debug)]
pub enum Agreement {
    /// Both accepted, and the reader handed over the same bundle.
    Accepted,
    /// Both refused, with the same error.
    SameError,
    /// Both refused, with different errors.
    OtherError { reference: Fault, streamed: Fault },
}

/// Reads `bytes` with `from_slice`, and through the reader whole, a byte at a time and unevenly,
/// and panics on anything but agreement about whether the bundle is good.
pub fn agree(name: &str, bytes: &[u8], strictness: Strictness) -> Agreement {
    agree_cut(name, bytes, strictness, &[BYTES, UNEVEN])
}

/// [`agree`], cutting the input only these ways besides reading it whole.
pub fn agree_cut(name: &str, bytes: &[u8], strictness: Strictness, cuts: &[&[usize]]) -> Agreement {
    let expected = reference(bytes, strictness);
    let whole = stream(bytes, strictness, Limits::NONE, WHOLE);

    for &pieces in cuts {
        let cut = stream(bytes, strictness, Limits::NONE, pieces);
        match (&whole, &cut) {
            (Ok(a), Ok(b)) => assert_eq!(a, b, "{name} ({strictness:?}): cut as {pieces:?}"),
            (Err(a), Err(b)) => {
                assert_eq!(uncounted(a), uncounted(b), "{name} ({strictness:?}): cut as {pieces:?}");
            }
            _ => panic!("{name} ({strictness:?}): read whole {whole:?}, cut as {pieces:?} {cut:?}"),
        }
    }

    match (expected, whole) {
        (Ok(bundle), Ok(streamed)) => {
            assert_same(name, &bundle, &streamed, bytes);
            Agreement::Accepted
        }
        (Err(reference), Err(streamed)) if reference == streamed => Agreement::SameError,
        (Err(reference), Err(streamed)) => Agreement::OtherError { reference, streamed },
        (Ok(_), Err(streamed)) => panic!(
            "{name} ({strictness:?}): from_slice accepts and every part verifies, the reader \
             refuses: {streamed:?}"
        ),
        (Err(reference), Ok(_)) => {
            panic!("{name} ({strictness:?}): the reader accepts, from_slice refuses: {reference:?}")
        }
    }
}

// ---- one thing wrong -------------------------------------------------------------------------------

/// One CBOR value's place in an encoding.
struct Value {
    start: usize,
    /// Where its head ends.
    body: usize,
    end: usize,
    major: u8,
    argument: u64,
}

/// Every value in an encoding, outside the contents of strings, and every map's entries as the
/// ranges of their keys and values.
#[derive(Default)]
struct Layout {
    values: Vec<Value>,
    maps: Vec<Vec<(Range<usize>, Range<usize>)>>,
}

impl Layout {
    /// The layout of a well-formed encoding.
    fn of(bytes: &[u8]) -> Self {
        let mut layout = Layout::default();
        layout.walk(bytes, 0);
        layout
    }

    fn walk(&mut self, bytes: &[u8], at: usize) -> usize {
        let initial = bytes[at];
        let (argument, len) = match initial & 0x1f {
            small @ 0..=23 => (u64::from(small), 1),
            additional => {
                let width = 1 << (additional - 24);
                let argument =
                    bytes[at + 1..=at + width].iter().fold(0, |value, &byte| (value << 8) | u64::from(byte));
                (argument, 1 + width)
            }
        };
        let major = initial >> 5;
        let body = at + len;
        let index = self.values.len();
        self.values.push(Value { start: at, body, end: body, major, argument });
        let end = match major {
            2 | 3 => body + usize::try_from(argument).unwrap(),
            4 => (0..argument).fold(body, |at, _| self.walk(bytes, at)),
            5 => {
                let mut at = body;
                let mut entries = Vec::new();
                for _ in 0..argument {
                    let value = self.walk(bytes, at);
                    let end = self.walk(bytes, value);
                    entries.push((at..value, value..end));
                    at = end;
                }
                self.maps.push(entries);
                at
            }
            6 => self.walk(bytes, body),
            _ => body,
        };
        self.values[index].end = end;
        end
    }
}

/// The value's head, one width longer than it has to be, where there is a longer width and the
/// value is not a float, which a longer width would make a different number.
fn longer_head(value: &Value) -> Option<Vec<u8>> {
    let width = value.body - value.start - 1;
    if value.major == 7 && width > 0 {
        return None;
    }
    let (additional, wider) = match width {
        0 => (24, 1),
        1 => (25, 2),
        2 => (26, 4),
        4 => (27, 8),
        _ => return None,
    };
    let mut head = vec![(value.major << 5) | additional];
    head.extend_from_slice(&value.argument.to_be_bytes()[8 - wider..]);
    Some(head)
}

/// A good bundle, broken in exactly one place, at every place it can be: each with a name saying
/// where and how.
///
/// - Every head, outside the contents of strings: written one width longer than it has to be,
///   given an indefinite length, given a reserved length, and cut off just before.
/// - Every text string: given a byte that is not UTF-8, and where it is ASCII, a sequence that is
///   not NFC.
/// - Every two neighbouring entries of every map: swapped, and the second's key made the first's.
/// - A byte after the end.
/// - Every payload but a stored nested bundle's: a byte of it flipped, so it no longer hashes to
///   its `sha256`, or inflates to something else.
/// - Every stored nested bundle: each of these, inside it, with the outer part's `sha256` made the
///   broken bundle's so that is not wrong too.
///
/// Not every one is malformed: a flipped byte inside a nested bundle's own part changes a payload
/// whose digest nothing checks. Those both readers accept.
pub fn single_faults(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let layout = Layout::of(bytes);
    let splice = |range: Range<usize>, with: &[u8]| {
        let mut out = bytes[..range.start].to_vec();
        out.extend_from_slice(with);
        out.extend_from_slice(&bytes[range.end..]);
        out
    };
    let mut faults = Vec::new();

    for value in &layout.values {
        let (at, head) = (value.start, value.start..value.body);
        if let Some(longer) = longer_head(value) {
            faults.push((format!("a longer head at {at}"), splice(head.clone(), &longer)));
        }
        if (2..=5).contains(&value.major) {
            faults.push((
                format!("an indefinite length at {at}"),
                splice(head.clone(), &[(value.major << 5) | 0x1f]),
            ));
        }
        faults.push((format!("a reserved head at {at}"), splice(head, &[(value.major << 5) | 0x1c])));
        faults.push((format!("cut at {at}"), bytes[..at].to_vec()));
        if value.major == 3 && value.argument > 0 {
            faults.push((format!("text not UTF-8 at {at}"), splice(value.body..value.body + 1, &[0xff])));
            if value.argument >= 3 && bytes[value.body..value.end].is_ascii() {
                faults.push((
                    format!("text not NFC at {at}"),
                    splice(value.body..value.body + 3, "e\u{301}".as_bytes()),
                ));
            }
        }
    }

    for entries in &layout.maps {
        for pair in entries.windows(2) {
            let [(first_key, first_value), (second_key, second_value)] = pair else { continue };
            let mut swapped = bytes[..first_key.start].to_vec();
            swapped.extend_from_slice(&bytes[second_key.start..second_value.end]);
            swapped.extend_from_slice(&bytes[first_key.start..first_value.end]);
            swapped.extend_from_slice(&bytes[second_value.end..]);
            faults.push((
                format!("the entries at {} and {} swapped", first_key.start, second_key.start),
                swapped,
            ));
            faults.push((
                format!("the key at {} repeated", second_key.start),
                splice(second_key.clone(), &bytes[first_key.clone()]),
            ));
        }
        // A part's map: flip a byte of its payload, unless that is a stored nested bundle.
        let is = |key: &Range<usize>, encoded: &[u8]| &bytes[key.clone()] == encoded;
        let stored_bundle = entries.iter().any(|(key, value)| is(key, &[0x01]) && is(value, b"\x66bundle"))
            && !entries.iter().any(|(key, _)| is(key, &[0x07]));
        if let Some((_, payload)) = entries.iter().find(|(key, _)| is(key, &[0x20]))
            && !stored_bundle
            && bytes[payload.start] >> 5 == 2
            && payload.end > payload.start + 1
        {
            let last = payload.end - 1;
            faults.push((
                format!("a payload byte flipped at {last}"),
                splice(last..payload.end, &[!bytes[last]]),
            ));
        }
    }

    let mut trailing = bytes.to_vec();
    trailing.push(0);
    faults.push(("a byte after the end".into(), trailing));

    if let Ok(bundle) = Bundle::from_slice(bytes) {
        for (index, part) in bundle.parts.iter().enumerate() {
            let (PartKind::Bundle, Payload::Embedded(inner)) = (&part.kind, &part.payload) else { continue };
            for (name, broken) in single_faults(inner) {
                let mut outer = bundle.clone();
                outer.parts[index].sha256 = HashValue::sha256_of(&broken);
                outer.parts[index].payload = Payload::Embedded(broken);
                faults.push((format!("parts[{index}]: {name}"), outer.to_cbor().to_cbor_data()));
            }
        }
    }
    faults
}
