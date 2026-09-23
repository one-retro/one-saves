//! What this crate reads out of real volumes, checked against dumps under the workspace's
//! `data/saves`.
//!
//! These live out here rather than beside the code because they need files the crate does not
//! carry: a published tarball holds the crate and nothing else, so a test that reaches for a
//! 512 KiB cartridge could not run from one. The unit tests build their own volumes and travel
//! with the crate; these are the ones that need a console to have written the bytes.
//!
//! Every fixture is Beetle Saturn's — the libretro fork of Mednafen — which names the console's
//! internal memory `.srm` and the Backup RAM Cart `.bcr`.

use saturn_backup::{BackupBuilder, BackupRam, Geometry, detect};

/// A dump off one game's internal memory, or off its cart.
fn fixture(game: &str, extension: &str) -> Vec<u8> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../data/saves/Saturn")
        .join(game)
        .join("Beetle Saturn");
    let entry = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .filter_map(std::result::Result::ok)
        .find(|e| e.path().extension().is_some_and(|x| x == extension))
        .unwrap_or_else(|| panic!("a .{extension} fixture in {}", dir.display()));
    std::fs::read(entry.path()).expect("reads")
}

/// Every volume here, as `(game, extension)`.
const VOLUMES: [(&str, &str); 5] = [
    ("Panzer Dragoon Saga", "srm"),
    ("Panzer Dragoon Saga", "bcr"),
    ("NiGHTS into Dreams", "srm"),
    ("Sonic 3D Blast", "srm"),
    ("Sonic 3D Blast", "bcr"),
];

#[test]
fn the_console_and_the_cart_state_their_own_block_sizes() {
    // The whole reason the block size is read off the signature: one filesystem, two media, and
    // nothing but the repeat count tells them apart.
    for (game, extension) in VOLUMES {
        let volume = BackupRam::parse(&fixture(game, extension)).expect("reads");
        let want = if extension == "srm" { (32_768, 64, 512) } else { (524_288, 512, 1024) };
        let g = volume.geometry();
        assert_eq!((g.size, g.block, g.blocks), want, "{game} .{extension}");
    }
}

#[test]
fn panzer_dragoon_saga_reads_off_both_media() {
    let internal = BackupRam::parse(&fixture("Panzer Dragoon Saga", "srm")).expect("reads");
    assert_eq!(internal.capacity(), 32_640, "every block past the two reserved ones");
    let saves = internal.saves();
    assert_eq!(saves.len(), 2);
    assert_eq!(saves[0].name, "PANDRA_3_01");
    assert_eq!(saves[1].name, "PANDRA_3_03", "the tail of the name counts the game's own slot");
    for save in saves {
        assert_eq!(save.comment, "AZEL#1Lv01");
        assert_eq!(save.data.len(), 1276);
        // 2026-09-22 17:40 UTC, which is when the dump was taken.
        assert_eq!(save.written_at(), 1_790_098_800);
    }
    // 1276 bytes comes to 23 of the console's 64-byte blocks, so the second save starts after.
    assert_eq!((saves[0].block, saves[1].block), (2, 25));

    // The same game on a cart: three 512-byte blocks where the console needed twenty-three.
    let cart = BackupRam::parse(&fixture("Panzer Dragoon Saga", "bcr")).expect("reads");
    assert_eq!(cart.saves().len(), 2);
    assert_eq!((cart.saves()[0].block, cart.saves()[1].block), (2, 5));
    assert_eq!(cart.saves()[0].data, internal.saves()[0].data, "the medium must not show through");
}

#[test]
fn a_save_of_two_hundred_blocks_reassembles() {
    // NiGHTS keeps its A-life data in 12544 bytes, which is 217 of the console's blocks. The
    // block list alone runs to 434 bytes there, so it spills across seven blocks of its own
    // before the save's data even starts — the case a short save cannot exercise at all.
    let volume = BackupRam::parse(&fixture("NiGHTS into Dreams", "srm")).expect("reads");
    let saves = volume.saves();
    assert_eq!(saves.len(), 2);

    assert_eq!(saves[0].name, "NIGHTS___01");
    assert_eq!(saves[0].comment, "Score data");
    assert_eq!(saves[0].data.len(), 1856);

    assert_eq!(saves[1].name, "NIGHTS___02");
    assert_eq!(saves[1].comment, "A-life");
    assert_eq!(saves[1].data.len(), 12_544);
    // Where the second save starts is where the first one's 33 blocks stop.
    assert_eq!((saves[0].block, saves[1].block), (2, 35));
    assert_eq!(volume.geometry().blocks_for(12_544), 217, "the count the console agreed on");
}

