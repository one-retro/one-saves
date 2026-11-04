//! What the N64 converters make of saves real emulators wrote, checked against files under the
//! workspace's `data/saves`.
//!
//! Each was written by an emulator playing the game from power-on to a save it made itself:
//! Mupen64Plus-Next (3.0-Vulkan 15bada8) and ParaLLEl N64 (1.0 e8cbdbb) under a frontend that saves
//! as RetroArch does, and standalone Mupen64Plus 2.6.0 on macOS. All three replayed the same input,
//! so where the play went the same way, the three files are one save three ways round, and normal
//! form is what makes them the same bytes.
//!
//! These live out here rather than beside the code because they need files the crate does not
//! carry: a published tarball holds the crate and nothing else.

// The libretro save and the paks in it are read behind the pak feature.
#![cfg(feature = "n64")]

use one_saves::extension::{Label, N64Save};
use one_saves::{Bundle, Part, Shape, Slug};
use one_saves_convert::n64::{
    N64Options, WordOrder, cartridge, game_save, read_paks, read_srm, unwrap, wrap, write_pak, write_paks,
    write_srm,
};
use one_saves_convert::{CardOptions, Format, card};

fn fixture(path: &str) -> Vec<u8> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/saves");
    std::fs::read(root.join(path)).expect("reads")
}

/// A game's `.srm` as Mupen64Plus-Next or ParaLLEl N64 wrote it.
fn libretro(core: &str, game: &str) -> Vec<u8> {
    fixture(&format!("retroarch/{core}/{game}.srm"))
}

/// A file as standalone Mupen64Plus named it: the ROM's good name cut at 32 characters, and the
/// first eight digits of its MD5.
fn mupen64plus(file: &str) -> Vec<u8> {
    fixture(&format!("macos/mupen64plus/{file}"))
}

const CORES: [&str; 2] = ["mupen64plus-next", "parallel-n64"];

/// The chip a cartridge save names, and its memory in normal form.
fn chip_of(bundle: &Bundle) -> (N64Save, Vec<u8>) {
    cartridge(bundle).expect("reads").expect("a cartridge save")
}

/// The pak a device keeps in a port, as the card it is.
fn pak_in(device: &Bundle, port: usize) -> Bundle {
    let role = format!("controller-pak-{port}");
    let component = device
        .parts
        .iter()
        .find(|part| part.role.as_ref().is_some_and(|r| r.as_str() == role))
        .expect("a pak in that port");
    Bundle::from_slice(&component.bytes().expect("inline")).expect("a card")
}

fn serial(part: &Part) -> Option<&str> {
    part.game.as_ref().and_then(|game| game.serial.as_deref())
}

fn label(part: &Part) -> String {
    let save = Bundle::from_slice(&part.bytes().expect("inline")).expect("a save");
    save.header.extensions.get::<Label>().expect("a label").title.clone()
}

/// Every chip of the three in the corpus whose play went the same way in each emulator: what the
/// two cores keep at their chip's offset in one file, and mupen64plus in a file of the chip's own.
#[test]
fn one_save_is_the_same_bytes_whichever_emulator_wrote_it() {
    let games = [
        ("Paper Mario (USA)", "Paper Mario (U) [!]-A722F816.fla", N64Save::Flash1m),
        ("Super Smash Bros. (USA)", "Super Smash Bros. (U) [!]-F7C52568.sra", N64Save::Sram256k),
        ("Perfect Dark (USA) (Rev 1)", "Perfect Dark (U) (V1.1) [!]-E03B088B.eep", N64Save::Eeprom16k),
    ];
    for (game, file, chip) in games {
        let standalone =
            wrap(&mupen64plus(file), chip, WordOrder::Swapped, &N64Options::default()).expect("wraps");
        let (_, normal) = chip_of(&standalone);

        for core in CORES {
            // Each of these files proves its own chip: one region written, and Perfect Dark's
            // EEPROM written past where a 4 Kbit chip ends.
            let device = read_srm(&libretro(core, game), None, &N64Options::default()).expect("reads");
            device.validate().expect("valid");
            assert_eq!(chip_of(&device), (chip, normal.clone()), "{game} from {core}");
        }
    }
}

#[test]
fn normal_form_is_the_order_the_console_reads() {
    // Paper Mario opens its FlashRAM with text, which every one of these files keeps reversed a
    // word at a time.
    let file = mupen64plus("Paper Mario (U) [!]-A722F816.fla");
    assert_eq!(&file[..16], b"iraMtS o yro\x00600");
    let (_, normal) =
        chip_of(&wrap(&file, N64Save::Flash1m, WordOrder::Swapped, &N64Options::default()).expect("wraps"));
    assert_eq!(&normal[..16], b"Mario Story 006\0");
}

