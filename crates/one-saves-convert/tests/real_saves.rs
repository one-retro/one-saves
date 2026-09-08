//! Runs the converters against saves real emulators wrote.
//!
//! Everything here came off an actual emulator or FPGA core rather than being constructed, which
//! is the point: a synthetic fixture proves the code agrees with itself, and these prove it
//! agrees with the thing it has to interoperate with. Between them they cover four producers and
//! four on-disk shapes of the same cartridge clock, and six Sega CD backup RAMs off two more —
//! both sockets, both of the filesystem's storage modes, a volume with nothing on it, and one
//! save sitting on two volumes at once.
//!
//! The expected clock values are recorded here rather than beside the files. They are assertions,
//! not data, and the Gambatte capture instant in particular **cannot** be taken from the
//! filesystem: git does not preserve modification times, so on a fresh clone the sidecar's mtime
//! is checkout time and the derived reading would differ per clone.
//!
//! The fixtures live at the workspace root, outside this crate, so a packaged copy of
//! `one-saves-convert` does not carry them. These tests skip in that case rather than fail.

use std::path::{Path, PathBuf};

use one_saves::Bundle;
use one_saves_convert::{Format, detect, profile, raw, rtc};

/// The workspace's fixtures, which are not packaged with the crate — see this file's header.
fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/saves")
}

/// What a real save should decode to.
struct Fixture {
    /// Where it sits under `data/saves/`.
    path: &'static str,
    /// The `--from` name, which is also what picks the shape it is written back in.
    producer: &'static str,
    /// The system slug, which tells a 16-byte GBA footer from a 16-byte Game Boy one.
    system: &'static str,
    /// Bytes on disk, so a truncated or replaced fixture is caught before anything is decoded.
    len: usize,
    /// The cartridge clock: days, hours, minutes, seconds.
    clock: (u32, u32, u32, u32),
    /// The instant the save was written: the point the counters advance *from*, never the
    /// reading itself.
    anchor: i64,
    /// What the cartridge clock therefore showed — the anchor plus the elapsed time above. This
    /// is the number `x.1sav.rtc` carries, and on the Pocket sample it is 2.8 days from the
    /// anchor, so writing the anchor there would be visibly wrong rather than subtly so.
    reading: i64,
    /// A sidecar clock file beside the save, and the instant it was written.
    sidecar: Option<(&'static str, i64)>,
}

const CRYSTAL: &str = "Pokemon - Crystal Version (USA, Europe) (Rev 1).sav";

fn fixtures() -> Vec<Fixture> {
    vec![
        // Appended, 48 bytes: ten little-endian registers and a write time.
        Fixture {
            path: "GBC/Pokemon Crystal/Sameboy",
            producer: "sameboy",
            system: "gb",
            len: 32_816,
            clock: (0, 0, 2, 13),
            anchor: 1_786_559_074,
            reading: 1_786_559_207,
            sidecar: None,
        },
        // Appended, one 512-byte SD block: a write time and the registers bit-packed.
        Fixture {
            path: "GBC/Pokemon Crystal/mister",
            producer: "gameboy-mister",
            system: "gb",
            len: 33_280,
            clock: (0, 0, 2, 17),
            anchor: 1_786_533_688,
            reading: 1_786_533_825,
            sidecar: None,
        },
        // The same packing as MiSTer, in a 16-byte reserved region.
        Fixture {
            path: "GBC/Pokemon Crystal/budude2",
            producer: "budude2-gbc",
            system: "gb",
            len: 32_784,
            clock: (2, 19, 49, 7),
            anchor: 1_786_532_469,
            reading: 1_786_776_616,
            sidecar: None,
        },
        // A sidecar holding an origin rather than a reading. The instant is stated because the
        // file does not record it and its mtime does not survive a clone.
        Fixture {
            path: "GBC/Pokemon Crystal/Gambatte",
            producer: "gambatte",
            system: "gb",
            len: 32_768,
            clock: (0, 0, 4, 54),
            anchor: 1_786_558_622,
            reading: 1_786_558_916,
            sidecar: Some(("Pokemon - Crystal Version (USA, Europe) (Rev 1).rtc", 1_786_558_622)),
        },
    ]
}

/// The second budude2 save, taken 2673 seconds after the first.
fn budude2_later() -> PathBuf {
    fixtures_root()
        .join("GBC/Pokemon Crystal/budude2/Pokemon - Crystal Version (USA, Europe) (Rev 1).5min.sav")
}

/// The instant a bundle's `x.1sav.rtc` carries, read back the way any consumer would.
fn reading_of(bundle: &Bundle) -> Option<i64> {
    let map = bundle.header.extensions.get(&rtc::rtc_key())?.as_map()?;
    match map.get::<u64, one_saves::dcbor::CBOR>(0)?.as_case() {
        one_saves::dcbor::CBORCase::Tagged(_, inner) => match inner.as_case() {
            one_saves::dcbor::CBORCase::Unsigned(n) => i64::try_from(*n).ok(),
            _ => None,
        },
        _ => None,
    }
}

/// Skips rather than fails when the fixtures are not present, which is the case for a packaged
/// copy of this crate.
fn skip_if_absent() -> bool {
    if fixtures_root().is_dir() {
        return false;
    }
    println!("skipping: {} is not present", fixtures_root().display());
    true
}

impl Fixture {
    fn save_path(&self) -> PathBuf {
        fixtures_root().join(self.path).join(CRYSTAL)
    }

