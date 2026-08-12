//! Flat cartridge saves: the `.srm`, `.sav`, `.eep` and `.fla` files most emulators write.
//!
//! There is nothing to parse. A flat save is the cartridge's save memory as a run of bytes, and
//! wrapping it is a matter of saying what it belongs to — which is exactly what the container is
//! for, and exactly what a bare file cannot say.
//!
//! The role stays absent, meaning `primary`. A great many systems have exactly one place a save
//! can live, and the registry's advice is to prefer a Common role where one fits rather than
//! reaching for `cartridge` because the medium happens to be a cartridge.

use one_saves::{Bundle, Game, Header, Part, Slug, Source};

use crate::error::{Error, Result};

/// How to wrap a flat save.
#[derive(Debug, Clone)]
pub struct RawOptions {
    /// The system these bytes are a save for.
    pub system: Option<String>,
    /// Which socket they came out of. Absent means `primary`.
    pub role: Option<String>,
    /// Who produced them.
    pub source: Option<Source>,
    /// What is known about the game.
    pub game: Option<Game>,
    /// A human-readable note about this bundle.
    pub description: Option<String>,
    /// When the bundle was assembled, in whole epoch seconds.
    pub created_at: Option<i64>,
    /// Whether to split an appended real-time-clock footer off the save.
    ///
    /// On by default. Leaving it inline keeps the payload byte-identical to the emulator's file,
    /// at the cost of a content hash that moves every time the clock does; see [`crate::rtc`].
    pub split_rtc: bool,
    /// A sidecar clock file written beside the save, and the instant it was written.
    ///
    /// Gambatte keeps the clock in its own `.rtc` file rather than appending it, and stores an
    /// origin rather than a reading, so the instant is what turns it into one. See
    /// [`parse_gambatte_sidecar`](crate::rtc::parse_gambatte_sidecar).
    pub rtc_sidecar: Option<(Vec<u8>, i64)>,
}

impl Default for RawOptions {
    fn default() -> Self {
        RawOptions {
            system: None,
            role: None,
            source: None,
            game: None,
            description: None,
            created_at: None,
            split_rtc: true,
            rtc_sidecar: None,
        }
    }
}

/// The extensions a flat save commonly arrives under, and what each usually is.
///
/// This is a hint for detection, not a rule. An emulator is free to write `.sav` for anything,
/// which is why `--from` and `--system` exist.
pub const EXTENSIONS: &[(&str, &str)] = &[
    ("srm", "cartridge save RAM, as libretro cores write it"),
    ("sav", "cartridge save memory"),
    ("eep", "EEPROM"),
    ("fla", "flash memory"),
    ("flash", "flash memory"),
    ("sa1", "SA-1 cartridge save RAM"),
    ("rtc", "real-time clock state"),
    ("bsv", "bsnes save RAM"),
];

/// Whether an extension is one a flat save commonly takes.
#[must_use]
pub fn is_raw_extension(extension: &str) -> bool {
    let lowered = extension.to_ascii_lowercase();
    EXTENSIONS.iter().any(|(known, _)| *known == lowered)
}

