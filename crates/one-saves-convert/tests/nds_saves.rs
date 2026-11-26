//! Real DS saves through every libretro core's file: the chip as melonDS, melonDS DS and NooDS keep
//! it, DeSmuME's `.dsv` with its footer, and SkyEmu's 8 MiB.
//!
//! Every file here was written by the core it is filed under, from the save corpus (CC0), by the
//! same scripted play in each core. Pokémon Mystery Dungeon: Explorers of Sky is the one game whose
//! chip DeSmuME gets wrong: its card carries a 1 Mbit EEPROM, which DeSmuME keeps as 2 Mbit flash,
//! 256 KiB with the upper half erased. Bomberman's is the smallest chip, 512 bytes, which SkyEmu,
//! left out here, pads to 8 MiB as it pads every other.
//!
//! The cores' saves are not byte for byte the same: each names the player as it does, and the game
//! wrote at its own moments. Each is the chip it holds, and each goes back to its own core as the
//! file the core wrote.

use std::path::{Path, PathBuf};

use one_saves::{Name, PartKind, Source};
use one_saves_convert::nds::{self, Container, NdsOptions};
use one_saves_convert::raw::{self, Reader};
use one_saves_registry::{apps, cores};

fn saves() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/saves/retroarch")
}

fn skip_if_absent() -> bool {
    let absent = !saves().is_dir();
    if absent {
        println!("skipping: {} is not present", saves().display());
    }
    absent
}

const SKY: &str = "Pokemon Mystery Dungeon - Explorers of Sky (USA)";
const BOMBERMAN: &str = "Bomberman (USA)";

/// The 1 Mbit EEPROM Explorers of Sky keeps its saves in, as `rom_id::nds` gives it for `C2SE`.
const SKY_CHIP: usize = 131_072;

/// Every core, its file's extension, and the core's entry in the registry.
const CORES: [(&str, &str, &one_saves_registry::Core); 6] = [
    ("desmume", "dsv", &cores::DESMUME),
    ("desmume-2015", "dsv", &cores::DESMUME_2015),
    ("melonds", "sav", &cores::MELONDS),
    ("melonds-ds", "srm", &cores::MELONDS_DS),
    ("noods", "sav", &cores::NOODS),
    ("skyemu", "srm", &cores::SKYEMU),
];