    /// Wraps the save the way the CLI would, with the producer's `source` set — which is what
    /// decides the shape it is written back in.
    fn wrap(&self) -> (Vec<u8>, Bundle) {
        let bytes = std::fs::read(self.save_path()).expect("read fixture");
        assert_eq!(bytes.len(), self.len, "{} is not the size it was", self.path);

        let sidecar = self.sidecar.map(|(name, captured_at)| {
            let path = fixtures_root().join(self.path).join(name);
            (std::fs::read(path).expect("read sidecar"), captured_at)
        });

        let options = raw::RawOptions {
            system: Some(self.system.to_owned()),
            source: Some(
                profile(self.producer)
                    .unwrap_or_else(|| panic!("{} is a known producer", self.producer))
                    .source(None),
            ),
            rtc_sidecar: sidecar,
            ..raw::RawOptions::default()
        };
        let bundle = raw::wrap(&bytes, &options).expect("wraps");
        (bytes, bundle)
    }
}

#[test]
fn every_real_save_decodes_to_the_clock_it_was_taken_with() {
    if skip_if_absent() {
        return;
    }
    for fixture in fixtures() {
        let (_, bundle) = fixture.wrap();
        bundle.validate().unwrap_or_else(|e| panic!("{}: {e}", fixture.path));

        // Both keys, on the header, whichever shape the producer wrote.
        assert!(
            bundle.header.extensions.contains_key(&rtc::rtc_key()),
            "{}: no portable reading",
            fixture.path
        );

        let footer = rtc::footer_of(&bundle).or_else(|| rtc::sidecar_of(&bundle));
        assert!(footer.is_some(), "{}: no clock to rebuild", fixture.path);

        // The reading itself, checked against what the clock actually said.
        let rebuilt = rtc::footer_of_form(&bundle, rtc::ClockForm::Mbc3Appended)
            .expect("the MBC3 form is always derivable");
        let clock = rtc::parse_mbc3(&rebuilt).expect("parses");
        assert_eq!(
            (clock.days(), clock.hours, clock.minutes, clock.seconds),
            fixture.clock,
            "{}: cartridge clock",
            fixture.path
        );
        assert_eq!(clock.written_at, fixture.anchor, "{}: anchor", fixture.path);

        // And the reading is the anchor plus that elapsed time, which is the distinction the key
        // exists to make. A producer that cannot compute it omits the key; none here has to.
        assert_eq!(clock.reading().instant, fixture.reading, "{}: reading", fixture.path);
        assert_eq!(
            reading_of(&bundle),
            Some(fixture.reading),
            "{}: the reading written to the header",
            fixture.path
        );
    }
}

#[test]
fn every_real_save_round_trips_byte_for_byte() {
    if skip_if_absent() {
        return;
    }
    for fixture in fixtures() {
        let (original, bundle) = fixture.wrap();

        // Through the encoded form, so the clock survives being written and read back.
        let reread = Bundle::from_slice(&bundle.to_vec().expect("encodes")).expect("decodes");
        let back = raw::unwrap(&reread).expect("unwraps");
        assert_eq!(back, original, "{}: the save should come back unchanged", fixture.path);

        // A producer that keeps its clock beside the save gets that back too, and the save stays
        // clean rather than gaining a footer it never had.
        if let Some((name, _)) = fixture.sidecar {
            let path = fixtures_root().join(fixture.path).join(name);
            let expected = std::fs::read(path).expect("read sidecar");
            assert_eq!(
                rtc::sidecar_of(&reread).expect("rebuilds the sidecar"),
                expected,
                "{}: sidecar",
                fixture.path
            );
        }
    }
}

#[test]
fn the_save_memory_is_what_gets_deduplicated() {
    if skip_if_absent() {
        return;
    }
    // The whole reason the clock is split out. Two saves from the same core minutes apart differ
    // in their clock and nowhere else that matters, so the part a store keys on must not move
    // just because time passed.
    let first = fixtures().into_iter().find(|f| f.producer == "budude2-gbc").expect("budude2");
    let (early_bytes, early) = first.wrap();
    let later_bytes = std::fs::read(budude2_later()).expect("read the later save");

    let options = raw::RawOptions {
        system: Some("gb".to_owned()),
        source: Some(profile("budude2-gbc").expect("known").source(None)),
        ..raw::RawOptions::default()
    };
    let later = raw::wrap(&later_bytes, &options).expect("wraps");

    // The clocks genuinely differ, 2673 seconds apart.
    let clock_of = |b: &Bundle| {
        let raw = rtc::footer_of_form(b, rtc::ClockForm::Mbc3Appended).expect("derivable");
        rtc::parse_mbc3(&raw).expect("parses")
    };
    assert_eq!(clock_of(&later).written_at - clock_of(&early).written_at, 2673);

    // The save memory differs too — the game really did save between the two — so this is a
    // check that the split works, not that the bytes happened to be identical.
    assert_ne!(early_bytes[..32_768], later_bytes[..32_768]);
    assert_eq!(early.parts[0].payload.len(), 32_768);
    assert_eq!(later.parts[0].payload.len(), 32_768);
}

#[test]
fn one_cartridge_clock_reads_the_same_through_every_producer() {
    if skip_if_absent() {
        return;
    }
    // Five producers, four on-disk shapes, one pair of extension keys. A consumer should never
    // have to learn whose filing convention a bundle came from.
    let mut shapes = std::collections::BTreeSet::new();
    for fixture in fixtures() {
        let (_, bundle) = fixture.wrap();
        shapes.insert(format!("{:?}", rtc::form_of(&bundle)));

        // Whatever the shape, the reading is in the same place and says the same kind of thing:
        // key 0, tagged, and nothing else. `accuracy_ms` is omitted because nothing here can
        // measure it, and which clock produced it is not a field at all.
        let reading = bundle.header.extensions.get(&rtc::rtc_key()).expect("a reading");
        let map = reading.as_map().expect("a map");
        assert!(
            map.get::<u64, one_saves::dcbor::CBOR>(0).is_some_and(|v| v.as_tagged_value().is_some()),
            "{}: key 0 should be a tagged instant",
            fixture.path
        );
        assert!(map.get::<u64, one_saves::dcbor::CBOR>(1).is_none(), "{}: no accuracy", fixture.path);
    }
    assert!(shapes.len() >= 3, "the fixtures should cover several shapes, got {shapes:?}");
}

// ---------------------------------------------------------------------------------------------
// Sega CD backup RAMs.
// ---------------------------------------------------------------------------------------------

/// A backup RAM volume, and what a reader will have to find in it.
///
/// Nothing here reads a BRAM's filesystem — a volume is wrapped whole — so the contents are
/// recorded as the counters in the footer, which are stored plainly and need no decoder. Each
/// entry's comment says what the directory holds, checked against `superctr/buram`, so whoever
/// writes the reader has something to aim at.
struct BackupRam {
    /// Where it sits under `data/saves/`, file included.
    path: &'static str,
    /// The `--from` name, which resolves through the Emulator Cores registry.
    producer: &'static str,
    /// Bytes on disk: 8 KiB for the console's internal RAM, more for a Backup RAM Cart.
    len: usize,
    /// The footer's free-block count, and the number of files in the directory.
    counters: (u16, u16),
}

fn backup_rams() -> Vec<BackupRam> {
    vec![
        // Three unprotected files, eleven blocks each, laid end to end from block 1:
        // SONICCD, SONICCD__01, SONICCD__02.
        BackupRam {
            path: "MegaCD/Sonic CD/MegaCD_MiSTer/Sonic CD (USA).sav",
            producer: "megacd-mister",
            len: 8192,
            counters: (91, 3),
        },
        // Two ECC-protected files, and one of them is 99 blocks — which is most of the volume,
        // and the reason the Backup RAM Cart exists: SFCD_DAT_09 (1 block), SFCD_DAT_00 (99).
        BackupRam {
            path: "MegaCD/Shining Force CD/MegaCD_MiSTer/Shining Force CD (USA) (3R).sav",
            producer: "megacd-mister",
            len: 8192,
            counters: (24, 2),
        },
        // The same volume after more play: the same two files at the same blocks, so the
        // directory and the counters are untouched and only contents moved. See
        // `one_saved_file_moving_does_not_move_the_others`.
        BackupRam {
            path: "MegaCD/Shining Force CD/MegaCD_MiSTer/Shining Force CD (USA) (3R).later.sav",
            producer: "megacd-mister",
            len: 8192,
            counters: (24, 2),
        },
        // The console's own volume rather than one game's: Genesis Plus GX names this after the
        // BIOS region, so every US disc played shares it. One file so far, SONICCD at block 1.
        BackupRam {
            path: "MegaCD/Sonic CD/Genesis Plus GX Wide/scd_U.brm",
            producer: "genesis-plus-gx-wide",
            len: 8192,
            counters: (114, 1),
        },
        // A Backup RAM Cart, formatted and empty — the other socket, and the shape a reader has
        // to handle when there is nothing to nest. Its three unusable blocks are the reserved
        // first, the footer, and one held back for the directory entry a first save would need.
        BackupRam {
            path: "MegaCD/Sonic CD/Genesis Plus GX Wide/4Mbit_cart.empty.brm",
            producer: "genesis-plus-gx-wide",
            len: 524_288,
            counters: (8189, 0),
        },
        // The same cart after the BIOS's own manager copied SONICCD onto it from the internal
        // RAM. Fourteen blocks are now spoken for: the reserved first, eleven of file, one of
        // directory and the footer — and with an odd file count nothing is held back.
        BackupRam {
            path: "MegaCD/Sonic CD/Genesis Plus GX Wide/4Mbit_cart.brm",
            producer: "genesis-plus-gx-wide",
            len: 524_288,
            counters: (8178, 1),
        },
    ]
}

/// The two dumps of the Shining Force CD volume, earlier first.
fn shining_force_pair() -> (Vec<u8>, Vec<u8>) {
    let rams = backup_rams();
    let of = |path: &str| rams.iter().find(|r| r.path == path).expect("a fixture").read();
    (
        of("MegaCD/Shining Force CD/MegaCD_MiSTer/Shining Force CD (USA) (3R).sav"),
        of("MegaCD/Shining Force CD/MegaCD_MiSTer/Shining Force CD (USA) (3R).later.sav"),
    )
}

impl BackupRam {
    fn read(&self) -> Vec<u8> {
        let bytes = std::fs::read(fixtures_root().join(self.path)).expect("read fixture");
        assert_eq!(bytes.len(), self.len, "{} is not the size it was", self.path);
        bytes
    }

