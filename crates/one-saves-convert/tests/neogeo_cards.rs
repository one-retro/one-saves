//! What the Neo Geo adapter makes of real cards, checked against dumps under the workspace's
//! `data/saves`.
//!
//! These live out here rather than beside the code because they need files the crate does not
//! carry: a published tarball holds the crate and nothing else, so a test that reaches for a
//! 72 KiB MiSTer save could not run from one.

// The adapter is behind its feature, so without it there is nothing here to test.
#![cfg(feature = "neogeo")]

use neogeo_memcard::{MISTER_BACKUP_RAM, MemoryCard};
use one_saves::{Bundle, Game, PartKind, Shape, Slug};
use one_saves_convert::card::CardOptions;
use one_saves_convert::card::neogeo::{read, write};

/// The real MiSTer cards under `data/`: `NEOGEO` for a cartridge system, `NEOGEO-CD` for a CD, and
/// `MVS` for a cartridge system run as a cabinet, its backup RAM set up in front of the card, or in
/// front of the cartridge's own RAM for the two that have some.
fn fixture_in(set: &str, name: &str) -> Vec<u8> {
    // The core keeps both sets in one folder, so the set and the game together pick out the file.
    let file = match (set, name) {
        ("NEOGEO", "Fatal Fury 2") => "Fatal Fury 2 (fatfury2).sav",
        ("NEOGEO", "Metal Slug X") => "Metal Slug X (mslugx).sav",
        ("MVS", "Metal Slug X") => "Metal Slug X (mslugx) (MVS).sav",
        ("MVS", "Jockey Grand Prix") => "Jockey Grand Prix (set 1) (jockeygp).sav",
        ("MVS", "V-Liner") => "V-Liner (set 1) (vliner).sav",
        ("NEOGEO-CD", "Fatal Fury 2") => "Garou Densetsu 2 ~ Fatal Fury 2 (Japan) (En,Ja).sav",
        ("NEOGEO-CD", "King of Fighters '96") => "King of Fighters '96, The (Japan, USA) (En,Ja).sav",
        ("NEOGEO-CD", "Metal Slug") => "Metal Slug (Japan) (En,Ja).sav",
        other => panic!("no MiSTer card for {other:?}"),
    };
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/saves/mister/neogeo-mister");
    std::fs::read(path.join(file)).expect("reads")
}

fn fixture(name: &str) -> Vec<u8> {
    fixture_in("NEOGEO", name)
}

/// A bare 8 KiB card, as the NeoCD libretro core writes a Neo Geo CD's memory.
fn neocd_fixture() -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../data/saves/retroarch/neocd/Metal Slug (Japan) (En,Ja).srm");
    std::fs::read(path).expect("reads")
}

fn system_of(bundle: &Bundle) -> &str {
    bundle.header.system.as_ref().expect("a card names its system").as_str()
}

#[test]
fn reads_a_real_card_and_names_the_game_by_its_ngh() {
    let bundle = read(&fixture("Metal Slug X"), &CardOptions::default()).expect("reads");

    assert_eq!(bundle.header.system.as_ref().unwrap().as_str(), "neogeo");
    let card = bundle.header.card.as_ref().expect("is a card");
    assert_eq!(card.format.as_str(), "neogeo-mc");
    // The whole card, its own blocks included, and of those only the header is the card's: the
    // directory and the tables follow from the saves.
    assert_eq!(card.capacity, 2048);
    assert_eq!(card.system_area.as_ref().map(Vec::len), Some(64));

    // Metal Slug X keeps stages and options apart, so one cartridge is two saves. The cabinet
    // RAM in both fixtures is erased, so nothing rides along for that.
    assert_eq!(bundle.parts.len(), 2);
    assert!(bundle.parts.iter().all(|p| p.kind == PartKind::Bundle));

    // A card has no filenames, so a save's path spells its NGH number and its sub-number.
    assert_eq!(bundle.parts[0].path.as_deref(), Some("0250-0"));
    assert_eq!(bundle.parts[1].path.as_deref(), Some("0250-1"));
    for part in &bundle.parts {
        assert_eq!(part.game.as_ref().unwrap().serial.as_deref(), Some("NGH-0250"));
        assert_eq!(part.dirent.as_ref().map(Vec::len), Some(4));
        // The card's own socket, not a numbered memory card slot.
        assert_eq!(part.role.as_ref().unwrap().as_str(), "neogeo-card");
    }
    assert_eq!((bundle.parts[0].slot, bundle.parts[1].slot), (Some(0), Some(1)));

    bundle.validate().expect("a card is a valid bundle");
}