/// Wraps flat save bytes in a bundle.
pub fn wrap(bytes: &[u8], options: &RawOptions) -> Result<Bundle> {
    // A zero-byte payload is legal, but an emulator that wrote one has almost certainly written
    // a placeholder for a game that has not saved yet, and wrapping it would claim otherwise.
    if bytes.is_empty() {
        return Err(Error::NotConvertible("this save is zero bytes".into()));
    }

    let system = match &options.system {
        Some(text) => Some(parse_slug(text, "system")?),
        None => None,
    };
    let role = match &options.role {
        Some(text) => Some(parse_slug(text, "role")?),
        None => None,
    };

    // An emulator may have appended clock state, which is a live timestamp riding inside what
    // otherwise looks like save data. Splitting it out is what keeps the save's own bytes stable.
    let system_slug = system.as_ref().map(one_saves::Slug::as_str);
    let split = options.split_rtc.then(|| crate::rtc::split(bytes, system_slug)).flatten();

    let mut extensions = one_saves::Extensions::new();
    let mut parts = Vec::new();

    // A clock kept in its own file says the same thing as an appended one, so it lands in the
    // same keys. Only one of the two can be present: an emulator either appends or does not.
    if split.is_none()
        && let Some((sidecar, captured_at)) = &options.rtc_sidecar
    {
        let clock = crate::rtc::parse_gambatte_sidecar(sidecar, *captured_at).ok_or_else(|| {
            Error::NotConvertible("this sidecar is not a clock, or its instant is wrong".into())
        })?;
        let reading =
            crate::rtc::Reading { instant: clock.written_at, source_clock: crate::rtc::SOURCE_CLOCK_MBC3 };
        let sidecar_split = crate::rtc::Split {
            sram: Vec::new(),
            footer: Vec::new(),
            reading,
            clock: crate::rtc::Clock::Mbc3(clock),
        };
        extensions = sidecar_split.extensions();
    }

    // The clock rides in the header's extension keys rather than in a part of its own. One
    // cartridge keeps one clock, so it is said once for the bundle; splitting a dump into several
    // parts does not turn one reading into several.
    let payload = split.as_ref().map_or(bytes, |split| split.sram.as_slice());
    let mut save = Part::new(0, payload);
    save.role = role;
    parts.push(save);
    if let Some(split) = &split {
        extensions = split.extensions();
    }

    Ok(Bundle {
        header: Header {
            created_at: options.created_at,
            system,
            game: options.game.clone(),
            source: options.source.clone(),
            card: None,
            description: options.description.clone(),
            extensions,
            unknown: one_saves::UnknownKeys::new(),
        },
        parts,
    })
}

fn parse_slug(text: &str, field: &str) -> Result<Slug> {
    Slug::parse(text).map_err(|e| Error::NotConvertible(format!("{field} {text:?} is not a slug: {e}")))
}

/// Pulls the flat save bytes back out of a bundle.
///
/// A bundle holding one part is the common case and unambiguous. A bundle holding several is
/// only unwrappable if exactly one of them is a save; anything else needs a target named, which
/// is a decision for the caller rather than a guess to make here.
pub fn unwrap(bundle: &Bundle) -> Result<Vec<u8>> {
    let saves: Vec<&Part> =
        bundle.parts.iter().filter(|part| part.kind == one_saves::PartKind::Save).collect();

    let mut bytes = match saves.as_slice() {
        [] => return Err(Error::NotConvertible("this bundle holds no save parts".into())),
        [only] => only.bytes()?.into_owned(),
        many => {
            return Err(Error::NotConvertible(format!(
                "this bundle holds {} save parts; name one with --part",
                many.len()
            )));
        }
    };

    // A clock footer goes back on the end, so the emulator is handed the file it wrote rather
    // than a save it will read a few dozen bytes short.
    if let Some(footer) = crate::rtc::footer_of(bundle) {
        bytes.extend_from_slice(&footer);
    }
    Ok(bytes)
}

/// Pulls the save memory out without re-appending any clock footer.
///
/// For a producer that keeps its clock in a separate file, the footer would be a second copy of
/// something already written elsewhere.
pub fn unwrap_save_only(bundle: &Bundle) -> Result<Vec<u8>> {
    let saves: Vec<&Part> =
        bundle.parts.iter().filter(|part| part.kind == one_saves::PartKind::Save).collect();
    match saves.as_slice() {
        [only] => Ok(only.bytes()?.into_owned()),
        _ => unwrap(bundle),
    }
}

