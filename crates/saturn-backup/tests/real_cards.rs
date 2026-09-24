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

/// A dump off one game's internal memory, or off its cart, as Beetle Saturn wrote it.
fn fixture(game: &str, extension: &str) -> Vec<u8> {
    producer_fixture(game, "Beetle Saturn", extension)
}

/// The same, naming the producer.
fn producer_fixture(game: &str, producer: &str, extension: &str) -> Vec<u8> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../data/saves/Saturn")
        .join(game)
        .join(producer);
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
fn the_smpc_files_beside_the_volumes_decode() {
    // Not backup RAM: the console's clock and its four bytes of BIOS settings, which Beetle
    // Saturn writes beside the volume. Read here because dropping the file loses both.
    for (game, hour, minute, second) in [("Panzer Dragoon Saga", 17, 56, 0), ("Sonic 3D Blast", 18, 14, 14)] {
        let smpc = saturn_backup::Smpc::parse(&fixture(game, "smpc")).expect("reads");
        assert!(smpc.valid, "{game}");
        assert_eq!((smpc.year(), smpc.month(), smpc.day()), (2026, 9, 22), "{game}");
        assert_eq!((smpc.hour(), smpc.minute(), smpc.second()), (hour, minute, second), "{game}");
        // 2026-09-22 was a Tuesday, and the console agrees.
        assert_eq!(smpc.weekday(), 2, "{game}");
        assert_eq!(smpc.to_bytes().as_slice(), fixture(game, "smpc"), "{game} does not round-trip");
    }
}

#[test]
fn the_console_language_is_what_a_save_records() {
    // The language byte in a save's entry is a copy of the console's, and the console's lives in
    // the low nibble of the SMPC's `SaveMem[3]`. Zero is English rather than unset, which is why
    // every save on these volumes states it: the console was English when they were written.
    //
    // Only that nibble has a meaning here. Sonic's volume has `SaveMem[2]` set to 4, which
    // something wrote and nothing in this crate claims to understand.
    for game in ["Panzer Dragoon Saga", "Sonic 3D Blast"] {
        let smpc = saturn_backup::Smpc::parse(&fixture(game, "smpc")).expect("reads");
        assert_eq!(smpc.language(), 0, "{game}: English");
    }
    let volume = BackupRam::parse(&fixture("Panzer Dragoon Saga", "srm")).expect("reads");
    for save in volume.saves() {
        assert_eq!(save.language, 0, "{}", save.name);
    }
}

#[test]
fn a_console_whose_settings_were_set_states_them() {
    // The same console after its clock was set to 2010 and its language to French, dumped from
    // the BIOS itself. This is the fixture that fixes the enumeration: 2 is French, which pins
    // the order as English, German, French, Spanish, Italian, Japanese rather than the one the
    // field is usually described in.
    let smpc = saturn_backup::Smpc::parse(&fixture("Saturn BIOS", "smpc")).expect("reads");
    assert!(smpc.valid);
    assert_eq!(smpc.language(), 2, "French, and 2 is what French is");
    assert_eq!(smpc.save_mem, [0, 0, 0, 2], "and nothing else in the settings was touched");

    // The clock moved with it, and the BIOS recomputed the weekday rather than leaving a stale
    // one: 2010-09-23 really was a Thursday, where 2026-09-23 is a Wednesday.
    assert_eq!((smpc.year(), smpc.month(), smpc.day()), (2010, 9, 23));
    assert_eq!((smpc.hour(), smpc.minute(), smpc.second()), (10, 11, 19));
    assert_eq!(smpc.weekday(), 4, "Thursday");
    assert_eq!(smpc.read_at(), Some(1_285_236_679));
    assert_eq!(smpc.to_bytes().as_slice(), fixture("Saturn BIOS", "smpc"));
}

#[test]
fn yabause_writes_the_address_space_and_it_reads_the_same() {
    // The second producer, and the one that proves the container is worth telling apart. Yabause
    // dumps the address space the 68000 sees rather than the data: 64 KiB for 32 KiB of volume,
    // each byte behind the 0xFF an unmapped even address reads. Same filesystem underneath.
    let wide = producer_fixture("NiGHTS into Dreams", "Yabause", "srm");
    assert_eq!(wide.len(), 65_536, "twice the volume it holds");
    assert!((0..wide.len()).step_by(2).all(|i| wide[i] == 0xFF), "every even address is open bus");

    let volume = BackupRam::parse(&wide).expect("reads");
    assert_eq!(volume.container(), saturn_backup::Container::Wide);
    assert_eq!(volume.data().len(), 32_768, "the volume itself is the console's usual 32 KiB");
    assert_eq!(volume.geometry().block, 64);
    assert_eq!(volume.capacity(), 32_640, "the same capacity Beetle Saturn's dump reports");

    // The same two saves, by the same names, at the same sizes as the other producer's.
    let saves = volume.saves();
    assert_eq!(saves.len(), 2);
    assert_eq!(saves[0].name, "NIGHTS___01");
    assert_eq!(saves[0].data.len(), 1856);
    assert_eq!(saves[1].name, "NIGHTS___02");
    assert_eq!(saves[1].data.len(), 12_544);
    assert!(detect(&wide), "and it is detected without being told");
}

