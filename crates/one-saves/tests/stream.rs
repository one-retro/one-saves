//! The streaming reader, held to `from_slice`.
//!
//! The contract, and the machinery that checks it, are in `differential/mod.rs`, which the fuzz
//! target in `fuzz/fuzz_targets/stream.rs` shares: the reader refuses what `from_slice` refuses and
//! accepts what it accepts unless a part fails `verify`; where only one thing is wrong it gives the
//! error `from_slice` gives; and where both accept it hands over what `from_slice` read, its
//! content hash included. Here that is checked over the conformance corpus, bundles built to reach
//! what the corpus does not, every single fault of each, and many thousands of random mutations,
//! and then the parts of the contract no comparison can show: when a verdict is reached, what a
//! limit refuses, and that nothing recurses.

use std::fs;
use std::path::Path;

use one_saves::dcbor::CBOR;
use one_saves::stream::{Event, Limit, Limits, Reader};
use one_saves::{
    Bundle, Card, Error, ErrorKind, Game, HashValue, Header, Name, Part, PartKind, Payload, ReverseDnsName,
    Shape, Slug, Source, Strictness,
};

mod differential;

use differential::{
    Agreement, BYTES, Fault, UNEVEN, WHOLE, agree, agree_cut, fault, reference, single_faults, stream,
};

// ---- the corpus ----------------------------------------------------------------------------------

fn corpus() -> Vec<(String, Vec<u8>)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/conformance");
    let mut cases = Vec::new();
    for tier in ["valid", "invalid"] {
        for entry in fs::read_dir(dir.join(tier)).expect("read the corpus") {
            let path = entry.expect("a corpus entry").path();
            let name = format!("{tier}/{}", path.file_stem().unwrap().to_string_lossy());
            cases.push((name, fs::read(&path).expect("read a case")));
        }
    }
    cases.sort();
    assert_eq!(cases.len(), 144, "the corpus is not the size it should be");
    cases
}

/// Cases with more than one thing wrong, where the reader names a different one than `from_slice`
/// and the error it names.
fn named_otherwise(name: &str) -> Option<Fault> {
    match name {
        // The payload is a plain save's bytes: it is not wrapped in the bundle tag, and read as
        // CBOR it is one small value followed by the rest. `from_slice` reads the payload's whole
        // encoding first and names the rest; the reader stops at the first byte, which is not the
        // tag.
        "invalid/bundle-part-payload-not-a-bundle" => Some((
            "parts[0]".into(),
            ErrorKind::NestedPayloadNotABundle(Box::new(error_at_root(&ErrorKind::NotABundle {
                found: None,
            }))),
        )),
        _ => None,
    }
}

/// An error at the top of a bundle, which is how a nested bundle's own error reads inside the
/// error that wraps it.
fn error_at_root(kind: &ErrorKind) -> Error {
    // `Error` has no public constructor, so take one from a decode that fails the same way.
    let error = match kind {
        ErrorKind::NotABundle { found: None } => Bundle::from_slice(&[0x00]).unwrap_err(),
        _ => unreachable!("only the kinds this file needs"),
    };
    assert_eq!(error.kind(), kind);
    error
}