/// Pulls one named part's bytes out, by `id`.
pub fn unwrap_part(bundle: &Bundle, id: u64) -> Result<Vec<u8>> {
    let part = bundle
        .parts
        .iter()
        .find(|part| part.id == id)
        .ok_or_else(|| Error::NotConvertible(format!("this bundle has no part with id {id}")))?;
    Ok(part.bytes()?.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::profile;

    #[test]
    fn wraps_a_save_with_what_the_profile_knows() {
        let options = RawOptions {
            system: Some("gba".into()),
            source: Some(profile("mgba").unwrap().source(Some("0.10.3".into()))),
            ..RawOptions::default()
        };
        let bundle = wrap(&vec![7u8; 32768], &options).expect("wraps");

        assert_eq!(bundle.header.system.as_ref().unwrap().as_str(), "gba");
        let source = bundle.header.source.as_ref().unwrap();
        assert_eq!(source.app.as_ref().unwrap().as_str(), "mgba");
        assert_eq!(source.device_kind.as_ref().unwrap().as_str(), "emulator");
        assert_eq!(source.app_version.as_deref(), Some("0.10.3"));

        // The role stays absent, which means `primary`, and writing it out would be malformed.
        assert!(bundle.parts[0].role.is_none());
        bundle.validate().expect("valid bundle");
    }

    #[test]
    fn a_wrapped_save_round_trips_to_the_bytes_it_came_from() {
        let bytes: Vec<u8> = (0..8192u32).map(|i| (i % 251) as u8).collect();
        let bundle = wrap(&bytes, &RawOptions::default()).expect("wraps");
        // Through the encoded form, not just the in-memory model.
        let reread = Bundle::from_slice(&bundle.to_vec().unwrap()).unwrap();
        assert_eq!(unwrap(&reread).unwrap(), bytes);
    }

    #[test]
    fn refuses_to_guess_between_several_saves() {
        let mut bundle = wrap(&[1u8; 16], &RawOptions::default()).unwrap();
        let mut second = Part::new(1, [2u8; 16]);
        second.role = Some(Slug::parse("cartridge").unwrap());
        bundle.parts.push(second);

        assert!(unwrap(&bundle).is_err(), "two saves is ambiguous");
        assert_eq!(unwrap_part(&bundle, 1).unwrap(), vec![2u8; 16]);
    }

    #[test]
    fn a_sidecar_clock_lands_in_the_same_keys_as_an_appended_one() {
        // A consumer should not have to learn which emulator's filing convention a bundle came
        // from: Gambatte keeps its clock in a separate file, and it still arrives as a reading.
        let options = RawOptions {
            system: Some("gb".into()),
            rtc_sidecar: Some((vec![0x6a, 0x7c, 0xb7, 0x78], 1_786_558_622)),
            ..RawOptions::default()
        };
        let bundle = wrap(&vec![7u8; 32768], &options).expect("wraps");

        assert!(bundle.header.extensions.contains_key(&crate::rtc::rtc_key()));
        assert!(bundle.header.extensions.contains_key(&crate::rtc::mbc3_key()));
        // The save itself is untouched: a sidecar was never part of it.
        assert_eq!(bundle.parts.len(), 1);
        assert_eq!(bundle.parts[0].payload.len(), 32_768);
        bundle.validate().expect("valid bundle");
    }

    #[test]
    fn the_capture_instant_changes_the_reading_but_never_the_origin() {
        // The instant is a judgement the file cannot make for us, so what has to hold is that
        // getting it wrong costs the reading and not the bytes.
        let sidecar = vec![0x6a, 0x7c, 0xb7, 0x78];
        let mut origins = Vec::new();
        for captured_at in [1_786_558_622i64, 1_786_600_000, 1_800_000_000] {
            let options = RawOptions {
                system: Some("gb".into()),
                rtc_sidecar: Some((sidecar.clone(), captured_at)),
                ..RawOptions::default()
            };
            let bundle = wrap(&vec![7u8; 32768], &options).expect("wraps");
            origins.push(crate::rtc::sidecar_of(&bundle).expect("rebuilds"));
        }
        assert!(origins.iter().all(|o| *o == sidecar), "the origin is what round-trips");
    }

    #[test]
    fn a_sidecar_whose_instant_predates_it_is_refused() {
        // A base later than the capture instant means the instant is wrong, not that the
        // cartridge ran backwards, and inventing a reading from it would be worse than failing.
        let options = RawOptions {
            system: Some("gb".into()),
            rtc_sidecar: Some((vec![0x6a, 0x7c, 0xb7, 0x78], 1_000_000_000)),
            ..RawOptions::default()
        };
        assert!(wrap(&vec![7u8; 32768], &options).is_err());
    }

    #[test]
    fn refuses_an_empty_save() {
        assert!(wrap(&[], &RawOptions::default()).is_err());
    }

    /// A Game Boy save with a clock footer, as an emulator writes it.
    fn crystal_save(written_at: i64) -> Vec<u8> {
        let mut save: Vec<u8> = (0..32768u32).map(|i| (i % 251) as u8).collect();
        for value in [30u32, 45, 13, 44, 1, 30, 45, 13, 44, 1] {
            save.extend_from_slice(&value.to_le_bytes());
        }
        save.extend_from_slice(&written_at.to_le_bytes());
        save
    }

    #[test]
    fn a_clock_footer_is_split_off_and_read() {
        let options = RawOptions { system: Some("gb".into()), ..RawOptions::default() };
        let bundle = wrap(&crystal_save(1_700_000_000), &options).expect("wraps");

        // One part: the bare save memory. The clock is not a part, it is what the header says
        // about the medium these bytes came off.
        assert_eq!(bundle.parts.len(), 1);
        assert_eq!(bundle.parts[0].payload.len(), 32_768);

        // Both keys land on the header: the portable instant and the footer's own bytes.
        assert!(bundle.header.extensions.contains_key(&crate::rtc::rtc_key()));
        assert!(bundle.header.extensions.contains_key(&crate::rtc::mbc3_key()));
        bundle.validate().expect("valid bundle");
    }

    #[test]
    fn splitting_the_clock_out_makes_the_save_hash_stable() {
        // This is the whole point. Two dumps of one save taken at different moments differ only
        // in the footer's timestamp; left inline that moved the content hash, which is the value
        // a store deduplicates on.
        let options = RawOptions { system: Some("gb".into()), ..RawOptions::default() };
        let morning = wrap(&crystal_save(1_700_000_000), &options).expect("wraps");
        let evening = wrap(&crystal_save(1_700_043_200), &options).expect("wraps");

        assert_eq!(
            morning.parts[0].sha256, evening.parts[0].sha256,
            "the save memory is identical, so its digest must be"
        );
        assert_ne!(
            morning.content_hash().unwrap(),
            evening.content_hash().unwrap(),
            "the bundles still differ, because the clock genuinely did"
        );

        // And with the footer left inline, even the save part's digest moves.
        let inline = RawOptions { split_rtc: false, ..options };
        let a = wrap(&crystal_save(1_700_000_000), &inline).expect("wraps");
        let b = wrap(&crystal_save(1_700_043_200), &inline).expect("wraps");
        assert_ne!(a.parts[0].sha256, b.parts[0].sha256);
    }

    #[test]
    fn a_split_save_round_trips_to_the_file_the_emulator_wrote() {
        let options = RawOptions { system: Some("gb".into()), ..RawOptions::default() };
        let original = crystal_save(1_700_000_000);
        let bundle = wrap(&original, &options).expect("wraps");

        // Through the encoded form, so the footer survives being written and read back.
        let reread = Bundle::from_slice(&bundle.to_vec().unwrap()).unwrap();
        assert_eq!(unwrap(&reread).unwrap(), original, "the emulator gets its file back byte for byte");
    }

    #[test]
    fn a_save_with_no_footer_still_has_one_part() {
        let options = RawOptions { system: Some("gb".into()), ..RawOptions::default() };
        let plain = vec![7u8; 32768];
        let bundle = wrap(&plain, &options).expect("wraps");
        assert_eq!(bundle.parts.len(), 1);
        assert!(bundle.header.extensions.is_empty());
        assert_eq!(unwrap(&bundle).unwrap(), plain);
    }
}
