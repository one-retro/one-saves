//! A card read into a bundle and written back is one a console loads exactly as it loaded the
//! original.
//!
//! Not byte for byte, and not meant to be. A bundle carries no block positions, so a writer
//! allocates its own, and where a console put a save depends on everything ever written to that
//! card. What has to survive is everything a console reads: the card's own structure, every
//! directory in the order it was listed, every entry as the game or the console wrote it, and
//! every byte of every file. The checks below compare exactly that, and mask exactly the fields
//! that only say where the bytes landed.
//!
//! The cards are real ones under the workspace's `data/saves`, so these live out here rather than
//! beside the code: a published tarball carries the crate and nothing else.

// Each adapter is behind its feature, so a build with neither has nothing here to test.
#![cfg(any(feature = "ps1", feature = "ps2"))]

use one_saves::{Bundle, PartKind};
use one_saves_convert::Format;
use one_saves_convert::card::{self, CardOptions};

fn fixture(rest: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/saves").join(rest);
    std::fs::read(path).expect("a vendored card")
}

/// A card through a bundle and back, the way `1saves convert` and `1saves extract` take it.
fn round_trip(format: Format, original: &[u8]) -> (Bundle, Vec<u8>) {
    let bundle = card::read(format, original, &CardOptions::default()).expect("reads");
    // Through the encoding too, since that is what a bundle that went anywhere has been through.
    let bundle = Bundle::from_slice(&bundle.to_vec().expect("encodes")).expect("decodes");
    let rebuilt = card::write(format, &bundle).expect("writes");
    (bundle, rebuilt)
}

/// Every save part on a card names a game, which is what makes it one game's save.
///
/// Only checked on cards whose every directory is a game's or the console's: a folder that is
/// neither is a save nothing identifies, which the format allows.
fn every_save_names_a_game(bundle: &Bundle) {
    for part in bundle.parts.iter().filter(|part| part.kind == PartKind::Bundle) {
        assert!(part.game.is_some(), "the save at {:?} names no game", part.path);
        let inner = Bundle::from_slice(&part.bytes().expect("a payload")).expect("a nested save");
        assert!(inner.header.game.is_some(), "the save at {:?} names no game inside", part.path);
    }
}

#[cfg(feature = "ps2")]
mod ps2 {
    use ps2_memcard::{Capacity, CardBuilder, ENTRY, File, MemoryCard, Save};

    use super::*;

    /// Where an entry records the first cluster of what it names, relative to the allocated area.
    ///
    /// The one field of a game's or the console's entry that a writer sets for itself: the length
    /// beside it is the file's own, and the rest is copied through.
    const CLUSTER: std::ops::Range<usize> = 0x10..0x14;

    /// Where an entry keeps its name.
    const NAME: std::ops::Range<usize> = 0x40..0x60;

    /// An entry with the field that only says where its bytes landed blanked out.
    fn placed_anywhere(entry: &[u8]) -> Vec<u8> {
        let mut entry = entry.to_vec();
        entry[CLUSTER].fill(0);
        entry
    }

    /// The card without spare areas, which is what a cluster number counts through.
    fn data_pages(image: &[u8], card: &MemoryCard) -> Vec<u8> {
        if card.had_spare() {
            image.chunks(528).flat_map(|page| page[..512].to_vec()).collect()
        } else {
            image.to_vec()
        }
    }

    /// The `.` and `..` opening every directory, as mode, length, cluster and entry index.
    ///
    /// Root first, then each directory the root lists. These are how a console gets from a
    /// directory to the entry naming it and back, so they have to agree exactly. They point at the
    /// root and at slots in it, both of which a writer lays down the same way every time.
    fn dot_entries(image: &[u8]) -> Vec<[u32; 4]> {
        let card = MemoryCard::parse(image).expect("parses");
        let data = data_pages(image, &card);
        let alloc = card.superblock().alloc_offset;
        let word = |at: usize| u32::from_le_bytes(data[at..at + 4].try_into().expect("four bytes"));
        let entry = |cluster: usize, index: usize| {
            let at = (alloc + cluster) * 1024 + index * ENTRY;
            [
                u32::from(u16::from_le_bytes([data[at], data[at + 1]])),
                word(at + 4),
                word(at + 0x10),
                word(at + 0x14),
            ]
        };
        let root = card.superblock().rootdir_cluster;
        let mut entries = vec![entry(root, 0), entry(root, 1)];
        for directory in card.saves().expect("reads the directory") {
            let cluster = usize::try_from(u32::from_le_bytes(
                directory.dirent[CLUSTER].try_into().expect("four bytes"),
            ))
            .expect("a cluster fits");
            entries.extend([entry(cluster, 0), entry(cluster, 1)]);
        }
        entries
    }

