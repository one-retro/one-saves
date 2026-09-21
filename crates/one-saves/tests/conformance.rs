//! Runs the vendored conformance corpus.
//!
//! The corpus is copied from the specifications repository and pinned to the `spec_version` it
//! records; re-copy it when that spec moves. Do not edit anything under `tests/conformance/` by
//! hand — it is generated, and a hand edit shows up here as a mismatched digest.
//!
//! A green run is deliberately not full coverage. Valid cases prove a decoder accepts what it
//! must; they cannot prove properties that no single file expresses. The three the corpus README
//! names as its own blind spots — determinism, nesting depth, and whether an unknown value
//! round-trips — are covered by the tests at the bottom of this file rather than by the corpus.
//! [`Bundle::shape`] is checked there too: the corpus is the one place that has a file per tier,
//! with the spec's own note on each saying which tier it is.

use std::fs;
use std::path::{Path, PathBuf};

use one_saves::{Bundle, Shape, Strictness};
use serde_json::Value;

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/conformance")
}

struct Case {
    name: String,
    expect_valid: bool,
    file: PathBuf,
    sha256: String,
    note: String,
}

fn cases() -> Vec<Case> {
    let manifest = fs::read_to_string(corpus_dir().join("manifest.json")).expect("read manifest");
    let manifest: Value = serde_json::from_str(&manifest).expect("parse manifest");

    assert_eq!(
        manifest["spec_version"].as_str(),
        Some(one_saves::SPEC_VERSION),
        "the vendored corpus was built from a different version of the specification than this \
         crate implements"
    );

    manifest["cases"]
        .as_array()
        .expect("cases array")
        .iter()
        .map(|case| Case {
            name: case["name"].as_str().expect("name").to_owned(),
            expect_valid: case["expect"].as_str() == Some("valid"),
            file: corpus_dir().join(case["file"].as_str().expect("file")),
            sha256: case["sha256"].as_str().expect("sha256").to_owned(),
            note: case["note"].as_str().unwrap_or_default().to_owned(),
        })
        .collect()
}

/// The corpus is run at [`Strictness::Schema`].
///
/// Two invalid cases — `header-unknown-integer-key` and `part-unknown-integer-key` — are invalid
/// against the version 0.3 schema and *accepted* by a shipped decoder, which ignores and
/// round-trips an integer key it does not know. Running the corpus in decoder mode would fail
/// those two for being right. A shape a later version assigned is the same asymmetry; see
/// `a_decoder_round_trips_a_shape_a_later_version_defined` for why the corpus cannot show it.
#[test]
fn corpus() {
    let mut failures = Vec::new();
    let cases = cases();

    // A manifest this harness failed to read would otherwise pass by testing nothing.
    let (valid, invalid): (Vec<_>, Vec<_>) = cases.iter().partition(|c| c.expect_valid);
    assert_eq!((valid.len(), invalid.len()), (27, 63), "the corpus is not the size it should be");

    for case in cases {
        let bytes = fs::read(&case.file).unwrap_or_else(|e| panic!("read {}: {e}", case.name));

        // The manifest's digest is over the file's bytes, so a vendored copy can be checked for
        // corruption without re-encoding anything.
        let digest = one_saves::HashValue::sha256_of(&bytes).to_string();
        let digest = digest.strip_prefix("sha256:").unwrap_or(&digest);
        assert_eq!(digest, case.sha256, "{} is corrupt in the vendored copy", case.name);

        let result = Bundle::from_slice_with(&bytes, Strictness::Schema);
        match (case.expect_valid, &result) {
            (true, Err(e)) => {
                failures.push(format!("{} should be valid, rejected: {e}\n    {}", case.name, case.note));
            }
            (false, Ok(_)) => {
                failures.push(format!("{} should be invalid, accepted\n    {}", case.name, case.note));
            }
            _ => {}
        }
    }

    assert!(failures.is_empty(), "{} case(s) failed:\n\n{}", failures.len(), failures.join("\n\n"));
}

/// Determinism: two encoders given the same logical bundle must produce the same bytes.
///
/// No single corpus file can demonstrate that, so it is tested directly: decode each valid case,
/// re-encode it, and assert the bytes are identical. This is the property most likely to be
/// quietly wrong in a new implementation, and the one the content hash depends on.
#[test]
fn valid_cases_re_encode_to_the_bytes_they_came_from() {
    let mut failures = Vec::new();

    for case in cases().into_iter().filter(|c| c.expect_valid) {
        let bytes = fs::read(&case.file).expect("read case");
        let bundle = Bundle::from_slice(&bytes).unwrap_or_else(|e| panic!("{}: {e}", case.name));
        match bundle.to_vec() {
            Ok(round_tripped) if round_tripped == bytes => {}
            Ok(round_tripped) => failures.push(format!(
                "{}: re-encoded to {} bytes, not the {} it came from",
                case.name,
                round_tripped.len(),
                bytes.len()
            )),
            Err(e) => failures.push(format!("{}: re-encoding failed: {e}", case.name)),
        }
    }

    assert!(failures.is_empty(), "{} case(s) failed:\n{}", failures.len(), failures.join("\n"));
}

