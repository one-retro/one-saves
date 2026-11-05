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

const PS2: &str = "macos/pcsx2/Mcd001.ps2";

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
    let gc = fixture("macos/dolphin");
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

const PS2_EMPTY: &str = "macos/pcsx2/Mcd002.ps2";

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
/// it by its path, which spells its NGH number and which of the game's saves it is, is what keeps
/// it identifiable, where falling through to its position on the card would not be.
#[test]
fn a_save_with_no_title_is_named_by_its_numbers() {
    const CARD: &str = "retroarch/neocd/Garou Densetsu 2 ~ Fatal Fury 2 (Japan) (En,Ja).srm";
    let card = Card::open(fixture(CARD)).expect("opens");

    let entries = card.entries();
    let titles: Vec<&str> = entries.iter().map(|e| e.title.as_str()).collect();
    assert_eq!(titles, vec!["METAL SLUG", "0047-0"]);
}

/// An archive of the given members, uncompressed so the test needs no encoder.
fn zip(members: &[(&str, Vec<u8>)]) -> Vec<u8> {
    use std::io::Write;

    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, bytes) in members {
        writer.start_file(*name, options).expect("a member");
        writer.write_all(bytes).expect("writes");
    }
    writer.finish().expect("finishes").into_inner()
}

/// Writes an archive to a temporary file and opens it.
fn open_archive(name: &str, members: &[(&str, Vec<u8>)]) -> Card {
    let path = std::env::temp_dir().join(name);
    std::fs::write(&path, zip(members)).expect("writes the archive");
    Card::open(&path).expect("opens")
}

/// Saves are shared as an archive of themselves, so opening one has to give a card.
#[test]
fn an_archive_of_loose_saves_opens_as_a_card() {
    let save = |rest: &str| std::fs::read(fixture(rest)).expect("a vendored save");
    let card = open_archive(
        "1cards-loose.zip",
        &[
            ("BASLUS-01360464634.PSV", save("ps3/BASLUS-01360464634.PSV")),
            ("BISLPM-87053.PSV", save("ps3/BISLPM-87053.PSV")),
        ],
    );

    let entries = card.entries();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].title, "ＦＦ４　３１０７／３１０７　　　４５：２０");
    assert_eq!(entries[1].title, "ＣＡＰＣＯＭ　ＶＳ．　ＳＮＫ　ＰＲＯ");
    // Two blocks for the first, one for the second, as they occupy on a real card.
    assert_eq!(entries[0].blocks, 2);
    assert_eq!(entries[1].blocks, 1);
}

/// An archive is where a card came from, never where one is written back to.
///
/// The file on disk is the archive. Writing a bare card over it would destroy every other thing it
/// held, so this refuses before it rebuilds anything and says where the saves can go instead.
#[test]
fn a_card_out_of_an_archive_is_not_written_back_into_it() {
    let save = std::fs::read(fixture("ps3/BASLUS-01360464634.PSV")).expect("a vendored save");
    let mut card = open_archive("1cards-readonly.zip", &[("BASLUS-01360464634.PSV", save)]);

    let before = std::fs::read(&card.path).expect("the archive");
    let error = card.save().expect_err("refuses").to_string();
    assert!(error.contains("archive is not written back into"), "{error}");
    assert_eq!(std::fs::read(&card.path).expect("the archive"), before, "and left it alone");
}

/// A path in the temporary directory with nothing at it, for a card to be started at.
fn nowhere(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(name);
    // A run that failed part-way leaves its card behind, and a card already there is opened
    // rather than started, which is not what these tests are asking about.
    let _ = std::fs::remove_file(&path);
    path
}

