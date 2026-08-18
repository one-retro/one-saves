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
/// against the version 0.1 schema and *accepted* by a shipped decoder, which ignores and
/// round-trips an integer key it does not know. Running the corpus in decoder mode would fail
/// those two for being right.
#[test]
fn corpus() {
    let mut failures = Vec::new();
    let cases = cases();

    // A manifest this harness failed to read would otherwise pass by testing nothing.
    let (valid, invalid): (Vec<_>, Vec<_>) = cases.iter().partition(|c| c.expect_valid);
    assert_eq!((valid.len(), invalid.len()), (24, 49), "the corpus is not the size it should be");

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

/// A shipped decoder is looser than the schema over exactly one thing.
#[test]
fn a_decoder_keeps_integer_keys_a_later_minor_version_wrote() {
    for name in ["header-unknown-integer-key", "part-unknown-integer-key"] {
        let bytes = fs::read(corpus_dir().join(format!("invalid/{name}.cbor"))).expect("read case");

        assert!(
            Bundle::from_slice_with(&bytes, Strictness::Schema).is_err(),
            "{name} must fail against the version 0.1 schema"
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

/// Nesting depth: the corpus reaches the cap but cannot express going past it.
///
/// A bundle deeper than the cap is rejected by a rule about structure rather than by anything a
/// decoder sees in one map, so the over-deep bundle is built here.
#[test]
fn rejects_a_bundle_nested_deeper_than_the_cap() {
    use one_saves::{Header, Part, PartKind};

    // A save, wrapped until it is one level past what a collection of cards of saves needs.
    let mut bundle = Bundle { header: Header::default(), parts: vec![Part::new(0, *b"SAVE")] };

    for depth in 0..=one_saves::MAX_NESTING_DEPTH {
        let payload = bundle.to_vec().expect("inner bundle encodes");
        let mut part = Part::new(0, payload);
        part.kind = PartKind::Bundle;
        bundle = Bundle { header: Header::default(), parts: vec![part] };

        let deep = depth == one_saves::MAX_NESTING_DEPTH;
        assert_eq!(
            bundle.validate().is_err(),
            deep,
            "a bundle part at depth {depth} should {} be rejected",
            if deep { "" } else { "not" }
        );
    }
}

/// What each corpus case *is*, checked against the shapes the corpus was written to demonstrate.
///
/// [`Bundle::shape`] is spec knowledge rather than plumbing, so it is pinned to the spec's own
/// files: every case named here has a note in the corpus manifest saying which tier it belongs
/// to, and this is that note as an assertion.
#[test]
fn valid_cases_classify_into_the_shapes_the_corpus_describes() {
    let expected = [
        // "An empty header and one bare save. The floor of what a bundle is."
        ("minimal", Shape::Save),
        // "No `system`, because several consoles read one and none of them owns it." Absence on
        // all three header keys is not enough on its own — a collection also holds bundles.
        ("token-bound-save", Shape::Save),
        ("compressed-and-aux", Shape::Save),
        ("binding", Shape::Save),
        ("transfer-pak", Shape::Save),
        ("zero-byte-payload", Shape::Save),
        ("ps1-card", Shape::Card),
        ("ps2-card", Shape::Card),
        ("card-of-one-game", Shape::Card),
        ("card-system-area", Shape::Card),
        ("card-image-beside-splits", Shape::Card),
        // The save spans two blocks, but the bundle around it is still a card: what the case
        // varies is the size of the one nested entry, not the tier it sits in.
        ("ps1-multi-block-save", Shape::Card),
        // A thin card's entries point at content hashes, which changes nothing about what it is.
        ("thin-card", Shape::Card),
        // "A `card_image` is the only part it can have, since there is no save to nest."
        ("empty-formatted-card", Shape::Card),
        // "No system, no game, no card of its own, and entries that do not agree on a system."
        ("collection", Shape::Collection),
        // "Neither a card (the bundle is not one) nor a collection (it has a system and a game)."
        ("cartridge-and-card", Shape::Mixed),
    ];

    let mut failures = Vec::new();
    for (name, want) in expected {
        let bytes = fs::read(corpus_dir().join(format!("valid/{name}.cbor")))
            .unwrap_or_else(|e| panic!("read {name}: {e}"));
        let bundle = Bundle::from_slice(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        let got = bundle.shape();
        if got != want {
            failures.push(format!("{name}: classified as {got}, not {want}"));
        }
    }

    assert!(failures.is_empty(), "{} case(s) failed:\n{}", failures.len(), failures.join("\n"));
}