fn file(core: &str, game: &str, extension: &str) -> Vec<u8> {
    let path = saves().join(core).join(format!("{game}.{extension}"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// A save as `core`, run by RetroArch, wrote it.
fn read(bytes: &[u8], core: &str, chip: Option<usize>) -> nds::Read {
    let source = Source::default().app(Name::parse("retroarch").unwrap()).core(Name::parse(core).unwrap());
    let options =
        NdsOptions { source: Some(source), chip, file_name: Some("save".into()), ..NdsOptions::default() };
    nds::read(bytes, &options).unwrap_or_else(|e| panic!("{core}: {e}"))
}

/// The file the bundle's save goes back to `core` as.
fn written_for(bundle: &one_saves::Bundle, core: &'static one_saves_registry::Core) -> Vec<u8> {
    raw::unwrap_as(bundle, &Reader { core: Some((core, Some(&apps::RETROARCH))), ..Reader::default() })
        .unwrap_or_else(|e| panic!("{}: {e}", core.slug))
        .save
}

/// A DeSmuME footer's first word says how far the game had written, which a bundle does not keep.
/// Everything else in the file comes back.
fn same_but_the_written_mark(rebuilt: &[u8], original: &[u8]) -> bool {
    let mark = original.len() - 40;
    rebuilt.len() == original.len()
        && rebuilt[..mark] == original[..mark]
        && rebuilt[mark + 4..] == original[mark + 4..]
}

#[test]
fn explorers_of_sky_is_one_1_mbit_eeprom_in_every_core_and_each_gets_its_own_file_back() {
    if skip_if_absent() {
        return;
    }
    for (core, extension, entry) in CORES {
        let original = file(core, SKY, extension);
        let read = read(&original, core, Some(SKY_CHIP));
        let chip = &read.bundle.parts[0].payload;
        assert_eq!((read.chip, chip.len()), (Some(SKY_CHIP), SKY_CHIP), "{core}");
        assert_eq!(&chip[4..17], b"POKE_DUN_SORA", "{core}: the game's own header, after its checksum");
        read.bundle.validate().unwrap_or_else(|e| panic!("{core}: {e}"));

        let rebuilt = written_for(&read.bundle, entry);
        if extension == "dsv" {
            assert_eq!(read.container, Container::Desmume);
            assert!(same_but_the_written_mark(&rebuilt, &original), "{core}: its .dsv, 256 KiB and all");
        } else {
            assert_eq!(rebuilt, original, "{core}");
        }
    }
}

#[test]
fn without_the_game_only_a_file_kept_at_the_chip_s_length_says_the_chip() {
    if skip_if_absent() {
        return;
    }
    for (core, extension, _) in CORES {
        let original = file(core, SKY, extension);
        let read = read(&original, core, None);
        let told = matches!(core, "melonds" | "melonds-ds" | "noods");
        assert_eq!(read.chip, told.then_some(SKY_CHIP), "{core}");
        if !told {
            // DeSmuME's 256 KiB could be 2 Mbit flash, and SkyEmu's 8 MiB anything: the file goes as
            // it came, for a consumer who knows the game to read.
            let part = &read.bundle.parts[0];
            assert_eq!((&part.kind, part.path.as_deref()), (&PartKind::Aux, Some("save")), "{core}");
            assert_eq!(part.payload, original, "{core}");
            read.bundle.validate().unwrap_or_else(|e| panic!("{core}: {e}"));
        }
    }
}

#[test]
fn a_save_goes_from_any_core_to_any_other_as_that_core_keeps_it() {
    if skip_if_absent() {
        return;
    }
    let melonds = read(&file("melonds", SKY, "sav"), "melonds", Some(SKY_CHIP));
    let chip = melonds.bundle.parts[0].payload.clone();

    let desmume = written_for(&melonds.bundle, &cores::DESMUME);
    let (data, footer) = nds::dsv(&desmume).expect("a .dsv DeSmuME reads");
    assert_eq!(
        (footer.pad_size, footer.kind, footer.addr_size),
        (262_144, 5, 3),
        "as DeSmuME keeps the chip"
    );
    assert_eq!((&data[..SKY_CHIP], data[SKY_CHIP..].iter().all(|&b| b == 0xFF)), (&chip[..], true));

    let skyemu = written_for(&melonds.bundle, &cores::SKYEMU);
    assert_eq!((skyemu.len(), &skyemu[..SKY_CHIP]), (8 << 20, &chip[..]));
    assert!(skyemu[SKY_CHIP..].iter().all(|&b| b == 0), "SkyEmu's own fill");

    for core in [&cores::MELONDS_DS, &cores::NOODS] {
        assert_eq!(written_for(&melonds.bundle, core), chip, "{}", core.slug);
    }
}

#[test]
fn bomberman_s_512_bytes_are_their_own_length_in_every_core_that_keeps_them() {
    if skip_if_absent() {
        return;
    }
    for (core, extension, entry) in CORES.into_iter().filter(|(core, ..)| *core != "skyemu") {
        let original = file(core, BOMBERMAN, extension);
        // DeSmuME's 512 bytes are not the ambiguous length, so its own file says the chip.
        let read = read(&original, core, None);
        assert_eq!(read.chip, Some(512), "{core}");
        let rebuilt = written_for(&read.bundle, entry);
        if extension == "dsv" {
            assert!(same_but_the_written_mark(&rebuilt, &original), "{core}");
        } else {
            assert_eq!(rebuilt, original, "{core}");
        }
    }
}