    /// Whether a PS2 loads `rebuilt` exactly as it loaded `original`.
    fn assert_loads_the_same(original: &[u8], rebuilt: &[u8]) {
        let before = MemoryCard::parse(original).expect("the original parses");
        let after = MemoryCard::parse(rebuilt).expect("the rebuilt card parses");

        // A console handed a card without spare areas does not see a card at all, and one with an
        // ECC that disagrees with its page corrects good data into bad.
        assert_eq!(after.had_spare(), before.had_spare(), "spare areas");
        assert_eq!(rebuilt.len(), original.len(), "a card keeps its size");
        if after.had_spare() {
            for (block, pages) in rebuilt.chunks(528 * 16).enumerate() {
                if pages.iter().all(|&byte| byte == 0xFF) {
                    continue;
                }
                for (index, page) in pages.chunks(528).enumerate() {
                    let mut spare = [0u8; 16];
                    ps2_memcard::ecc::fill_spare(&page[..512], &mut spare);
                    assert_eq!(page[512..], spare, "the ECC of page {index} of block {block}");
                }
            }
        }

        assert_eq!(after.superblock(), before.superblock(), "the superblock");
        assert_eq!(dot_entries(rebuilt), dot_entries(original), "the `.` and `..` of every directory");

        let (before, after) = (before.saves().expect("reads"), after.saves().expect("reads"));
        let names = |directories: &[Save]| directories.iter().map(|d| d.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&after), names(&before), "the root lists the same directories in the same order");
        for (before, after) in before.iter().zip(&after) {
            let name = &before.name;
            assert_eq!(placed_anywhere(&after.dirent), placed_anywhere(&before.dirent), "{name}: its entry");
            let files = |directory: &Save| directory.files.iter().map(|f| f.name.clone()).collect::<Vec<_>>();
            assert_eq!(files(after), files(before), "{name}: the same files in the same order");
            for (before, after) in before.files.iter().zip(&after.files) {
                let file = format!("{name}/{}", before.name);
                assert_eq!(
                    placed_anywhere(&after.dirent),
                    placed_anywhere(&before.dirent),
                    "{file}: its entry"
                );
                assert!(after.data == before.data, "{file}: its contents");
            }
        }
    }

    #[test]
    fn a_card_a_console_wrote_loads_the_same_after_a_round_trip() {
        for name in ["macos/pcsx2/Mcd001.ps2", "macos/pcsx2/Mcd002.ps2"] {
            let original = fixture(name);
            let (bundle, rebuilt) = round_trip(Format::Ps2Card, &original);
            every_save_names_a_game(&bundle);
            assert_loads_the_same(&original, &rebuilt);
        }
    }

    /// An entry like `like` names, renamed, which is how the console's folders below get entries
    /// carrying real mode bits and real timestamps.
    fn renamed(like: &[u8], name: &str) -> Vec<u8> {
        let mut entry = like.to_vec();
        entry[NAME].fill(0);
        entry[NAME.start..NAME.start + name.len()].copy_from_slice(name.as_bytes());
        entry
    }

    /// A card shaped like one a console has been using: its browser history first, as booting a
    /// disc with the card in writes it, then the real saves PCSX2 wrote, with the network settings
    /// between them as a later visit to the network setup disc would leave them.
    ///
    /// No card in `data/saves` carries the console's folders, so these are assembled out of the
    /// real card's own entries — renamed, the mode bits and timestamps as the console wrote them —
    /// around contents shaped as the console writes them: a history of 21 records of 22 bytes, and
    /// the `icon.sys` and `SYS_NET.ICO` Sony's network library insists are 964 and 33,688 bytes.
    /// The browser marks its own folder hidden from games, which is a bit in the entry a writer
    /// has to carry through like any other.
    fn card_with_the_consoles_folders() -> Vec<u8> {
        let real = MemoryCard::parse(&fixture("macos/pcsx2/Mcd001.ps2")).expect("parses");
        let mut saves = real.saves().expect("reads");
        let (directory, file) = (saves[0].dirent.clone(), saves[0].files[0].dirent.clone());
        let folder = |name: &str, files: &[(&str, Vec<u8>)]| Save {
            name: name.to_owned(),
            dirent: renamed(&directory, name),
            files: files
                .iter()
                .map(|(file_name, data)| File {
                    name: (*file_name).to_owned(),
                    dirent: renamed(&file, file_name),
                    data: data.clone(),
                })
                .collect(),
        };
        let icon_sys = |fill: u8| [b"PS2D".as_slice(), &[fill; 960]].concat();
        let patterned = |len: usize, seed: usize| {
            (0..len).map(|i| u8::try_from((i * 7 + seed) % 251).expect("a byte")).collect()
        };

        let mut history =
            folder("BADATA-SYSTEM", &[("icon.sys", icon_sys(1)), ("history", patterned(21 * 22, 1))]);
        // Hidden from games, though not from the browser.
        history.dirent[1] |= 0x20;
        let network = folder(
            "BWNETCNF",
            &[
                ("icon.sys", icon_sys(2)),
                ("SYS_NET.ICO", patterned(33_688, 2)),
                ("BWNETCNF", patterned(5_120, 3)),
                ("ifc000.dat", patterned(512, 4)),
            ],
        );
        saves.insert(0, history);
        saves.insert(2, network);

        let mut builder = CardBuilder::new(Capacity::Mb8);
        for save in saves {
            builder.add(save);
        }
        builder.build_with_spare().expect("builds")
    }

    #[test]
    fn a_card_with_the_consoles_folders_loads_the_same_after_a_round_trip() {
        let original = card_with_the_consoles_folders();
        let (bundle, rebuilt) = round_trip(Format::Ps2Card, &original);

        // The console's folders are not saves, so every save that is left names its game.
        every_save_names_a_game(&bundle);
        let system: Vec<&str> = bundle
            .parts
            .iter()
            .filter(|part| part.kind == PartKind::System)
            .map(|part| part.path.as_deref().expect("a path"))
            .collect();
        assert_eq!(
            system,
            [
                "BADATA-SYSTEM",
                "BADATA-SYSTEM/icon.sys",
                "BADATA-SYSTEM/history",
                "BWNETCNF",
                "BWNETCNF/icon.sys",
                "BWNETCNF/SYS_NET.ICO",
                "BWNETCNF/BWNETCNF",
                "BWNETCNF/ifc000.dat",
            ]
        );

        assert_loads_the_same(&original, &rebuilt);
    }

    /// The comparison above is worth something only if it fails on a card a console would load
    /// differently. Moving one file's modification time by a second is that card: the browser
    /// shows the change.
    #[test]
    #[should_panic(expected = "BWNETCNF/BWNETCNF: its entry")]
    fn a_card_that_would_load_differently_is_caught() {
        let original = card_with_the_consoles_folders();
        let mut bundle = card::read(Format::Ps2Card, &original, &CardOptions::default()).expect("reads");
        let file = bundle
            .parts
            .iter_mut()
            .find(|part| part.path.as_deref() == Some("BWNETCNF/BWNETCNF"))
            .expect("the settings file");
        // The seconds of `sceMcStDateTime` at 0x18, the entry's modification time.
        file.dirent.as_mut().expect("an entry")[0x19] ^= 1;
        assert_loads_the_same(&original, &card::write(Format::Ps2Card, &bundle).expect("writes"));
    }

    /// A card of nothing but the console's folders is still a card of parts, not an image of one.
    ///
    /// That is the common case for a card that has only ever been in a console: booting a disc
    /// writes the browser's history before any game saves anything.
    #[test]
    fn a_card_holding_only_the_consoles_folders_is_still_split() {
        let real = MemoryCard::parse(&card_with_the_consoles_folders()).expect("parses");
        let mut builder = CardBuilder::new(Capacity::Mb8);
        builder.add(real.saves().expect("reads").remove(0));
        let original = builder.build_with_spare().expect("builds");

        let (bundle, rebuilt) = round_trip(Format::Ps2Card, &original);
        assert!(bundle.parts.iter().all(|part| part.kind == PartKind::System), "no image, no save");
        assert_loads_the_same(&original, &rebuilt);
    }
}

