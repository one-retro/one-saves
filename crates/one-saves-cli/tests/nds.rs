//! `1saves` on DS saves, end to end through the binary: a chip named by its game moving between
//! cores, a file that cannot say its chip kept whole with a note, the flags a DS save refuses, and
//! the in-game save lifted out of a DeSmuME state.
//!
//! The saves are the ones under `data/saves/`, which a packaged copy of this crate does not carry;
//! the tests needing them skip in that case rather than fail.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

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

/// A directory of this test's own, empty.
fn scratch(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("1saves-nds-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    directory
}

fn run(directory: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_1saves"))
        .current_dir(directory)
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

/// What a run that has to succeed printed: its standard output and its standard error.
fn ok(directory: &Path, arguments: &[&str]) -> (String, String) {
    let output = run(directory, arguments);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(output.status.success(), "1saves {arguments:?} failed: {stderr}");
    (String::from_utf8_lossy(&output.stdout).into_owned(), stderr)
}

/// What a run that has to fail said.
fn refused(directory: &Path, arguments: &[&str]) -> String {
    let output = run(directory, arguments);
    assert_eq!(output.status.code(), Some(1), "1saves {arguments:?} did not fail");
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn read(directory: &Path, name: &str) -> Vec<u8> {
    std::fs::read(directory.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn copy(from: &str, directory: &Path, name: &str) {
    std::fs::copy(saves().join(from), directory.join(name)).unwrap_or_else(|e| panic!("{from}: {e}"));
}

const SKY: &str = "Pokemon Mystery Dungeon - Explorers of Sky (USA)";

#[test]
fn a_melonds_save_named_by_its_game_goes_to_desmume_and_back_as_it_was() {
    if skip_if_absent() {
        return;
    }
    let here = scratch("melonds");
    copy(&format!("melonds/{SKY}.sav"), &here, "sky.sav");
    let (printed, _) =
        ok(&here, &["convert", "sky.sav", "--from", "retroarch", "--core", "melonds", "--nds-game", "C2SE"]);
    assert!(printed.contains("131072 bytes of save chip"), "{printed}");

    // DeSmuME keeps the 1 Mbit EEPROM at 256 KiB, behind its footer.
    let (printed, _) =
        ok(&here, &["extract", "sky.1saves", "--to", "retroarch", "--core", "desmume", "-o", "sky.dsv"]);
    assert!(printed.contains("save for DeSmuME in RetroArch"), "{printed}");
    let dsv = read(&here, "sky.dsv");
    assert_eq!((dsv.len(), &dsv[..131_072]), (262_144 + 122, &read(&here, "sky.sav")[..]));

    // And its .dsv, named by its game, comes back to melonDS as the file melonDS wrote.
    ok(&here, &["convert", "sky.dsv", "--nds-game", "NTR-C2SE-USA", "-o", "again.1saves"]);
    ok(&here, &["extract", "again.1saves", "--to", "retroarch", "--core", "melonds", "-o", "back.sav"]);
    assert_eq!(read(&here, "back.sav"), read(&here, "sky.sav"));
}

#[test]
fn a_desmume_file_is_known_by_its_extension_and_footer_and_says_its_own_small_chip() {
    if skip_if_absent() {
        return;
    }
    let here = scratch("dsv");
    copy("desmume/Bomberman (USA).dsv", &here, "bomb.dsv");
    let (printed, _) = ok(&here, &["convert", "bomb.dsv"]);
    assert!(printed.contains("DeSmuME DS save, 512 bytes of save chip"), "{printed}");
    // Under a name that says nothing, the footer still does.
    copy("desmume/Bomberman (USA).dsv", &here, "bomb.bin");
    let (printed, _) = ok(&here, &["convert", "bomb.bin", "-o", "bin.1saves"]);
    assert!(printed.contains("512 bytes of save chip"), "{printed}");
}

#[test]
fn skyemu_s_8_mib_goes_whole_with_a_note_until_the_game_says_its_chip() {
    if skip_if_absent() {
        return;
    }
    let here = scratch("skyemu");
    copy(&format!("skyemu/{SKY}.srm"), &here, "sky.srm");
    let (printed, note) =
        ok(&here, &["convert", "sky.srm", "--from", "retroarch", "--core", "skyemu", "--system", "nds"]);
    assert!(printed.contains("chip not known"), "{printed}");
    assert!(note.contains("kept the file as it came") && note.contains("8 MiB"), "{note}");

    let (printed, note) = ok(
        &here,
        &[
            "convert",
            "sky.srm",
            "--from",
            "retroarch",
            "--core",
            "skyemu",
            "--nds-game",
            "C2SE",
            "-o",
            "known.1saves",
        ],
    );
    assert!(printed.contains("131072 bytes of save chip") && note.is_empty(), "{printed}{note}");
    ok(&here, &["extract", "known.1saves", "--to", "retroarch", "--core", "melonds", "-o", "chip.sav"]);
    assert_eq!(read(&here, "chip.sav"), read(&here, "sky.srm")[..131_072]);
    ok(&here, &["extract", "known.1saves", "-o", "back.srm"]);
    assert_eq!(read(&here, "back.srm"), read(&here, "sky.srm"), "SkyEmu's own file back");
}

#[test]
fn a_ds_save_refuses_what_a_card_has_not() {
    let here = scratch("refusals");
    std::fs::write(here.join("save.sav"), vec![0x5A; 8192]).unwrap();
    let game = ["convert", "save.sav", "--nds-game", "ASMP"];

    let said = refused(&here, &["convert", "save.sav", "--nds-game", "ZZZZ"]);
    assert!(said.contains("no DS game is known by the code \"ZZZZ\""), "{said}");
    let said = refused(&here, &[&game[..], &["--role", "card"]].concat());
    assert!(said.contains("--role has nothing to name"), "{said}");
    std::fs::write(here.join("clock.rtc"), [0; 16]).unwrap();
    let said = refused(&here, &[&game[..], &["--rtc-file", "clock.rtc"]].concat());
    assert!(said.contains("keeps no clock"), "{said}");
    let said = refused(&here, &[&game[..], &["--in-game-save"]].concat());
    assert!(said.contains("this file is not one"), "{said}");
    // The game says 8 KiB, and what is past them is not padding: the file goes whole.
    std::fs::write(here.join("long.sav"), [vec![0x5A; 8192], vec![0x42; 808]].concat()).unwrap();
    let (_, note) = ok(&here, &["convert", "long.sav", "--nds-game", "ASMP"]);
    assert!(note.contains("past the end of the card's 8 KiB chip"), "{note}");
}

#[test]
fn a_clock_file_beside_a_ds_save_is_passed_over_and_said_to_be() {
    let here = scratch("rtc");
    std::fs::write(here.join("save.sav"), vec![0x5A; 8192]).unwrap();
    std::fs::write(here.join("save.rtc"), [0; 16]).unwrap();
    let (printed, _) = ok(&here, &["convert", "save.sav", "--nds-game", "ASMP"]);
    assert!(printed.contains("the .rtc beside it is the console's clock"), "{printed}");
    assert!(printed.contains("8192 bytes of save chip"), "{printed}");
}

/// A DeSmuME state, stored rather than compressed, whose memory controller holds `save` and whose
/// ROM header names the game `code`.
fn desmume_state(save: &[u8], code: [u8; 4]) -> Vec<u8> {
    let word = |n: usize| u32::try_from(n).unwrap().to_le_bytes();
    let mut mmu = 8u32.to_le_bytes().to_vec();
    mmu.extend_from_slice(&[0; 24]);
    mmu.extend_from_slice(&word(save.len()));
    mmu.extend_from_slice(save);
    let mut header = vec![0u8; 0x160];
    header[..9].copy_from_slice(b"BOMBERMAN");
    header[0x0C..0x10].copy_from_slice(&code);
    let mut rom = b"GINF".to_vec();
    rom.extend_from_slice(&word(1));
    rom.extend_from_slice(&word(header.len()));
    rom.extend_from_slice(&header);
    let mut data = Vec::new();
    for (kind, chunk) in [(61, &mmu), (130, &rom)] {
        data.extend_from_slice(&word(kind));
        data.extend_from_slice(&word(chunk.len()));
        data.extend_from_slice(chunk);
    }
    data.extend_from_slice(&u32::MAX.to_le_bytes());
    let mut file = b"DeSmuME SState\0\0".to_vec();
    for value in [word(12), word(0x164A4), word(data.len()), u32::MAX.to_le_bytes()] {
        file.extend_from_slice(&value);
    }
    file.extend(data);
    file
}

#[test]
fn a_desmume_state_is_a_state_and_its_in_game_save_lifts_out_by_its_game() {
    let here = scratch("state");
    let save: Vec<u8> = (0..512u32).map(|i| u8::try_from(i % 251).unwrap()).collect();
    std::fs::write(here.join("bomb.ds1"), desmume_state(&save, *b"ASMP")).unwrap();

    let (printed, _) = ok(&here, &["convert", "bomb.ds1", "-o", "state.1saves"]);
    assert!(printed.contains("state.1saves"), "{printed}");
    let (printed, _) = ok(&here, &["inspect", "state.1saves"]);
    assert!(printed.contains("state") && printed.contains("ASMP"), "{printed}");

    let (printed, _) = ok(&here, &["convert", "bomb.ds1", "--in-game-save", "-o", "save.1saves"]);
    assert!(printed.contains("from a DeSmuME state, 512 bytes of save chip"), "{printed}");
    ok(&here, &["extract", "save.1saves", "--to", "retroarch", "--core", "melonds", "-o", "bomb.sav"]);
    assert_eq!(read(&here, "bomb.sav"), save);
}