#[test]
fn a_wide_dump_rebuilds_byte_for_byte() {
    // The container survives the round trip, so a Yabause volume comes back as Yabause wrote it
    // rather than in the form this crate happens to work in.
    let wide = producer_fixture("NiGHTS into Dreams", "Yabause", "srm");
    let volume = BackupRam::parse(&wide).expect("reads");
    assert_eq!(BackupBuilder::from_volume(&volume).build().expect("writes"), wide);

    // And a caller can move a volume between the two, which is what converting between producers
    // amounts to. The saves are untouched either way.
    let mut packed = BackupBuilder::from_volume(&volume);
    packed.container(saturn_backup::Container::Packed);
    let packed = packed.build().expect("writes");
    assert_eq!(packed.len(), 32_768);
    assert_eq!(BackupRam::parse(&packed).expect("reads").saves(), volume.saves());
}

#[test]
fn standalone_mednafen_writes_packed_and_gzips_only_the_cart() {
    // The third producer. Its internal memory is packed and plain, like its own libretro fork's
    // — so the fork's container is not a libretro invention — but the Backup RAM Cart it writes
    // is a gzip member. This crate depends on nothing and so does not inflate it; what it does is
    // say which of those two things went wrong.
    let internal = producer_fixture("NiGHTS into Dreams", "Mednafen", "bkr");
    let volume = BackupRam::parse(&internal).expect("reads");
    assert_eq!(volume.container(), saturn_backup::Container::Packed);
    assert_eq!(volume.geometry().block, 64);
    assert_eq!(volume.saves().len(), 2);
    assert_eq!(BackupBuilder::from_volume(&volume).build().expect("writes"), internal);

    let cart = producer_fixture("NiGHTS into Dreams", "Mednafen", "bcr");
    assert_eq!(&cart[..2], &saturn_backup::GZIP_MAGIC, "gzip, not a volume");
    assert_eq!(cart.len(), 804, "512 KiB of mostly-empty volume comes down to this");
    check_gzipped_cart(&cart);
}

/// With `gzip`, the member inflates and reads like any other volume.
#[cfg(feature = "gzip")]
fn check_gzipped_cart(cart: &[u8]) {
    assert!(detect(cart), "a gzip member is a volume this build can read");
    let volume = BackupRam::parse(cart).expect("inflates");
    assert!(volume.compressed());
    assert_eq!(volume.container(), saturn_backup::Container::Packed, "gzip wraps a packed volume");
    assert_eq!(volume.geometry().block, 512, "a 512 KiB cart");
    assert_eq!(volume.geometry().blocks, 1024);
    assert_eq!(volume.saves().len(), 1);
    assert_eq!(volume.saves()[0].name, "NIGHTS___01");
    assert_eq!(volume.saves()[0].data.len(), 1856);

    // The volume inside rebuilds exactly. The gzip member around it does not and is not meant to:
    // a deflate stream is one of many for the same bytes, and which one a producer emits is its
    // own business. `image()` is what keeps the file that was actually read.
    let rebuilt = BackupBuilder::from_volume(&volume).build().expect("writes");
    assert_eq!(rebuilt.len(), 524_288, "the volume, not the member");
    assert_eq!(volume.image(), cart, "and the member as it arrived is still there");
}

/// Without it, the shape is still recognised and the refusal says which container it is.
#[cfg(not(feature = "gzip"))]
fn check_gzipped_cart(cart: &[u8]) {
    let error = BackupRam::parse(cart).expect_err("nothing here inflates");
    assert!(matches!(error, saturn_backup::Error::Compressed), "{error:?}");
    assert!(error.to_string().contains("decompress"), "{error}");
    assert!(!detect(cart), "and it is not claimed as a volume this build can read");
}