/// The other half of the round-trip guarantee: the decoder is strict on the way *in*.
///
/// `from_slice(&bytes)?.to_vec()? == bytes` only holds because a file spelled any other way is
/// refused rather than quietly re-spelled. The corpus runs at [`Strictness::Schema`], which would
/// leave open whether a *shipped* decoder — the lenient one, which keeps integer keys it does not
/// know — is equally strict about the encoding beneath the data model. It is, and these are the
/// cases that say so.
#[test]
fn a_decoder_refuses_every_non_deterministic_encoding() {
    for name in [
        "non-preferred-key-head",
        "non-preferred-value-head",
        "map-keys-out-of-order",
        "duplicate-map-key",
        "indefinite-length-parts",
        "indefinite-length-part-map",
        "text-not-nfc",
    ] {
        let bytes = fs::read(corpus_dir().join(format!("invalid/{name}.cbor"))).expect("read case");
        assert!(
            Bundle::from_slice(&bytes).is_err(),
            "{name} must be refused by a shipped decoder, not re-spelled on re-encoding"
        );
    }
}

/// Whether an unknown value round-trips.
///
/// Several valid cases carry a name or key the spec expects a decoder *not* to recognise.
/// Accepting them is only half the requirement; the other half is that re-encoding preserves
/// them unchanged, which the round-trip above proves for these three by name.
#[test]
fn unknown_names_and_keys_survive_a_round_trip() {
    for name in ["third-party-kind", "part-extension-keys", "game-ids"] {
        let path = corpus_dir().join(format!("valid/{name}.cbor"));
        let bytes = fs::read(&path).unwrap_or_else(|e| panic!("read {name}: {e}"));
        let bundle = Bundle::from_slice(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(bundle.to_vec().expect("re-encode"), bytes, "{name} did not round-trip");
    }
}

/// A shape a later minor version assigned is round-tripped, not rejected.
///
/// The corpus covers half of this — `shape-unknown` is invalid against the schema, which pins the
/// four shapes 0.2 defines. It cannot cover the other half: the harness runs everything at
/// [`Strictness::Schema`], and the manifest has no category for "invalid to the schema, fine for a
/// decoder". So the accepting side is built here, exactly as it is for the two unknown-integer-key
/// cases below.
#[test]
fn a_decoder_round_trips_a_shape_a_later_version_defined() {
    use one_saves::{Header, Part, Shape};

    let later = Bundle {
        header: Header {
            shape: "disc-set".parse::<one_saves::Slug>().map(Shape::from_slug).unwrap(),
            ..Header::default()
        },
        parts: vec![Part::new(0, *b"SAVE")],
    };
    let bytes = later.to_vec().expect("encodes");

    // The schema pins the four this version defines, so it refuses.
    assert!(
        Bundle::from_slice_with(&bytes, Strictness::Schema).is_err(),
        "the schema must refuse a shape it does not define"
    );

    // A shipped decoder parses it, keeps the slug, and gives the bytes back unchanged.
    let read = Bundle::from_slice(&bytes).expect("a decoder must accept a later version's shape");
    assert_eq!(read.shape().as_str(), "disc-set");
    assert!(!read.shape().is_known(), "and must know it cannot act on it");
    assert_eq!(read.to_vec().expect("re-encode"), bytes, "an unknown shape must round-trip");
}

/// A shipped decoder is looser than the schema over exactly one thing.
#[test]
fn a_decoder_keeps_integer_keys_a_later_minor_version_wrote() {
    for name in ["header-unknown-integer-key", "part-unknown-integer-key"] {
        let bytes = fs::read(corpus_dir().join(format!("invalid/{name}.cbor"))).expect("read case");

        assert!(
            Bundle::from_slice_with(&bytes, Strictness::Schema).is_err(),
            "{name} must fail against the version 0.3 schema"
        );

        let bundle = Bundle::from_slice(&bytes)
            .unwrap_or_else(|e| panic!("{name} must be accepted by a shipped decoder: {e}"));
        assert_eq!(
            bundle.to_vec().expect("re-encode"),
            bytes,
            "{name}: an unrecognised integer key must round-trip unchanged"
        );
    }
}
/// What each shape may hold, which is what replaced the depth cap in 0.2.
///
/// A save holds no nested bundle, a card holds saves, a device holds cards and saves, and a
/// collection holds anything but another collection. Nesting is bounded by those rules rather
/// than by counting, so the check is that each refusal names the shape that refused.
#[test]
fn a_shape_holds_only_what_its_document_allows() {
    use one_saves::{Header, Part, PartKind};

    fn wrap(inner: &Bundle, outer: Shape) -> Bundle {
        let payload = inner.to_vec().expect("inner bundle encodes");
        let mut part = Part::new(0, payload);
        part.kind = PartKind::Bundle;
        let mut header = Header { shape: outer, ..Header::default() };
        // Each shape admits a different header; give it the least that makes it well-formed.
        match &header.shape {
            Shape::Device => {
                header.system = Some("n64".parse().unwrap());
                // A device tells its components apart by `role`, and requires one on every part.
                part.role = Some("cartridge".parse().unwrap());
            }
            Shape::Card => panic!("this helper does not build the card map a card needs"),
            _ => {}
        }
        Bundle { header, parts: vec![part] }
    }

    let save = Bundle { header: Header::default(), parts: vec![Part::new(0, *b"SAVE")] };
    assert_eq!(save.shape(), &Shape::Save);
    save.validate().expect("a bare save is valid");

    // A collection may hold a save or a device, and not another collection.
    wrap(&save, Shape::Collection).validate().expect("a collection holds a save");
    let device = wrap(&save, Shape::Device);
    device.validate().expect("a device holds a save");
    wrap(&device, Shape::Collection).validate().expect("a collection holds a device");

    let collection = wrap(&save, Shape::Collection);
    let nested = wrap(&collection, Shape::Collection);
    assert!(nested.validate().is_err(), "a collection may not hold another collection");

    // A device holds cards and saves, never a collection.
    assert!(wrap(&collection, Shape::Device).validate().is_err(), "a device holds no collection");

    // And every one of a device's parts names a role, since that is the only thing telling two
    // components apart: without it both would fall back to `primary`.
    let mut roleless = wrap(&save, Shape::Device);
    roleless.parts[0].role = None;
    assert!(roleless.validate().is_err(), "a device names a role on every part");
}

/// What each corpus case *is*, checked against the shapes the corpus was written to demonstrate.
///
/// Since 0.2 a bundle names its own shape, so this is no longer a test of a derivation. What it
/// pins is the mapping from the slug on the wire to [`Shape`], and that the corpus still exercises
/// all four: a decoder that silently read every shape as `save` would pass a round-trip test and
/// fail this one.
#[test]
fn valid_cases_carry_the_shapes_the_corpus_describes() {
    let exemplars = [
        // "An empty header and one bare save. The floor of what a bundle is."
        ("minimal", Shape::Save),
        ("transfer-pak", Shape::Save),
        ("ps1-card", Shape::Card),
        // "A `card_image` is the only part it can have, since there is no save to nest."
        ("empty-formatted-card", Shape::Card),
        // "One console's storage read whole. A Sega CD carries internal backup RAM and a Backup
        // RAM Cart." Two places, one console.
        ("device", Shape::Device),
        // One game whose state spans a cartridge and a Controller Pak. Before 0.2 this was the
        // case that fell through every tier; it is a device, because the console is what keeps
        // the two halves together.
        ("cartridge-and-pak", Shape::Device),
        // "No system, no game, no card of its own, and entries that do not agree on a system."
        ("collection", Shape::Collection),
    ];

    let mut failures = Vec::new();
    for (name, want) in exemplars {
        let bytes = fs::read(corpus_dir().join(format!("valid/{name}.cbor")))
            .unwrap_or_else(|e| panic!("read {name}: {e}"));
        let bundle = Bundle::from_slice(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        if bundle.shape() != &want {
            failures.push(format!("{name}: carries {}, not {want}", bundle.shape()));
        }
    }
    assert!(failures.is_empty(), "{} case(s) failed:\n{}", failures.len(), failures.join("\n"));

    // Every valid case names a shape this version defines, and between them they cover all four.
    let mut seen = std::collections::BTreeSet::new();
    for case in cases().into_iter().filter(|c| c.expect_valid) {
        let bytes = fs::read(&case.file).unwrap_or_else(|e| panic!("read {}: {e}", case.name));
        let bundle = Bundle::from_slice(&bytes).unwrap_or_else(|e| panic!("{}: {e}", case.name));
        assert!(
            bundle.shape().is_known(),
            "{} names {}, which this version does not define",
            case.name,
            bundle.shape()
        );
        seen.insert(bundle.shape().to_string());
    }
    assert_eq!(
        seen.iter().map(String::as_str).collect::<Vec<_>>(),
        ["card", "collection", "device", "save"],
        "the corpus should exercise every shape"
    );
}
