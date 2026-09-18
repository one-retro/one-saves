//! Cards and saves read out of the archives they are shared in.
//!
//! The archives are built here rather than vendored: what an archive holds is the whole subject of
//! these tests, so it is better stated in them than hidden in a binary fixture.

#![cfg(all(feature = "archive", feature = "ps1"))]

use std::io::Write;

use one_saves_convert::archive::{is_archive, unpack};

/// An archive of the given members, uncompressed so the test needs no encoder.
fn zip(members: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, bytes) in members {
        if name.ends_with('/') {
            writer.add_directory(*name, options).expect("a directory");
        } else {
            writer.start_file(*name, options).expect("a member");
            writer.write_all(bytes).expect("writes");
        }
    }
    writer.finish().expect("finishes").into_inner()
}

fn fixture(rest: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/saves/PS1").join(rest);
    std::fs::read(path).expect("a vendored fixture")
}

fn a_card() -> Vec<u8> {
    fixture("Fragmented card/one-saves/fragmented.mcd")
}

fn a_save() -> Vec<u8> {
    fixture("Final Fantasy Chronicles/PS3/BASLUS-01360464634.PSV")
}

#[test]
fn only_an_archive_is_treated_as_one() {
    assert!(is_archive(&zip(&[("card.mcr", a_card())])));
    assert!(!is_archive(&a_card()));
    assert!(!is_archive(b""));
}

#[test]
fn an_archived_card_is_read_as_that_card() {
    let unpacked = unpack(&zip(&[("card.mcr", a_card())])).expect("unpacks");
    assert!(!unpacked.assembled, "it was found whole, not built");
    assert_eq!(unpacked.bytes, a_card());
    assert_eq!(unpacked.source, "card.mcr");
}

/// The case the format is actually shared in: the saves, not a card.
#[test]
fn archived_loose_saves_are_assembled_onto_a_card() {
    let archive = zip(&[
        ("BASLUS-01360464634.PSV", a_save()),
        ("BISLPM-87053.PSV", fixture("Capcom vs SNK Millennium Fight 2000 Pro/PS3/BISLPM-87053.PSV")),
    ]);
    let unpacked = unpack(&archive).expect("unpacks");
    assert!(unpacked.assembled, "no card was in there, so one was built");

    let card = ps1_memcard::MemoryCard::parse(&unpacked.bytes).expect("a card");
    let names: Vec<&str> = card.saves().iter().map(|s| s.name.as_str()).collect();
    // The names come out of the `.psv` headers, not the filenames.
    assert_eq!(names, vec!["BASLUS-01360FF4", "BISLPM-87053"]);
}

/// A card and the saves beside it: the card wins, because it is the thing that was dumped.
#[test]
fn a_card_beside_loose_saves_is_preferred_to_assembling_one() {
    let unpacked = unpack(&zip(&[("save.PSV", a_save()), ("card.mcr", a_card())])).expect("unpacks");
    assert!(!unpacked.assembled);
    assert_eq!(unpacked.source, "card.mcr");
}

/// Two cards is a choice, and it is not this crate's to make.
#[test]
fn an_archive_of_two_cards_is_refused_by_name() {
    let archive = zip(&[("one.mcr", a_card()), ("two.mcr", a_card())]);
    let error = unpack(&archive).expect_err("refuses").to_string();
    assert!(error.contains("one.mcr") && error.contains("two.mcr"), "{error}");
}

#[test]
fn an_archive_with_nothing_readable_says_what_was_in_it() {
    let archive = zip(&[("readme.txt", b"nothing here".to_vec())]);
    let error = unpack(&archive).expect_err("refuses").to_string();
    assert!(error.contains("readme.txt"), "{error}");

    assert!(unpack(&zip(&[])).expect_err("refuses").to_string().contains("nothing"));
}

/// What an archiver leaves behind is not a save, and must not be weighed as one.
#[test]
fn archiver_leavings_are_ignored() {
    let archive = zip(&[
        ("saves/", Vec::new()),
        ("__MACOSX/._card.mcr", b"junk".to_vec()),
        (".DS_Store", b"junk".to_vec()),
        ("saves/card.mcr", a_card()),
    ]);
    let unpacked = unpack(&archive).expect("unpacks");
    assert_eq!(unpacked.source, "saves/card.mcr");
}

/// More saves than a card has room for is refused, rather than silently losing the rest.
#[test]
fn more_saves_than_fit_is_refused() {
    // Sixteen two-block saves want 32 blocks; a card has 15.
    let members: Vec<(String, Vec<u8>)> = (0..16).map(|i| (format!("save{i}.PSV"), a_save())).collect();
    let borrowed: Vec<(&str, Vec<u8>)> = members.iter().map(|(n, b)| (n.as_str(), b.clone())).collect();
    let error = unpack(&zip(&borrowed)).expect_err("refuses").to_string();
    assert!(error.contains("do not fit"), "{error}");
}