#[test]
fn the_reader_agrees_with_from_slice_on_every_corpus_case() {
    let mut differences = Vec::new();
    for (name, bytes) in corpus() {
        for strictness in [Strictness::Schema, Strictness::Decoder] {
            match agree(&name, &bytes, strictness) {
                Agreement::Accepted | Agreement::SameError => {}
                Agreement::OtherError { reference, streamed } => {
                    if named_otherwise(&name).as_ref() != Some(&streamed) {
                        differences.push(format!(
                            "{name} ({strictness:?}):\n  from_slice {reference:?}\n  reader     {streamed:?}"
                        ));
                    }
                }
            }
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

/// The one valid case the reader refuses, and why it has to.
///
/// The corpus writes `encoding: "zstd"` over bytes that are not a zstd frame: its note says only
/// the parts around a compressed one can be read without inflating, which is true of `from_slice`.
/// A reader that hands over every payload uncompressed cannot hand that one over, and
/// `Part::verify`, which is what a caller of `from_slice` reading the part would run, refuses it in
/// the same words.
#[test]
fn a_compressed_payload_that_is_not_zstd_is_refused_though_from_slice_reads_around_it() {
    let bytes = fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/conformance/valid/compressed-and-aux.cbor"),
    )
    .unwrap();
    assert!(Bundle::from_slice(&bytes).is_ok(), "from_slice does not inflate it");
    let refused = stream(&bytes, Strictness::Decoder, Limits::NONE, WHOLE).unwrap_err();
    #[cfg(feature = "zstd")]
    assert_eq!(refused, ("parts[0]".into(), ErrorKind::ZstdInvalid("Unknown frame descriptor".into())));
    #[cfg(not(feature = "zstd"))]
    assert_eq!(refused, ("parts[0]".into(), ErrorKind::ZstdUnavailable));
}

// ---- bundles the corpus does not have ------------------------------------------------------------

fn slug(text: &str) -> Slug {
    Slug::parse(text).unwrap()
}

fn rdns(text: &str) -> ReverseDnsName {
    ReverseDnsName::parse(text).unwrap()
}

/// Bytes that are not all alike, so a compressor has something to do and a mutation something to
/// break.
fn patterned(len: usize, seed: u8) -> Vec<u8> {
    let low = |n: usize| n.to_le_bytes()[0];
    (0..len).map(|i| low(i).wrapping_mul(31).wrapping_add(seed) ^ low(i >> 7)).collect()
}

/// A part of these bytes, stored compressed with zstd: `frames` of them, one after the other.
#[cfg(feature = "zstd")]
fn compressed(id: u64, plain: &[u8], frames: usize) -> Part {
    // An empty payload is still a frame, of nothing.
    let mut bytes = zstd::stream::encode_all(&plain[..plain.len().min(1)], 3).unwrap();
    let step = plain.len().div_ceil(frames.max(1)).max(1);
    for piece in plain.get(1..).unwrap_or_default().chunks(step) {
        bytes.extend(zstd::stream::encode_all(piece, 3).unwrap());
    }
    let mut part = Part::new(id, plain.to_vec());
    part.payload = Payload::Compressed { bytes, size: plain.len() as u64 };
    part
}

/// A save as a card or a device holds one.
fn save_bytes(title: &str, payload: &[u8]) -> Vec<u8> {
    Bundle::new(
        Header::new(Shape::Save).system(slug("psx")).game(Game::default().title(title)),
        vec![Part::new(0, payload.to_vec())],
    )
    .to_vec()
    .unwrap()
}

/// A save with something in every corner a part map has: roles, paths, extension keys on the
/// header and on a part, and integer keys a later version might write on either side of the
/// payload.
fn busy_save() -> Bundle {
    let mut header = Header::new(Shape::Save)
        .system(slug("gba"))
        .game(Game::default().title("Pokémon Emerald").serial("AGB-BPEE").rom_filename("emerald.gba"))
        .source(Source::default().app(Name::parse("retroarch").unwrap()).core(Name::parse("mgba").unwrap()))
        .bundled_at(1_793_000_000)
        .description("before the Elite Four");
    header.extensions.insert_cbor(rdns("com.example.note"), CBOR::from("kept as is"));
    let mut aux = Part::new(2, patterned(300, 9))
        .kind(PartKind::Aux)
        .path("rtc.bin")
        .content_type("application/octet-stream");
    aux.extensions
        .insert_cbor(rdns("com.example.tag"), CBOR::from(vec![CBOR::from(1u64), CBOR::from("two")]));
    aux.extensions.insert_cbor(rdns("org.example.z"), CBOR::from(false));
    let mut later = Part::new(3, patterned(40, 3)).path("later");
    later.unknown.insert(20, CBOR::from(7u64));
    later.unknown.insert(-5, CBOR::from("after the payload"));
    Bundle::new(
        header,
        vec![
            Part::new(0, patterned(1024, 1)).role(slug("cartridge")),
            Part::new(1, Vec::new()).path("empty"),
            aux,
            later,
        ],
    )
}

/// A PS1 card of two saves and a PS2 card with a system folder beside its save.
fn cards() -> Vec<Bundle> {
    let ps1 = Bundle::new(
        Header::new(Shape::Card).system(slug("psx")).card(Card::new(slug("ps1-mc"), 131_072)),
        vec![
            Part::new(0, save_bytes("Metal Gear Solid", &patterned(8192, 2)))
                .kind(PartKind::Bundle)
                .path("BASLUS-00594")
                .dirent(vec![0x51; 128]),
            Part::new(1, save_bytes("Final Fantasy VII", &patterned(8192, 5)))
                .kind(PartKind::Bundle)
                .path("BASCUS-94163")
                .dirent(vec![0x51; 128])
                .slot(3),
        ],
    );
    let ps2 = Bundle::new(
        Header::new(Shape::Card).card(Card::new(slug("ps2-mc"), 8_388_608)),
        vec![
            Part::new(0, save_bytes("Ico", &patterned(512, 7))).kind(PartKind::Bundle).path("BASCUS-97113"),
            Part::new(1, Vec::new()).kind(PartKind::System).path("BWNETCNF"),
            Part::new(2, *b"NET").kind(PartKind::System).path("BWNETCNF/BWNETCNF"),
        ],
    );
    vec![ps1, ps2]
}

/// A collection of a device of a card of a save, which is as deep as the shapes let bundles nest,
/// beside a save of its own.
fn deepest() -> Bundle {
    let pak = Bundle::new(
        Header::new(Shape::Card).system(slug("n64")).card(Card::new(slug("n64-cpak"), 32_768)),
        vec![
            Part::new(0, save_bytes("Mario Kart 64", &patterned(256, 4)))
                .kind(PartKind::Bundle)
                .path("MARIOKART64")
                .dirent(vec![0; 32]),
        ],
    );
    let device = Bundle::new(
        Header::new(Shape::Device).system(slug("n64")),
        vec![
            Part::new(0, patterned(512, 6)).role(slug("cartridge")),
            Part::new(1, pak.to_vec().unwrap()).kind(PartKind::Bundle).role(slug("controller-pak-1")),
        ],
    );
    Bundle::new(
        Header::new(Shape::Collection),
        vec![
            Part::new(0, device.to_vec().unwrap()).kind(PartKind::Bundle),
            Part::new(1, save_bytes("Ridge Racer", &patterned(64, 8))).kind(PartKind::Bundle).path("other"),
        ],
    )
}

fn state() -> Bundle {
    Bundle::new(
        Header::new(Shape::State)
            .source(Source::default().core(Name::parse("mgba").unwrap()))
            .taken_at(1_793_000_100),
        vec![
            Part::new(0, patterned(2000, 11)).kind(PartKind::State).slot(1),
            Part::new(1, *b"\x89PNG\r\n\x1a\n").kind(PartKind::Screenshot).content_type("image/png"),
        ],
    )
}

/// Bundles with compressed parts: plain ones, ones of two frames, an empty one, and a card whose
/// entry is compressed on the outside.
#[cfg(feature = "zstd")]
fn compressed_bundles() -> Vec<Bundle> {
    let save = Bundle::new(
        Header::new(Shape::Save).system(slug("snes")),
        vec![
            compressed(0, &patterned(40_000, 1), 1).role(slug("cartridge")),
            compressed(1, &patterned(7_000, 2), 3).path("frames"),
            compressed(2, &[], 1).path("empty"),
            Part::new(3, patterned(100, 3)).path("plain"),
        ],
    );
    let mut entry = compressed(0, &save_bytes("Crash Bandicoot", &patterned(8192, 3)), 1);
    entry = entry.kind(PartKind::Bundle).path("BASCUS-94900").dirent(vec![0; 128]);
    let card = Bundle::new(
        Header::new(Shape::Card).system(slug("psx")).card(Card::new(slug("ps1-mc"), 131_072)),
        vec![entry],
    );
    vec![save, card]
}

/// Every bundle built here, encoded.
fn built() -> Vec<(String, Vec<u8>)> {
    let mut bundles = vec![("busy-save".to_owned(), busy_save())];
    for (index, card) in cards().into_iter().enumerate() {
        bundles.push((format!("card-{index}"), card));
    }
    bundles.push(("deepest".into(), deepest()));
    bundles.push(("state".into(), state()));
    #[cfg(feature = "zstd")]
    for (index, bundle) in compressed_bundles().into_iter().enumerate() {
        bundles.push((format!("compressed-{index}"), bundle));
    }
    bundles.into_iter().map(|(name, bundle)| (name, bundle.to_vec().expect("a valid bundle"))).collect()
}

#[test]
fn the_reader_hands_over_what_from_slice_reads_from_every_built_bundle() {
    for (name, bytes) in built() {
        // A later version's integer key is the schema's to refuse, and the busy save has two.
        let expect_schema = name != "busy-save";
        assert!(matches!(agree(&name, &bytes, Strictness::Decoder), Agreement::Accepted), "{name}");
        let schema = agree(&name, &bytes, Strictness::Schema);
        assert_eq!(matches!(schema, Agreement::Accepted), expect_schema, "{name} at the schema");
    }
}

// ---- one thing wrong -----------------------------------------------------------------------------

/// Every good bundle the tests have, at the strictnesses it is good at: the corpus's valid cases
/// the reader accepts too, and every bundle built here.
fn good_bundles() -> Vec<(String, Vec<u8>, Strictness)> {
    let mut good = Vec::new();
    for (name, bytes) in corpus().into_iter().chain(built()) {
        for strictness in [Strictness::Decoder, Strictness::Schema] {
            if reference(&bytes, strictness).is_ok() {
                good.push((name.clone(), bytes.clone(), strictness));
            }
        }
    }
    good
}

/// Where a bundle has one thing wrong with it, the reader names it as `from_slice` does.
///
/// Every good bundle is broken in every single place `single_faults` knows, inside its nested
/// bundles too, and read both ways. Agreement about the verdict is `agree`'s to check; this checks
/// the error.
#[test]
fn one_thing_wrong_is_named_as_from_slice_names_it() {
    let mut differences = Vec::new();
    let (mut faults, mut refused) = (0, 0);
    for (name, bytes, strictness) in good_bundles() {
        for (fault, broken) in single_faults(&bytes) {
            faults += 1;
            let name = format!("{name} ({strictness:?}), {fault}");
            let cuts: &[&[usize]] = if broken.len() <= 4 << 10 { &[BYTES, UNEVEN] } else { &[UNEVEN] };
            match agree_cut(&name, &broken, strictness, cuts) {
                Agreement::Accepted => {}
                Agreement::SameError => refused += 1,
                Agreement::OtherError { reference, streamed } => {
                    differences
                        .push(format!("{name}:\n  from_slice {reference:?}\n  reader     {streamed:?}"));
                }
            }
        }
    }
    assert!(differences.is_empty(), "{} of {faults}:\n{}", differences.len(), differences.join("\n"));
    // Nearly every one is malformed; the few that are not change a nested part's bytes.
    assert!(refused * 50 > faults * 49, "only {refused} of {faults} refused");
}

/// `Done::content_hash` is `Bundle::content_hash` for the same bytes.
///
/// `agree` checks it of everything both accept; this makes sure that includes bundles where the
/// two hashes part, which is only where a part is compressed and the content hash is taken over the
/// bundle as it would be with every part stored.
#[test]
fn the_content_hash_is_bundle_content_hash() {
    let mut parted = 0;
    for (name, bytes) in built().into_iter().chain(corpus()) {
        let Ok(bundle) = reference(&bytes, Strictness::Decoder) else { continue };
        for pieces in [WHOLE, UNEVEN] {
            let done = stream(&bytes, Strictness::Decoder, Limits::NONE, pieces).unwrap().done;
            assert_eq!(done.content_hash, bundle.content_hash().unwrap(), "{name}");
            assert_eq!(done.file_hash, bundle.file_hash().unwrap(), "{name}");
            if done.content_hash != done.file_hash {
                parted += 1;
                assert!(
                    bundle.parts.iter().any(|part| matches!(part.payload, Payload::Compressed { .. })),
                    "{name}"
                );
            }
        }
    }
    #[cfg(feature = "zstd")]
    assert!(parted >= 4, "only {parted} readings of a bundle with a compressed part");
    #[cfg(not(feature = "zstd"))]
    assert_eq!(parted, 0);
}

// ---- mutations -----------------------------------------------------------------------------------

/// A small deterministic generator, so a failure here reproduces.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % n.max(1) as u64).unwrap()
    }
}