/// A card that is not there yet starts out empty and in memory, and comes into existence when it
/// is written, holding whatever was copied onto it.
#[test]
fn a_card_that_is_not_there_yet_starts_empty_and_is_written_on_save() {
    let path = nowhere("1cards-started.mcr");
    let mut card = Card::create(&path, None).expect("starts");
    assert!(card.is_new() && card.dirty(), "a started card has something to write");
    assert_eq!(card.entries().len(), 0);
    assert_eq!(card.used_blocks(), 0);
    assert!(!path.exists(), "and nothing is written until asked");

    let source = Card::open(fixture("macos/duckstation/shared_card_1.mcd")).expect("opens");
    card.copy_from(&source, 0).expect("a PS1 save goes on a PS1 card");
    card.save().expect("writes");
    assert!(!card.is_new() && !card.dirty());

    let again = Card::open(&path).expect("reopens what it wrote");
    assert_eq!(again.entries().len(), 1);
    assert_eq!(again.entries()[0].title, source.entries()[0].title);
}

/// Started beside a card of its own kind, a card is made like that one, since that is the card
/// whose saves are about to go onto it. A name that says nothing takes the kind from beside it,
/// and a name that says something else is that kind's standard card.
#[test]
fn a_card_started_beside_another_is_made_like_it_unless_its_name_says_otherwise() {
    use one_saves_convert::Format;
    use one_saves_convert::card::{BlankOptions, blank};

    // A 16 MB card, which is not the size a PS2 card is started in on its own.
    let large = std::env::temp_dir().join("1cards-large.ps2");
    let options = BlankOptions { capacity: Some(16 << 20), ..BlankOptions::default() };
    std::fs::write(&large, blank(Format::Ps2Card, &options).expect("builds")).expect("writes");
    let beside = Card::open(&large).expect("opens");

    let like = Card::create(nowhere("1cards-like.ps2"), Some(&beside)).expect("starts");
    assert_eq!(like.capacity(), 16 << 20, "the size of the card beside it");
    let unnamed = Card::create(nowhere("1cards-unnamed.bin"), Some(&beside)).expect("starts");
    assert_eq!((unnamed.format, unnamed.capacity()), (Format::Ps2Card, 16 << 20), "and its kind");
    let other = Card::create(nowhere("1cards-other.mcr"), Some(&beside)).expect("starts");
    assert_eq!(other.format, Format::Ps1Card, "the name wins where it says");
    assert_eq!(Card::create(nowhere("1cards-alone.ps2"), None).expect("starts").capacity(), 8 << 20);

    let refused = Card::create(nowhere("1cards-unknown.bin"), None).err().expect("refuses").to_string();
    assert!(refused.contains(".mcr, .ps2"), "says what would have worked: {refused}");
}

/// Formatting a card empties it, and like every other edit it is nothing until it is written.
#[test]
fn formatting_a_card_empties_it_and_writing_makes_it_so() {
    let mut card = open_copy(PS2, "1cards-format.ps2");
    let capacity = card.capacity();
    let before = std::fs::read(&card.path).expect("the copy");

    card.format().expect("formats");
    assert!(card.dirty());
    assert_eq!((card.entries().len(), card.used_blocks()), (0, 0));
    assert_eq!(std::fs::read(&card.path).expect("the copy"), before, "nothing written yet");

    card.save().expect("writes");
    let again = Card::open(&card.path).expect("reopens");
    assert_eq!(again.entries().len(), 0);
    assert_eq!(again.capacity(), capacity, "the same card, empty");
}

const PS1: &str = "macos/duckstation/shared_card_1.mcd";

/// A directory of its own in the temporary one, emptied first.
fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a directory");
    dir
}

