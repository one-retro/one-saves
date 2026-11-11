//! Flat cartridge saves: the `.srm`, `.sav`, `.eep` and `.fla` files most emulators write.
//!
//! There is nothing to parse. A flat save is the cartridge's save memory as a run of bytes, and
//! wrapping it is a matter of saying what it belongs to — which is exactly what the container is
//! for, and exactly what a bare file cannot say.
//!
//! One file here does say. A Sega CD backup RAM ends in a volume footer whose second half never
//! varies, and that names the system as plainly as a ROM header would, so it settles `system` when
//! the caller did not. Its filesystem stays unread — a `.brm` becomes one part holding the whole
//! volume — which is why it is a flat save here and not a card.
//!
//! The role stays absent, meaning `primary`. A great many systems have exactly one place a save
//! can live, and the registry's advice is to prefer a Common role where one fits rather than
//! reaching for `cartridge` because the medium happens to be a cartridge.
//!
//! A Sega CD is the exception to that too: it carries internal backup RAM and a Backup RAM Cart at
//! the same time, so which of `internal` and `ram-cart` a dump came out of is worth stating rather
//! than letting it claim to be the only socket.

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
    pub bundled_at: Option<i64>,
    /// Whether to split an appended real-time-clock footer off the save.
    ///
    /// On by default. Leaving it inline keeps the payload byte-identical to the emulator's file,
    /// at the cost of a content hash that moves every time the clock does; see [`crate::rtc`].
    pub split_rtc: bool,
    /// A sidecar clock file written beside the save, and the instant to read it at.
    ///
    /// Every libretro core that keeps a clock has its frontend write it to a `.rtc` file of its
    /// own rather than appending it, and so does mincer_ray's GBA core. What the file holds is the
    /// core's own record, read as the registry says the producer `source` names keeps it
    /// ([`sidecar_record_of`](crate::rtc::sidecar_record_of)), or by its length where that does
    /// not say ([`parse_sidecar`](crate::rtc::parse_sidecar)). Gambatte's stores an origin rather
    /// than a reading, so the instant is what turns it into one, and is ordinarily now. Every other
    /// record holds its own and ignores the instant.
    ///
    /// [`clock_chip`](Self::clock_chip) says which chip's clock the file is read as. A producer
    /// the registry says keeps no clock of that chip has nothing of it in the file, so the bundle
    /// gets no clock from it: SameBoy's libretro core writes an empty MBC3 section beside a HuC-3
    /// game's save.
    pub rtc_sidecar: Option<(Vec<u8>, i64)>,
    /// The key of the clock chip the cartridge carries, as its ROM's header says, such as
    /// `x.1sav.rtc.huc3`.
    ///
    /// What tells apart two chips a producer keeps in the same footer: the MiSTer Game Boy cores
    /// pack a HuC-3 where they pack an MBC3. See [`split_for`](crate::rtc::split_for).
    pub clock_chip: Option<String>,
    /// The instant to read an appended clock that holds an origin at, ordinarily now: the
    /// VisualBoyAdvance-M app's HuC-3 counts from one, as Gambatte's sidecar does.
    pub captured_at: Option<i64>,
    /// What the cartridge keeps its saves in, as its ROM's header says, which is what tells a Mega
    /// Drive file's SRAM from its EEPROM and a GBA file's chip from its padding. See
    /// [`layout`](crate::layout).
    pub memory: Option<crate::layout::SaveMemory>,
}