#[test]
fn the_other_fixture_is_the_same_card_with_a_different_game() {
    let bundle = read(&fixture("Fatal Fury 2"), &CardOptions::default()).expect("reads");
    assert_eq!(bundle.parts[0].game.as_ref().unwrap().serial.as_deref(), Some("NGH-0047"));
    assert_eq!(bundle.parts[0].path.as_deref(), Some("0047-0"));
}

#[test]
fn a_neo_geo_cd_card_is_the_same_format_at_a_bigger_size() {
    let bundle = read(&fixture_in("NEOGEO-CD", "Metal Slug"), &CardOptions::default()).expect("reads");
    let card = bundle.header.card.as_ref().expect("is a card");
    assert_eq!(card.format.as_str(), "neogeo-mc");
    // The core emulates the CD's 8 KiB card only in CD mode, so in a MiSTer save its size says
    // which system ran, and the card is the console's own memory rather than one in a slot.
    assert_eq!(system_of(&bundle), "neogeo-cd");
    assert_eq!(card.capacity, 8192);
    assert_eq!(card.system_area.as_ref().map(Vec::len), Some(64), "a header the same size");

    assert_eq!(bundle.parts.len(), 1);
    assert_eq!(bundle.parts[0].path.as_deref(), Some("0201-0"));
    assert_eq!(bundle.parts[0].game.as_ref().unwrap().serial.as_deref(), Some("NGH-0201"));
    // The only save here that spans more than one block: fourteen of them, chained.
    let inner = Bundle::from_slice(&bundle.parts[0].bytes().unwrap()).expect("inner bundle");
    assert_eq!(inner.parts[0].payload.len(), 14 * 64);
    assert_eq!(system_of(&inner), "neogeo-cd", "a save sliced out still says it is a CD's");
    assert_eq!(bundle.parts[0].role.as_ref().unwrap().as_str(), "internal");
    assert_eq!(bundle.parts[0].system.as_ref().unwrap().as_str(), "neogeo-cd");
    bundle.validate().expect("valid bundle");
}

#[test]
fn a_bare_card_is_a_neo_geo_cd_s_only_when_the_caller_says() {
    // NeoCD writes the CD's memory bare, and nothing in it tells it from a cartridge console's
    // 8 KiB card, so the size alone settles nothing here.
    let bare = neocd_fixture();
    let unsaid = read(&bare, &CardOptions::default()).expect("reads");
    assert_eq!(system_of(&unsaid), "neogeo");
    assert_eq!(unsaid.parts[0].role.as_ref().unwrap().as_str(), "neogeo-card");

    let cd = read(&bare, &said("neogeo-cd")).expect("reads");
    assert_eq!(system_of(&cd), "neogeo-cd");
    assert_eq!(cd.parts[0].role.as_ref().unwrap().as_str(), "internal");
    let save = |bundle: &Bundle| {
        Bundle::from_slice(&bundle.parts[0].bytes().unwrap()).unwrap().parts[0].payload.clone()
    };
    assert_eq!(save(&cd), save(&unsaid), "the same save either way");

    // A system the format does not hold saves for came from a core or a ROM, not from the card,
    // and is not taken: MAME's core lists only `mame`.
    assert_eq!(system_of(&read(&bare, &said("mame")).expect("reads")), "neogeo");
    // What the caller names wins over what a MiSTer save's size suggests.
    let named = read(&fixture_in("NEOGEO-CD", "Metal Slug"), &said("neogeo")).expect("reads");
    assert_eq!(system_of(&named), "neogeo");
    assert_eq!(named.parts[0].role.as_ref().unwrap().as_str(), "neogeo-card");
    // And a cartridge console's card in a MiSTer save stays one.
    assert_eq!(system_of(&read(&fixture("Metal Slug X"), &CardOptions::default()).expect("reads")), "neogeo");
}

