//! Empty cards made here, held against cards a console or an emulator formatted.
//!
//! A format crate's own tests check its builder against its own reader, and a reader that never
//! looks at a field cannot notice one written in the wrong place, which is how a GameCube header
//! and a VMU root block both came to have every field misplaced. These compare what
//! [`card::blank`] writes with the real cards in `data/saves`, over everything a formatter writes
//! and a save does not.
//!
//! The real cards have saves on them, so what those saves occupy — their directory entries, their
//! allocation, their data — is left out of each comparison, and so is anything a console keeps
//! that an empty card made here cannot know, such as when it was formatted. Each exclusion says
//! why.
//!
//! The fixtures live at the workspace root, outside this crate, so a packaged copy does not carry
//! them. These tests skip in that case rather than fail.

#![cfg(any(
    feature = "gc",
    feature = "ps1",
    feature = "ps2",
    feature = "saturn",
    feature = "neogeo",
    feature = "vmu"
))]

use std::path::{Path, PathBuf};

use one_saves_convert::Format;
use one_saves_convert::card::{self, BlankOptions, Layout};

fn fixture(rest: &str) -> Option<Vec<u8>> {
    let root: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/saves");
    if !root.is_dir() {
        println!("skipping: {} is not present", root.display());
        return None;
    }
    Some(std::fs::read(root.join(rest)).unwrap_or_else(|e| panic!("{rest}: {e}")))
}

fn blank(format: Format, size: Option<&str>, layout: Layout) -> Vec<u8> {
    let capacity = size.map(|name| card::size(format, name).expect("a listed size").capacity);
    card::blank(format, &BlankOptions { capacity, layout, ..BlankOptions::default() }).expect("builds")
}

/// The index of the first differing chunk of `size` bytes, for a message that says where.
#[cfg(feature = "ps1")]
fn first_difference(ours: &[u8], theirs: &[u8], size: usize) -> Option<usize> {
    ours.chunks(size).zip(theirs.chunks(size)).position(|(a, b)| a != b)
}

/// PCSX2's empty card, formatted by the console's own browser, is this card to the byte, but for
/// when it was formatted.
///
/// That is two stamps in the root directory's `.` and `..` entries, which a card made here leaves
/// at zero for want of a clock, and the ECC of the two pages they sit on. Everything else — the
/// superblock and its copy in the first backup block, the indirect FAT, the FAT, which erase
/// blocks carry an ECC and which are left erased — is compared whole, spare areas and all.
#[test]
#[cfg(feature = "ps2")]
fn an_empty_ps2_card_is_the_one_a_console_formats_but_for_the_clock() {
    const PAGE: usize = 528;
    let Some(theirs) = fixture("macos/pcsx2/Mcd002.ps2") else { return };
    let ours = blank(Format::Ps2Card, None, Layout::Bare);
    assert_eq!(ours.len(), theirs.len());

    // The root directory's cluster is its first two pages, one entry each.
    let card = ps2_memcard::MemoryCard::parse(&theirs).expect("reads");
    let root = (card.superblock().alloc_offset + card.superblock().rootdir_cluster) * 2;
    let stamps = |page: &[u8]| {
        let mut entry = page[..512].to_vec();
        entry[0x08..0x10].fill(0);
        entry[0x18..0x20].fill(0);
        entry
    };
    for (at, (a, b)) in ours.chunks(PAGE).zip(theirs.chunks(PAGE)).enumerate() {
        if at == root || at == root + 1 {
            assert!(stamps(a) == stamps(b), "page {at}, the root directory's, differs beyond its stamps");
        } else {
            assert!(a == b, "page {at} differs");
        }
    }
}