/// Bytes that mean something to a CBOR reader: each major type's short and long heads, the
/// indefinite and reserved ones, the simple values, the float heads.
const INTERESTING: [u8; 24] = [
    0x00, 0x17, 0x18, 0x19, 0x1b, 0x1c, 0x1f, 0x20, 0x38, 0x40, 0x58, 0x5f, 0x60, 0x78, 0x80, 0x9f, 0xa0,
    0xbf, 0xc1, 0xd9, 0xf6, 0xf7, 0xf9, 0xff,
];

fn mutate(rng: &mut Rng, bytes: &[u8]) -> Vec<u8> {
    let mut out = bytes.to_vec();
    for _ in 0..=rng.below(2) {
        let at = rng.below(out.len());
        match rng.below(8) {
            0 if !out.is_empty() => out[at] ^= 1 << rng.below(8),
            1 if !out.is_empty() => out[at] = INTERESTING[rng.below(INTERESTING.len())],
            2 => out.insert(at.min(out.len()), INTERESTING[rng.below(INTERESTING.len())]),
            3 if !out.is_empty() => {
                out.remove(at);
            }
            4 => out.truncate(at),
            5 if !out.is_empty() => {
                // A length one off either way, which is most of what a hand-edited file gets wrong.
                out[at] = if rng.below(2) == 0 { out[at].wrapping_add(1) } else { out[at].wrapping_sub(1) };
            }
            6 if !out.is_empty() => {
                let end = (at + 1 + rng.below(16)).min(out.len());
                let copy = out[at..end].to_vec();
                let to = rng.below(out.len());
                out.splice(to..to, copy);
            }
            _ => out.push(INTERESTING[rng.below(INTERESTING.len())]),
        }
    }
    out
}

