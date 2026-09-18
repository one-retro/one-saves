//! Cards a real PlayStation 2 filesystem wrote, as against ones this crate built itself.
//!
//! A round trip through [`CardBuilder`] cannot catch a field this crate writes and misreads the
//! same way. These two came off PCSX2, and the first of them is what found exactly that: a save
//! directory's entry count is on the parent's entry naming it, not on the `.` inside it, which a
//! real card writes with a length of zero.

use std::path::{Path, PathBuf};

fn card(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/saves/PS2").join(name)
}

#[test]
fn a_real_card_holds_the_files_each_save_is_made_of() {
    let bytes =
        std::fs::read(card("Dragon Quest VIII and Tekken 4/PCSX2/Mcd001.ps2")).expect("the vendored card");
    // 8 MB of data plus a 16-byte spare area on every 512-byte page.
    assert_eq!(bytes.len(), 8_650_752);

    let card = ps2_memcard::MemoryCard::parse(&bytes).expect("parses");
    assert!(card.had_spare(), "a dump from a card carries its ECC");
    assert_eq!(card.capacity(), 8_388_608, "capacity counts the data area, not the dump");

    let saves = card.saves().expect("reads the directory");
    let names: Vec<&str> = saves.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["BASLUS-20328Tekken-4", "BASLUS-21207dq8_0", "BASLUS-21207dq8_1"]);

    // A PS2 save is a directory, so a save with no files in it is a save that was misread.
    for save in &saves {
        assert!(!save.files.is_empty(), "{} came back empty", save.name);
        assert_eq!(save.files[0].name, "icon.sys", "{}: every save carries one", save.name);
        assert!(
            save.files.iter().any(|f| f.name == save.name),
            "{}: the save data is named for the directory",
            save.name
        );
    }
    // Tekken keeps one icon; Dragon Quest keeps three, one per state it shows.
    assert_eq!(saves[0].files.len(), 3);
    assert_eq!(saves[1].files.len(), 5);
}

#[test]
fn a_formatted_card_with_nothing_on_it_is_still_a_card() {
    let bytes = std::fs::read(card("Formatted empty card/PCSX2/Mcd002.ps2")).expect("the vendored card");
    let card = ps2_memcard::MemoryCard::parse(&bytes).expect("a formatted card parses");
    assert!(card.had_spare());
    assert_eq!(card.capacity(), 8_388_608);
    // A root holding nothing but `.` and `..` is the boundary of the entry-count rule: the count
    // is two, both are skipped, and what comes back is empty rather than an error.
    assert!(card.saves().expect("reads the directory").is_empty());
}

/// What the console's timezone setting does to the times a card records, which is nothing.
///
/// A PS2 left at its defaults is configured as Japan, so a card written nine hours ahead of the
/// host proves nothing on its own. This card was formatted with the BIOS set to a zone seven hours
/// *behind* UTC, and the entry that format wrote still reads +9 — so the offset is the hardware's
/// rather than the configuration's, which is what lets `x.1sav.dirent` qualify a PS2 reading at
/// all. The bytes are read here rather than through the API because the root's own `.` entry is
/// not a save, and because a control deserves to be checked against the card and not against the
/// code that reads it.
#[test]
fn the_consoles_timezone_setting_does_not_move_what_the_card_records() {
    let raw = std::fs::read(card("Formatted empty card/PCSX2/Mcd002.ps2")).expect("the card");
    // Every 512-byte page carries 16 further bytes of spare area holding ECC.
    let data: Vec<u8> = raw.chunks(528).flat_map(|page| page[..512].to_vec()).collect();
    let u16at = |at: usize| u16::from_le_bytes(data[at..at + 2].try_into().unwrap());
    let u32at = |at: usize| u32::from_le_bytes(data[at..at + 4].try_into().unwrap());

    let cluster = usize::from(u16at(0x28)) * usize::from(u16at(0x2a));
    let root = (u32at(0x34) as usize + u32at(0x3c) as usize) * cluster;
    let entry = &data[root..root + 512];
    assert_eq!(&entry[0x40..0x41], b".", "the first entry of a directory names it");

    // sceMcStDateTime: a reserved byte, then second, minute, hour, day and month as plain binary,
    // then a little-endian year.
    let created = &entry[0x08..0x10];
    let (year, month, day) = (u16::from_le_bytes([created[6], created[7]]), created[5], created[4]);
    let (hour, minute) = (created[3], created[2]);
    assert_eq!(
        (year, month, day, hour, minute),
        (2026, 9, 18, 12, 55),
        "the format wrote a Japanese wall clock on a console set seven hours behind UTC"
    );
}

#[test]
fn a_card_shaped_file_that_was_never_formatted_is_not_a_card() {
    // PCSX2 creates a new card as 0xff from end to end, and the PS2 BIOS writes the superblock
    // only when the card is first formatted. A file of exactly the right length is therefore not
    // evidence of anything, which is what the magic is for. Synthesised rather than vendored:
    // eight megabytes of one repeated byte is not worth carrying in the repository.
    let blank = vec![0xffu8; 8_650_752];
    assert!(
        matches!(ps2_memcard::MemoryCard::parse(&blank), Err(ps2_memcard::Error::NotAMemoryCard)),
        "an unformatted card is rejected for what it says rather than for how long it is"
    );
}

/// The superblock this crate writes is the one a console wrote, field for field.
///
/// The tail of it — cluster size, FAT entries a cluster holds, clusters a block holds, and
/// `cardform` — used to be left at zero, because the parser never needed those fields and so
/// nothing noticed they were missing. `cardform` at zero is a card saying it is not formatted,
/// which is a console offering to format a card that is full of saves. A reader that recomputes
/// the geometry, as this crate's does, cannot see any of that; only a card from somewhere else
/// can.
#[test]
fn a_rebuilt_superblock_matches_the_one_a_console_wrote() {
    for name in ["Dragon Quest VIII and Tekken 4/PCSX2/Mcd001.ps2", "Formatted empty card/PCSX2/Mcd002.ps2"] {
        let original = std::fs::read(card(name)).expect("a vendored card");
        let parsed = ps2_memcard::MemoryCard::parse(&original).expect("parses");

        let mut builder = ps2_memcard::CardBuilder::new(ps2_memcard::Capacity::Mb8);
        for save in parsed.saves().expect("reads the directory") {
            builder.add(save);
        }
        let rebuilt = builder.build_with_spare().expect("builds");

        assert_eq!(rebuilt.len(), original.len(), "{name}: a card keeps its size");
        // Through `cardform` at 0x160, which is the last field either of them sets.
        assert_eq!(&rebuilt[0x150..0x164], &original[0x150..0x164], "{name}: the superblock tail");
        // And the geometry above it, which was right all along.
        assert_eq!(&rebuilt[..0x60], &original[..0x60], "{name}: magic and geometry");
    }
}
