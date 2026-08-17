//! What this crate reads out of real cards, checked against dumps under the workspace's
//! `data/saves`.
//!
//! These live out here rather than beside the code because they need files the crate does not
//! carry: a published tarball holds the crate and nothing else, so a test that reaches for a
//! 72 KiB MiSTer save could not run from one. The unit tests build their own cards and travel
//! with the crate; these are the ones that need a console to have written the bytes.

use neogeo_memcard::{
    BLOCK, CHECKSUM_OFFSET, CardBuilder, Container, Error, Geometry, MISTER_BACKUP_RAM, MemoryCard, Region,
    detect, fat, fat_checksum,
};

/// The real cards this crate was written against, as MiSTer wrote them.
///
/// `set` is `NEOGEO` for the 2 KiB card a cartridge system takes, `NEOGEO-CD` for the 8 KiB one
/// a Neo Geo CD keeps inside itself.
///
/// `tests/` symlinks the workspace's `data/saves`, which cargo follows when packaging, so the
/// cards travel with the crate and these tests run from a published tarball too.
fn fixture_in(set: &str, name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/saves").join(set);
    let dir = path.join(name).join("MiSTer");
    let entry = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .filter_map(std::result::Result::ok)
        .find(|e| e.path().extension().is_some_and(|x| x == "sav"))
        .expect("a .sav fixture");
    std::fs::read(entry.path()).expect("reads")
}

/// A card off a cartridge system.
fn fixture(name: &str) -> Vec<u8> {
    fixture_in("NEOGEO", name)
}

/// A bare card, as the NeoCD libretro core writes it: no container and no word swap.
fn neocd_fixture(name: &str) -> Vec<u8> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../data/saves/NEOGEO-CD")
        .join(name)
        .join("NeoCD");
    let entry = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .filter_map(std::result::Result::ok)
        .find(|e| e.path().extension().is_some_and(|x| x == "srm"))
        .expect("a .srm fixture");
    std::fs::read(entry.path()).expect("reads")
}

#[test]
fn reads_a_real_mister_card() {
    let card = MemoryCard::parse(&fixture("Metal Slug X")).expect("reads");
    assert_eq!(card.container(), Container::MiSter);
    // 2 KiB of card; the header states 0x1000 because that is the address span it answers to.
    assert_eq!(card.capacity(), 2048);
    assert_eq!(card.region(), Region::Europe);
    assert_eq!(card.backup_ram().map(<[u8]>::len), Some(MISTER_BACKUP_RAM));

    let geometry = card.geometry();
    assert_eq!(geometry.size_word(), 0x1000);
    assert_eq!((geometry.blocks, geometry.entries), (32, 32));
    assert_eq!((geometry.directory_block, geometry.fat1_block), (1, 3));
    assert_eq!((geometry.fat2_block, geometry.first_data_block), (4, 5));

    // Metal Slug X keeps its stage progress and its options apart, so one cartridge writes two
    // saves: same NGH, different sub-numbers.
    assert_eq!(card.saves().len(), 2);
    let (stages, options) = (&card.saves()[0], &card.saves()[1]);

    assert_eq!((stages.slot, stages.sub), (0, 0));
    assert_eq!((options.slot, options.sub), (1, 1));
    // Metal Slug X is NGH-250, and the card stores it as binary-coded decimal.
    assert_eq!((stages.ngh, options.ngh), (0x0250, 0x0250));
    assert_eq!(stages.title(), "METAL SLUG X");
    assert_eq!(options.title(), "METAL SLUG X OPTION");
    assert_eq!(stages.dirent, vec![0x00, 0x02, 0x50, 0x05]);
    assert_eq!(options.dirent, vec![0x01, 0x02, 0x50, 0x06]);

    // Two one-block saves in adjacent blocks, each its own chain of one. A reader that took a
    // run of non-free blocks as one save's extent would hand the first save both and lose the
    // second; the chain is what says otherwise.
    assert_eq!((stages.data.len(), options.data.len()), (BLOCK, BLOCK));
    let fat = &card.image()[geometry.fat1_block * BLOCK..][..geometry.fat_len()];
    assert_eq!(&fat[5..8], &[fat::CHAIN_END, fat::CHAIN_END, fat::FREE]);
}