/// How many mutations each seed gets, which `STREAM_MUTATIONS` raises for a longer run.
fn mutations() -> usize {
    std::env::var("STREAM_MUTATIONS").ok().and_then(|n| n.parse().ok()).unwrap_or(100)
}

#[test]
fn the_reader_agrees_with_from_slice_on_mutated_bundles() {
    let mut seeds = corpus();
    seeds.extend(built());
    let mut rng = Rng(0x5eed_15a5_u64.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
    let (mut accepted, mut refused) = (0, 0);
    for (name, bytes) in &seeds {
        for round in 0..mutations() {
            let mutated = mutate(&mut rng, bytes);
            let strictness = if round % 2 == 0 { Strictness::Decoder } else { Strictness::Schema };
            // A byte at a time is a call per byte, which an unoptimized build makes slow on the
            // larger seeds; the uneven cut still splits every head somewhere.
            let cuts: &[&[usize]] = if mutated.len() <= 16 << 10 { &[BYTES, UNEVEN] } else { &[UNEVEN] };
            match agree_cut(&format!("{name} mutation {round}"), &mutated, strictness, cuts) {
                Agreement::Accepted => accepted += 1,
                _ => refused += 1,
            }
        }
    }
    // Most mutations break a digest, and a run that accepts none at all has not reached past one.
    assert!(accepted > 0 && refused > accepted, "{accepted} accepted, {refused} refused");
}

// ---- when things are decided ---------------------------------------------------------------------

/// Feeds `bytes` until the reader says something other than "more": the first event, or an error.
fn first_word(reader: &mut Reader, bytes: &[u8]) -> Result<Option<String>, Error> {
    let step = reader.read(bytes)?;
    Ok(step.event.map(|event| format!("{event:?}")))
}

/// The encoding of a bundle as it stands, rules or no rules.
fn unchecked(bundle: &Bundle) -> Vec<u8> {
    bundle.to_cbor().to_cbor_data()
}

/// Where a part's payload starts in `bytes`, found by its contents.
fn find(bytes: &[u8], needle: &[u8]) -> usize {
    bytes.windows(needle.len()).position(|window| window == needle).expect("the needle")
}

#[test]
fn a_header_is_refused_before_any_part_arrives() {
    // A state that does not say what it loads into, ahead of a megabyte of the core's data.
    let payload = patterned(1 << 20, 4);
    let bytes = unchecked(&Bundle::new(
        Header::new(Shape::State),
        vec![Part::new(0, payload.clone()).kind(PartKind::State)],
    ));
    let header_end = find(&bytes, b"state") + 5;

    let mut reader = Reader::new(Limits::NONE);
    let error = first_word(&mut reader, &bytes[..header_end]).unwrap_err();
    assert_eq!((error.path(), error.kind()), ("header", &ErrorKind::MissingKey("source")));
    assert_eq!(Bundle::from_slice(&bytes).unwrap_err(), error, "the error from_slice gives");
    // And the reader stays refused.
    assert_eq!(reader.read(&bytes[header_end..]).unwrap_err(), error);
    assert_eq!(reader.finish().unwrap_err(), error);
}

#[test]
fn a_part_is_described_before_its_payload_arrives() {
    let payload = patterned(1 << 16, 2);
    let bundle = Bundle::new(Header::default(), vec![Part::new(7, payload.clone()).path("SAVE")]);
    let bytes = bundle.to_vec().unwrap();
    let payload_at = find(&bytes, &payload[..32]);

    let mut reader = Reader::new(Limits::NONE);
    let mut input = &bytes[..payload_at];
    let mut heads = Vec::new();
    loop {
        let step = reader.read(input).unwrap();
        input = &input[step.consumed..];
        match step.event {
            Some(Event::PartStart(head)) => heads.push(head),
            Some(Event::Data(_)) => panic!("data before the payload arrived"),
            Some(_) => {}
            None => break,
        }
    }
    let [head] = heads.as_slice() else { panic!("one part head, before its payload: {heads:?}") };
    assert_eq!((head.id, head.size, head.compressed), (7, payload.len() as u64, None));
    assert_eq!(head.sha256, HashValue::sha256_of(&payload));
    assert_eq!(head.path.as_deref(), Some("SAVE"));
}

#[test]
fn an_uncompressed_payload_is_handed_over_from_the_input_itself() {
    let payload = patterned(10_000, 1);
    let bytes = Bundle::new(Header::default(), vec![Part::new(0, payload.clone())]).to_vec().unwrap();
    let mut reader = Reader::new(Limits::NONE);
    let mut input = bytes.as_slice();
    let range = bytes.as_ptr_range();
    let mut seen = 0;
    loop {
        let step = reader.read(input).unwrap();
        if let Some(Event::Data(data)) = &step.event {
            assert!(range.contains(&data.as_ptr()), "a copy, not the input");
            seen += data.len();
        }
        input = &input[step.consumed..];
        if step.event.is_none() {
            break;
        }
    }
    assert_eq!(seen, payload.len());
    reader.finish().unwrap();
}

#[test]
fn a_digest_that_does_not_match_is_refused_at_the_end_of_its_part() {
    // `from_slice` leaves a plain part's digest to whoever reads it, and the reader reads it.
    let mut bundle =
        Bundle::new(Header::default(), vec![Part::new(0, *b"SAVE"), Part::new(1, *b"MORE").path("b")]);
    bundle.parts[0].sha256 = HashValue::sha256_of(b"something else");
    let bytes = bundle.to_vec().unwrap();
    assert!(Bundle::from_slice(&bytes).is_ok());

    let mut reader = Reader::new(Limits::NONE);
    let mut input = bytes.as_slice();
    let mut data = Vec::new();
    let error = loop {
        match reader.read(input) {
            Ok(step) => {
                match &step.event {
                    Some(Event::Data(bytes)) => data.extend_from_slice(bytes),
                    Some(Event::PartEnd(_)) => panic!("the part ended as though good"),
                    Some(_) => {}
                    None => panic!("the whole bundle taken without a verdict"),
                }
                input = &input[step.consumed..];
            }
            Err(error) => break error,
        }
    };
    assert_eq!(data, b"SAVE", "every byte went by before the verdict on them");
    assert_eq!((error.path(), error.kind()), ("parts[0]", &ErrorKind::PayloadHashMismatch));
}

#[test]
fn bytes_after_the_bundle_are_refused_when_they_arrive() {
    let mut bytes = Bundle::new(Header::default(), vec![Part::new(0, *b"SAVE")]).to_vec().unwrap();
    let end = bytes.len();
    bytes.extend_from_slice(&[0; 9]);
    assert!(matches!(agree("trailing", &bytes, Strictness::Decoder), Agreement::SameError));

    // Cut, the count is what had arrived.
    let mut reader = Reader::new(Limits::NONE);
    let mut input = &bytes[..end + 4];
    while let Ok(step) = reader.read(input) {
        input = &input[step.consumed..];
        if step.event.is_none() {
            break;
        }
    }
    let error = reader.finish().unwrap_err();
    assert_eq!(error.kind(), &ErrorKind::Encoding("the decoded CBOR had 4 extra bytes at the end".into()));
}

#[test]
fn a_bundle_cut_short_is_refused_at_the_end() {
    let bytes = Bundle::new(Header::default(), vec![Part::new(0, *b"SAVE")]).to_vec().unwrap();
    for len in 0..bytes.len() {
        let error = stream(&bytes[..len], Strictness::Decoder, Limits::NONE, UNEVEN).unwrap_err();
        assert_eq!(error, fault(&Bundle::from_slice(&bytes[..len]).unwrap_err()), "cut at {len}");
    }
}

// ---- nesting ---------------------------------------------------------------------------------------

/// A header carrying an extension value nested `depth` arrays deep, and nothing after it.
fn nested_header(depth: usize) -> Vec<u8> {
    let mut bytes = vec![0xda, 0x31, 0x53, 0x41, 0x56, 0x82];
    // {0: "save", "com.example.deep": [[[ ... 0 ... ]]]}
    bytes.extend_from_slice(&[0xa2, 0x00]);
    bytes.extend(CBOR::from("save").to_cbor_data());
    bytes.extend(CBOR::from("com.example.deep").to_cbor_data());
    bytes.extend(std::iter::repeat_n(0x81, depth));
    bytes.push(0x00);
    bytes
}

/// The parts array of a bundle of one empty part, to follow a header.
fn one_empty_part() -> Vec<u8> {
    // [{0: 0, 9: sha256(""), -1: h''}]
    let mut bytes = vec![0x81, 0xa3, 0x00, 0x00, 0x09, 0xd9, 0x48, 0x6c, 0x58, 0x20];
    bytes.extend_from_slice(HashValue::sha256_of(b"").digest());
    bytes.extend_from_slice(&[0x20, 0x40]);
    bytes
}

#[test]
fn a_value_nested_without_end_is_refused_without_recursing() {
    let bytes = nested_header(1_000_000);
    let expected = fault(&Bundle::from_slice(&bytes).unwrap_err());
    assert!(matches!(expected.1, ErrorKind::TooDeep { .. }));
    // A million levels of anything that recursed would take tens of megabytes of stack. This is
    // what an unoptimized build's frames need without recursing, with room to spare.
    let streamed = std::thread::Builder::new()
        .stack_size(256 << 10)
        .spawn(move || stream(&bytes, Strictness::Decoder, Limits::NONE, UNEVEN).unwrap_err())
        .unwrap()
        .join()
        .expect("the reader overflowed a small stack");
    assert_eq!(streamed, expected);
}

#[test]
fn nesting_up_to_the_limit_reads() {
    // The header map, inside the tag and the envelope, is at depth 3; the value's arrays go on
    // from there, and dcbor's limit counts them all.
    for (depth, reads) in [(124, true), (125, true), (126, false)] {
        let mut bytes = nested_header(depth);
        bytes.extend(one_empty_part());
        let agreement = agree(&format!("nested {depth}"), &bytes, Strictness::Decoder);
        assert_eq!(matches!(agreement, Agreement::Accepted), reads, "{depth} deep");
    }
}

/// A card holding one entry made of these bytes, its digest theirs, unchecked.
fn card_holding(entry: Vec<u8>) -> Vec<u8> {
    unchecked(&Bundle::new(
        Header::new(Shape::Card).card(Card::new(slug("ps1-mc"), 131_072)),
        vec![Part::new(0, entry).kind(PartKind::Bundle).path("BASLUS-00594").dirent(vec![0; 128])],
    ))
}

#[test]
fn a_nested_bundle_is_held_to_what_from_slice_holds_it_to() {
    let save = save_bytes("Metal Gear Solid", b"SAVE");
    let mut cases: Vec<(&str, Vec<u8>)> = Vec::new();

    // Its own header is wrong.
    cases.push((
        "inner header",
        unchecked(&Bundle::new(
            Header::new(Shape::Save).card(Card::new(slug("ps1-mc"), 1)),
            vec![Part::new(0, *b"S")],
        )),
    ));
    // It is a card, which a card does not hold.
    cases.push(("card in a card", card_holding(save.clone())));
    // Its part is compressed.
    let mut squeezed = Bundle::new(Header::new(Shape::Save), vec![Part::new(0, *b"SAVE")]);
    squeezed.parts[0].payload = Payload::Compressed { bytes: b"not even zstd".to_vec(), size: 4 };
    cases.push(("compressed inside", unchecked(&squeezed)));
    // Its part's entry is the wrong length for the card around it.
    cases.push((
        "inner dirent",
        unchecked(&Bundle::new(Header::new(Shape::Save), vec![Part::new(0, *b"S").dirent(vec![0; 3])])),
    ));
    // It goes on past its end, and stops short of it.
    let mut longer = save.clone();
    longer.extend_from_slice(&[0; 5]);
    cases.push(("trailing", longer));
    cases.push(("short", save[..save.len() - 2].to_vec()));
    // It nests without end.
    cases.push(("deep", nested_header(10_000)));
    // It is not CBOR at all.
    cases.push(("garbage", vec![0xff, 0x00]));
    // It is a later version's shape, whose rules are its own.
    cases.push((
        "unknown shape",
        Bundle::new(Header::new(Shape::from_slug(slug("disc-set"))), vec![Part::new(0, *b"S")])
            .to_vec()
            .unwrap(),
    ));

    for (name, entry) in cases {
        let outer =
            if name == "card in a card" { card_holding(card_holding(entry)) } else { card_holding(entry) };
        match agree(name, &outer, Strictness::Decoder) {
            Agreement::SameError => {}
            Agreement::Accepted => assert_eq!(name, "unknown shape"),
            Agreement::OtherError { reference, streamed } => {
                panic!("{name}:\n  from_slice {reference:?}\n  reader     {streamed:?}")
            }
        }
    }

    // A digest that is not the inner bundle's.
    let mut wrong = Bundle::from_slice(&card_holding(save)).unwrap();
    wrong.parts[0].sha256 = HashValue::sha256_of(b"another");
    let bytes = unchecked(&wrong);
    assert!(matches!(agree("nested digest", &bytes, Strictness::Decoder), Agreement::SameError));
    let error = stream(&bytes, Strictness::Decoder, Limits::NONE, WHOLE).unwrap_err();
    assert_eq!(error, ("parts[0]".into(), ErrorKind::NestedHashMismatch));
}

#[test]
fn a_fault_three_bundles_down_is_named_from_the_outside_in() {
    let mut collection = Bundle::from_slice(&deepest().to_vec().unwrap()).unwrap();
    let mut device = Bundle::from_slice(&collection.parts[0].payload_bytes()).unwrap();
    let mut pak = Bundle::from_slice(&device.parts[1].payload_bytes()).unwrap();
    // The note's name is not spelled from an N64 pak's table.
    pak.parts[0].path = Some("mariokart64".into());
    device.parts[1] = Part::new(1, unchecked(&pak)).kind(PartKind::Bundle).role(slug("controller-pak-1"));
    collection.parts[0] = Part::new(0, unchecked(&device)).kind(PartKind::Bundle);
    let bytes = unchecked(&collection);
    assert!(matches!(agree("three down", &bytes, Strictness::Decoder), Agreement::SameError));
    let error = stream(&bytes, Strictness::Decoder, Limits::NONE, WHOLE).unwrap_err();
    assert_eq!(error.0, "parts[0].payload.parts[1].payload.parts[0].path");
}

trait PayloadBytes {
    fn payload_bytes(&self) -> Vec<u8>;
}

impl PayloadBytes for Part {
    fn payload_bytes(&self) -> Vec<u8> {
        self.bytes().unwrap().into_owned()
    }
}

// ---- zstd ------------------------------------------------------------------------------------------

#[cfg(feature = "zstd")]
mod zstd_payloads {
    use super::*;

    /// A save of one part whose payload is these compressed bytes, claiming `size`.
    fn claiming(bytes: Vec<u8>, size: u64, plain: &[u8]) -> Vec<u8> {
        let mut part = Part::new(0, plain.to_vec());
        part.payload = Payload::Compressed { bytes, size };
        Bundle::new(Header::default(), vec![part]).to_vec().unwrap()
    }

    #[test]
    fn a_payload_that_inflates_past_its_size_is_abandoned_there() {
        // Four megabytes of nothing, in a few hundred bytes, claiming 64.
        let plain = vec![0u8; 4 << 20];
        let bomb = zstd::stream::encode_all(&plain[..], 3).unwrap();
        let bytes = claiming(bomb, 64, &plain[..64]);

        let mut reader = Reader::new(Limits::NONE);
        let mut input = bytes.as_slice();
        let mut handed = 0;
        let error = loop {
            match reader.read(input) {
                Ok(step) => {
                    match &step.event {
                        Some(Event::Data(data)) => handed += data.len(),
                        Some(_) => {}
                        None => panic!("the whole bundle taken without a verdict"),
                    }
                    input = &input[step.consumed..];
                }
                Err(error) => break error,
            }
        };
        assert!(handed <= 64, "{handed} bytes handed over of a payload that claimed 64");
        assert_eq!(error.kind(), &ErrorKind::ZstdInvalid("inflated past the stated 64 bytes".into()));
        assert!(matches!(agree("bomb", &bytes, Strictness::Decoder), Agreement::SameError));
    }

    #[test]
    fn a_payload_that_falls_short_or_trails_off_is_refused_in_part_bytes_words() {
        let plain = patterned(5000, 3);
        let frame = zstd::stream::encode_all(&plain[..], 3).unwrap();
        let mut trailing = frame.clone();
        trailing.extend_from_slice(b"junk");
        let cases = [
            ("short", claiming(frame.clone(), 6000, &plain)),
            ("trailing", claiming(trailing, 5000, &plain)),
            ("truncated", claiming(frame[..frame.len() - 3].to_vec(), 5000, &plain)),
            ("empty", claiming(Vec::new(), 0, &[])),
            ("corrupt", {
                let mut corrupt = frame.clone();
                let middle = corrupt.len() / 2;
                corrupt[middle] ^= 0xff;
                claiming(corrupt, 5000, &plain)
            }),
        ];
        for (name, bytes) in cases {
            match agree(name, &bytes, Strictness::Decoder) {
                Agreement::SameError => {}
                Agreement::Accepted => panic!("{name} accepted"),
                Agreement::OtherError { reference, streamed } => {
                    panic!("{name}: {reference:?} / {streamed:?}")
                }
            }
        }
    }

    #[test]
    fn a_window_past_the_limit_is_refused_and_one_within_it_is_not() {
        // zstd's streaming encoder, which does not know the input's size, takes the window level
        // 19 takes for anything: 8 MiB.
        let plain = patterned(100_000, 5);
        let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), 19).unwrap();
        std::io::Write::write_all(&mut encoder, &plain).unwrap();
        let frame = encoder.finish().unwrap();
        let bytes = claiming(frame, plain.len() as u64, &plain);

        let refused =
            stream(&bytes, Strictness::Decoder, Limits::NONE.zstd_window(1 << 20), WHOLE).unwrap_err();
        assert_eq!(
            refused,
            ("parts[0]".into(), ErrorKind::OverLimit { limit: Limit::ZstdWindow, max: 1 << 20 })
        );
        stream(&bytes, Strictness::Decoder, Limits::NONE.zstd_window(8 << 20), WHOLE)
            .expect("8 MiB is enough");
        stream(&bytes, Strictness::Decoder, Limits::NONE, WHOLE).expect("and no limit is zstd's own");
    }

    #[test]
    fn a_compressed_part_is_judged_by_its_size_before_it_inflates() {
        let plain = patterned(10_000, 5);
        let bytes = claiming(zstd::stream::encode_all(&plain[..], 3).unwrap(), 10_000, &plain);
        let error = stream(&bytes, Strictness::Decoder, Limits::NONE.part_size(9_999), BYTES).unwrap_err();
        assert_eq!(error, ("parts[0]".into(), ErrorKind::OverLimit { limit: Limit::PartSize, max: 9_999 }));
    }
}