/// DuckStation's cards were formatted by DuckStation, and outside the saves on them they are this
/// card: the header frame, every free directory frame, and the frames after the directory that
/// list and replace broken sectors.
///
/// The last frame of the first block is left out. A console writes it to test that the card
/// takes a write, so what is there is whatever was written last.
#[test]
#[cfg(feature = "ps1")]
fn an_empty_ps1_card_is_formatted_as_duckstation_formats_one() {
    const FRAME: usize = 128;
    let ours = blank(Format::Ps1Card, None, Layout::Bare);
    for name in ["shared_card_1.mcd", "shared_card_2.mcd"] {
        let Some(theirs) = fixture(&format!("macos/duckstation/{name}")) else { return };
        assert_eq!(ours[..FRAME], theirs[..FRAME], "{name}: the header frame");

        // A directory frame opening 0xA0 is a free one, and a free one is as the formatter left it.
        let free: Vec<usize> = (1..16).filter(|&frame| theirs[frame * FRAME] == 0xA0).collect();
        assert!(!free.is_empty(), "{name}: a card with room left on it");
        for frame in free {
            let at = frame * FRAME..(frame + 1) * FRAME;
            assert_eq!(ours[at.clone()], theirs[at], "{name}: free directory frame {frame}");
        }

        let rest = 16 * FRAME..63 * FRAME;
        assert_eq!(
            first_difference(&ours[rest.clone()], &theirs[rest], FRAME).map(|frame| frame + 16),
            None,
            "{name}: the broken-sector frames"
        );
    }
}

/// Mednafen, Beetle and Yabause all keep a Saturn volume's two reserved blocks as a console
/// formats them, and so does a card made here, for the console's own memory and for a 512 KiB
/// cart alike, and wrapped the way Yabause keeps one.
#[test]
#[cfg(feature = "saturn")]
fn an_empty_saturn_volume_reserves_what_a_console_reserves() {
    let internal = blank(Format::SaturnBup, None, Layout::Bare);
    for path in [
        "retroarch/beetle-saturn/Sonic 3D Blast (USA).srm",
        "macos/mednafen/Nights into Dreams... (USA, Brazil).506588eb980fd81053346c3c929f1911.bkr",
    ] {
        let Some(theirs) = fixture(path) else { return };
        assert_eq!(internal[..2 * 64], theirs[..2 * 64], "{path}");
    }

    let Some(theirs) = fixture("retroarch/yabause/Nights into Dreams... (USA, Brazil).srm") else { return };
    let wide = blank(Format::SaturnBup, None, Layout::Wide);
    assert_eq!(wide[..2 * 2 * 64], theirs[..2 * 2 * 64], "as Yabause keeps it");

    let Some(theirs) = fixture("retroarch/beetle-saturn/Sonic 3D Blast (USA).bcr") else { return };
    let cart = blank(Format::SaturnBup, Some("512K"), Layout::Bare);
    assert_eq!(cart[..2 * 512], theirs[..2 * 512], "a 512 KiB cart, in 512-byte blocks");
}