#[test]
fn a_note_is_named_by_its_serial_and_spelled_through_its_pak_s_table() {
    // Perfect Dark writes its pak at boot, beside its EEPROM; Tony Hawk's has nowhere else to save.
    let notes = [
        ("Perfect Dark (USA) (Rev 1)", "NUS-NPDE", "PERFECT DARK", "PERFECT DARK", 28),
        ("Tony Hawk's Pro Skater (USA) (Rev 1)", "NUS-NTFE", "TH-AA.G", "TH-AA", 11),
    ];
    for (game, code, path, title, pages) in notes {
        for core in CORES.iter().filter(|&&core| core == "mupen64plus-next" || game.starts_with("Perfect")) {
            let device = read_srm(&libretro(core, game), None, &N64Options::default()).expect("reads");
            let pak = pak_in(&device, 1);
            let [note] = pak.parts.as_slice() else { panic!("{game} from {core}: one note") };
            assert_eq!(serial(note), Some(code));
            assert_eq!(note.path.as_deref(), Some(path));
            assert_eq!(note.slot, Some(0));
            assert_eq!(label(note), title);
            let save = Bundle::from_slice(&note.bytes().expect("inline")).expect("a save");
            assert_eq!(save.parts[0].bytes().expect("inline").len(), pages * 256);
        }
    }
}

#[test]
fn a_game_that_saves_only_to_its_pak_is_a_device_of_one_pak() {
    let device = read_srm(
        &libretro("mupen64plus-next", "Tony Hawk's Pro Skater (USA) (Rev 1)"),
        None,
        &N64Options::default(),
    )
    .expect("reads");
    assert_eq!(device.header.shape, Shape::Device);
    assert_eq!(device.parts.len(), 1, "no cartridge, and no pak nobody saved to");
    assert!(cartridge(&device).expect("reads").is_none());

    // The same note off standalone Mupen64Plus's file of four paks is the same bytes.
    let paks =
        read_paks(&mupen64plus("Tony Hawk's Pro Skater (U) (V1.1-AFF424A1.mpk"), &N64Options::default())
            .expect("reads");
    assert_eq!(pak_in(&paks, 1).parts[0].bytes().unwrap(), pak_in(&device, 1).parts[0].bytes().unwrap());
}

#[test]
fn paks_a_game_only_formatted_hold_nothing_to_carry() {
    // Banjo-Kazooie looks for a pak at boot and writes nothing to it.
    let paks = mupen64plus("Banjo-Kazooie (U) (V1.1) [!]-B11F476D.mpk");
    assert!(read_paks(&paks, &N64Options::default()).is_err());

    let device = read_srm(&libretro("mupen64plus-next", "Paper Mario (USA)"), None, &N64Options::default())
        .expect("reads");
    assert_eq!(device.parts.len(), 1, "the cartridge alone");
}

#[test]
fn a_4_kbit_eeprom_is_named_rather_than_told_from_a_16_kbit_one() {
    // Super Mario 64 writes only the first 512 bytes, which a 16 Kbit game could have done too.
    let file = libretro("mupen64plus-next", "Super Mario 64 (USA)");
    assert!(read_srm(&file, None, &N64Options::default()).is_err());

    let device = read_srm(&file, Some(N64Save::Eeprom4k), &N64Options::default()).expect("reads");
    let (chip, normal) = chip_of(&device);
    assert_eq!((chip, normal.len()), (N64Save::Eeprom4k, 512));
    assert_eq!(write_srm(&device, None).expect("writes")[..512], normal[..]);

    // Standalone Mupen64Plus writes that chip at its own size, and it comes back so.
    let file = mupen64plus("Super Mario 64 (U) [!]-20B854B2.eep");
    let bundle = wrap(&file, N64Save::Eeprom4k, WordOrder::Swapped, &N64Options::default()).expect("wraps");
    assert_eq!(unwrap(&bundle, WordOrder::Swapped).expect("unwraps"), (N64Save::Eeprom4k, file));
}

/// Written back onto the file it came from, a device is that file again, byte for byte: its paks
/// keep the ID block of the ones they replace, which the two cores fill differently.
#[test]
fn a_device_written_onto_its_own_file_is_that_file() {
    let srms = [
        ("mupen64plus-next", "Perfect Dark (USA) (Rev 1)", None),
        ("parallel-n64", "Perfect Dark (USA) (Rev 1)", None),
        ("mupen64plus-next", "Tony Hawk's Pro Skater (USA) (Rev 1)", None),
        ("mupen64plus-next", "Super Smash Bros. (USA)", None),
        ("parallel-n64", "Paper Mario (USA)", None),
        ("mupen64plus-next", "Super Mario 64 (USA)", Some(N64Save::Eeprom4k)),
    ];
    for (core, game, chip) in srms {
        let file = libretro(core, game);
        let device = read_srm(&file, chip, &N64Options::default()).expect("reads");
        assert!(write_srm(&device, Some(&file)).expect("writes") == file, "{game} from {core}");
    }

    for name in ["Perfect Dark (U) (V1.1) [!]-E03B088B.mpk", "Tony Hawk's Pro Skater (U) (V1.1-AFF424A1.mpk"]
    {
        let file = mupen64plus(name);
        let device = read_paks(&file, &N64Options::default()).expect("reads");
        assert!(write_paks(&device, Some(&file)).expect("writes") == file, "{name}");

        // One port at a time, as Project64 and an EverDrive keep their paks.
        let pak = &file[..32 * 1024];
        assert!(write_pak(&device, 1, Some(pak)).expect("writes").expect("a pak in port 1") == pak, "{name}");
    }
}