#[cfg(not(feature = "zstd"))]
#[test]
fn a_compressed_part_cannot_be_handed_over_without_zstd() {
    let mut part = Part::new(0, *b"SAVE");
    part.payload = Payload::Compressed { bytes: vec![0; 13], size: 4 };
    let bytes = Bundle::new(Header::default(), vec![part]).to_vec().unwrap();
    let mut reader = Reader::new(Limits::NONE);
    let payload_at = bytes.len() - 13;
    let mut input = &bytes[..payload_at];
    let error = loop {
        match reader.read(input) {
            Ok(step) => {
                assert!(step.event.is_some(), "the part's head taken without a verdict");
                input = &input[step.consumed..];
            }
            Err(error) => break error,
        }
    };
    assert_eq!((error.path(), error.kind()), ("parts[0]", &ErrorKind::ZstdUnavailable));
}

// ---- limits ----------------------------------------------------------------------------------------

/// Reads `bytes` only as far as `upto`, and returns the error the reader gave by then.
fn refused_by(bytes: &[u8], upto: usize, limits: Limits) -> Fault {
    let mut reader = Reader::new(limits);
    let mut input = &bytes[..upto];
    loop {
        match reader.read(input) {
            Ok(step) => {
                assert!(!matches!(step.event, Some(Event::Data(_))), "data before the verdict");
                assert!(step.event.is_some(), "taken {upto} bytes without refusing them");
                input = &input[step.consumed..];
            }
            Err(error) => return fault(&error),
        }
    }
}