/// A card is written to a temporary beside it, under a name anyone can guess, and a card often
/// sits in a directory an archive was extracted into. A link planted at that name was followed:
/// the card was written over whatever it pointed to, outside the directory, and the rename then
/// moved the link over the card. Now anything at that name, a link included, is refused without
/// being followed, and nothing is written anywhere.
#[cfg(unix)]
#[test]
fn writing_a_card_never_writes_through_a_link_planted_beside_it() {
    use std::os::unix::fs::symlink;

    let dir = fresh_dir("1cards-planted");
    let victim = dir.join("precious.txt");
    std::fs::write(&victim, b"not a card").expect("writes the victim");
    let path = dir.join("card.mcd");
    std::fs::copy(fixture(PS1), &path).expect("copies the fixture");
    let temporary = dir.join("card.1cards-new");
    symlink(&victim, &temporary).expect("plants a link");

    let mut card = Card::open(&path).expect("opens");
    let (before, saves) = (std::fs::read(&path).expect("the card"), card.entries().len());
    card.remove(0).expect("removes");
    let refused = card.save().expect_err("refuses").to_string();
    assert!(refused.contains("in the way"), "says why: {refused}");
    assert_eq!(std::fs::read(&victim).expect("the victim"), b"not a card", "the link was not followed");
    assert_eq!(std::fs::read(&path).expect("the card"), before, "nor the card replaced");
    let link = std::fs::symlink_metadata(&temporary).expect("still there");
    assert!(link.file_type().is_symlink(), "and the link is left as it was found");
    assert!(card.dirty(), "the edit is still waiting to be written");

    // A link to nothing was followed too, and created its target wherever it pointed.
    let elsewhere = dir.join("elsewhere.mcd");
    std::fs::remove_file(&temporary).expect("removes the link");
    symlink(&elsewhere, &temporary).expect("plants a dangling link");
    assert!(card.save().is_err(), "refused");
    assert!(!elsewhere.exists(), "and nothing was created at the far end");

    // Out of the way, the write goes through as it always did.
    std::fs::remove_file(&temporary).expect("removes the link");
    card.save().expect("writes");
    assert_eq!(Card::open(&path).expect("reopens").entries().len(), saves - 1);
    assert_eq!(std::fs::read(&victim).expect("the victim"), b"not a card");
}

/// A card started here is written where it goes rather than renamed into place, and a link that
/// turns up at that path before it is written is refused the same way: the card is new, so
/// whatever is there is not this card's to replace.
#[cfg(unix)]
#[test]
fn a_new_card_is_not_written_through_a_link_planted_where_it_goes() {
    use std::os::unix::fs::symlink;

    let dir = fresh_dir("1cards-planted-new");
    let victim = dir.join("precious.txt");
    std::fs::write(&victim, b"not a card").expect("writes the victim");
    let path = dir.join("new.mcr");
    let mut card = Card::create(&path, None).expect("starts");
    symlink(&victim, &path).expect("plants a link");

    assert!(card.save().expect_err("refuses").to_string().contains("in the way"));
    assert_eq!(std::fs::read(&victim).expect("the victim"), b"not a card");
    assert!(card.is_new(), "and the card is still waiting to be created");

    std::fs::remove_file(&path).expect("removes the link");
    card.save().expect("writes");
    assert!(Card::open(&path).expect("reopens").entries().is_empty());
}

/// A bundle is not a card, so nothing in one is inflated here.
///
/// A `.1saves` part can be compressed, and the size it says it inflates to is its producer's say:
/// the CLI holds what it inflates to a budget for that reason. This opens cards, which carry their
/// saves uncompressed, and a bundle is refused for not being one before any part of it is looked
/// at, under its own extension or a card's.
#[test]
fn a_bundle_is_refused_rather_than_inflated() {
    use one_saves::{Bundle, Header, Part, Payload, Shape};

    // A zstd frame's magic and nothing else, claiming to inflate to 4 GiB.
    let mut part = Part::new(0, Vec::new());
    part.payload = Payload::Compressed { bytes: vec![0x28, 0xb5, 0x2f, 0xfd], size: 4 << 30 };
    let bundle = Bundle::new(Header::new(Shape::Card), vec![part]).to_cbor().to_cbor_data();

    for name in ["1cards-bundle.1saves", "1cards-bundle.mcr"] {
        let path = std::env::temp_dir().join(name);
        std::fs::write(&path, &bundle).expect("writes");
        let refused = Card::open(&path).err().expect("refuses").to_string();
        assert!(refused.contains("not a card"), "{name}: {refused}");
    }
}
