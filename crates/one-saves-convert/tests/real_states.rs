//! Runs the state readers against files real emulators wrote.
//!
//! None of those files is in this repository, and none may be: a state is the emulated machine's
//! memory, which holds the game's own code beside the player's progress. So these tests read them
//! where they lie. `ONE_SAVES_STATES` names the folders to look in, separated as `PATH` is, and a
//! state is found in one of them or in a folder inside one. With the variable unset, every test
//! here passes without reading anything.
//!
//! Every state of an emulator found there is read and rebuilt. What a few should read as is
//! recorded here, from what the emulator was told to do when it took them; those are checked
//! where they are found, and skipped where they are not.

#![cfg(feature = "states")]

use std::path::{Path, PathBuf};

use one_saves::extension::{CheatsMgba, Rtc, RtcMbc3};
use one_saves::{Bundle, HashAlgorithm, HashValue, PartKind};
use one_saves_convert::state::{self, Emulator, StateFile, StateOptions};

const VARIABLE: &str = "ONE_SAVES_STATES";

/// Every file in the folders the variable names, and in the folders directly inside them.
fn files() -> Vec<PathBuf> {
    let Some(roots) = std::env::var_os(VARIABLE) else {
        eprintln!("{VARIABLE} is unset, so no real state is read");
        return Vec::new();
    };
    let mut found = Vec::new();
    for root in std::env::split_paths(&roots) {
        let Ok(entries) = std::fs::read_dir(&root) else {
            panic!("{VARIABLE} names {}, which is not a folder", root.display());
        };
        for path in entries.flatten().map(|entry| entry.path()) {
            if path.is_dir() {
                let inner = std::fs::read_dir(&path).into_iter().flatten().flatten();
                found.extend(inner.map(|entry| entry.path()).filter(|path| path.is_file()));
            } else {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

fn file_name(path: &Path) -> String {
    path.file_name().expect("a file").to_string_lossy().into_owned()
}

/// The states among `files` that `emulator` wrote, with their names and bytes.
///
/// Only a file `named` says could be one is read, since the folders a player keeps states in are
/// the ones they keep ROMs and disc images in.
fn states_of(emulator: Emulator, named: impl Fn(&str) -> bool) -> Vec<(String, Vec<u8>)> {
    files()
        .into_iter()
        .map(|path| (file_name(&path), path))
        .filter(|(name, _)| named(name))
        .filter_map(|(name, path)| {
            let bytes = std::fs::read(&path).ok()?;
            (state::detect(&bytes, &name) == Some(emulator)).then_some((name, bytes))
        })
        .collect()
}

fn crc32(digest: [u8; 4]) -> HashValue {
    HashValue::new(HashAlgorithm::Crc32, digest.to_vec()).unwrap()
}

/// What an mGBA state should read as.
struct Mgba {
    file: &'static str,
    /// The screenshot's width and height, where mGBA kept one, which makes the file a PNG.
    screen: Option<(u32, u32)>,
    system: &'static str,
    title: &'static str,
    serial: Option<&'static str>,
    /// The ROM's CRC-32, as a DAT writes it.
    crc32: [u8; 4],
    slot: u64,
    /// The in-game save's length, as mGBA keeps it in the state.
    save: usize,
    /// The name of the one cheat set the player had.
    cheat: Option<&'static str>,
    /// An MBC3's registers from the footer on the save, live and latched alike, and the anchor
    /// mGBA wrote beside them.
    clock: Option<([u32; 5], i64)>,
}

/// Taken with mGBA 0.10.5 for macOS: slot 1 as mGBA writes it, and slot 2 with its screenshot
/// turned off. Each game had one cheat set on.
const MGBA: &[Mgba] = &[
    Mgba {
        file: "Pokemon - Emerald Version (USA, Europe).ss1",
        screen: Some((240, 160)),
        system: "gba",
        title: "POKEMON EMER",
        serial: Some("BPEE"),
        crc32: [0x1F, 0x1C, 0x08, 0xFB],
        slot: 1,
        // The flash chip alone: mGBA keeps a GBA cartridge's clock out of the save it puts here.
        save: 0x20000,
        cheat: Some("Unlimited money"),
        clock: None,
    },
    Mgba {
        file: "Pokemon - Emerald Version (USA, Europe).ss2",
        screen: None,
        system: "gba",
        title: "POKEMON EMER",
        serial: Some("BPEE"),
        crc32: [0x1F, 0x1C, 0x08, 0xFB],
        slot: 2,
        save: 0x20000,
        cheat: Some("Unlimited money"),
        clock: None,
    },
    // A cartridge that runs on both, taken on a Game Boy Color.
    Mgba {
        file: "Pokemon - Gold Version (USA, Europe) (SGB Enhanced) (GB Compatible).ss1",
        screen: Some((160, 144)),
        system: "gbc",
        title: "POKEMON_GLDAAUE",
        serial: None,
        crc32: [0x6B, 0xDE, 0x3C, 0x3E],
        slot: 1,
        // The SRAM alone. mGBA hands it over with the clock footer its save file has, which goes
        // into the header.
        save: 0x8000,
        cheat: None,
        // Three seconds and two minutes, as mGBA last wrote the save, fifty seconds before the
        // state: 2026-10-30 16:57:31 UTC.
        clock: Some(([3, 2, 0, 0, 0], 1_793_379_451)),
    },
    Mgba {
        file: "Pokemon - Gold Version (USA, Europe) (SGB Enhanced) (GB Compatible).ss2",
        screen: None,
        system: "gbc",
        title: "POKEMON_GLDAAUE",
        serial: None,
        crc32: [0x6B, 0xDE, 0x3C, 0x3E],
        slot: 2,
        save: 0x8000,
        cheat: None,
        clock: Some(([3, 2, 0, 0, 0], 1_793_379_451)),
    },
];

fn check_mgba(expected: &Mgba, bundle: &Bundle) {
    let file = expected.file;
    assert_eq!(bundle.header.system.as_ref().map(one_saves::Slug::as_str), Some(expected.system), "{file}");
    let game = bundle.header.game.as_ref().expect("the data names the game");
    assert_eq!(game.title.as_deref(), Some(expected.title), "{file}");
    assert_eq!(game.serial.as_deref(), expected.serial, "{file}");
    assert_eq!(game.rom_hashes, [crc32(expected.crc32)], "{file}");

    let source = bundle.header.source.as_ref().expect("a state names what it loads into");
    assert_eq!(source.core.as_ref().map(one_saves::Name::as_str), Some("mgba"), "{file}");
    assert!(source.app_version.is_some(), "{file}: mGBA says which version took it");
    assert!(bundle.header.taken_at.is_some(), "{file}: mGBA says when it took it");

    let state = &bundle.parts[0];
    assert_eq!(state.kind, PartKind::State, "{file}");
    assert_eq!(state.slot, Some(expected.slot), "{file}");
    let sets = &state.extensions.get::<CheatsMgba>().expect("the player's cheats").sets;
    assert_eq!(sets.len(), 1, "{file}");
    assert_eq!(sets[0].name.as_deref(), expected.cheat, "{file}");

    let screenshot = bundle.parts.iter().find(|part| part.kind == PartKind::Screenshot);
    match (expected.screen, screenshot) {
        (Some((width, height)), Some(part)) => {
            let png = part.bytes().unwrap();
            let size = |at: usize| u32::from_be_bytes(png[at..at + 4].try_into().unwrap());
            assert_eq!((size(16), size(20)), (width, height), "{file}: the screen it showed");
        }
        (None, None) => {}
        (screen, part) => panic!("{file}: expected a screenshot {screen:?}, found {:?}", part.map(|p| p.id)),
    }
    let saves: Vec<_> = bundle.parts.iter().filter(|part| part.kind == PartKind::Save).collect();
    assert_eq!(saves.len(), 1, "{file}");
    assert_eq!(saves[0].bytes().unwrap().len(), expected.save, "{file}");

    let mbc3 = bundle.header.extensions.get::<RtcMbc3>();
    match (expected.clock, mbc3) {
        (Some((registers, anchor)), Some(clock)) => {
            assert_eq!((clock.live, clock.latched), (registers, Some(registers)), "{file}");
            assert_eq!(clock.taken_at, anchor, "{file}: the anchor is when mGBA wrote the footer");
            assert!(anchor < bundle.header.taken_at.unwrap(), "{file}: which was before the state");
            assert!(bundle.header.extensions.get::<Rtc>().is_some(), "{file}: and the reading beside it");
        }
        (None, None) => assert!(bundle.header.extensions.is_empty(), "{file}: no clock key"),
        (clock, found) => panic!("{file}: expected a clock {clock:?}, found {found:?}"),
    }
}

#[test]
fn mgba_states_read_as_what_mgba_was_told_to_take_and_rebuild() {
    let mut checked = 0;
    let slot = |name: &str| {
        name.rsplit_once(".ss")
            .is_some_and(|(_, slot)| slot.len() == 1 && slot.as_bytes()[0].is_ascii_digit())
    };
    for (name, bytes) in states_of(Emulator::Mgba, slot) {
        let file = StateFile { bytes: &bytes, name: &name, screenshot: None };
        let bundle = state::read(Emulator::Mgba, &file, &StateOptions::default())
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        if let Some(expected) = MGBA.iter().find(|expected| expected.file == name) {
            check_mgba(expected, &bundle);
            checked += 1;
        }

        let stem = name.rsplit_once(".ss").expect("an mGBA slot").0;
        let rebuilt = state::write(Emulator::Mgba, &bundle, stem).unwrap();
        assert_eq!(rebuilt.name, name, "the slot is in the name");
        assert!(rebuilt.left_out.is_empty(), "{name}: mGBA has a place for everything it wrote");
        let file = StateFile { bytes: &rebuilt.bytes, name: &rebuilt.name, screenshot: None };
        let again = state::read(Emulator::Mgba, &file, &StateOptions::default()).unwrap();
        assert_eq!(again.to_vec().unwrap(), bundle.to_vec().unwrap(), "{name}: the rebuild reads the same");

        // Without a picture the file is mGBA's own, apart from the time it was taken, which mGBA
        // keeps in microseconds and the format in whole seconds.
        if !bundle.parts.iter().any(|part| part.kind == PartKind::Screenshot) {
            assert_eq!(rebuilt.bytes.len(), bytes.len(), "{name}");
            let differ: Vec<usize> = (0..bytes.len()).filter(|&at| rebuilt.bytes[at] != bytes[at]).collect();
            if let (Some(first), Some(last)) = (differ.first(), differ.last()) {
                assert!(
                    last - first < 8,
                    "{name}: bytes {first}..={last} differ, beyond the time's fraction"
                );
            }
        }
    }
    eprintln!("checked {checked} of {} recorded mGBA states", MGBA.len());
}
