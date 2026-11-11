//! `1saves` moving a cartridge's clock between the files real cores wrote, end to end through the
//! binary: which files `convert` reads, which `extract` writes, what they are named and what is in
//! them.
//!
//! The saves are the ones under `data/saves/`, which a packaged copy of this crate does not carry;
//! these skip in that case rather than fail.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use one_saves_convert::rtc;

/// The workspace's real saves.
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
    let directory = std::env::temp_dir().join(format!("1saves-clocks-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    directory
}

/// Runs `1saves` in `directory` with these arguments.
fn run(directory: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_1saves"))
        .current_dir(directory)
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

/// Runs `1saves` and expects it to succeed, handing back what it printed.
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

/// Copies a real save into `directory` under `name`.
fn copy(from: &str, directory: &Path, name: &str) {
    std::fs::copy(saves().join(from), directory.join(name)).unwrap_or_else(|e| panic!("{from}: {e}"));
}

const CRYSTAL_MISTER: &str = "mister/gameboy-mister/Pokemon - Crystal Version (USA, Europe) (Rev 1).sav";

#[test]
fn a_mister_save_goes_to_each_reader_in_the_shape_it_reads() {
    if skip_if_absent() {
        return;
    }
    let here = scratch("mister");
    copy(CRYSTAL_MISTER, &here, "crystal.sav");
    ok(&here, &["convert", "crystal.sav", "--from", "gameboy-mister", "--system", "gbc"]);
    let original = read(&here, "crystal.sav");
    let clock = rtc::parse_packed(&original[32_768..]).expect("the MiSTer's clock");

    // Written back as it was written, it is the MiSTer's file under the MiSTer's name.
    std::fs::remove_file(here.join("crystal.sav")).unwrap();
    ok(&here, &["extract", "crystal.1saves"]);
    assert_eq!(read(&here, "crystal.sav"), original);

    // And asked for MiSTer by name, without saying which core: the save's system says it is the
    // Gameboy core.
    let printed = ok(&here, &["extract", "crystal.1saves", "--to", "mister", "-o", "for-mister.sav"]);
    assert!(printed.contains("save for Gameboy_MiSTer in MiSTer"), "{printed}");
    assert_eq!(read(&here, "for-mister.sav"), original);

    // RetroArch's mGBA reads the save memory from the `.srm` and the clock from the `.rtc`.
    ok(&here, &["extract", "crystal.1saves", "--to", "retroarch", "--core", "mgba"]);
    assert_eq!(read(&here, "crystal.srm"), original[..32_768]);
    let beside = rtc::parse_registers_sidecar(&read(&here, "crystal.rtc")).expect("mGBA's record");
    assert_eq!((beside.live(), beside.taken_at), (clock.live(), clock.taken_at));

    // The mGBA app the same record on the end of its `.sav`.
    ok(&here, &["extract", "crystal.1saves", "--to", "mgba", "-o", "app.sav"]);
    let app = read(&here, "app.sav");
    assert_eq!((app.len(), &app[32_768..]), (32_816, &read(&here, "crystal.rtc")[..]));

    // RetroArch runs a dozen Game Boy cores that each keep this clock their own way.
    let refused = run(&here, &["extract", "crystal.1saves", "--to", "retroarch"]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("--core"));
}

#[test]
fn a_save_with_no_clock_goes_to_retroarch_without_naming_a_core() {
    let here = scratch("no-clock");
    std::fs::write(here.join("zelda.sav"), vec![0x5Au8; 8192]).unwrap();
    ok(&here, &["convert", "zelda.sav", "--from", "gameboy-mister", "--system", "gb"]);
    // Every core RetroArch runs for a Game Boy writes this one the same.
    let printed = ok(&here, &["extract", "zelda.1saves", "--to", "retroarch"]);
    assert!(printed.contains("save for RetroArch"), "{printed}");
    assert_eq!(read(&here, "zelda.srm"), vec![0x5Au8; 8192]);
    assert!(!here.join("zelda.rtc").exists());
}

#[test]
fn retroarch_saves_round_trip_with_the_clock_beside_them() {
    if skip_if_absent() {
        return;
    }
    let name = "Pokemon - Crystal Version (USA, Europe) (Rev 1)";
    for core in ["mgba", "vba-m", "gambatte", "sameboy"] {
        let here = scratch(&format!("retroarch-{core}"));
        copy(&format!("retroarch/{core}/{name}.srm"), &here, "crystal.srm");
        copy(&format!("retroarch/{core}/{name}.rtc"), &here, "crystal.rtc");
        // Only Gambatte's is an origin, which takes the instant it is read at; the others record
        // their own, and are refused one.
        let mut arguments =
            vec!["convert", "crystal.srm", "--from", "retroarch", "--core", core, "--system", "gbc"];
        if core == "gambatte" {
            arguments.extend(["--rtc-captured-at", "1793777560"]);
        }
        let printed = ok(&here, &arguments);
        assert!(printed.contains("read the clock from crystal.rtc"), "{core}: {printed}");

        // A plain extract puts the clock back where RetroArch keeps it, beside the save.
        ok(&here, &["extract", "crystal.1saves", "-o", "back.srm"]);
        assert_eq!(read(&here, "back.srm"), read(&here, "crystal.srm"), "{core}");
        let mut expected = read(&here, "crystal.rtc");
        if core == "sameboy" {
            // The cycles into the current second, which no bundle keeps.
            expected[24..28].fill(0);
        }
        assert_eq!(read(&here, "back.rtc"), expected, "{core}");
    }
}

/// A Game Boy ROM header as `rom-id` reads one: the logo's opening bytes, and the cartridge type
/// and save size it names, here a HuC-3 with 32 KiB of RAM. No game's code is in it.
#[cfg(feature = "rom")]
fn huc3_rom() -> Vec<u8> {
    let mut rom = vec![0u8; 0x8000];
    rom[0x104..0x10C].copy_from_slice(&[0xCE, 0xED, 0x66, 0x66, 0xCC, 0x0D, 0x00, 0x0B]);
    rom[0x134..0x13F].copy_from_slice(b"ROBOPON SUN");
    rom[0x147] = 0xFE;
    rom[0x149] = 3;
    rom
}

#[cfg(feature = "rom")]
#[test]
fn a_huc3_is_read_as_one_when_the_rom_says_so_and_goes_where_each_reader_keeps_one() {
    if skip_if_absent() {
        return;
    }
    let name = "Robopon - Sun Version (USA) (SGB Enhanced) (GB Compatible)";
    let here = scratch("huc3");
    std::fs::write(here.join("robopon.gbc"), huc3_rom()).unwrap();
    copy(&format!("retroarch/gambatte/{name}.srm"), &here, "robopon.srm");
    copy(&format!("retroarch/gambatte/{name}.rtc"), &here, "robopon.rtc");
    // Gambatte's origin is 2026-11-04 07:31:33 UTC; read a day, an hour and seven minutes later.
    let at = 1_793_777_493 + 86_400 + 3_600 + 7 * 60;
    let at = at.to_string();
    ok(
        &here,
        &[
            "convert",
            "robopon.srm",
            "--from",
            "retroarch",
            "--core",
            "gambatte",
            "--rom",
            "robopon.gbc",
            "--rtc-captured-at",
            &at,
        ],
    );
    let inspected = ok(&here, &["inspect", "robopon.1saves"]);
    assert!(inspected.contains("x.1sav.rtc.huc3"), "{inspected}");
    assert!(!inspected.contains("x.1sav.rtc.mbc3"), "{inspected}");

    // For MiSTer, the counters packed into its SD block.
    ok(&here, &["extract", "robopon.1saves", "--to", "mister", "-o", "mister.sav"]);
    let mister = read(&here, "mister.sav");
    assert_eq!(mister.len(), 33_280);
    let packed = rtc::parse_packed_huc3(&mister[32_768..]).expect("MiSTer's HuC-3");
    assert_eq!((packed.days, packed.minutes, packed.seconds), (1, 67, 0));

    // For VBA-M under RetroArch, the same origin Gambatte keeps.
    ok(&here, &["extract", "robopon.1saves", "--to", "retroarch", "--core", "vba-m", "-o", "vbam.srm"]);
    assert_eq!(read(&here, "vbam.rtc"), read(&here, "robopon.rtc"));

    // SameBoy under RetroArch keeps no HuC-3, and says so rather than writing a clock it ignores.
    let printed =
        ok(&here, &["extract", "robopon.1saves", "--to", "retroarch", "--core", "sameboy", "-o", "sb.srm"]);
    assert!(printed.contains("keeps no clock for this cartridge"), "{printed}");
    assert!(!here.join("sb.rtc").exists());

    // And SameBoy's own `.rtc` beside such a save has no clock in it to read.
    copy(&format!("retroarch/sameboy/{name}.srm"), &here, "sameboy.srm");
    copy(&format!("retroarch/sameboy/{name}.rtc"), &here, "sameboy.rtc");
    let printed = ok(
        &here,
        &["convert", "sameboy.srm", "--from", "retroarch", "--core", "sameboy", "--rom", "robopon.gbc"],
    );
    assert!(printed.contains("holds no clock for this cartridge"), "{printed}");
    let inspected = ok(&here, &["inspect", "sameboy.1saves"]);
    assert!(!inspected.contains("x.1sav.rtc"), "{inspected}");
}