#[test]
fn a_payload_over_the_limit_is_refused_at_its_head() {
    let payload = patterned(1000, 1);
    let bytes = Bundle::new(Header::default(), vec![Part::new(0, payload.clone())]).to_vec().unwrap();
    let head_end = find(&bytes, &payload[..16]);
    assert_eq!(
        refused_by(&bytes, head_end, Limits::NONE.part_size(999)),
        ("parts[0]".into(), ErrorKind::OverLimit { limit: Limit::PartSize, max: 999 })
    );
    stream(&bytes, Strictness::Decoder, Limits::NONE.part_size(1000), WHOLE).expect("at the limit");
}

#[test]
fn a_header_over_the_limit_is_refused_at_the_string_that_takes_it_over() {
    let description = "x".repeat(1000);
    let bytes = Bundle::new(Header::default().description(description.clone()), vec![Part::new(0, *b"S")])
        .to_vec()
        .unwrap();
    let string_at = find(&bytes, description.as_bytes());
    assert_eq!(
        refused_by(&bytes, string_at, Limits::NONE.header_bytes(500)),
        ("header".into(), ErrorKind::OverLimit { limit: Limit::HeaderBytes, max: 500 })
    );
}

#[test]
fn a_map_declaring_more_than_could_fit_is_refused_at_its_head() {
    // A header map that says it holds four billion entries, and nothing after.
    let bytes = [0xda, 0x31, 0x53, 0x41, 0x56, 0x82, 0xba, 0xff, 0xff, 0xff, 0xff];
    assert_eq!(
        refused_by(&bytes, bytes.len(), Limits::NONE.header_bytes(64 << 10)),
        ("header".into(), ErrorKind::OverLimit { limit: Limit::HeaderBytes, max: 64 << 10 })
    );
}