impl Default for RawOptions {
    fn default() -> Self {
        RawOptions {
            system: None,
            role: None,
            source: None,
            game: None,
            description: None,
            bundled_at: None,
            split_rtc: true,
            rtc_sidecar: None,
            clock_chip: None,
            captured_at: None,
            memory: None,
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
    ("sra", "N64 cartridge SRAM, as mupen64plus and Project64 name it"),
    ("fla", "flash memory"),
    ("flash", "flash memory"),
    ("sa1", "SA-1 cartridge save RAM"),
    ("brm", "Sega CD backup RAM, internal or Backup RAM Cart"),
    ("rtc", "real-time clock state"),
    ("bsv", "bsnes save RAM"),
];

/// Whether an extension is one a flat save commonly takes.
#[must_use]
pub fn is_raw_extension(extension: &str) -> bool {
    let lowered = extension.to_ascii_lowercase();
    EXTENSIONS.iter().any(|(known, _)| *known == lowered)
}

/// The system a backup RAM belongs to.
const SEGA_CD: &str = "sega-cd";

/// The half of a Sega CD backup RAM's volume footer that never varies.
///
/// The last 0x40 bytes of a volume are its footer, and only the first half of that moves: the
/// volume name, the free-block count and the file count all change as saves come and go. These 32
/// bytes do not, on the console's internal 8 KiB and on a Backup RAM Cart alike, which is what
/// makes them a signature rather than a guess.
const SEGA_CD_BRAM_FOOTER: &[u8] = b"SEGA_CD_ROM\0\x01\0\0\0RAM_CARTRIDGE___";

/// How big a block is on a backup RAM. A volume is a whole number of them.
const SEGA_CD_BRAM_BLOCK: usize = 0x40;

/// The smallest volume there is: the console's internal backup RAM, which is always this.
///
/// Every Backup RAM Cart is this size or larger, so nothing shorter is a volume — and requiring
/// it keeps a short file that happens to end the right way from being read as one.
const SEGA_CD_BRAM_MIN: usize = 8192;

/// Whether these bytes are a Sega CD backup RAM.
///
/// The footer is checked at the end of the volume, so a dump padded past it — as some producers
/// write one — reads as an ordinary flat save rather than being trimmed to fit. Cutting bytes off
/// a save is not something to do on a guess.
#[must_use]
pub fn is_segacd_bram(bytes: &[u8]) -> bool {
    bytes.len() >= SEGA_CD_BRAM_MIN
        && bytes.len().is_multiple_of(SEGA_CD_BRAM_BLOCK)
        && bytes.ends_with(SEGA_CD_BRAM_FOOTER)
}

/// Wraps flat save bytes in a bundle.
pub fn wrap(bytes: &[u8], options: &RawOptions) -> Result<Bundle> {
    // A zero-byte payload is legal, but an emulator that wrote one has almost certainly written
    // a placeholder for a game that has not saved yet, and wrapping it would claim otherwise.
    if bytes.is_empty() {
        return Err(Error::NotConvertible("this save is zero bytes".into()));
    }

    let system = match &options.system {
        // What the caller says is a statement and keeps the last word, whatever the bytes hold.
        Some(text) => Some(parse_slug(text, "system")?),
        None => is_segacd_bram(bytes).then(|| slug(SEGA_CD)),
    };
    let role = match &options.role {
        Some(text) => Some(parse_slug(text, "role")?),
        None => None,
    };

    // An emulator may have appended clock state, which is a live timestamp riding inside what
    // otherwise looks like save data. Splitting it out is what keeps the save's own bytes stable.
    let system_slug = system.as_ref().map(one_saves::Slug::as_str);
    let chip = options.clock_chip.as_deref();
    let captured_at = options.captured_at.unwrap_or_else(crate::rtc::now);
    let split =
        options.split_rtc.then(|| crate::rtc::split_at(bytes, system_slug, chip, captured_at)).flatten();

    let mut extensions = one_saves::Extensions::new();
    let mut parts = Vec::new();

    // A clock kept in its own file says the same thing as an appended one, so it lands in the
    // same keys. Only one of the two can be present: an emulator either appends or does not.
    if split.is_none()
        && let Some((sidecar, captured_at)) = &options.rtc_sidecar
        && let Some(clock) = read_sidecar(sidecar, *captured_at, options)?
    {
        let sidecar_split = crate::rtc::Split {
            sram: Vec::new(),
            footer: Vec::new(),
            // From an origin, the time the clock has run since it, never the origin itself.
            reading: clock.reading(),
            clock,
        };
        extensions = sidecar_split.extensions();
    }

    // The clock rides in the header's extension keys rather than in a part of its own. One
    // cartridge keeps one clock, so it is said once for the bundle; splitting a dump into several
    // parts does not turn one reading into several.
    let file = split.as_ref().map_or(bytes, |split| split.sram.as_slice());
    // The memory alone, out of the file the core keeps it in.
    let payload = match system_slug {
        Some(system) => {
            crate::layout::normalize(file, system, layout_of(options.source.as_ref(), system), options.memory)
        }
        None => file.to_vec(),
    };
    parts.push(Part::new(0, payload).role(role));
    if let Some(split) = &split {
        extensions = split.extensions();
    }

    // A raw dump is one game's in-game save: the parts are the regions that save is made of.
    let mut header = Header::new(one_saves::Shape::Save)
        .bundled_at(options.bundled_at)
        .system(system)
        .game(options.game.clone())
        .source(options.source.clone())
        .extensions(extensions);
    header.description.clone_from(&options.description);
    Ok(Bundle::new(header, parts))
}

/// How the producer a `source` names keeps `system`'s save memory in its file, as the registry
/// records it for the core in the app that ran it.
fn layout_of(source: Option<&Source>, system: &str) -> Option<one_saves_registry::SaveLayout> {
    let source = source?;
    let core = one_saves_registry::core(source.core.as_ref()?.as_str())?;
    let app = source.app.as_ref().and_then(|app| one_saves_registry::app(app.as_str()));
    core.layout_in(app, system)
}

/// Parses a slug the specification defines, which is always well-formed.
fn slug(text: &str) -> Slug {
    Slug::parse(text).expect("a spec slug is well-formed")
}

/// Reads a sidecar clock file as the producer `options.source` names keeps it, or `None` where that
/// producer keeps no clock of the cartridge's chip in it.
///
/// Its record is the registry's answer for the core in the app that ran it. A file that is not
/// that record is read by its length, which is what a source naming the core alone needs: the
/// registry's Gambatte is the libretro core's, eight little-endian bytes, and OpenEmu's build of it
/// writes four big-endian ones.
///
/// The cartridge's chip, where its ROM says, decides which chip the file is read as. Gambatte keeps
/// a HuC-3's origin in a file the same length as an MBC3's, and nothing else tells them apart. A
/// producer known to keep no clock of that chip has nothing of it in the file: SameBoy's libretro
/// core writes its MBC3 section beside a HuC-3 game's save with nothing in it. Without the chip, the
/// file is read as an MBC3's, the commonest, and a clock read as another chip than the cartridge's
/// is refused rather than filed under the wrong key.
fn read_sidecar(bytes: &[u8], captured_at: i64, options: &RawOptions) -> Result<Option<crate::rtc::Clock>> {
    use one_saves::Extension as _;
    use one_saves::extension::{RtcHuc3, RtcMbc3, RtcS3511a};
    let not_a_clock = || Error::NotConvertible("this sidecar is not a clock, or its instant is wrong".into());
    let chip = options.clock_chip.as_deref();
    if chip == Some(RtcHuc3::KEY.as_str()) {
        let layout = crate::rtc::huc3_layout_of(options.source.as_ref());
        if layout == Some(crate::rtc::Huc3Layout::NotKept) {
            return Ok(None);
        }
        return crate::rtc::parse_huc3_sidecar(bytes, layout, captured_at)
            .or_else(|| crate::rtc::parse_huc3_sidecar(bytes, None, captured_at))
            .map(|clock| Some(crate::rtc::Clock::Huc3(clock)))
            .ok_or_else(not_a_clock);
    }

    let record = crate::rtc::sidecar_record_of(options.source.as_ref());
    let clock = record
        .and_then(|record| crate::rtc::parse_sidecar_as(bytes, record, captured_at))
        .or_else(|| crate::rtc::parse_sidecar(bytes, captured_at))
        .ok_or_else(not_a_clock)?;
    let key = match clock {
        crate::rtc::Clock::Mbc3(_) => RtcMbc3::KEY,
        crate::rtc::Clock::S3511a(_) => RtcS3511a::KEY,
        crate::rtc::Clock::Huc3(_) => RtcHuc3::KEY,
    };
    match chip {
        Some(chip) if chip != key.as_str() => Err(Error::NotConvertible(format!(
            "the cartridge's clock is {chip}, and this sidecar was read as {key}, so it is not this cartridge's"
        ))),
        _ => Ok(Some(clock)),
    }
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
    // The file its producer wrote: the memory as that core keeps it, a clock footer back on the
    // end. A clock it keeps in a file of its own is for [`unwrap_as`] to hand back.
    Ok(unwrap_as(bundle, &Reader::default())?.save)
}

/// The one save part's bytes, refusing a bundle with none or several.
fn save_part(bundle: &Bundle) -> Result<Vec<u8>> {
    let saves: Vec<&Part> =
        bundle.parts.iter().filter(|part| part.kind == one_saves::PartKind::Save).collect();

    let bytes = match saves.as_slice() {
        [] => return Err(Error::NotConvertible("this bundle holds no save parts".into())),
        [only] => only.bytes()?.into_owned(),
        many => {
            return Err(Error::NotConvertible(format!(
                "this bundle holds {} save parts; name one with --part",
                many.len()
            )));
        }
    };

    Ok(bytes)
}

/// Pulls the save memory out without re-appending any clock footer.
///
/// For a producer that keeps its clock in a separate file, the footer would be a second copy of
/// something already written elsewhere.
pub fn unwrap_save_only(bundle: &Bundle) -> Result<Vec<u8>> {
    save_part(bundle)
}

/// Who a save is being written for, and what is known that writing it takes.
///
/// The default is the bundle's own producer: the file it wrote, laid out as it keeps it, its
/// clock where it keeps one and on the scale it stamps.
#[derive(Debug, Clone, Copy, Default)]
pub struct Reader {
    /// The core that will read the save, and the app running it; `None` for the bundle's own.
    pub core: Option<(&'static one_saves_registry::Core, Option<&'static one_saves_registry::App>)>,
    /// How far ahead of UTC the reader's clock runs, for a reader that stamps local time, such as a
    /// MiSTer: what its own bundles would carry as `utc_offset`. Without it a MiSTer is taken to
    /// run on the bundle's own scale.
    pub utc_offset: Option<i32>,
    /// What the cartridge keeps its saves in, as its ROM's header says.
    pub memory: Option<crate::layout::SaveMemory>,
}

/// A save written for a [`Reader`]: the file, the clock file to write beside it, and what was
/// assumed along the way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unwrapped {
    /// The save's file, its clock on the end where the reader appends one.
    pub save: Vec<u8>,
    /// How much of [`save`](Self::save) is the save memory, the rest being the clock appended to it.
    pub memory_len: usize,
    /// The clock file to write beside it, where the reader keeps one there.
    pub sidecar: Option<Vec<u8>>,
    /// The shape the clock went in.
    pub form: crate::rtc::ClockForm,
    /// How far the clock's anchor moved, from the bundle's scale to the reader's.
    pub rescaled_by: i64,
    /// Whether the bundle's clock was stamped on, or is going to, a local time nobody named, so it
    /// could not be put on the reader's scale and may be off by the zone's offset.
    pub scale_unknown: bool,
    /// Whether what the cartridge keeps was assumed in laying the save out: a Mega Drive save
    /// spread out for Genesis Plus GX or PicoDrive with no [`Reader::memory`].
    pub memory_assumed: bool,
}

/// Writes a bundle's save for a reader: the memory laid out as the reader keeps it, the clock in
/// the reader's shape and on its scale. See [`Reader`].
pub fn unwrap_as(bundle: &Bundle, reader: &Reader) -> Result<Unwrapped> {
    use crate::rtc::{footer_of_form, form_in, form_of, scale_of, sidecar_of_form, stamps_local_time};
    let source = producer(bundle.header.source.as_ref());
    let (core, app) = match reader.core {
        Some((core, app)) => (Some(core), app),
        None => (source.map(|(core, _)| core), source.and_then(|(_, app)| app)),
    };
    let has_clock = crate::rtc::footer_of(bundle).is_some() || crate::rtc::sidecar_of(bundle).is_some();

    // The clock's anchor, from the bundle's scale to the reader's.
    let from = scale_of(bundle);
    let local = |core: Option<&one_saves_registry::Core>| core.is_some_and(stamps_local_time);
    let (rescaled_by, scale_unknown) = match (local(core), reader.utc_offset, from) {
        // Another of the same kind, not told otherwise, runs on the bundle's own scale.
        (true, None, _) if local(source.map(|(core, _)| core)) => (0, false),
        (true, Some(to), Some(from)) => (i64::from(to) - i64::from(from), false),
        (false, _, Some(from)) => (-i64::from(from), false),
        (true, _, _) | (false, _, None) => (0, has_clock),
    };
    let mut bundle = std::borrow::Cow::Borrowed(bundle);
    if rescaled_by != 0 {
        crate::rtc::rescale(bundle.to_mut(), rescaled_by);
    }
    let form = match (reader.core, core) {
        (Some(_), Some(core)) => form_in(&bundle, core, app),
        _ => form_of(&bundle),
    };

    // The memory, out of whatever shape a bundle from before layouts kept it in, and into the
    // reader's.
    let memory = save_part(&bundle)?;
    let laid = match bundle.header.system.as_ref().map(one_saves::Slug::as_str) {
        Some(system) => {
            let normal = crate::layout::normalize_stored(
                &memory,
                system,
                layout_of(bundle.header.source.as_ref(), system),
                reader.memory,
            );
            let layout = core.and_then(|core| core.layout_in(app, system));
            crate::layout::lay_out(&normal, system, layout, reader.memory)
        }
        None => crate::layout::Laid { bytes: memory, assumed: false },
    };

    let mut save = laid.bytes;
    let memory_len = save.len();
    if let Some(footer) = footer_of_form(&bundle, form) {
        save.extend_from_slice(&footer);
    }
    Ok(Unwrapped {
        save,
        memory_len,
        sidecar: sidecar_of_form(&bundle, form),
        form,
        rescaled_by,
        scale_unknown,
        memory_assumed: laid.assumed,
    })
}

/// The core and app a `source` names, where the registry lists them.
fn producer(
    source: Option<&Source>,
) -> Option<(&'static one_saves_registry::Core, Option<&'static one_saves_registry::App>)> {
    let source = source?;
    let core = one_saves_registry::core(source.core.as_ref()?.as_str())?;
    Some((core, source.app.as_ref().and_then(|app| one_saves_registry::app(app.as_str()))))
}

/// Pulls the flat save bytes out laid out for a reader whose clock shape is `form`: the save, with
/// its clock appended where the reader appends one, and the clock file to write beside it where
/// the reader keeps one there.
///
/// [`rtc::form_for`](crate::rtc::form_for) gives the form, from the reader's registry entry.
pub fn unwrap_for(bundle: &Bundle, form: crate::rtc::ClockForm) -> Result<(Vec<u8>, Option<Vec<u8>>)> {
    let mut bytes = unwrap_save_only(bundle)?;
    if let Some(footer) = crate::rtc::footer_of_form(bundle, form) {
        bytes.extend_from_slice(&footer);
    }
    Ok((bytes, crate::rtc::sidecar_of_form(bundle, form)))
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
            source: Some(profile("mgba").unwrap().source().core_version("0.10.3")),
            ..RawOptions::default()
        };
        let bundle = wrap(&vec![7u8; 32768], &options).expect("wraps");

