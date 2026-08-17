//! Runs the converters against saves real emulators wrote.
//!
//! Everything here came off an actual emulator or FPGA core rather than being constructed, which
//! is the point: a synthetic fixture proves the code agrees with itself, and these prove it
//! agrees with the thing it has to interoperate with. Between them they cover five producers and
//! four on-disk shapes of the same cartridge clock.
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
use one_saves_convert::{profile, raw, rtc};

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
