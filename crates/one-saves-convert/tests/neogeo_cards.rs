//! What the Neo Geo adapter makes of real cards, checked against dumps under the workspace's
//! `data/saves`.
//!
//! These live out here rather than beside the code because they need files the crate does not
//! carry: a published tarball holds the crate and nothing else, so a test that reaches for a
//! 72 KiB MiSTer save could not run from one.

// The adapter is behind its feature, so without it there is nothing here to test.
#![cfg(feature = "neogeo")]

use neogeo_memcard::MemoryCard;
use one_saves::{Bundle, PartKind, Slug};
use one_saves_convert::card::CardOptions;
use one_saves_convert::card::neogeo::{read, write};

/// The real MiSTer cards under `data/`: `NEOGEO` for a cartridge system, `NEOGEO-CD` for a CD.
fn fixture_in(set: &str, name: &str) -> Vec<u8> {
    // The core keeps both sets in one folder, so the set and the game together pick out the file.
    let file = match (set, name) {
        ("NEOGEO", "Fatal Fury 2") => "Fatal Fury 2 (fatfury2).sav",
        ("NEOGEO", "Metal Slug X") => "Metal Slug X (mslugx).sav",
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
    assert_eq!(card.system_area.as_ref().map(Vec::len), Some(5 * 64));

    // Metal Slug X keeps stages and options apart, so one cartridge is two saves. The cabinet
    // RAM in both fixtures is erased, so nothing rides along for that.
    assert_eq!(bundle.parts.len(), 2);
    assert!(bundle.parts.iter().all(|p| p.kind == PartKind::Bundle));

    // A card has no filenames; the title the game wrote is what names a save.
    assert_eq!(bundle.parts[0].path.as_deref(), Some("METAL SLUG X"));
    assert_eq!(bundle.parts[1].path.as_deref(), Some("METAL SLUG X OPTION"));
    // Same game, so the same serial; `slot` is what tells the two apart.
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
}

#[test]
fn a_neo_geo_cd_card_is_the_same_format_at_a_bigger_size() {
    let bundle = read(&fixture_in("NEOGEO-CD", "Metal Slug"), &CardOptions::default()).expect("reads");
    let card = bundle.header.card.as_ref().expect("is a card");
    assert_eq!(card.format.as_str(), "neogeo-mc");
    // The core emulates the CD's 8 KiB card only in CD mode, so in a MiSTer save its size says
    // which system ran, and the card is the console's own memory rather than one in a slot.
    assert_eq!(system_of(&bundle), "neogeo-cd");
    // Thirteen blocks of structure rather than five, so a bigger system area and more room.
    assert_eq!(card.system_area.as_ref().map(Vec::len), Some(13 * 64));

    assert_eq!(bundle.parts.len(), 1);
    assert_eq!(bundle.parts[0].path.as_deref(), Some("METAL SLUG"));
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

    let said =
        |system: &str| CardOptions { system: Some(Slug::parse(system).unwrap()), ..CardOptions::default() };
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

#[test]
fn a_cabinet_s_backup_ram_is_carried_rather_than_dropped() {
    // Both fixtures have an erased one, so this writes into a copy to prove it survives.
    let mut raw = fixture("Metal Slug X");
    raw[0x100] = 0x42;
    let bundle = read(&raw, &CardOptions::default()).expect("reads");

    let ram = bundle.parts.iter().find(|p| p.kind == PartKind::Aux).expect("the cabinet RAM rides along");
    assert_eq!(ram.role.as_ref().unwrap().as_str(), "internal");
    assert_eq!(ram.payload.len(), neogeo_memcard::MISTER_BACKUP_RAM as u64);
    // It is a part of its own rather than something `write` puts back: extraction gives the
    // bare card, and the cabinet's RAM comes out with `--part`.
    assert_eq!(ram.bytes().expect("reads")[0x100], 0x42);
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
/// name to show should not have to know that a Neo Geo card keeps it somewhere else. It is the
/// same string as the part's `path`, which is deliberate: here one string is both the save's
/// identity on the card and its name.
#[test]
fn a_saves_title_is_carried_as_a_label() {
    use one_saves::extension::Label;

    let bundle = read(&fixture("Metal Slug X"), &CardOptions::default()).expect("reads");
    let part = bundle.parts.iter().find(|p| p.kind == PartKind::Bundle).expect("a save");
    let save = Bundle::from_slice(&part.bytes().expect("nested")).expect("a save");

    let title = save.header.extensions.get::<Label>().expect("a label").title.clone();

    assert_eq!(title, "METAL SLUG X");
    assert_eq!(part.path.as_deref(), Some("METAL SLUG X"));
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
