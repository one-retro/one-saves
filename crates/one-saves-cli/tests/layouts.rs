//! `1saves` moving real saves between cores that keep them in different shapes, end to end
//! through the binary: Mega Drive SRAM packed for a MiSTer and spread out for Genesis Plus GX, a
//! GBA EEPROM padded for a MiSTer, and a MiSTer's clock moved off its local time.
//!
//! The saves are the ones under `data/saves/`, which a packaged copy of this crate does not carry;
//! these skip in that case rather than fail.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn saves() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/saves")
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
    let directory = std::env::temp_dir().join(format!("1saves-layouts-{name}-{}", std::process::id()));
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

fn ok(directory: &Path, arguments: &[&str]) -> String {
    let output = run(directory, arguments);
    assert!(
        output.status.success(),
        "1saves {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn read(directory: &Path, name: &str) -> Vec<u8> {
    std::fs::read(directory.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn copy(from: &str, directory: &Path, name: &str) {
    std::fs::copy(saves().join(from), directory.join(name)).unwrap_or_else(|e| panic!("{from}: {e}"));
}

const PSIV: &str = "Phantasy Star IV (USA)";
const EVANDER: &str = "Evander Holyfield's 'Real Deal' Boxing (World)";

#[test]
fn a_genesis_plus_gx_save_goes_to_a_mister_packed_and_comes_back_as_it_was() {
    if skip_if_absent() {
        return;
    }
    let here = scratch("gpgx");
    copy(&format!("retroarch/genesis-plus-gx/{PSIV}.srm"), &here, "psiv.srm");
    ok(
        &here,
        &["convert", "psiv.srm", "--from", "retroarch", "--core", "genesis-plus-gx", "--system", "genesis"],
    );

    // A MiSTer reads the SRAM's own bytes, at the 64 KiB it writes.
    let printed = ok(&here, &["extract", "psiv.1saves", "--to", "mister", "-o", "mister.sav"]);
    assert!(printed.contains("save for MegaDrive_MiSTer in MiSTer"), "{printed}");
    let mister = read(&here, "mister.sav");
    let blastem = std::fs::read(saves().join(format!("retroarch/blastem/{PSIV}.srm"))).unwrap();
    assert_eq!((mister.len(), &mister[..8192]), (65_536, &blastem[..]));

    // And Genesis Plus GX gets its own file back.
    ok(&here, &["extract", "psiv.1saves", "-o", "back.srm"]);
    assert_eq!(read(&here, "back.srm"), read(&here, "psiv.srm"));
}

#[test]
fn a_mister_save_goes_to_genesis_plus_gx_spread_out_and_says_what_it_assumed() {
    if skip_if_absent() {
        return;
    }
    let here = scratch("mister-md");
    copy(&format!("mister/megadrive-mister/{EVANDER}.sav"), &here, "evander.sav");
    ok(&here, &["convert", "evander.sav", "--from", "megadrive-mister", "--system", "genesis"]);
    let printed = ok(
        &here,
        &["extract", "evander.1saves", "--to", "retroarch", "--core", "genesis-plus-gx", "-o", "gpgx.srm"],
    );
    // 64 KiB is no EEPROM's size, so with nothing to say otherwise it is taken for SRAM, and said so.
    assert!(printed.contains("laid out as SRAM"), "{printed}");
}

#[cfg(feature = "rom")]
#[test]
fn a_mister_eeprom_goes_to_genesis_plus_gx_as_its_rom_says() {
    if skip_if_absent() {
        return;
    }
    let here = scratch("mister-eeprom");
    copy(&format!("mister/megadrive-mister/{EVANDER}.sav"), &here, "evander.sav");
    // Evander Holyfield's header, as far as rom-id reads it: the console, and backup memory
    // declared as the one word an EEPROM's two lines sit in.
    let mut rom = vec![0x20u8; 0x200];
    rom[0x100..0x110].copy_from_slice(b"SEGA GENESIS    ");
    rom[0x150..0x16C].copy_from_slice(b"EVANDER HOLYFIELD BOXING    ");
    rom[0x1B0..0x1BC].copy_from_slice(b"RA\xE8\x40\x00\x20\x00\x01\x00\x20\x00\x01");
    std::fs::write(here.join("evander.md"), rom).unwrap();
    ok(&here, &["convert", "evander.sav", "--from", "megadrive-mister", "--system", "genesis"]);
    let printed = ok(
        &here,
        &[
            "extract",
            "evander.1saves",
            "--to",
            "retroarch",
            "--core",
            "genesis-plus-gx",
            "--rom",
            "evander.md",
            "-o",
            "gpgx.srm",
        ],
    );
    assert!(!printed.contains("laid out as SRAM"), "{printed}");
    let gpgx = read(&here, "gpgx.srm");
    assert_eq!(gpgx[..128], read(&here, "evander.sav")[..128], "the EEPROM's own bytes, not spread out");
}

#[test]
fn an_mgba_eeprom_goes_to_a_mister_at_8_kib() {
    if skip_if_absent() {
        return;
    }
    let here = scratch("klonoa");
    copy("retroarch/mgba/Klonoa - Empire of Dreams (USA).srm", &here, "klonoa.srm");
    ok(&here, &["convert", "klonoa.srm", "--from", "retroarch", "--core", "mgba", "--system", "gba"]);
    ok(&here, &["extract", "klonoa.1saves", "--to", "mister", "-o", "mister.sav"]);
    let mister = read(&here, "mister.sav");
    assert_eq!((mister.len(), &mister[..512]), (8192, &read(&here, "klonoa.srm")[..]));
}

#[test]
fn a_mister_clock_is_recorded_on_its_scale_and_moved_off_it() {
    if skip_if_absent() {
        return;
    }
    let here = scratch("offset");
    copy("mister/gameboy-mister/Pokemon - Crystal Version (USA, Europe) (Rev 1).sav", &here, "crystal.sav");

    // Without an offset the clock is kept as it was, and the user is told what it costs.
    let printed = ok(&here, &["convert", "crystal.sav", "--from", "gameboy-mister", "--system", "gbc"]);
    assert!(printed.contains("--utc-offset"), "{printed}");
    let printed =
        ok(&here, &["extract", "crystal.1saves", "--to", "retroarch", "--core", "mgba", "-o", "plain.srm"]);
    assert!(printed.contains("may be off by the zone's offset"), "{printed}");

    // With one, the anchor is moved eight hours on its way to an emulator that stamps UTC.
    ok(
        &here,
        &[
            "convert",
            "crystal.sav",
            "--from",
            "gameboy-mister",
            "--system",
            "gbc",
            "--utc-offset",
            "-08:00",
            "-o",
            "pacific.1saves",
            "--force",
        ],
    );
    let printed =
        ok(&here, &["extract", "pacific.1saves", "--to", "retroarch", "--core", "mgba", "-o", "moved.srm"]);
    assert!(printed.contains("moved the clock's anchor +8 h"), "{printed}");
    let anchor = |name: &str| i64::from_le_bytes(read(&here, name)[40..48].try_into().unwrap());
    assert_eq!(anchor("moved.rtc") - anchor("plain.rtc"), 8 * 3600);

    // And back to the MiSTer it is the file it was.
    ok(&here, &["extract", "pacific.1saves", "-o", "back.sav"]);
    assert_eq!(read(&here, "back.sav"), read(&here, "crystal.sav"));

    // An emulator that stamps UTC has nothing for an offset to say.
    let refused = run(
        &here,
        &[
            "convert",
            "crystal.sav",
            "--from",
            "retroarch",
            "--core",
            "gambatte",
            "--system",
            "gbc",
            "--utc-offset",
            "-08:00",
            "-o",
            "x.1saves",
        ],
    );
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("stamps its clock in UTC"));
}