fn said(system: &str) -> CardOptions {
    CardOptions { system: Some(Slug::parse(system).unwrap()), ..CardOptions::default() }
}

/// In cart mode the core addresses 2 KiB of card and wraps everything past it, so a card whose
/// header claims 8 KiB is the 2 KiB there are, and is written back claiming 2 KiB.
#[test]
fn a_cartridge_s_card_in_a_mister_save_is_2_kib_whatever_its_header_claims() {
    let bundle = read(&fixture_in("NEOGEO-CD", "Metal Slug"), &said("neogeo")).expect("reads");
    assert_eq!(bundle.header.card.as_ref().expect("a card").capacity, 2048);

    let rebuilt = MemoryCard::parse(&write(&bundle).expect("writes")).expect("reads");
    assert_eq!(rebuilt.capacity(), 2048);
    assert_eq!(rebuilt.geometry().size, 2048, "its header now says so");
    let original = MemoryCard::parse(&fixture_in("NEOGEO-CD", "Metal Slug")).expect("reads");
    assert_eq!(rebuilt.saves()[0].data, original.saves()[0].data, "the fourteen blocks fit in 2 KiB");
    assert_eq!(rebuilt.region(), original.region());
}

#[test]
fn both_real_cards_round_trip_to_the_bare_card_they_hold() {
    // A MiSTer save is a container, so what comes back is the card inside it — byte for byte,
    // which is what the round trip is actually about.
    for (set, name) in [
        ("NEOGEO", "Metal Slug X"),
        ("NEOGEO", "Fatal Fury 2"),
        ("NEOGEO-CD", "Metal Slug"),
        ("NEOGEO-CD", "Fatal Fury 2"),
    ] {
        let original = fixture_in(set, name);
        let name = format!("{set}/{name}");
        let bare = MemoryCard::parse(&original).expect("reads").image().to_vec();
        let bundle = read(&original, &CardOptions::default()).expect("reads");
        assert_eq!(write(&bundle).expect("writes"), bare, "{name} did not round-trip");

        // And that bare card reads back to the same bundle, so nothing about the card was
        // carried by the container but the system its size implied, which a bare card is told.
        let options = CardOptions { system: bundle.header.system.clone(), ..CardOptions::default() };
        let again = read(&bare, &options).expect("reads the bare card");
        assert_eq!(again.parts.len(), bundle.parts.len());
        assert_eq!(again.parts[0].payload, bundle.parts[0].payload);
    }
}

/// The RAM an MVS BIOS leaves in a cabinet with this game in its one slot, in the 68000's order:
/// its signature, the slot table naming the game by its NGH number, and the game's name.
fn bios_ram(ngh: u16, name: &str) -> Vec<u8> {
    let mut ram = vec![0u8; MISTER_BACKUP_RAM];
    ram[0x10..0x20].copy_from_slice(b"BACKUP RAM OK !\x80");
    for slot in 0..8 {
        ram[0x124 + slot * 4..0x128 + slot * 4].copy_from_slice(&[0, 0, 0xFF, 0xFF]);
    }
    let [high, low] = ngh.to_be_bytes();
    ram[0x124..0x128].copy_from_slice(&[high, low, 0, 0]);
    let mut field = [b' '; 16];
    field[..name.len()].copy_from_slice(name.as_bytes());
    ram[0x2A0..0x2B0].copy_from_slice(&field);
    // And bookkeeping beside it, which the BIOS keeps whatever is in the slot.
    ram[0x8320..0x8324].copy_from_slice(&[1, 2, 3, 4]);
    ram
}

/// A MiSTer save with this RAM in place of the erased RAM the fixture carries, swapped the way
/// the core keeps it.
fn with_cabinet_ram(file: &[u8], ram: &[u8]) -> Vec<u8> {
    let region = neogeo_memcard::split_mister(file).expect("a MiSTer save").1;
    neogeo_memcard::join_mister(ram, &region).expect("fits")
}