#[test]
fn a_formatted_volume_with_nothing_on_it_is_still_a_volume() {
    // Sonic 3D Blast keeps no saves at all, so both its volumes are formatted and empty. That is
    // a real card rather than a broken one, and reading it must say so.
    for extension in ["srm", "bcr"] {
        let volume = BackupRam::parse(&fixture("Sonic 3D Blast", extension)).expect("reads");
        assert!(volume.saves().is_empty(), ".{extension} holds no saves");
        assert!(detect(&fixture("Sonic 3D Blast", extension)));
    }
}

#[test]
fn every_real_volume_rebuilds_byte_for_byte() {
    // The test the writer exists to pass. Each save's entry and block list are written from
    // scratch rather than copied, so matching the console's own bytes is what says the layout is
    // right and not merely self-consistent.
    for (game, extension) in VOLUMES {
        let bytes = fixture(game, extension);
        let volume = BackupRam::parse(&bytes).expect("reads");
        let rebuilt = BackupBuilder::from_volume(&volume).build().expect("writes");
        assert_eq!(rebuilt, bytes, "{game} .{extension} does not rebuild byte for byte");
    }
}

#[test]
fn every_real_save_states_language_zero() {
    // Three games across two regions, and not one of them sets the byte. It is recorded here
    // because it is the one entry field a bundle has nowhere to keep, and what that costs in
    // practice is exactly nothing until a save turns up that disagrees.
    for (game, extension) in VOLUMES {
        let volume = BackupRam::parse(&fixture(game, extension)).expect("reads");
        for save in volume.saves() {
            assert_eq!(save.language, 0, "{game} .{extension}: {}", save.name);
        }
    }
}

#[test]
fn a_save_moved_between_media_keeps_its_bytes() {
    // Off the console's 64-byte blocks and onto a cart's 512-byte ones, which re-lays the block
    // list at a different length. What must survive is the payload.
    let internal = BackupRam::parse(&fixture("NiGHTS into Dreams", "srm")).expect("reads");
    let mut builder = BackupBuilder::new(524_288, 512).expect("a cart geometry");
    for save in internal.saves() {
        builder.add(save.clone());
    }
    let moved = BackupRam::parse(&builder.build().expect("writes")).expect("reads");
    assert_eq!(moved.saves().len(), 2);
    for (before, after) in internal.saves().iter().zip(moved.saves()) {
        assert_eq!(before.data, after.data, "{}", before.name);
        assert_eq!(before.name, after.name);
        assert_eq!(before.written_at(), after.written_at());
    }
    // And the cart lays that 217-block save out in far fewer, larger blocks.
    assert!(moved.saves()[1].block < internal.saves()[1].block);
}

#[test]
fn an_entry_is_the_same_bytes_wherever_the_save_sits() {
    // The claim the `entry` field rests on, and the one that decides whether this format could
    // carry a `dirent` like every other. Nothing in the entry depends on placement — name,
    // language, comment, date and length are all the save's — so moving a save to a different
    // block, on a different medium, at a different block size must leave those thirty bytes
    // untouched. Only the block list that follows them moves.
    let internal = BackupRam::parse(&fixture("NiGHTS into Dreams", "srm")).expect("reads");

    let mut cart = BackupBuilder::new(524_288, 512).expect("a cart geometry");
    // Reversed, so neither save lands on the block it came from.
    for save in internal.saves().iter().rev() {
        cart.add(save.clone());
    }
    let moved = BackupRam::parse(&cart.build().expect("writes")).expect("reads");

    for before in internal.saves() {
        let after = moved.saves().iter().find(|s| s.name == before.name).expect("still there");
        assert_ne!(before.block, after.block, "{}: did not actually move", before.name);
        assert_eq!(before.entry, after.entry, "{}: the entry moved with the save", before.name);
        assert_eq!(before.data, after.data);
    }
}

#[test]
fn a_game_may_leave_bytes_in_a_field_it_did_not_fill() {
    // NiGHTS writes `A-life` into ten bytes of comment and leaves an 0xEE in the eighth, past the
    // terminator. It is one byte, it is meaningless, and reproducing it is the difference between
    // a volume that rebuilds and one that nearly does — which is why the entry is kept verbatim
    // rather than re-encoded from the fields.
    let volume = BackupRam::parse(&fixture("NiGHTS into Dreams", "srm")).expect("reads");
    let alife = volume.saves().iter().find(|s| s.comment == "A-life").expect("the A-life save");
    let comment = &alife.entry[0x10 - saturn_backup::TAG_LEN..][..saturn_backup::COMMENT_LEN];
    assert_eq!(comment, b"A-life\0\xee\0\0", "the padding a decode throws away");
}

#[test]
fn a_flat_save_is_not_taken_for_a_volume() {
    assert!(!detect(&vec![0u8; 32_768]), "an unformatted volume carries no signature");
    assert_eq!(Geometry::of(&fixture("NiGHTS into Dreams", "srm")).map(|g| g.block), Some(64));
}