#[cfg(feature = "ps1")]
mod ps1 {
    use ps1_memcard::{BLOCK, FRAME, MemoryCard, SLOTS, frame_checksum};

    use super::*;

    /// Where a directory entry links to the block its save carries on in.
    const LINK: std::ops::Range<usize> = 8..10;

    /// An entry with what only says where its blocks landed blanked out: the link to the next
    /// block, and the checksum that covers it.
    fn placed_anywhere(entry: &[u8]) -> Vec<u8> {
        let mut entry = entry.to_vec();
        entry[LINK].fill(0);
        entry[FRAME - 1] = 0;
        entry
    }

    /// Whether a PlayStation loads `rebuilt` exactly as it loaded `original`.
    ///
    /// The BIOS and every game find a save by its name, in the order the directory lists them, so
    /// that is what has to agree. Which block a save starts at does not: on a card that has had a
    /// save deleted from the middle, the rebuilt one has them packed from the front instead.
    fn assert_loads_the_same(original: &[u8], rebuilt: &[u8]) {
        let before = MemoryCard::parse(original).expect("the original parses");
        let after = MemoryCard::parse(rebuilt).expect("the rebuilt card parses");
        assert_eq!(after.image().len(), before.image().len(), "a card keeps its size");

        // A frame whose checksum is wrong is one the BIOS stops trusting, and the saves after it
        // stop being listed.
        for index in 0..=SLOTS {
            let frame = &after.image()[index * FRAME..(index + 1) * FRAME];
            assert_eq!(frame[FRAME - 1], frame_checksum(frame), "the checksum of frame {index}");
        }
        // Everything in the header block but the directory comes back as it was: the magic, the
        // broken-sector list and its spares. Frame 63 is left out, being the frame a console
        // scribbles on to test the card is writable.
        let header = |card: &MemoryCard| {
            let block = &card.image()[..BLOCK];
            [&block[..FRAME], &block[(SLOTS + 1) * FRAME..63 * FRAME]].concat()
        };
        assert_eq!(header(&after), header(&before), "the header block around the directory");

        let names = |card: &MemoryCard| card.saves().iter().map(|save| save.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&after), names(&before), "the same saves, listed in the same order");
        for (before, after) in before.saves().iter().zip(after.saves()) {
            let name = &before.name;
            assert_eq!(placed_anywhere(&after.dirent), placed_anywhere(&before.dirent), "{name}: its entry");
            assert!(after.data == before.data, "{name}: its contents");
        }
    }

    #[test]
    fn a_card_a_console_wrote_loads_the_same_after_a_round_trip() {
        for name in ["macos/duckstation/shared_card_1.mcd", "macos/duckstation/shared_card_2.mcd"] {
            let original = fixture(name);
            let (bundle, rebuilt) = round_trip(Format::Ps1Card, &original);
            every_save_names_a_game(&bundle);
            assert_loads_the_same(&original, &rebuilt);
        }
    }

    /// A card a console has deleted from and written to again, with a save's blocks out of order.
    #[test]
    fn a_scattered_card_loads_the_same_after_a_round_trip() {
        let original = fixture("one-saves/fragmented.mcd");
        let (_, rebuilt) = round_trip(Format::Ps1Card, &original);
        assert_loads_the_same(&original, &rebuilt);
    }
}