fn metal_slug_x_in_a_cabinet() -> Vec<u8> {
    with_cabinet_ram(&fixture("Metal Slug X"), &bios_ram(0x0250, "METAL SLUG X"))
}

fn nested(bundle: &Bundle, index: usize) -> Bundle {
    Bundle::from_slice(&bundle.parts[index].bytes().expect("nested")).expect("a bundle")
}

/// The cabinet's RAM is the machine's memory rather than anything on the card, so a file holding
/// both is a device: the RAM in `internal`, the card in `neogeo-card`, each a bundle of its own.
#[test]
fn a_cabinet_s_backup_ram_beside_its_card_is_a_device() {
    let bundle = read(&metal_slug_x_in_a_cabinet(), &CardOptions::default()).expect("reads");
    bundle.validate().expect("a valid device");

    assert_eq!(bundle.header.shape, Shape::Device);
    assert_eq!(system_of(&bundle), "neogeo");
    assert_eq!(bundle.header.game, None, "a device names no game");
    let roles: Vec<&str> = bundle.parts.iter().map(|p| p.role.as_ref().expect("a role").as_str()).collect();
    assert_eq!(roles, ["internal", "neogeo-card"], "in the order the core writes them");
    assert!(bundle.parts.iter().all(|p| p.kind == PartKind::Bundle));

    // The RAM in the 68000's order, and naming the game the BIOS recorded in it.
    let ram = nested(&bundle, 0);
    assert_eq!(ram.header.shape, Shape::Save);
    assert_eq!(ram.parts[0].bytes().expect("reads").as_ref(), bios_ram(0x0250, "METAL SLUG X"));
    let game = Game::default().serial("NGH-0250").title("METAL SLUG X");
    assert_eq!(ram.header.game.as_ref(), Some(&game));
    assert_eq!(bundle.parts[0].game.as_ref(), Some(&game));

    // And the card is the card it always was, its saves each naming their game, and not said to
    // be any one game's itself.
    let card = nested(&bundle, 1);
    assert_eq!(card, read(&fixture("Metal Slug X"), &CardOptions::default()).expect("reads"));
    assert_eq!(bundle.parts[1].game, None);
}

/// Metal Slug X run as a cabinet, as MiSTer wrote it: the RAM names the game the BIOS recorded,
/// the card holds the save the game wrote, and the two write back as the file they came from.
#[test]
fn a_real_cabinet_reads_as_its_ram_and_its_card() {
    let original = fixture_in("MVS", "Metal Slug X");
    let bundle = read(&original, &CardOptions::default()).expect("reads");
    bundle.validate().expect("a valid device");

    assert_eq!(bundle.header.shape, Shape::Device);
    let roles: Vec<&str> = bundle.parts.iter().map(|p| p.role.as_ref().expect("a role").as_str()).collect();
    assert_eq!(roles, ["internal", "neogeo-card"]);
    let game = Game::default().serial("NGH-0250").title("METAL SLUG X");
    assert_eq!(nested(&bundle, 0).header.game.as_ref(), Some(&game));
    assert_eq!(nested(&bundle, 0).parts[0].payload.len(), MISTER_BACKUP_RAM as u64);

    let card = nested(&bundle, 1);
    assert_eq!(card.header.card.as_ref().expect("a card").capacity, 2048);
    assert_eq!(card.parts.len(), 1);
    assert_eq!(card.parts[0].path.as_deref(), Some("0250-0"));

    assert_eq!(write(&bundle).expect("writes"), original, "the core's 73728 bytes, byte for byte");
}

/// RAM that something wrote without the BIOS setting it up names no game, and is still kept.
#[test]
fn cabinet_ram_the_bios_did_not_set_up_names_no_game() {
    let mut ram = vec![0xFF; MISTER_BACKUP_RAM];
    ram[0x100] = 0x42;
    let bundle =
        read(&with_cabinet_ram(&fixture("Metal Slug X"), &ram), &CardOptions::default()).expect("reads");
    assert_eq!(bundle.header.shape, Shape::Device);
    assert_eq!(nested(&bundle, 0).header.game, None);
    assert_eq!(nested(&bundle, 0).parts[0].bytes().expect("reads").as_ref(), ram);
}