/// Written afresh, a device's paks are formatted ones carrying its notes, which read back as the
/// same device.
#[test]
fn a_device_written_afresh_reads_back_as_itself() {
    for core in CORES {
        let device = read_srm(&libretro(core, "Perfect Dark (USA) (Rev 1)"), None, &N64Options::default())
            .expect("reads");
        let file = write_srm(&device, None).expect("writes");
        let again = read_srm(&file, None, &N64Options::default()).expect("reads");
        assert_eq!(again.to_vec().unwrap(), device.to_vec().unwrap(), "{core}");
    }
}

#[test]
fn one_game_s_save_cut_out_of_a_device_goes_back_where_it_was() {
    let file = libretro("parallel-n64", "Perfect Dark (USA) (Rev 1)");
    let device = read_srm(&file, None, &N64Options::default()).expect("reads");

    let save = game_save(&device, "NUS-NPDE").expect("cuts");
    save.validate().expect("valid");
    assert_eq!(save.header.shape, Shape::Save);
    assert_eq!(save.parts.len(), 2, "the cartridge and its note");
    assert!(write_srm(&save, Some(&file)).expect("writes") == file, "the file it came out of");

    // With no ROM to name it, the save is named by the serial its note carries, and listed as the
    // pak lists the note.
    let game = save.header.game.as_ref().expect("a game");
    assert_eq!(game.serial.as_deref(), Some("NUS-NPDE"));
    assert_eq!(save.header.extensions.get::<Label>().expect("a label").title, "PERFECT DARK");
}

/// The pak in port 1 of a file of four, which is the whole file a producer keeping one per pak
/// writes: Project64's `_Cont_1.mpk`, an EverDrive's `.mpk`, the MiSTer core's `_1.cpk`.
const PAK: usize = 32 * 1024;

#[test]
fn one_game_s_notes_cut_out_of_a_lone_pak_are_those_cut_out_of_all_four() {
    let file = mupen64plus("Tony Hawk's Pro Skater (U) (V1.1-AFF424A1.mpk");
    let pak = &file[..PAK];
    let card = card::read(Format::N64Pak, pak, &CardOptions::default()).expect("reads");
    assert_eq!(
        card.parts[0].role.as_ref().map(Slug::as_str),
        Some("controller-pak-1"),
        "a controller's port"
    );

    let from_pak = game_save(&card, "NUS-NTFE").expect("cuts");
    from_pak.validate().expect("valid");
    let from_paks =
        game_save(&read_paks(&file, &N64Options::default()).expect("reads"), "NTFE").expect("cuts");
    assert_eq!(from_pak.to_vec().unwrap(), from_paks.to_vec().unwrap());
    assert_eq!(from_pak.header.game.as_ref().and_then(|game| game.serial.as_deref()), Some("NUS-NTFE"));
    assert_eq!(from_pak.header.extensions.get::<Label>().expect("a label").title, "TH-AA");
    let written = write_pak(&from_pak, 1, Some(pak)).expect("writes").expect("a pak in port 1");
    assert!(written == pak, "the pak it came out of");

    // The role is the port the notes go back to, so one naming none is refused at the cut rather
    // than at the restore.
    let options = CardOptions { role: Some(Slug::parse("memcard-1").unwrap()), ..CardOptions::default() };
    let card = card::read(Format::N64Pak, pak, &options).expect("reads");
    assert!(game_save(&card, "NTFE").is_err());
}

/// The ID block a console knows a pak by is no bundle's to carry. Only the file being written over
/// has it, which ParaLLEl fills with a serial of its own.
#[test]
fn a_lone_pak_written_onto_its_own_file_is_that_file() {
    let srm = libretro("parallel-n64", "Perfect Dark (USA) (Rev 1)");
    let pak = &srm[0x800..0x800 + PAK];
    let card = card::read(Format::N64Pak, pak, &CardOptions::default()).expect("reads");

    assert!(card::write_onto(Format::N64Pak, &card, pak).expect("writes") == pak, "the same pak");
    let fresh = card::write(Format::N64Pak, &card).expect("writes");
    assert!(fresh != pak, "formatted afresh, with an ID block of its own");
    let again = card::read(Format::N64Pak, &fresh, &CardOptions::default()).expect("reads");
    assert_eq!(again.to_vec().unwrap(), card.to_vec().unwrap(), "the same notes on another pak");

    assert!(card::write_onto(Format::GcCard, &card, pak).is_err(), "no other card keeps anything");
}
