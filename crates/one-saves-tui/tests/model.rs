//! The rules the interface obeys, checked without an interface.

use std::path::PathBuf;

use one_saves_tui::model::Card;

fn fixture(rest: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/saves").join(rest)
}

/// Opens a copy, so a test that edits never touches what is vendored.
fn open_copy(rest: &str, name: &str) -> Card {
    let to = std::env::temp_dir().join(name);
    std::fs::copy(fixture(rest), &to).expect("copies the fixture");
    Card::open(&to).expect("opens")
}

const PS2: &str = "PS2/Dragon Quest VIII and Tekken 4/PCSX2/Mcd001.ps2";

#[test]
fn a_card_lists_what_is_on_it_with_its_block_usage() {
    let card = Card::open(fixture(PS2)).expect("opens");
    assert_eq!(card.capacity(), 8_388_608);
    // PS2 allocates in clusters of two 512-byte pages, which the registry knows.
    assert_eq!(card.block_size(), 1024);
    assert_eq!(card.blocks(), 8192);

    let entries = card.entries();
    assert_eq!(entries.len(), 3);
    assert!(entries.iter().all(|e| e.blocks > 0), "every save occupies something");
    assert_eq!(card.used_blocks(), entries.iter().map(|e| e.blocks).sum::<u64>());
    assert!(card.free_blocks() < card.blocks(), "a card with saves on it is not empty");
    assert!(!card.dirty(), "opening a card does not edit it");
}

#[test]
fn deleting_a_save_frees_its_blocks() {
    let mut card = open_copy(PS2, "1cards-delete.ps2");
    let before = card.entries();
    let freed = before[0].blocks;
    let used = card.used_blocks();

    card.remove(0).expect("removes");
    assert!(card.dirty());
    assert_eq!(card.entries().len(), before.len() - 1);
    assert_eq!(card.used_blocks(), used - freed);
    // The rest keep their order and are renumbered to close the gap.
    assert_eq!(card.entries()[0].title, before[1].title);
    assert_eq!(card.entries()[0].index, 0);
}

#[test]
fn a_save_only_goes_on_a_card_its_system_can_read() {
    let gc = fixture("GameCube");
    assert!(gc.exists(), "the GameCube fixtures are .gci, not cards, so this stays a PS2 test");

    let source = Card::open(fixture(PS2)).expect("opens");
    let mut target = open_copy(PS2, "1cards-copy.ps2");
    let before = target.entries().len();

    target.copy_from(&source, 0).expect("a PS2 save goes on a PS2 card");
    assert_eq!(target.entries().len(), before + 1);
    assert!(target.dirty());
}

#[test]
fn a_save_that_will_not_fit_is_refused_rather_than_truncated() {
    let source = Card::open(fixture(PS2)).expect("opens");
    let mut target = open_copy(PS2, "1cards-full.ps2");

    // Fill the card, then ask for one more. The refusal says what was needed and what was left,
    // because a status line that only says "no" cannot be acted on.
    let mut refused = None;
    for _ in 0..9000 {
        if let Err(e) = target.copy_from(&source, 0) {
            refused = Some(e.to_string());
            break;
        }
    }
    let refused = refused.expect("a card fills up");
    assert!(refused.contains("blocks"), "the refusal names blocks: {refused}");
    assert!(target.free_blocks() < target.entries()[0].blocks, "and it really was full");
}

#[test]
fn writing_a_card_back_keeps_what_is_on_it() {
    let mut card = open_copy(PS2, "1cards-write.ps2");
    card.remove(0).expect("removes");
    let expected: Vec<String> = card.entries().iter().map(|e| e.title.clone()).collect();

    card.save().expect("writes back");
    assert!(!card.dirty(), "a written card is no longer edited");

    let again = Card::open(&card.path).expect("reopens what it wrote");
    let got: Vec<String> = again.entries().iter().map(|e| e.title.clone()).collect();
    assert_eq!(got, expected);
}

const PS2_EMPTY: &str = "PS2/Formatted empty card/PCSX2/Mcd002.ps2";

/// A card with nothing on it carries an image of itself, and gaining a save makes that untrue.
///
/// The image is the card's bytes as they were. Keeping it once a save is added would carry a
/// picture of a card that no longer exists, and rebuilding from it would undo the edit — which is
/// how writing a copy onto an empty card used to fail, reporting a save it had never lost.
#[test]
fn copying_onto_an_empty_card_drops_the_image_it_was_carrying() {
    let source = Card::open(fixture(PS2)).expect("opens");
    let mut target = open_copy(PS2_EMPTY, "1cards-onto-empty.ps2");
    assert_eq!(target.entries().len(), 0, "an empty card lists no saves");

    target.copy_from(&source, 0).expect("copies");
    assert_eq!(target.entries().len(), 1);

    target.save().expect("writes, rather than claiming it lost a save");
    let again = Card::open(&target.path).expect("reopens what it wrote");
    assert_eq!(again.entries().len(), 1, "and the save is on it");
    assert_eq!(again.entries()[0].title, source.entries()[0].title);
}

/// The same holds the other way: emptying a card leaves no stale picture behind either.
#[test]
fn deleting_the_last_save_leaves_a_card_that_reads_back_empty() {
    let mut card = open_copy(PS2, "1cards-empty-out.ps2");
    while !card.entries().is_empty() {
        card.remove(0).expect("removes");
    }
    card.save().expect("writes");

    let again = Card::open(&card.path).expect("reopens");
    assert_eq!(again.entries().len(), 0);
    assert_eq!(again.used_blocks(), 0, "and nothing is left occupying it");
}

/// A PS2 card is written the way hardware holds one: every page followed by its ECC.
///
/// Writing the bare data pages produces a file this tool reads back perfectly and a console does
/// not see a card in at all — it reports the card as unformatted, because the sixteen bytes after
/// every page are where it looks to check that a page is intact.
#[test]
fn a_written_ps2_card_keeps_the_spare_areas_a_console_reads() {
    let source = Card::open(fixture(PS2)).expect("opens");
    let mut target = open_copy(PS2_EMPTY, "1cards-spare.ps2");
    let before = std::fs::metadata(&target.path).expect("the copy").len();

    target.copy_from(&source, 0).expect("copies");
    target.save().expect("writes");

    let after = std::fs::metadata(&target.path).expect("the written card").len();
    assert_eq!(after, before, "a card keeps the shape it came in");
    // 512 bytes of data and 16 of spare, over and over.
    assert_eq!(after % 528, 0, "which is pages of 528 bytes");
    assert_eq!(after / 528 * 512, target.capacity(), "of which the data area is the capacity");
}

/// A Neo Geo CD's memory belongs to the console, not to the game in the drive.
///
/// The file is named for Fatal Fury 2 and holds two saves, because every game played on that
/// console writes into the same 8 KiB. The one the filename promises is the *second* of them, and
/// it carries no title at all: Fatal Fury 2 writes a binary header where a title would go. Naming
/// it by its NGH number is what keeps it identifiable, where falling through to its position on
/// the card would not be.
#[test]
fn a_save_with_no_title_is_named_by_its_serial() {
    const CARD: &str = "NEOGEO-CD/Fatal Fury 2/NeoCD/Garou Densetsu 2 ~ Fatal Fury 2 (Japan) (En,Ja).srm";
    let card = Card::open(fixture(CARD)).expect("opens");

    let entries = card.entries();
    let titles: Vec<&str> = entries.iter().map(|e| e.title.as_str()).collect();
    assert_eq!(titles, vec!["METAL SLUG", "NGH-0047"]);
}