#[test]
fn the_other_card_differs_only_in_the_game_it_holds() {
    let card = MemoryCard::parse(&fixture("Fatal Fury 2")).expect("reads");
    let save = &card.saves()[0];
    // Fatal Fury 2 is NGH-047.
    assert_eq!(save.ngh, 0x0047);
    assert_eq!(save.dirent, vec![0x00, 0x00, 0x47, 0x05]);
    // Fatal Fury 2 writes a binary header where the BIOS's title convention puts text, so
    // there is no title rather than a run of mojibake.
    assert_eq!(save.title(), "");
    // Both cards were formatted by the same core, so everything but the entry matches.
    let other = MemoryCard::parse(&fixture("Metal Slug X")).expect("reads");
    assert_eq!(card.username(), other.username());
    assert_eq!(card.region(), other.region());
    assert_eq!(card.geometry(), other.geometry());
}

/// The two fixtures allocate a different number of blocks, so their checksums differ — which
/// is what makes this a check of the rule rather than of one constant.
#[test]
fn the_table_checksum_is_the_sum_the_header_carries() {
    for name in ["Metal Slug X", "Fatal Fury 2"] {
        let card = MemoryCard::parse(&fixture(name)).expect("reads");
        let g = card.geometry();
        let fat1 = &card.image()[g.fat1_block * BLOCK..][..g.fat_len()];
        let fat2 = &card.image()[g.fat2_block * BLOCK..][..g.fat_len()];
        assert_eq!(fat1, fat2, "{name}: the second table mirrors the first");
        assert_eq!(fat_checksum(fat1), card.image()[CHECKSUM_OFFSET], "{name}: FAT 1 checksum");
        assert_eq!(fat_checksum(fat2), card.image()[CHECKSUM_OFFSET + 1], "{name}: FAT 2 checksum");
    }
}

/// The 8 KiB card a Neo Geo CD keeps inside itself: a second size, a second geometry, and the
/// only save here that spans more than one block.
#[test]
fn reads_a_neo_geo_cd_card() {
    let card = MemoryCard::parse(&fixture_in("NEOGEO-CD", "Metal Slug")).expect("reads");
    assert_eq!(card.capacity(), 8192);
    assert_eq!(card.geometry().size_word(), 0x4000, "the header states the address span");
    assert_eq!((card.geometry().blocks, card.geometry().entries), (128, 128));
    assert_eq!(card.geometry().first_data_block, 13);

    assert_eq!(card.saves().len(), 1);
    let save = &card.saves()[0];
    assert_eq!(save.ngh, 0x0201);
    assert_eq!(save.title(), "METAL SLUG");
    // Fourteen blocks, chained 13 through 26. Nothing in the entry says how long it is; the
    // chain is the only thing that does.
    assert_eq!(save.data.len(), 14 * BLOCK);
    assert_eq!(save.dirent[3], 13);
}

/// The same game on both systems, which is what makes the two card sizes comparable.
#[test]
fn a_cd_card_and_a_cartridge_card_are_the_same_format_at_two_sizes() {
    let cart = MemoryCard::parse(&fixture("Fatal Fury 2")).expect("reads");
    let cd = MemoryCard::parse(&fixture_in("NEOGEO-CD", "Fatal Fury 2")).expect("reads");
    // Fatal Fury 2 is NGH-047 wherever it runs.
    assert_eq!(cart.saves()[0].ngh, cd.saves()[0].ngh);
    assert_eq!(cart.region(), cd.region());
    // What differs is the card, not the format.
    assert_ne!(cart.capacity(), cd.capacity());
    assert_ne!(cart.geometry().first_data_block, cd.geometry().first_data_block);
}