/// The device is everything the MiSTer file held, so it writes back as that file.
#[test]
fn a_cabinet_and_its_card_write_back_as_the_mister_save() {
    for (name, ngh) in [("Metal Slug X", 0x0250), ("Fatal Fury 2", 0x0047)] {
        let original = with_cabinet_ram(&fixture(name), &bios_ram(ngh, &name.to_uppercase()));
        let bundle = read(&original, &CardOptions::default()).expect("reads");
        assert_eq!(write(&bundle).expect("writes"), original, "{name}: the core's 73728 bytes");
    }
}

/// A cabinet whose card nothing ever formatted holds only its RAM, and the card's place comes
/// back as the core starts it.
#[test]
fn a_cabinet_with_no_card_is_a_device_of_its_ram_alone() {
    let original = neogeo_memcard::join_mister(&bios_ram(0x0250, "METAL SLUG X"), &[]).expect("fits");
    let bundle = read(&original, &CardOptions::default()).expect("reads");
    bundle.validate().expect("a valid device");
    let roles: Vec<&str> = bundle.parts.iter().map(|p| p.role.as_ref().expect("a role").as_str()).collect();
    assert_eq!(roles, ["internal"]);
    assert_eq!(write(&bundle).expect("writes"), original);
}

/// Jockey Grand Prix and V-Liner keep RAM of their own on the cartridge, which the core puts where
/// the card would be. That is no card, and it is the game's, as the RAM beside it says.
#[test]
fn a_cartridge_s_own_ram_in_the_card_s_place_is_the_cartridge_s() {
    for (name, serial, title, at, text) in [
        ("Jockey Grand Prix", "NGH-0008", "JOCKEY GRANDPRIX", 0x14, b"JOCKEY GRANDPRIX".as_slice()),
        ("V-Liner", "NGH-03E7", "DYNA VLINER", 0x00, b"DYNA V-L".as_slice()),
    ] {
        let original = fixture_in("MVS", name);
        assert_eq!(
            one_saves_convert::detect(&original, "sav").ok(),
            Some(one_saves_convert::Format::NeoGeoCard),
            "{name}"
        );

        let bundle = read(&original, &CardOptions::default()).expect("reads");
        bundle.validate().expect("a valid device");
        assert_eq!(bundle.header.shape, Shape::Device);
        let roles: Vec<&str> =
            bundle.parts.iter().map(|p| p.role.as_ref().expect("a role").as_str()).collect();
        assert_eq!(roles, ["internal", "cartridge"], "{name}");

        // Both halves are the game's: the cabinet's RAM because the BIOS recorded it there, and
        // the cartridge's because it is the cartridge.
        let game = Game::default().serial(serial).title(title);
        for index in 0..2 {
            assert_eq!(nested(&bundle, index).header.game.as_ref(), Some(&game), "{name}");
            assert_eq!(bundle.parts[index].game.as_ref(), Some(&game), "{name}");
        }
        let cart = nested(&bundle, 1);
        assert_eq!(cart.header.shape, Shape::Save);
        let bytes = cart.parts[0].bytes().expect("reads");
        assert_eq!(bytes.len(), 8192, "{name}: the whole region");
        assert_eq!(&bytes[at..at + text.len()], text, "{name}: in the 68000's order");

        assert_eq!(write(&bundle).expect("writes"), original, "{name}: the core's 73728 bytes");
    }
}

/// A Neo Geo CD leaves the cabinet RAM alone, so a file with RAM in it is a cartridge system's:
/// its 8 KiB card is read as the 2 KiB the core addresses. Told it is a CD's, it is refused.
#[test]
fn cabinet_ram_beside_a_neo_geo_cd_s_memory_is_refused() {
    let raw = with_cabinet_ram(&fixture_in("NEOGEO-CD", "Metal Slug"), &bios_ram(0x0201, "METAL SLUG"));
    assert!(matches!(
        read(&raw, &said("neogeo-cd")),
        Err(one_saves_convert::Error::NotConvertible(why)) if why.contains("Neo Geo CD has none")
    ));

    let bundle = read(&raw, &CardOptions::default()).expect("reads");
    assert_eq!(bundle.header.shape, Shape::Device);
    assert_eq!(nested(&bundle, 1).header.card.as_ref().expect("a card").capacity, 2048);
}