    fn wrap(&self) -> (Vec<u8>, Bundle) {
        let bytes = self.read();
        let options = raw::RawOptions {
            // Deliberately unset: the point is that the bytes settle it.
            system: None,
            role: Some("internal".to_owned()),
            source: Some(profile(self.producer).expect("a listed core").source(None)),
            ..raw::RawOptions::default()
        };
        let bundle = raw::wrap(&bytes, &options).expect("wraps");
        (bytes, bundle)
    }
}

/// The footer's two counters, each written four times over for redundancy.
///
/// They sit in the last 0x40 bytes and are stored plainly, unlike the directory entries above
/// them, which are held behind the volume's error correction and need a decoder to read.
fn counters(bytes: &[u8]) -> (u16, u16) {
    let footer = bytes.len() - 0x40;
    let at = |offset: usize| u16::from_be_bytes([bytes[footer + offset], bytes[footer + offset + 1]]);
    (at(0x10), at(0x18))
}

#[test]
fn a_backup_ram_is_known_by_its_bytes_and_not_by_its_name() {
    if skip_if_absent() {
        return;
    }
    // This is what the signature is for. A MiSTer core writes `.sav`, which is the extension a
    // flat cartridge save takes, so the name puts these in the right *format* and says nothing
    // about the system. Without reading the footer they would wrap as saves for no console.
    for fixture in backup_rams() {
        let bytes = fixture.read();
        assert!(raw::is_segacd_bram(&bytes), "{}: not recognised", fixture.path);
        assert_eq!(detect::detect(&bytes, "sav").expect("detects"), Format::Raw, "{}", fixture.path);
        // And with no extension at all, where the bytes are the only thing there is to go on.
        assert_eq!(detect::detect(&bytes, "").expect("detects"), Format::Raw, "{}", fixture.path);
    }
}

#[test]
fn every_backup_ram_names_its_own_system_and_is_wrapped_whole() {
    if skip_if_absent() {
        return;
    }
    for fixture in backup_rams() {
        let (bytes, bundle) = fixture.wrap();
        bundle.validate().unwrap_or_else(|e| panic!("{}: {e}", fixture.path));

        // Nothing said `sega-cd`: `system` is left unset above, so the footer is the only thing
        // that could have settled it. Note that the CLI would not get this far — `megacd-mister`
        // covers one system, so `--from` settles it before the bytes are ever consulted. What is
        // under test is a volume arriving with nothing attached, which is how most of them do.
        assert_eq!(
            bundle.header.system.as_ref().map(one_saves::Slug::as_str),
            Some("sega-cd"),
            "{}",
            fixture.path
        );
        // The socket is stated, because a Sega CD has two and an absent role would claim
        // otherwise.
        assert_eq!(
            bundle.parts[0].role.as_ref().map(one_saves::Slug::as_str),
            Some("internal"),
            "{}",
            fixture.path
        );
        // One part holding the whole volume, footer and all: the filesystem is not taken apart,
        // and a volume is not a cartridge, so nothing is split off the end of it either.
        assert_eq!(bundle.parts.len(), 1, "{}", fixture.path);
        let len = u64::try_from(fixture.len).expect("a volume fits");
        assert_eq!(bundle.parts[0].payload.len(), len, "{}", fixture.path);
        assert!(bundle.header.extensions.is_empty(), "{}: a volume carries no clock", fixture.path);

        // What the volume holds, so a fixture that is quietly replaced by an emptier one is
        // caught rather than silently weakening the tests that will read it.
        assert_eq!(counters(&bytes), fixture.counters, "{}: free blocks and files", fixture.path);
    }
}

#[test]
fn every_backup_ram_round_trips_byte_for_byte() {
    if skip_if_absent() {
        return;
    }
    for fixture in backup_rams() {
        let (original, bundle) = fixture.wrap();
        let reread = Bundle::from_slice(&bundle.to_vec().expect("encodes")).expect("decodes");
        let back = raw::unwrap(&reread).expect("unwraps");
        assert_eq!(back, original, "{}: the volume should come back unchanged", fixture.path);
    }
}

#[test]
fn one_saved_file_moving_does_not_move_the_others() {
    if skip_if_absent() {
        return;
    }
    // Two dumps of one volume, taken a session apart. This is the Sega CD's answer to the pair of
    // budude2 saves above, and it is what says whether cutting a volume into its files is worth
    // doing: 75 of SFCD_DAT_00's 99 blocks moved, and SFCD_DAT_09 did not move at all.
    let (early, later) = shining_force_pair();
    let block = |bytes: &[u8], n: usize| bytes[n * 0x40..(n + 1) * 0x40].to_vec();

    assert_ne!(early, later, "the two dumps should differ");
    assert_eq!(block(&early, 1), block(&later, 1), "SFCD_DAT_09 did not change");
    assert_ne!(block(&early, 2), block(&later, 2), "SFCD_DAT_00 did change");

    // The directory and the footer are identical, since the files kept their names, blocks and
    // sizes. A reader that rebuilds a volume has to reproduce both exactly.
    //
    // Where the directory starts is not a constant: it grows down from the footer at 0x20 to an
    // entry, so it depends on how many files there are. Taking that from the volume itself is
    // what stops a fixture with a different directory from quietly turning this into a
    // comparison of some file's contents.
    assert_eq!(counters(&early), counters(&later), "the directory did not change");
    let (_, files) = counters(&early);
    let directory = 127 - (usize::from(files) * 0x20).div_ceil(0x40);
    assert_eq!(early[directory * 0x40..], later[directory * 0x40..], "directory and footer");

    // And the block the filesystem never allocates is *not* dead padding: it carries a 32-bit
    // value written four times over, in the same belt-and-braces style as the footer's counters,
    // and it moved between the two dumps. It belongs to no file, so a reader that splits this
    // volume into parts has to keep it — a `system_area`, the way every card format here does —
    // or a rebuilt volume comes back subtly wrong in a way no file's contents would show.
    assert_ne!(block(&early, 0), block(&later, 0), "the reserved block is live state");

    // What that costs today: the volume is one part, so the whole of it hashes anew.
    let bundle_of = |bytes: &[u8]| raw::wrap(bytes, &raw::RawOptions::default()).expect("wraps");
    assert_ne!(
        bundle_of(&early).parts[0].sha256,
        bundle_of(&later).parts[0].sha256,
        "wrapped whole, two dumps of one volume share nothing"
    );
}

#[test]
fn a_save_is_the_same_bytes_whatever_volume_it_sits_on() {
    if skip_if_absent() {
        return;
    }
    // The Sega CD BIOS's own manager copied SONICCD from the console's 8 KiB internal RAM onto a
    // 512 KB Backup RAM Cart, so the same save now sits on two volumes that share nothing but the
    // format. This is the property the whole exercise rests on: what a save *is* does not depend
    // on the medium under it, so splitting a volume into its files gives a store something it can
    // deduplicate, and a save carried between sockets keeps one identity rather than gaining a
    // second. Both volumes hold it unprotected at block 1, eleven blocks long, which the
    // `counters` above pin down.
    let rams = backup_rams();
    let of = |path: &str| rams.iter().find(|r| r.path == path).expect("a fixture").read();
    let internal = of("MegaCD/Sonic CD/Genesis Plus GX Wide/scd_U.brm");
    let cart = of("MegaCD/Sonic CD/Genesis Plus GX Wide/4Mbit_cart.brm");

    let file = |volume: &[u8]| volume[0x40..12 * 0x40].to_vec();
    assert_eq!(file(&internal), file(&cart), "one save, two volumes");
    assert_eq!(file(&internal).len(), 704);

    // The volumes themselves are nothing alike, which is what makes that worth asserting.
    assert_ne!(internal.len(), cart.len());
    // And the block the filesystem never allocates is live on the volume a game writes through
    // and untouched on the one it does not: a reader has to carry whatever is there rather than
    // assume either.
    assert!(internal[..0x40].iter().any(|&b| b != 0), "the console's own volume uses block 0");
    assert!(cart[..0x40].iter().all(|&b| b == 0), "the cart's is still blank");
}
