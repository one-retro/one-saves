//! Real saves through the layouts each core keeps them in: Mega Drive SRAM spread over a 16-bit
//! bus or packed, GBA chips padded or moved, and a MiSTer's clock on its local scale.
//!
//! Every file here was written by the core it is filed under, from the save corpus (CC0), most of
//! them by the same scripted play in each core, so where the game wrote the same bytes the files
//! agree once their layouts are undone. A MiSTer was asked to read several of them back: it read
//! BlastEm's World Series Baseball '95 and Genesis Plus GX's packed, and not Genesis Plus GX's own;
//! it read mGBA's Klonoa padded to 8 KiB and not at 512 bytes.

use std::path::{Path, PathBuf};

use one_saves::extension::{RtcHuc3, RtcMbc3};
use one_saves::{Bundle, Name, Source};
use one_saves_convert::layout::SaveMemory;
use one_saves_convert::raw::{self, RawOptions, Reader};
use one_saves_convert::rtc;
use one_saves_registry::{apps, cores};

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

fn read(path: &str) -> Vec<u8> {
    std::fs::read(saves().join(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn source(app: &str, core: &str) -> Source {
    Source::default().app(Name::parse(app).unwrap()).core(Name::parse(core).unwrap())
}

/// A save as `core`, run by `app`, wrote it.
fn wrap(file: &[u8], app: &str, core: &str, system: &str, memory: Option<SaveMemory>) -> Bundle {
    let options = RawOptions {
        system: Some(system.into()),
        source: Some(source(app, core)),
        memory,
        ..RawOptions::default()
    };
    raw::wrap(file, &options).expect("wraps")
}

fn payload(bundle: &Bundle) -> Vec<u8> {
    raw::unwrap_save_only(bundle).expect("one save part")
}

/// The save written for `core`, run by `app`.
fn written_for(
    bundle: &Bundle,
    core: &'static one_saves_registry::Core,
    app: &'static one_saves_registry::App,
    memory: Option<SaveMemory>,
) -> raw::Unwrapped {
    raw::unwrap_as(bundle, &Reader { core: Some((core, Some(app))), memory, ..Reader::default() })
        .expect("unwraps")
}

const PSIV: &str = "Phantasy Star IV (USA)";
const WSB: &str = "World Series Baseball '95 (USA)";
const EVANDER: &str = "Evander Holyfield's 'Real Deal' Boxing (World)";
const KLONOA: &str = "Klonoa - Empire of Dreams (USA)";
const ROBOPON: &str = "Robopon - Sun Version (USA) (SGB Enhanced) (GB Compatible)";

/// The Mega Drive cores that ran Phantasy Star IV, and their files' lengths.
const PSIV_CORES: [(&str, usize); 4] =
    [("genesis-plus-gx", 16_384), ("picodrive", 16_384), ("clownmdemu", 65_536), ("blastem", 8192)];

#[test]
fn phantasy_star_iv_is_one_sram_in_every_core_and_each_gets_its_own_file_back() {
    if skip_if_absent() {
        return;
    }
    let blastem = read(&format!("retroarch/blastem/{PSIV}.srm"));
    for (core, len) in PSIV_CORES {
        let file = read(&format!("retroarch/{core}/{PSIV}.srm"));
        assert_eq!(file.len(), len, "{core}");
        let bundle = wrap(&file, "retroarch", core, "genesis", None);
        // The bundle holds the SRAM's own bytes, which are the same whoever wrote them.
        let memory = payload(&bundle);
        assert_eq!(&memory[..8192], &blastem[..], "{core}: the SRAM, packed");
        assert!(memory.len() <= 32_768, "{core}: half the window at most");
        // And the file goes back as the core wrote it.
        assert_eq!(raw::unwrap(&bundle).unwrap(), file, "{core}: byte for byte");
    }
}

#[test]
fn a_genesis_plus_gx_save_goes_to_mister_and_blastem_packed() {
    if skip_if_absent() {
        return;
    }
    let gpgx = wrap(
        &read(&format!("retroarch/genesis-plus-gx/{PSIV}.srm")),
        "retroarch",
        "genesis-plus-gx",
        "genesis",
        None,
    );
    let blastem = read(&format!("retroarch/blastem/{PSIV}.srm"));

    let mister = written_for(&gpgx, &cores::MEGADRIVE_MISTER, &apps::MISTER, None);
    assert_eq!(mister.save.len(), 65_536, "the size a MiSTer writes every save at");
    assert_eq!(&mister.save[..8192], &blastem[..]);
    assert!(!mister.memory_assumed, "packing needs nothing said about the cartridge");

    let to_blastem = written_for(&gpgx, &cores::BLASTEM, &apps::RETROARCH, None);
    assert_eq!(&to_blastem.save[..8192], &blastem[..]);
}

#[test]
fn a_mister_save_goes_to_genesis_plus_gx_spread_over_the_odd_lane() {
    if skip_if_absent() {
        return;
    }
    let file = read(&format!("mister/megadrive-mister/{WSB}.sav"));
    let bundle = wrap(&file, "mister", "megadrive-mister", "genesis", None);
    assert_eq!(raw::unwrap(&bundle).unwrap(), file, "a MiSTer's own file back byte for byte");

    let gpgx = written_for(&bundle, &cores::GENESIS_PLUS_GX, &apps::RETROARCH, None);
    assert!(gpgx.memory_assumed, "nothing said this cartridge keeps SRAM");
    assert!(gpgx.save.len() <= 65_536);
    assert!(gpgx.save.iter().step_by(2).all(|&b| b == 0xFF), "the even lane is Genesis Plus GX's fill");
    let odd: Vec<u8> = gpgx.save.iter().skip(1).step_by(2).copied().collect();
    assert_eq!(odd, file[..odd.len()], "and the odd lane the MiSTer's bytes");

    // PicoDrive's fill is zero, and its file twice the SRAM it was given.
    let pico = written_for(&bundle, &cores::PICODRIVE, &apps::RETROARCH, None);
    assert_eq!(pico.save.len(), 65_536);
    assert!(pico.save.iter().step_by(2).all(|&b| b == 0));

    // Its header says the SRAM is 32 KiB on the odd lane, so the MiSTer's padding comes off.
    let header = Some(SaveMemory::ByteLane { odd: true, size: Some(32_768) });
    let sized = wrap(&file, "mister", "megadrive-mister", "genesis", header);
    assert_eq!(payload(&sized), file[..32_768]);
    assert!(!written_for(&sized, &cores::GENESIS_PLUS_GX, &apps::RETROARCH, header).memory_assumed);
}

#[test]
fn an_eeprom_is_kept_as_its_own_bytes_by_every_core() {
    if skip_if_absent() {
        return;
    }
    let gpgx = read(&format!("retroarch/genesis-plus-gx/{EVANDER}.srm"));
    assert_eq!(gpgx, read(&format!("retroarch/blastem/{EVANDER}.srm")), "Genesis Plus GX's is BlastEm's");
    let pico = read(&format!("retroarch/picodrive/{EVANDER}.srm"));
    let mister = read(&format!("mister/megadrive-mister/{EVANDER}.sav"));
    for (app, core, file) in [
        ("retroarch", "genesis-plus-gx", &gpgx),
        ("retroarch", "blastem", &gpgx),
        ("retroarch", "picodrive", &pico),
        ("mister", "megadrive-mister", &mister),
    ] {
        let bundle = wrap(file, app, core, "genesis", None);
        assert_eq!(raw::unwrap(&bundle).unwrap(), *file, "{core}: byte for byte");
    }

    // 128 bytes is an EEPROM's size, so it goes into PicoDrive's 8 KiB file and a MiSTer's 64 KiB.
    let bundle = wrap(&gpgx, "retroarch", "genesis-plus-gx", "genesis", None);
    let to_pico = written_for(&bundle, &cores::PICODRIVE, &apps::RETROARCH, None);
    assert_eq!((to_pico.save.len(), &to_pico.save[..128]), (8192, &gpgx[..]));
    let to_mister = written_for(&bundle, &cores::MEGADRIVE_MISTER, &apps::MISTER, None);
    assert_eq!((to_mister.save.len(), &to_mister.save[..128]), (65_536, &gpgx[..]));

    // A MiSTer's 64 KiB file is no EEPROM's size, so where it goes to Genesis Plus GX the ROM has to
    // say; without it, it would be spread out as SRAM.
    let from_mister = wrap(&mister, "mister", "megadrive-mister", "genesis", None);
    let eeprom = Some(SaveMemory::Eeprom { size: None });
    let told = written_for(&from_mister, &cores::GENESIS_PLUS_GX, &apps::RETROARCH, eeprom);
    assert_eq!(&told.save[..128], &mister[..128]);
    assert!(!told.memory_assumed);
    assert!(written_for(&from_mister, &cores::GENESIS_PLUS_GX, &apps::RETROARCH, None).memory_assumed);
}

#[test]
fn a_bundle_from_before_layouts_still_goes_where_it_should() {
    if skip_if_absent() {
        return;
    }
    // One-saves 0.7 kept Genesis Plus GX's file as it was. That bundle still goes back to Genesis
    // Plus GX as it came, and to a MiSTer packed.
    let file = read(&format!("retroarch/genesis-plus-gx/{PSIV}.srm"));
    let mut old = wrap(&file, "retroarch", "genesis-plus-gx", "genesis", None);
    old.parts[0] = one_saves::Part::new(0, file.clone());
    assert_eq!(raw::unwrap(&old).unwrap(), file);
    let mister = written_for(&old, &cores::MEGADRIVE_MISTER, &apps::MISTER, None);
    assert_eq!(&mister.save[..8192], &read(&format!("retroarch/blastem/{PSIV}.srm"))[..]);
}

/// The GBA cores that ran Klonoa, their files' lengths, and whether the file shows where the chip is.
const KLONOA_CORES: [(&str, usize, bool); 4] =
    [("mgba", 512, true), ("gpsp", 131_072, false), ("skyemu", 131_072, false), ("vba-next", 0x2_2000, true)];

#[test]
fn klonoa_s_eeprom_is_one_chip_in_every_core_and_each_gets_its_own_file_back() {
    if skip_if_absent() {
        return;
    }
    let chip = read(&format!("retroarch/mgba/{KLONOA}.srm"));
    let eeprom = Some(SaveMemory::Eeprom { size: None });
    for (core, len, found) in KLONOA_CORES {
        let file = read(&format!("retroarch/{core}/{KLONOA}.srm"));
        assert_eq!(file.len(), len, "{core}");
        let bundle = wrap(&file, "retroarch", core, "gba", None);
        assert_eq!(raw::unwrap(&bundle).unwrap(), file, "{core}: byte for byte");
        // The chip, where the file shows where it is or the ROM says it is an EEPROM.
        if found {
            assert_eq!(payload(&bundle), chip, "{core}");
        }
        let told = wrap(&file, "retroarch", core, "gba", eeprom);
        assert_eq!(payload(&told), chip, "{core}, told it is an EEPROM");
        assert_eq!(raw::unwrap(&told).unwrap(), file, "{core}: and back again");
    }
}

#[test]
fn a_4_kbit_eeprom_goes_to_mister_at_the_8_kib_it_reads() {
    if skip_if_absent() {
        return;
    }
    let chip = read(&format!("retroarch/mgba/{KLONOA}.srm"));
    let bundle = wrap(&chip, "retroarch", "mgba", "gba", None);
    let mister = written_for(&bundle, &cores::GBA_MISTER, &apps::MISTER, None);
    assert_eq!((mister.save.len(), &mister.save[..512]), (8192, &chip[..]));
    assert!(mister.save[512..].iter().all(|&b| b == 0xFF));

    // SkyEmu keeps each eight-byte block back to front, and VBA Next 128 KiB in.
    let skyemu = written_for(&bundle, &cores::SKYEMU, &apps::RETROARCH, None).save;
    assert_eq!(skyemu, read(&format!("retroarch/skyemu/{KLONOA}.srm")));
    let vba_next = written_for(&bundle, &cores::VBA_NEXT, &apps::RETROARCH, None).save;
    assert_eq!(vba_next, read(&format!("retroarch/vba-next/{KLONOA}.srm")));

    // A MiSTer's own file is read back as it is, and goes back to it unchanged.
    let own = read(&format!("mister/gba-mister/{KLONOA}.sav"));
    let from_mister = wrap(&own, "mister", "gba-mister", "gba", None);
    assert_eq!(raw::unwrap(&from_mister).unwrap(), own);
}

/// A MiSTer in Pacific time, eight hours west, its standard offset all year.
const PACIFIC: i32 = -8 * 3600;

/// The Robopon save a MiSTer wrote at 2026-11-10 20:57:31 PST: day 0, 00:04 and 5 seconds.
fn robopon(offset: Option<i32>) -> (Vec<u8>, Bundle) {
    let file = read(&format!("mister/gameboy-mister/{ROBOPON}.sav"));
    let options = RawOptions {
        system: Some("gbc".into()),
        source: Some(source("mister", "gameboy-mister").utc_offset(offset)),
        clock_chip: Some("x.1sav.rtc.huc3".into()),
        ..RawOptions::default()
    };
    let bundle = raw::wrap(&file, &options).expect("wraps");
    (file, bundle)
}

#[test]
fn a_mister_stamps_its_clock_in_local_time_and_says_how_far_that_is() {
    if skip_if_absent() {
        return;
    }
    let (file, bundle) = robopon(Some(PACIFIC));
    let chip = bundle.header.extensions.get::<RtcHuc3>().expect("a HuC-3");
    assert_eq!((chip.days(), chip.minutes(), chip.seconds), (Some(0), Some(4), Some(5)));
    // 20:57:31 counted as though it were UTC; the instant is eight hours later.
    assert_eq!(chip.taken_at, Some(1_794_344_251));
    assert_eq!(rtc::scale_of(&bundle), Some(PACIFIC));
    assert_eq!(raw::unwrap(&bundle).unwrap(), file, "a MiSTer's file back byte for byte");

    // Gambatte under RetroArch counts from an origin on UTC: the instant, less the 4:05 the clock
    // had run.
    let gambatte = written_for(&bundle, &cores::GAMBATTE, &apps::RETROARCH, None);
    assert_eq!(gambatte.rescaled_by, 8 * 3600);
    let origin = i64::from_le_bytes(gambatte.sidecar.expect("a .rtc").try_into().unwrap());
    assert_eq!(origin, 1_794_344_251 + 8 * 3600 - 245);

    // Another MiSTer, told its zone, gets its own local time; one not told gets the bundle's.
    let tokyo = Reader {
        core: Some((&cores::GAMEBOY_MISTER, Some(&apps::MISTER))),
        utc_offset: Some(9 * 3600),
        memory: None,
    };
    let in_tokyo = raw::unwrap_as(&bundle, &tokyo).unwrap();
    assert_eq!(in_tokyo.rescaled_by, 17 * 3600);
    assert_eq!(written_for(&bundle, &cores::GAMEBOY_MISTER, &apps::MISTER, None).save, file);
}

#[test]
fn a_mister_clock_with_no_offset_goes_elsewhere_as_it_always_has_and_says_so() {
    if skip_if_absent() {
        return;
    }
    let (_, bundle) = robopon(None);
    assert_eq!(rtc::scale_of(&bundle), None);
    let gambatte = written_for(&bundle, &cores::GAMBATTE, &apps::RETROARCH, None);
    assert_eq!((gambatte.rescaled_by, gambatte.scale_unknown), (0, true));
    // Back to a MiSTer is no move at all.
    assert!(!written_for(&bundle, &cores::GAMEBOY_MISTER, &apps::MISTER, None).scale_unknown);
    // A Crystal from a MiSTer, its MBC3 anchor moved the same way when the offset is known.
    let crystal = read("mister/gameboy-mister/Pokemon - Crystal Version (USA, Europe) (Rev 1).sav");
    let options = RawOptions {
        system: Some("gbc".into()),
        source: Some(source("mister", "gameboy-mister").utc_offset(Some(PACIFIC))),
        ..RawOptions::default()
    };
    let bundle = raw::wrap(&crystal, &options).unwrap();
    let anchor = bundle.header.extensions.get::<RtcMbc3>().unwrap().taken_at;
    let mgba = written_for(&bundle, &cores::MGBA, &apps::RETROARCH, None);
    let record = rtc::parse_registers_sidecar(&mgba.sidecar.expect("a .rtc")).unwrap();
    assert_eq!(record.taken_at, anchor + 8 * 3600);
}

#[test]
fn a_huc3_goes_to_the_standalone_sameboy_and_visualboyadvance_m_on_the_end_of_the_save() {
    if skip_if_absent() {
        return;
    }
    let (_, bundle) = robopon(Some(PACIFIC));
    let memory = &payload(&bundle);

    // SameBoy appends seventeen bytes: its anchor, the counters and the alarm.
    let sameboy = written_for(&bundle, &cores::SAMEBOY, &apps::SAMEBOY, None);
    assert_eq!(sameboy.save.len(), 32_768 + rtc::SAMEBOY_HUC3_LEN);
    assert_eq!(&sameboy.save[..32_768], &memory[..]);
    let back = wrap_huc3(&sameboy.save, "sameboy", "sameboy", None);
    let chip = back.header.extensions.get::<RtcHuc3>().unwrap();
    assert_eq!((chip.days(), chip.minutes()), (Some(0), Some(4)));
    assert_eq!(chip.taken_at, Some(1_794_344_251 + 8 * 3600), "on UTC, as SameBoy stamps it");
    assert_eq!(raw::unwrap(&back).unwrap(), sameboy.save, "and SameBoy's file back byte for byte");

    // VisualBoyAdvance-M appends 24, an origin first, which reads as the counters at the instant
    // given.
    let vbam = written_for(&bundle, &cores::VBA_M, &apps::VISUALBOYADVANCE_M, None);
    assert_eq!(vbam.save.len(), 32_768 + rtc::VBAM_HUC3_LEN);
    let instant = 1_794_344_251 + 8 * 3600 + 86_400 + 60;
    let back = wrap_huc3(&vbam.save, "visualboyadvance-m", "vba-m", Some(instant));
    let chip = back.header.extensions.get::<RtcHuc3>().unwrap();
    assert_eq!(
        (chip.days(), chip.minutes(), chip.seconds),
        (Some(1), Some(5), Some(5)),
        "a day and a minute on"
    );
    // The origin goes back as it came. The word after it is the counters as the game last read
    // them, which nothing keeps, so it is written as they stand at the anchor.
    let again = raw::unwrap(&back).unwrap();
    assert_eq!(again[..32_768 + 8], vbam.save[..32_768 + 8], "VBA-M's origin back byte for byte");
}

/// A Game Boy save with a HuC-3 on its end, as `core` run by `app` wrote it.
fn wrap_huc3(file: &[u8], app: &str, core: &str, captured_at: Option<i64>) -> Bundle {
    let options = RawOptions {
        system: Some("gbc".into()),
        source: Some(source(app, core)),
        clock_chip: Some("x.1sav.rtc.huc3".into()),
        captured_at,
        ..RawOptions::default()
    };
    raw::wrap(file, &options).expect("wraps")
}