/// A second emulator's take on the same card, and the only one here holding two *different*
/// games. Nothing about it needed a change: a NeoCD `.srm` is the bare card, byte for byte in
/// the layout a MiSTer save carries behind its container and its word swap.
#[test]
fn reads_a_bare_card_from_another_emulator_holding_two_games() {
    let card = MemoryCard::parse(&neocd_fixture("Fatal Fury 2")).expect("reads");
    assert_eq!(card.container(), Container::Bare);
    assert_eq!(card.backup_ram(), None, "a bare card carries no cabinet RAM");
    assert_eq!(card.capacity(), 8192);
    assert_eq!(card.geometry(), Geometry::of(8192).expect("a real size"));
    // This core's BIOS is a Japanese one where MiSTer's was European, so the byte differs and
    // is carried rather than assumed.
    assert_eq!(card.region(), Region::Japan);

    // Two games sharing one card, which is what a Neo Geo CD does and a cartridge cannot.
    assert_eq!(card.saves().len(), 2);
    assert_eq!(card.saves()[0].ngh, 0x0201, "Metal Slug");
    assert_eq!(card.saves()[1].ngh, 0x0047, "Fatal Fury 2");
    assert_eq!(card.saves()[0].title(), "METAL SLUG");
    // The 14-block chain runs first, so the second game starts where it left off.
    assert_eq!(card.saves()[0].data.len(), 14 * BLOCK);
    assert_eq!(card.saves()[1].data.len(), BLOCK);
    assert_eq!((card.saves()[0].dirent[3], card.saves()[1].dirent[3]), (13, 27));
}

/// The same 8 KiB card written by two unrelated emulators. What matches is the format; what
/// differs is the container, the region byte and which games were saved.
#[test]
fn two_emulators_agree_on_the_format() {
    let mister = MemoryCard::parse(&fixture_in("NEOGEO-CD", "Metal Slug")).expect("reads");
    let neocd = MemoryCard::parse(&neocd_fixture("Metal Slug")).expect("reads");
    assert_eq!(mister.geometry(), neocd.geometry());
    assert_eq!(mister.saves().len(), neocd.saves().len());
    for (a, b) in mister.saves().iter().zip(neocd.saves()) {
        assert_eq!((a.ngh, a.sub), (b.ngh, b.sub));
        assert_eq!(a.title(), b.title());
        assert_eq!(a.data.len(), b.data.len(), "the same 14-block chain");
    }
    assert_ne!(mister.container(), neocd.container());
}

/// King of Fighters '96 was run and quit without saving, so its card was never formatted.
#[test]
fn a_card_nothing_was_ever_saved_to_is_not_a_card() {
    let raw = fixture_in("NEOGEO-CD", "King of Fighters '96");
    assert!(!detect(&raw), "no signature, so nothing to read");
    assert!(matches!(MemoryCard::parse(&raw), Err(Error::WrongLength(_))));
}

#[test]
fn a_card_a_console_wrote_is_rebuilt_byte_for_byte() {
    // The strongest check there is: every real MiSTer save, read and written back whole. Two
    // card sizes, one save and two, one block and fourteen.
    for (set, name) in [
        ("NEOGEO", "Metal Slug X"),
        ("NEOGEO", "Fatal Fury 2"),
        ("NEOGEO-CD", "Metal Slug"),
        ("NEOGEO-CD", "Fatal Fury 2"),
    ] {
        let original = fixture_in(set, name);
        let card = MemoryCard::parse(&original).expect("reads");
        let rebuilt = CardBuilder::from_card(&card).build().expect("writes");
        assert_eq!(rebuilt, original, "{set}/{name} did not round-trip");
    }
}

/// A bare card from another emulator, rebuilt as it arrived. No container to put back, and a
/// directory holding two games rather than one game's two saves.
#[test]
fn a_bare_card_from_another_emulator_round_trips() {
    for name in ["Fatal Fury 2", "Metal Slug"] {
        let original = neocd_fixture(name);
        let card = MemoryCard::parse(&original).expect("reads");
        assert_eq!(CardBuilder::from_card(&card).build().expect("writes"), original, "{name}");
    }
}

#[test]
fn a_bare_card_round_trips_too() {
    let card = MemoryCard::parse(&fixture("Metal Slug X")).expect("reads");
    let bare = card.image().to_vec();
    let reparsed = MemoryCard::parse(&bare).expect("reads the bare card");
    assert_eq!(reparsed.container(), Container::Bare);
    assert_eq!(reparsed.saves(), card.saves());

    let mut builder = CardBuilder::from_card(&reparsed);
    assert_eq!(builder.container(Container::Bare).build().expect("writes"), bare);
}