#[test]
fn too_many_parts_are_refused_at_the_array_head() {
    let parts = (0..5).map(|id| Part::new(id, *b"S").slot(id)).collect();
    let bytes = Bundle::new(Header::default(), parts).to_vec().unwrap();
    // The tag, the envelope, the header's one key, and the parts array's head.
    let upto = 5 + 1 + 7 + 1;
    assert_eq!(bytes[upto - 1], 0x85, "the parts array's head");
    assert_eq!(
        refused_by(&bytes, upto, Limits::NONE.parts(4)),
        ("parts".into(), ErrorKind::OverLimit { limit: Limit::Parts, max: 4 })
    );
}

#[test]
fn a_part_whose_keys_run_long_is_refused_at_the_key_that_takes_them_over() {
    let dirent = vec![0xaa; 1000];
    let bytes =
        Bundle::new(Header::default(), vec![Part::new(0, *b"S").dirent(dirent.clone())]).to_vec().unwrap();
    let dirent_at = find(&bytes, &dirent);
    assert_eq!(
        refused_by(&bytes, dirent_at, Limits::NONE.part_metadata_bytes(256)),
        ("parts[0]".into(), ErrorKind::OverLimit { limit: Limit::PartMetadataBytes, max: 256 })
    );
}

#[test]
fn a_nested_bundle_is_held_to_the_same_limits() {
    let entry = Bundle::new(Header::new(Shape::Save).description("y".repeat(300)), vec![Part::new(0, *b"S")])
        .to_vec()
        .unwrap();
    let bytes = card_holding(entry);
    let error = stream(&bytes, Strictness::Decoder, Limits::NONE.header_bytes(200), WHOLE).unwrap_err();
    assert_eq!(
        error,
        ("parts[0].payload.header".into(), ErrorKind::OverLimit { limit: Limit::HeaderBytes, max: 200 })
    );
}