#[test]
fn three_producers_agree_on_the_filesystem() {
    // Beetle Saturn packed, Yabause wide, standalone Mednafen packed: three containers' worth of
    // disagreement about how to store a volume, and none about what is in one. The saves differ
    // in content because they are different play sessions; what must match is the shape.
    let volumes = [
        BackupRam::parse(&fixture("NiGHTS into Dreams", "srm")).expect("Beetle Saturn"),
        BackupRam::parse(&producer_fixture("NiGHTS into Dreams", "Yabause", "srm")).expect("Yabause"),
        BackupRam::parse(&producer_fixture("NiGHTS into Dreams", "Mednafen", "bkr")).expect("Mednafen"),
    ];
    for volume in &volumes {
        assert_eq!(volume.geometry().block, 64);
        assert_eq!(volume.geometry().blocks, 512);
        assert_eq!(volume.capacity(), 32_640);
        let saves = volume.saves();
        assert_eq!(saves.len(), 2);
        assert_eq!((saves[0].name.as_str(), saves[0].comment.as_str()), ("NIGHTS___01", "Score data"));
        assert_eq!((saves[1].name.as_str(), saves[1].comment.as_str()), ("NIGHTS___02", "A-life"));
        assert_eq!((saves[0].data.len(), saves[1].data.len()), (1856, 12_544));
        assert_eq!((saves[0].block, saves[1].block), (2, 35), "even the allocation agrees");
    }
}

#[test]
fn a_fragmented_volume_reads_and_rebuilds_as_the_console_left_it() {
    // Seeded rather than found, then handed back by a console. NiGHTS' two real saves were laid
    // down with their blocks interleaved one for one — the shape a volume reaches when a save is
    // deleted and a larger one written into the hole — and the result was loaded in Mednafen.
    //
    // The BIOS read both saves, and the game then wrote one of them back. It stayed exactly where
    // it was: still on blocks 2, 4, 6, 8 …, still threaded through the other save. So this is a
    // layout a console produces and maintains, not one this crate invented, and the reader
    // following the block list rather than walking forward is what the console itself requires.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../data/saves/Saturn/Fragmented card/one-saves/fragmented.bkr");
    let image = std::fs::read(&path).expect("the fixture");
    let volume = BackupRam::parse(&image).expect("reads");

    let saves = volume.saves();
    assert_eq!(saves.len(), 2);
    assert_eq!((saves[0].name.as_str(), saves[1].name.as_str()), ("NIGHTS___01", "NIGHTS___02"));

    // The small save is every even block from 2; the large one takes the odd blocks beside it
    // until the small one is satisfied, then runs on through what is left. So one save is
    // scattered end to end and the other is scattered for its first 33 blocks and contiguous
    // after — two different ways of not being a run, in one volume.
    let evens: Vec<usize> = (0..33).map(|n| 2 + n * 2).collect();
    assert_eq!(saves[0].blocks, evens, "the console kept the scattered layout");

    let big = &saves[1].blocks;
    assert_eq!(big.len(), 217);
    assert_eq!(big[..33], (0..33).map(|n| 3 + n * 2).collect::<Vec<_>>()[..], "interleaved first");
    assert_eq!(big[33..], (68..=251).collect::<Vec<_>>()[..], "then whatever was left");
    assert!(big.iter().all(|b| !evens.contains(b)), "and never where the other save sits");
    assert_eq!(saves[0].block, saves[0].blocks[0]);

    // And it rebuilds byte for byte, which needs the layout to come back too: an allocator that
    // tidied these into runs would produce a correct volume that is not this one.
    assert_eq!(BackupBuilder::from_volume(&volume).build().expect("writes"), image);
}

#[test]
fn rebuilding_keeps_a_layout_and_allocating_afresh_does_not() {
    // The two halves of the contract, told apart. `from_volume` puts every save back where it
    // was; a builder given the same saves with nothing said about placement lays them out in
    // runs. Both are right, and only one of them reproduces a file.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../data/saves/Saturn/Fragmented card/one-saves/fragmented.bkr");
    let volume = BackupRam::parse(&std::fs::read(&path).expect("the fixture")).expect("reads");

    let mut fresh =
        BackupBuilder::new(volume.geometry().size, volume.geometry().block).expect("a real geometry");
    fresh.system_area(volume.system_area().to_vec());
    for save in volume.saves() {
        fresh.add(save.clone());
    }
    let tidied = BackupRam::parse(&fresh.build().expect("writes")).expect("reads");

    // Same saves, same bytes, different addresses.
    for (before, after) in volume.saves().iter().zip(tidied.saves()) {
        assert_eq!(before.data, after.data, "{}", before.name);
        assert_eq!(before.entry, after.entry);
    }
    assert_eq!(tidied.saves()[0].blocks, (2..35).collect::<Vec<_>>(), "a run, where the console had none");
    assert_ne!(tidied.saves()[1].blocks, volume.saves()[1].blocks);
}

#[test]
fn a_flat_save_is_not_taken_for_a_volume() {
    assert!(!detect(&vec![0u8; 32_768]), "an unformatted volume carries no signature");
    assert_eq!(Geometry::of(&fixture("NiGHTS into Dreams", "srm")).map(|g| g.block), Some(64));
}