        assert_eq!(bundle.header.system.as_ref().unwrap().as_str(), "gba");
        let source = bundle.header.source.as_ref().unwrap();
        assert_eq!(source.core.as_ref().unwrap().as_str(), "mgba");
        assert_eq!(source.device_kind.as_ref().unwrap().as_str(), "emulator");
        assert_eq!(source.core_version.as_deref(), Some("0.10.3"));

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
        bundle.parts.push(Part::new(1, [2u8; 16]).role(Slug::parse("cartridge").unwrap()));

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

        assert!(bundle.header.extensions.get::<one_saves::extension::Rtc>().is_some());
        assert!(bundle.header.extensions.get::<one_saves::extension::RtcMbc3>().is_some());
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
                source: Some(openemu_gambatte()),
                rtc_sidecar: Some((sidecar.clone(), captured_at)),
                ..RawOptions::default()
            };
            let bundle = wrap(&vec![7u8; 32768], &options).expect("wraps");
            origins.push(crate::rtc::sidecar_of(&bundle).expect("rebuilds"));
        }
        assert!(origins.iter().all(|o| *o == sidecar), "the origin is what round-trips");
    }

    #[test]
    fn a_gba_sidecar_lands_in_the_seiko_keys_and_goes_back_beside_the_save() {
        // mincer_ray's `.rtc`: the instant it records and the calendar packed beside it. The
        // instant passed in is ignored, since the record does not need one.
        let record = vec![0xdf, 0xaf, 0x7c, 0x6a, 0xc3, 0x1c, 0x34, 0x09, 0x99, 0, 0, 0, 0, 0, 0, 0];
        let options = RawOptions {
            system: Some("gba".into()),
            source: Some(Source::default().core(one_saves::Name::parse("mincer-ray-gba").ok())),
            rtc_sidecar: Some((record.clone(), 0)),
            ..RawOptions::default()
        };
        let bundle = wrap(&vec![7u8; 131_072], &options).expect("wraps");

        let extensions = &bundle.header.extensions;
        // The calendar is the reading; the instant the record holds is the anchor, kept as key 2.
        assert_eq!(extensions.get::<one_saves::extension::Rtc>().expect("a reading").reading, 1_786_531_183);
        let chip = extensions.get::<one_saves::extension::RtcS3511a>().expect("the chip");
        assert_eq!(chip.taken_at, Some(1_786_556_383));
        bundle.validate().expect("valid bundle");

        // The save goes back the size a cartridge's is, and the clock to its own file.
        assert_eq!(unwrap(&bundle).expect("unwraps").len(), 131_072);
        assert_eq!(crate::rtc::sidecar_of(&bundle), Some(record));
    }

    /// Gambatte as OpenEmu runs it, whose `.rtc` is four big-endian bytes.
    fn openemu_gambatte() -> Source {
        Source::default()
            .app(one_saves::Name::parse("openemu").ok())
            .core(one_saves::Name::parse("gambatte").ok())
    }

    /// A core as RetroArch runs it.
    fn retroarch(core: &str) -> Source {
        Source::default()
            .app(one_saves::Name::parse("retroarch").ok())
            .core(one_saves::Name::parse(core).ok())
    }

    /// The `.rtc` mGBA's libretro core had RetroArch write: one second in, latched, then the host
    /// clock.
    fn mgba_record() -> Vec<u8> {
        let mut record = Vec::new();
        for value in [1u32, 0, 0, 0, 0, 1, 0, 0, 0, 0] {
            record.extend_from_slice(&value.to_le_bytes());
        }
        record.extend_from_slice(&1_793_777_489i64.to_le_bytes());
        record
    }

    #[test]
    fn a_libretro_cores_clock_comes_from_beside_the_save_and_goes_back_there() {
        // RetroArch writes mGBA's save memory to the `.srm` and its clock to the `.rtc`, so the
        // save is the bare memory both ways and the record comes back as it was.
        let options = RawOptions {
            system: Some("gbc".into()),
            source: Some(retroarch("mgba")),
            rtc_sidecar: Some((mgba_record(), 0)),
            ..RawOptions::default()
        };
        let save = vec![7u8; 32_768];
        let bundle = wrap(&save, &options).expect("wraps");
        let chip = bundle.header.extensions.get::<one_saves::extension::RtcMbc3>().expect("the chip");
        assert_eq!((chip.live, chip.taken_at), ([1, 0, 0, 0, 0], 1_793_777_489));
        assert_eq!(chip.timestamp_width, None, "no footer, so no width");

        assert_eq!(unwrap(&bundle).expect("unwraps"), save, "nothing appended");
        assert_eq!(crate::rtc::sidecar_of(&bundle), Some(mgba_record()));

        // Laid out for the mGBA app instead, the same record goes on the end of the save.
        let layout = one_saves_registry::cores::MGBA.clock_in(Some(&one_saves_registry::apps::MGBA));
        let (file, beside) = unwrap_for(&bundle, crate::rtc::form_for(&bundle, layout)).expect("unwraps");
        assert_eq!((file.len(), &file[32_768..], beside), (32_816, &mgba_record()[..], None));
    }

    #[test]
    fn a_sidecar_that_is_not_the_producers_record_is_read_by_its_length() {
        // `--from gambatte` names the core alone, whose libretro record is eight little-endian
        // bytes; an OpenEmu file under that name is four big-endian ones, and still reads as one.
        let options = RawOptions {
            system: Some("gb".into()),
            source: Some(Source::default().core(one_saves::Name::parse("gambatte").ok())),
            rtc_sidecar: Some((vec![0x6a, 0x7c, 0xb7, 0x78], 1_786_558_622)),
            ..RawOptions::default()
        };
        let bundle = wrap(&vec![7u8; 32768], &options).expect("wraps");
        let reading = bundle.header.extensions.get::<one_saves::extension::Rtc>().expect("a reading");
        assert_eq!(reading.reading, 294, "four minutes and 54 seconds, as OpenEmu's file says");
    }

    #[test]
    fn a_sidecar_is_read_as_the_cartridges_chip() {
        use one_saves::extension::{RtcHuc3, RtcMbc3};
        // Gambatte keeps a HuC-3's origin in a file the length of an MBC3's, so only the ROM says
        // which it is: three minutes in, either way.
        let origin = 1_793_777_493i64.to_le_bytes().to_vec();
        let options = |chip: Option<&str>, source: Source, sidecar: Vec<u8>| RawOptions {
            system: Some("gbc".into()),
            source: Some(source),
            rtc_sidecar: Some((sidecar, 1_793_777_493 + 180)),
            clock_chip: chip.map(Into::into),
            ..RawOptions::default()
        };
        let huc3 =
            wrap(&vec![7u8; 32768], &options(Some("x.1sav.rtc.huc3"), retroarch("gambatte"), origin.clone()))
                .expect("wraps");
        let chip = huc3.header.extensions.get::<RtcHuc3>().expect("a HuC-3");
        assert_eq!((chip.minutes(), chip.days()), (Some(3), Some(0)));
        assert!(huc3.header.extensions.get::<RtcMbc3>().is_none());
        // And with no word from the ROM, as the commonest chip.
        let mbc3 =
            wrap(&vec![7u8; 32768], &options(None, retroarch("gambatte"), origin.clone())).expect("wraps");
        assert!(mbc3.header.extensions.get::<RtcMbc3>().is_some());

        // SameBoy's libretro core keeps no HuC-3: the file beside such a save is its MBC3 section
        // with nothing in it, so there is no clock to read, and none is made up.
        let mut sameboy = vec![0u8; 32];
        sameboy[16..24].copy_from_slice(&1_793_777_496i64.to_le_bytes());
        let none = wrap(&vec![7u8; 32768], &options(Some("x.1sav.rtc.huc3"), retroarch("sameboy"), sameboy))
            .expect("wraps");
        assert!(none.header.extensions.is_empty(), "no clock at all");

        // A file that reads as some other chip than the cartridge's is refused.
        let seiko = vec![0xdf, 0xaf, 0x7c, 0x6a, 0xc3, 0x1c, 0x34, 0x09, 0x99, 0, 0, 0, 0, 0, 0, 0];
        let source = Source::default().core(one_saves::Name::parse("mincer-ray-gba").ok());
        assert!(wrap(&vec![7u8; 32768], &options(Some("x.1sav.rtc.mbc3"), source, seiko)).is_err());
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

    /// A Sega CD backup RAM: blocks of 0x40 with the volume footer at the end of the last one.
    fn backup_ram(size: usize) -> Vec<u8> {
        let footer = b"SEGA_CD_ROM\0\x01\0\0\0RAM_CARTRIDGE___";
        let mut bram: Vec<u8> = (0..size).map(|i| u8::try_from(i % 251).expect("under 251")).collect();
        bram[size - footer.len()..].copy_from_slice(footer);
        bram
    }

    #[test]
    fn a_backup_ram_says_which_system_it_belongs_to() {
        // The one flat save that identifies itself. Nothing was passed here: no `--system`, no
        // `--from`, no ROM — and the bundle still knows what it is.
        let bundle = wrap(&backup_ram(8192), &RawOptions::default()).expect("wraps");
        assert_eq!(bundle.header.system.as_ref().unwrap().as_str(), "sega-cd");
        bundle.validate().expect("valid bundle");
    }

    #[test]
    fn what_the_caller_says_beats_what_the_bytes_say() {
        // The signature fills a gap; it does not overrule a statement.
        let options = RawOptions { system: Some("genesis".into()), ..RawOptions::default() };
        let bundle = wrap(&backup_ram(8192), &options).expect("wraps");
        assert_eq!(bundle.header.system.as_ref().unwrap().as_str(), "genesis");
    }

    #[test]
    fn a_backup_ram_is_wrapped_whole_and_comes_back_byte_for_byte() {
        // The filesystem is not read, so the volume is one part and nothing in it is rearranged.
        let bram = backup_ram(524_288);
        let bundle = wrap(&bram, &RawOptions::default()).expect("wraps");
        assert_eq!(bundle.parts.len(), 1);
        assert!(bundle.header.extensions.is_empty(), "a backup RAM carries no clock");

        let reread = Bundle::from_slice(&bundle.to_vec().unwrap()).unwrap();
        assert_eq!(unwrap(&reread).unwrap(), bram);
    }

    #[test]
    fn a_save_that_is_not_a_backup_ram_is_left_unnamed() {
        // Nothing else says what system it is for, and guessing would be worse than an absence.
        assert!(wrap(&[7u8; 8192], &RawOptions::default()).unwrap().header.system.is_none());
        // Nor is a run of bytes that merely ends the right way: a volume is whole blocks.
        let mut ragged = backup_ram(8192);
        ragged.insert(0, 0);
        assert!(wrap(&ragged, &RawOptions::default()).unwrap().header.system.is_none());
        // Nor a short one. The console's own backup RAM is the smallest volume there is, so a
        // file under 8 KiB carrying the footer is something else that happens to end that way.
        assert!(!is_segacd_bram(&backup_ram(4096)));
        assert!(wrap(&backup_ram(4096), &RawOptions::default()).unwrap().header.system.is_none());
    }

    #[test]
    fn refuses_an_empty_save() {
        assert!(wrap(&[], &RawOptions::default()).is_err());
    }

    /// A Game Boy save with a clock footer, as an emulator writes it.
    fn crystal_save(taken_at: i64) -> Vec<u8> {
        let mut save: Vec<u8> = (0..32768u32).map(|i| (i % 251) as u8).collect();
        for value in [30u32, 45, 13, 44, 1, 30, 45, 13, 44, 1] {
            save.extend_from_slice(&value.to_le_bytes());
        }
        save.extend_from_slice(&taken_at.to_le_bytes());
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
        assert!(bundle.header.extensions.get::<one_saves::extension::Rtc>().is_some());
        assert!(bundle.header.extensions.get::<one_saves::extension::RtcMbc3>().is_some());
        bundle.validate().expect("valid bundle");
    }

    #[test]
    fn a_huc3_clock_is_split_off_when_the_rom_names_the_chip() {
        // `huc3.v`'s packing: 3000 days, minute 825, 37 seconds, behind a four-byte host clock.
        let (seconds, minutes, days) = (37u64, 825u64, 3000u64);
        let packed = seconds | minutes << 6 | days << 18;
        let mut save = vec![0x33u8; 32_768];
        save.extend_from_slice(&1_786_533_688u32.to_le_bytes());
        save.extend_from_slice(&packed.to_le_bytes()[..6]);
        save.resize(32_768 + crate::rtc::PACKED_LEN_MISTER, 0xFF);

        let options = RawOptions {
            system: Some("gb".into()),
            clock_chip: Some("x.1sav.rtc.huc3".into()),
            ..RawOptions::default()
        };
        let bundle = wrap(&save, &options).expect("wraps");
        assert_eq!(bundle.parts[0].payload.len(), 32_768);
        let clock = bundle.header.extensions.get::<one_saves::extension::RtcHuc3>().expect("the HuC-3");
        assert_eq!(clock.seconds, Some(37));
        bundle.validate().expect("valid bundle");
        assert_eq!(unwrap(&bundle).expect("unwraps"), save, "and back as the core wrote it");
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