/// A Neo Geo CD's card, formatted by its BIOS under NeoCD, is this card wherever its saves are
/// not: the cardholder's name, the signature and the region, every free directory entry and
/// table entry, and the pattern an unused block holds.
///
/// The first sixteen bytes are left out. Two of them are the tables' checksums, which the saves
/// change, and nothing documents the rest: NeoCD's BIOS and the MiSTer core's leave different
/// values there, and a card made here leaves the erased pattern.
#[test]
#[cfg(feature = "neogeo")]
fn an_empty_neo_geo_card_is_formatted_as_a_neo_geo_cd_formats_one() {
    const BLOCK: usize = 64;
    let ours = blank(Format::NeoGeoCard, Some("8K"), Layout::Bare);
    for name in ["Metal Slug (Japan) (En,Ja).srm", "Garou Densetsu 2 ~ Fatal Fury 2 (Japan) (En,Ja).srm"] {
        let Some(theirs) = fixture(&format!("retroarch/neocd/{name}")) else { return };
        assert_eq!(ours.len(), theirs.len(), "{name}");
        assert_eq!(ours[0x10..BLOCK], theirs[0x10..BLOCK], "{name}: name, signature and region");

        // What the saves occupy, which is theirs and not the formatter's: a directory entry each,
        // and for every block a save's chain runs through, the block and its entry in both tables.
        let card = neogeo_memcard::MemoryCard::parse(&theirs).expect("reads");
        let g = card.geometry();
        let mut occupied = vec![false; theirs.len()];
        occupied[..0x10].fill(true);
        for save in card.saves() {
            let entry = g.directory_block * BLOCK + save.slot * neogeo_memcard::ENTRY_LEN;
            occupied[entry..entry + neogeo_memcard::ENTRY_LEN].fill(true);
            let mut block = usize::from(*save.dirent.last().expect("a save read off a card has an entry"));
            for _ in 0..g.blocks {
                occupied[block * BLOCK..(block + 1) * BLOCK].fill(true);
                occupied[g.fat1_block * BLOCK + block] = true;
                occupied[g.fat2_block * BLOCK + block] = true;
                match theirs[g.fat1_block * BLOCK + block] {
                    0x01 => break,
                    next => block = usize::from(next),
                }
            }
        }
        let differs = (0..theirs.len()).find(|&at| !occupied[at] && ours[at] != theirs[at]);
        assert_eq!(differs, None, "{name}: a byte outside the saves");
    }
}

/// Dolphin's own empty card, the 2043-block one it makes when a game boots with none in the slot,
/// is this card but for where and when it was formatted.
///
/// That is the serial and the format time in the header's first twenty bytes, and the checksum
/// over them. Dolphin works the serial out from the time and the flash ID it formats slot A with,
/// and a card made here does the same with both zero, so the two are different numbers got the
/// same way. Everything else — the size, the character set, both directories, both allocation
/// tables, every erased block — is compared whole.
#[test]
#[cfg(feature = "gc")]
fn an_empty_gamecube_card_is_the_one_dolphin_formats_but_for_the_serial() {
    let Some(theirs) = fixture("macos/dolphin/MemoryCardA.USA.raw") else { return };
    let ours = blank(Format::GcCard, Some("2043"), Layout::Bare);
    assert_eq!(ours.len(), theirs.len());
    assert_eq!(ours[0x14..0x1FC], theirs[0x14..0x1FC], "the header past the serial and the time");
    assert_eq!(ours[0x200..], theirs[0x200..], "everything after the header's checksum");
}

/// Flycast's empty VMU, which it writes for a slot with none in it and which was dumped from one a
/// console formatted, is this VMU in its allocation table, its directory, its hidden blocks and its
/// root block, but for three things the console's owner or its clock chose.
///
/// Those are the custom colour at 0x10, the format time at 0x30 and the icon shape at 0x4E, which
/// a VMU made here leaves at the standard colour, zero and the first shape. The 200 blocks a save
/// can take are left out: they are free, and a console does not wipe a block it frees, so the
/// dumped VMU still carries scraps of whatever was on it.
#[test]
#[cfg(feature = "vmu")]
fn an_empty_vmu_is_the_one_flycast_ships_but_for_its_colour_clock_and_icon() {
    const BLOCK: usize = 512;
    let Some(theirs) = fixture("macos/flycast/vmu_save_A1.bin") else { return };
    let ours = blank(Format::Vmu, None, Layout::Bare);
    assert_eq!(ours.len(), theirs.len());
    let root = 255 * BLOCK;
    assert_eq!(
        ours[200 * BLOCK..root],
        theirs[200 * BLOCK..root],
        "the hidden blocks, the directory and the table"
    );

    let owned = |root: &[u8]| {
        let mut root = root.to_vec();
        root[0x10..0x15].fill(0);
        root[0x30..0x38].fill(0);
        root[0x4E..0x50].fill(0);
        root
    };
    assert_eq!(owned(&ours[root..]), owned(&theirs[root..]), "the root block");
}