/// A writer never puts a card image onto a medium smaller than its card: a header promising room
/// past the end is one the BIOS writes a save over. A card of saves is rebuilt to fit instead.
#[test]
fn an_image_of_a_larger_card_is_not_put_in_a_cabinet() {
    let mut device = read(&metal_slug_x_in_a_cabinet(), &CardOptions::default()).expect("reads");
    let empty_cd = read(&neogeo_memcard::CardBuilder::new(8192).unwrap().build().unwrap(), &said("neogeo"))
        .expect("an empty card");
    assert_eq!(empty_cd.parts[0].kind, PartKind::CardImage);
    let role = device.parts[1].role.clone().expect("a role");
    device.parts[1] =
        one_saves::Part::new(1, empty_cd.to_vec().expect("encodes")).kind(PartKind::Bundle).role(role);
    assert!(matches!(
        write(&device),
        Err(one_saves_convert::Error::NotConvertible(why)) if why.contains("does not shrink")
    ));
}

#[test]
fn a_caller_can_still_name_the_socket() {
    let options =
        CardOptions { role: Some(Slug::parse("memcard-2").expect("a slug")), ..CardOptions::default() };
    let bundle = read(&fixture("Metal Slug X"), &options).expect("reads");
    assert_eq!(bundle.parts[0].role.as_ref().unwrap().as_str(), "memcard-2");
}

/// The title a game writes into its own save is carried as `x.1sav.label`.
///
/// Every other format puts what the console displays under this key, and a consumer looking for a
/// name to show should not have to know that a Neo Geo card keeps it in the save's own bytes. The
/// path is the save's identity on the card, which is its numbers rather than its title.
#[test]
fn a_saves_title_is_carried_as_a_label() {
    use one_saves::extension::Label;

    let bundle = read(&fixture("Metal Slug X"), &CardOptions::default()).expect("reads");
    let part = bundle.parts.iter().find(|p| p.kind == PartKind::Bundle).expect("a save");
    let save = Bundle::from_slice(&part.bytes().expect("nested")).expect("a save");

    let title = save.header.extensions.get::<Label>().expect("a label").title.clone();

    assert_eq!(title, "METAL SLUG X");
    assert_eq!(part.path.as_deref(), Some("0250-0"));
}

/// A game that writes no title gets no label, rather than an empty one.
///
/// Fatal Fury 2 puts a binary header where the title convention says a title goes. There is
/// nothing legible to carry, and inventing one would say something the save does not.
#[test]
fn a_save_with_no_title_carries_no_label() {
    use one_saves::extension::Label;

    // The CD's memory belongs to the console, so this file holds Metal Slug's save as well as the
    // one the filename promises. Fatal Fury 2 is NGH-0047.
    let bytes = {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../data/saves/retroarch/neocd/Garou Densetsu 2 ~ Fatal Fury 2 (Japan) (En,Ja).srm");
        std::fs::read(path).expect("reads")
    };
    let bundle = read(&bytes, &CardOptions::default()).expect("reads");
    let saves: Vec<Bundle> = bundle
        .parts
        .iter()
        .filter(|p| p.kind == PartKind::Bundle)
        .map(|p| Bundle::from_slice(&p.bytes().expect("nested")).expect("a save"))
        .collect();
    assert_eq!(saves.len(), 2, "one console's memory, two games' saves");

    assert!(saves[0].header.extensions.get::<Label>().is_some(), "Metal Slug titles its save");
    assert!(saves[1].header.extensions.get::<Label>().is_none(), "Fatal Fury 2 does not");
    // What identifies it instead, and what the interface falls back to naming it by.
    assert_eq!(saves[1].header.game.as_ref().expect("a game").serial.as_deref(), Some("NGH-0047"));
}