// ---- unknown keys ---------------------------------------------------------------------------------

#[test]
fn a_later_versions_keys_land_either_side_of_the_payload() {
    let bytes = busy_save().to_vec().unwrap();
    let read = stream(&bytes, Strictness::Decoder, Limits::NONE, WHOLE).unwrap();
    let (head, _, tail) = &read.parts[3];
    assert_eq!(head.unknown.keys().copied().collect::<Vec<_>>(), [20]);
    assert_eq!(tail.unknown.keys().copied().collect::<Vec<_>>(), [-5]);
    assert_eq!(head.keys_after_payload, 1);
    let (head, _, tail) = &read.parts[2];
    assert_eq!((head.keys_after_payload, tail.extensions.len()), (2, 2));

    // The schema refuses both, and `from_slice` names the lower, which follows the payload. The
    // part is refused for the first at its payload's head, but its map goes on past the payload,
    // so the reader reads to its end before saying which.
    let refused = stream(&bytes, Strictness::Schema, Limits::NONE, WHOLE).unwrap_err();
    assert_eq!(refused, ("parts[3]".into(), ErrorKind::UnknownIntegerKey(-5)));
    assert_eq!(refused, fault(&Bundle::from_slice_with(&bytes, Strictness::Schema).unwrap_err()));
}

/// A key out of order after the payload is what a part lacked at the payload's head, and the
/// error is the order, as `from_slice` gives it; where nothing follows the payload the part is
/// refused at its head, before its payload arrives.
#[test]
fn a_part_refused_at_its_payload_waits_for_its_map_only_where_the_map_goes_on() {
    // {0: 0, -1: h'..', 9: sha256}: the digest's key, after the payload.
    let payload = patterned(1000, 4);
    let mut misordered = vec![0xda, 0x31, 0x53, 0x41, 0x56, 0x82, 0xa1, 0x00, 0x64];
    misordered.extend_from_slice(b"save");
    misordered.extend_from_slice(&[0x81, 0xa3, 0x00, 0x00, 0x20, 0x59, 0x03, 0xe8]);
    misordered.extend_from_slice(&payload);
    misordered.extend_from_slice(&[0x09, 0xd9, 0x48, 0x6c, 0x58, 0x20]);
    misordered.extend_from_slice(HashValue::sha256_of(&payload).digest());
    let expected = fault(&Bundle::from_slice(&misordered).unwrap_err());
    assert_eq!(expected.1, ErrorKind::Encoding(one_saves::dcbor::Error::MisorderedMapKey.to_string()));
    assert!(matches!(
        agree("digest after the payload", &misordered, Strictness::Decoder),
        Agreement::SameError
    ));

    // Its payload is passed over rather than handed on.
    let mut reader = Reader::new(Limits::NONE);
    let mut input = misordered.as_slice();
    let error = loop {
        match reader.read(input) {
            Ok(step) => {
                assert!(
                    !matches!(step.event, Some(Event::PartStart(_) | Event::Data(_))),
                    "a refused part handed on"
                );
                assert!(step.event.is_some(), "the whole bundle taken without a verdict");
                input = &input[step.consumed..];
            }
            Err(error) => break fault(&error),
        }
    };
    assert_eq!(error, expected);

    // Without the digest at all, nothing follows the payload: refused at its head.
    let mut missing = misordered[..misordered.len() - 38].to_vec();
    missing[misordered.iter().position(|&byte| byte == 0xa3).unwrap()] = 0xa2;
    let head_end = missing.len() - payload.len();
    assert_eq!(
        refused_by(&missing, head_end, Limits::NONE),
        ("parts[0]".into(), ErrorKind::MissingKey("sha256"))
    );
    assert_eq!(
        fault(&Bundle::from_slice(&missing).unwrap_err()),
        ("parts[0]".into(), ErrorKind::MissingKey("sha256"))
    );
}

// ---- the io::Read wrapper -------------------------------------------------------------------------

#[test]
fn read_from_drives_the_reader_from_a_reader() {
    let bytes = deepest().to_vec().unwrap();
    let mut parts = 0;
    let mut data = 0;
    let done = Reader::new(Limits::NONE)
        .read_from(std::io::Cursor::new(&bytes), |event| {
            match event {
                Event::PartStart(_) => parts += 1,
                Event::Data(bytes) => data += bytes.len(),
                _ => {}
            }
            Ok::<_, Box<dyn std::error::Error>>(())
        })
        .unwrap();
    assert_eq!(parts, 2);
    assert_eq!(done.file_hash, HashValue::sha256_of(&bytes));
    assert!(data > 0);

    let error = Reader::new(Limits::NONE)
        .read_from(std::io::Cursor::new(&bytes[..bytes.len() - 1]), |_| {
            Ok::<_, Box<dyn std::error::Error>>(())
        })
        .unwrap_err();
    assert!(error.downcast_ref::<Error>().is_some(), "{error}");
}
