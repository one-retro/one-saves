//! Real-time clock state that emulators append to a save file.
//!
//! A cartridge with a clock chip — Pokémon Crystal, Pokémon Ruby and friends — has state the save
//! memory does not hold, and emulators disagree about where to put it. Three arrangements are
//! handled here, all confirmed against files real emulators wrote:
//!
//! | producer | where | what it holds |
//! | -------- | ----- | ------------- |
//! | mGBA, GBA | appended, 16 bytes | latched BCD, a control register, a latch instant |
//! | SameBoy, Game Boy | appended, 48 bytes | ten MBC3 registers and a write time |
//! | MiSTer Gameboy core | appended, 512 bytes | a write time and the registers **bit-packed** |
//! | its openFPGA ports | appended, 16 bytes | the same ten bytes, a smaller reserved region |
//! | Gambatte, Game Boy | a `.rtc` file, 4 bytes | an **origin**, big-endian |
//! | mincer_ray, GBA | a `.rtc` file, 16 bytes | an instant and the calendar **bit-packed** |
//! | GBA_MiSTer, GBA | appended, 512 bytes | the same two, the other way round, **repeated** |
//!
//! Two of the appended forms are sixteen bytes and two are 512, and each pair means entirely
//! different things by them, so the system is what tells them apart. Which shape a clock goes back into is decided by
//! [`form_of`] from the bundle's `source`, since the encoding belongs to the producer rather than
//! to the cartridge.
//!
//! The appended forms say nothing about where the save memory ends, so they are found by length.
//! The two sidecars are told apart by length too, and Gambatte's is a different problem: it stores
//! the instant the clock counts *from*, which is the other operand of a reading rather than a
//! reading, and turning one into the other needs an instant the file does not record. See
//! [`parse_gambatte_sidecar`]. mincer_ray's records its own instant; see [`parse_mincer_sidecar`].
//!
//! Left inline, that footer is a problem rather than a curiosity: it carries a **live timestamp**,
//! so two dumps of the same save minutes apart hash differently, and a
//! [content hash](one_saves::Bundle::content_hash) that moves on its own is one a store cannot
//! deduplicate on. Splitting it out is what makes a save's bytes stable.
//!
//! The shape follows what [`x.1sav.rtc`] prescribes, which is to carry two keys rather than one:
//!
//! - the save part's payload is the bare save memory,
//! - the portable reading goes in [`x.1sav.rtc`] as an [`Rtc`], normalized to a Unix instant, so a
//!   consumer can read it without knowing which chip produced it,
//! - the chip's own state goes in its key under that one, as an [`RtcMbc3`] or an [`RtcS3511a`],
//!   read out as fields rather than kept as a blob, because that is the state only something that
//!   understands the layout can rebuild.
//!
//! > a producer that wants byte-exact round-tripping carries both: this key for the instant
//! > anyone can read, and its own for the state only it can rebuild.
//!
//! Neither goes on a part here. The specification is explicit that **the header is the ordinary
//! answer** — one cartridge keeps one clock, so a producer that splits a dump into several parts
//! sets the key once and leaves the parts alone — and that the key **MUST NOT** sit on a `bundle`
//! part at all, since a nested bundle inherits nothing and its clock belongs in its own header.
//!
//! # Why this chip has its own key
//!
//! The specification carries one portable key and one key per chip, because the chips do not
//! share a shape. [`x.1sav.rtc.s3511a`] is the GBA's Seiko S-3511A: seven BCD bytes of year,
//! month, day, weekday, hour, minute and second, plus a control register.
//!
//! An MBC3 counts rather than dates. It has seconds, minutes, hours, a 9-bit day counter, a halt
//! flag and a carry flag, plus the latched copy of all five that the game actually reads. There
//! is no year, month or weekday to put in the S-3511A's `components`, and no field there for the
//! day counter, the flags or the latched set.
//!
//! Neither belongs to an emulator. The MBC3 footer is written identically by VBA, BGB, Gambatte
//! and mGBA, which is what makes it readable at all, so both keys sit under `x.1sav.rtc` in the
//! [`x` tree](https://docs.1retro.com/specifications/common-types/reverse-dns-name/#the-x-tree)
//! rather than under a vendor domain. What is left under [`io.mgba.rtc`] is only mGBA's
//! *reconstruction model* — how it runs the clock between reads — which is genuinely one
//! emulator's and which this crate does not write.
//!
//! # What is not read
//!
//! A trailing block that matches no layout here is left in the payload rather than split off.
//! Splitting would stabilise the save's bytes, but nothing would say the bytes are a clock, and
//! filing unidentified bytes under a chip's key claims more than is known.
//!
//! [`x.1sav.rtc`]: https://docs.1retro.com/specifications/extensions/x.1sav.rtc/
//! [`io.mgba.rtc`]: https://docs.1retro.com/specifications/extensions/io.mgba.rtc/
//! [`x.1sav.rtc.s3511a`]: https://docs.1retro.com/specifications/extensions/x.1sav.rtc.s3511a/

use one_saves::extension::{Rtc, RtcMbc3, RtcS3511a};
use one_saves::{Extension, Extensions};
use one_saves_registry::ClockLayout;

/// The footer mGBA appends for a GBA cartridge clock: seven BCD bytes, a control register, and an
/// eight-byte little-endian instant.
const S3511A_LEN: usize = 16;

/// Save memory sizes a Game Boy or Game Boy Color cartridge comes in.
const GB_SRAM_SIZES: &[usize] = &[2048, 8192, 32768, 65536, 131_072];

/// Save memory sizes a Game Boy Advance cartridge comes in: EEPROM, SRAM, then the flash parts.
const GBA_SRAM_SIZES: &[usize] = &[512, 8192, 32768, 65536, 131_072];

/// The MBC3 footer with a 32-bit timestamp.
///
/// This is the VisualBoyAdvance lineage, where the trailing field was a bare `time_t` written by
/// `fwrite` of the whole struct, so it came out four bytes narrower on a 32-bit host. It is
/// **not** what any 32-bit build writes today: VBA-M now unions the field with a `uint64_t`
/// carrying the comment *"so that 32bit and 64bit saves are compatible"*, mGBA types it
/// `uint64_t` and stores it with `STORE_64LE`, and the FPGA cores pack their own layout entirely.
/// All of those write 48 on every platform.
///
/// No sample this crate has been tested against is 44 bytes. What makes the form real rather than
/// folklore is that **both** surviving implementations carry an explicit allowance for it on read
/// — VBA-M registers the region with `leeway = -4`, and mGBA accepts a buffer
/// `sizeof(rtcBuffer) - 4` short. Nobody writes a compatibility shim for a file that does not
/// exist. So the branch is justified and only an old save will ever exercise it.
const MBC3_LEN_32: usize = 44;
/// The MBC3 footer with a 64-bit timestamp, which is what every producer sampled here writes.
const MBC3_LEN_64: usize = 48;

/// A flat save split into the parts an emulator glued together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split {
    /// The save memory itself, which is what the cartridge held.
    pub sram: Vec<u8>,
    /// The bytes the emulator appended, verbatim.
    pub footer: Vec<u8>,
    /// What the clock showed, when it can be computed.
    ///
    /// Absent rather than approximated: the key must be omitted rather than filled with the
    /// anchor or a guess, because nothing downstream can tell a wrong reading from a right one.
    pub reading: Option<Rtc>,
    /// The chip's own state.
    pub clock: Clock,
}

/// The state of whichever clock a footer turned out to hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clock {
    /// A Game Boy MBC3 cartridge's clock.
    Mbc3(Mbc3Clock),
    /// A GBA cartridge's Seiko S-3511A.
    S3511a(S3511aClock),
}

impl Clock {
    /// What this clock showed, as a Unix instant, when that can be computed.
    ///
    /// An MBC3's always can. A Seiko chip's cannot when its components are not a date.
    #[must_use]
    pub fn reading(self) -> Option<Rtc> {
        match self {
            Clock::Mbc3(clock) => Some(clock.reading()),
            Clock::S3511a(clock) => clock.reading(),
        }
    }
}

/// A Seiko S-3511A as a GBA cartridge latches it.
///
/// The chip dates rather than counts, which is why it cannot share the MBC3's shape: it holds a
/// two-digit year, a month, a day and a weekday alongside the time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct S3511aClock {
    /// The seven latched BCD bytes: year since 2000, month, day, weekday, hour, minute, second.
    ///
    /// Kept as the chip encodes them rather than decoded into a date, because the weekday is the
    /// chip's own idea and a producer that rewrote it would be inventing agreement.
    pub components: [u8; 7],
    /// The control register as read. Bit 6 is 24-hour mode; bit 7 of the hour byte is PM when it
    /// is clear.
    pub control: u8,
    /// When the producer took the components, in epoch seconds on its own clock: the anchor it
    /// advances the chip from on the next load, which is key 2 of `x.1sav.rtc.s3511a`.
    ///
    /// Never the reading. mGBA's chip follows the host clock, so there the two are one moment on
    /// two scales, but an FPGA core's runs from whatever the game set it to, and MiSTer gives its
    /// cores local standard time counted as though it were UTC.
    pub latched_at: i64,
}

impl S3511aClock {
    /// Whether the chip is in 24-hour mode, which is bit 6 of the control register.
    #[must_use]
    pub fn hour24(self) -> bool {
        self.control & 0x40 != 0
    }

    /// What this chip showed, as a Unix instant: its components, read as though they were UTC.
    ///
    /// The chip keeps no zone, so its wall clock goes on the Unix scale the way `x.1sav.dirent`
    /// puts a console's there, rather than under a zone nobody recorded. `latched_at` is not the
    /// answer either. It is when the components were taken rather than what they say, and the two
    /// can be years apart: GBA_MiSTer starts the chip at 2009-12-31 23:59:45, and a game that never
    /// sets it leaves it there. Where the chip does follow the host, as mGBA's does, the reading
    /// sits the host's zone offset from the anchor, which is one wall clock on two scales.
    ///
    /// `None` when the components are not a date, since a reading that cannot be computed is
    /// omitted rather than approximated.
    #[must_use]
    pub fn reading(self) -> Option<Rtc> {
        components_as_utc(self.components, self.control).map(Rtc::new)
    }
}

/// The instant a Seiko chip's components spell, read as though they were UTC.
fn components_as_utc(components: [u8; 7], control: u8) -> Option<i64> {
    let year = from_bcd(components[0])?;
    let month = from_bcd(components[1])?;
    let day = from_bcd(components[2])?;
    let hour = from_bcd(hour24(components[4], control))?;
    let minute = from_bcd(components[5])?;
    let second = from_bcd(components[6])?;
    let in_range = (1..=12).contains(&month)
        && (1..=days_in_month(year, month)).contains(&day)
        && hour <= 23
        && minute <= 59
        && second <= 59;
    in_range.then(|| {
        days_from_civil(2000 + i64::from(year), month, day) * 86_400
            + i64::from(hour) * 3_600
            + i64::from(minute) * 60
            + i64::from(second)
    })
}

/// How many days a month has, in a year counted from 2000 as the chip counts it.
///
/// Every fourth year of the century is a leap year, 2000 included.
fn days_in_month(year: u8, month: u8) -> u8 {
    match month {
        2 if year.is_multiple_of(4) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days from 1970-01-01 to a civil date, by Howard Hinnant's algorithm.
fn days_from_civil(year: i64, month: u8, day: u8) -> i64 {
    let (month, day) = (i64::from(month), i64::from(day));
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

impl From<S3511aClock> for RtcS3511a {
    /// The chip's own key carries the components, the control register and the moment the
    /// producer took them, which between them are everything its file is rebuilt from.
    fn from(clock: S3511aClock) -> Self {
        RtcS3511a { components: clock.components, control: clock.control, taken_at: Some(clock.latched_at) }
    }
}

/// The Seiko chip a bundle carries, and the anchor to rebuild its producer's file from.
///
/// The anchor is key 2. A bundle without one, which is every bundle this crate wrote before the
/// key existed, carried the anchor as its reading instead, so that stands in for it rather than
/// leaving the file unbuildable.
fn s3511a_of(bundle: &one_saves::Bundle) -> Option<(RtcS3511a, i64)> {
    let extensions = &bundle.header.extensions;
    let chip = extensions.get::<RtcS3511a>()?;
    let anchor = match chip.taken_at {
        Some(taken_at) => taken_at,
        None => extensions.get::<Rtc>()?.reading,
    };
    Some((*chip, anchor))
}

/// The MBC3 clock, as the de-facto save footer stores it.
///
/// Ten little-endian 32-bit registers — the live clock, then the latched copy a game reads — and
/// then the host's clock at the moment the emulator wrote the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mbc3Clock {
    /// Seconds, 0 to 59.
    pub seconds: u32,
    /// Minutes, 0 to 59.
    pub minutes: u32,
    /// Hours, 0 to 23.
    pub hours: u32,
    /// The low eight bits of the day counter.
    pub days_low: u32,
    /// Day counter bit 8, the halt flag and the overflow flag.
    pub days_high: u32,
    /// The latched copy the game reads, in the same order.
    pub latched: [u32; 5],
    /// The host clock when the emulator last wrote this save, in epoch seconds.
    pub written_at: i64,
    /// How many bytes that timestamp occupied: 4 or 8, depending on the emulator.
    ///
    /// Carried because the footer is regenerated rather than stored, and the two widths give
    /// different files for the same clock.
    pub timestamp_width: usize,
}

impl Mbc3Clock {
    /// How long the cartridge clock has been running, in seconds.
    #[must_use]
    pub fn elapsed_seconds(self) -> i64 {
        i64::from(self.days()) * 86_400
            + i64::from(self.hours) * 3_600
            + i64::from(self.minutes) * 60
            + i64::from(self.seconds)
    }

    /// What this clock showed when the save was written.
    ///
    /// The anchor plus the elapsed time the counters describe. The anchor alone — key 2 of
    /// `x.1sav.rtc.mbc3`, which is what the footer stores — is wrong by exactly that elapsed
    /// time, which on a cartridge running for days is days.
    #[must_use]
    pub fn reading(self) -> Rtc {
        Rtc::new(self.written_at + self.elapsed_seconds())
    }

    /// The five live registers, in the order the footer stores them.
    #[must_use]
    pub fn live(self) -> [u32; 5] {
        [self.seconds, self.minutes, self.hours, self.days_low, self.days_high]
    }

    /// How many days the cartridge's clock has counted.
    #[must_use]
    pub fn days(self) -> u32 {
        self.days_low | ((self.days_high & 1) << 8)
    }

    /// Whether the clock is halted.
    #[must_use]
    pub fn halted(self) -> bool {
        self.days_high & 0x40 != 0
    }

    /// Whether the day counter has overflowed past 511 days.
    #[must_use]
    pub fn overflowed(self) -> bool {
        self.days_high & 0x80 != 0
    }
}

impl From<Mbc3Clock> for RtcMbc3 {
    /// Every byte of the footer is accounted for by these fields, so the footer is rebuilt from
    /// them rather than stored. The width is what keeps that byte-exact, since emulators differ
    /// over whether the timestamp is 32 or 64 bits.
    fn from(clock: Mbc3Clock) -> Self {
        RtcMbc3 {
            live: clock.live(),
            latched: clock.latched,
            written_at: clock.written_at,
            timestamp_width: u8::try_from(clock.timestamp_width).expect("4 or 8"),
        }
    }
}

/// Splits a flat save into save memory and an appended footer.
///
/// Returns `None` when the file is exactly a save memory size, which is the common case: most
/// cartridges have no clock and most saves have no footer.
///
/// The footer is not self-delimiting — nothing in the file says where the save memory ends — so
/// this works by length: a file that is a known save size **plus** a plausible footer is one, and
/// anything else is left alone. `system` narrows which sizes are plausible; without it both
/// families are tried.
#[must_use]
pub fn split(bytes: &[u8], system: Option<&str>) -> Option<Split> {
    let sizes: &[usize] = match system {
        // An unstated system could be either family, and the two size tables overlap almost
        // entirely, so the Game Boy one covers both.
        Some("gb" | "gbc") | None => GB_SRAM_SIZES,
        Some("gba") => GBA_SRAM_SIZES,
        // Any other system has no footer convention this module knows.
        Some(_) => return None,
    };

    for &size in sizes {
        if bytes.len() <= size {
            continue;
        }
        let footer = &bytes[size..];

        // A footer of exactly a known length that also *parses* is one. Requiring both is what
        // stops a 32816-byte file whose tail is save data from being mistaken for a clock.
        // Two producers write sixteen bytes and two write 512, and each pair means different
        // things by them, so the system decides which to try: a Seiko S-3511A is a GBA chip and
        // an MBC3 is a Game Boy one. A GBA save read as an MBC3 gets a Game Boy clock in its
        // header and a Game Boy footer written back in place of the one its core wrote.
        let gba = system == Some("gba");
        let parsed = match footer.len() {
            MBC3_LEN_32 | MBC3_LEN_64 if !gba => parse_mbc3(footer).map(Clock::Mbc3),
            PACKED_LEN_POCKET if gba => parse_s3511a(footer).map(Clock::S3511a),
            PACKED_LEN_MISTER if gba => parse_s3511a_packed(footer).map(Clock::S3511a),
            // GBA_MiSTer's block opens every record with a word the Game Boy core's never holds
            // there, so an unstated system can try it first without risking a misread.
            PACKED_LEN_MISTER if system.is_none() => parse_s3511a_packed(footer)
                .map(Clock::S3511a)
                .or_else(|| parse_packed(footer).map(Clock::Mbc3)),
            PACKED_LEN_POCKET | PACKED_LEN_MISTER => parse_packed(footer).map(Clock::Mbc3),
            _ => None,
        };
        let Some(clock) = parsed else {
            // A trailing run this module cannot read is left where it is. Splitting it would
            // stabilise the save's bytes, but there is nothing to say it is a clock at all, and
            // filing unidentified bytes under a chip's key would claim more than is known.
            continue;
        };
        // The stored host time is the Unix instant this state corresponds to: it is what the
        // emulator advances the clock from on the next load. The registers themselves count
        // elapsed time from a cartridge epoch nothing records, so they are not an instant and
        // cannot be normalized into one.
        let reading = clock.reading();
        return Some(Split { sram: bytes[..size].to_vec(), footer: footer.to_vec(), reading, clock });
    }
    None
}

/// Reads the MBC3 footer, checking the registers make sense as a clock.
///
/// The length alone is a weak signal, so every field is range-checked. A file whose tail happens
/// to be the right length but says 97 minutes past the hour is not a clock, and reading it as one
/// would put a fictional instant in the header.
#[must_use]
pub fn parse_mbc3(footer: &[u8]) -> Option<Mbc3Clock> {
    if footer.len() != MBC3_LEN_32 && footer.len() != MBC3_LEN_64 {
        return None;
    }
    let word = |index: usize| {
        let at = index * 4;
        u32::from_le_bytes([footer[at], footer[at + 1], footer[at + 2], footer[at + 3]])
    };

    let clock = Mbc3Clock {
        seconds: word(0),
        minutes: word(1),
        hours: word(2),
        days_low: word(3),
        days_high: word(4),
        latched: [word(5), word(6), word(7), word(8), word(9)],
        written_at: if footer.len() == MBC3_LEN_64 {
            i64::from_le_bytes(footer[40..48].try_into().ok()?)
        } else {
            i64::from(u32::from_le_bytes(footer[40..44].try_into().ok()?))
        },
        timestamp_width: footer.len() - 40,
    };

    let sane = |c: &Mbc3Clock| {
        c.seconds < 60
            && c.minutes < 60
            && c.hours < 24
            && c.days_low < 256
            && c.latched[0] < 60
            && c.latched[1] < 60
            && c.latched[2] < 24
            && c.latched[3] < 256
    };
    // A negative or absurd host time is not a time; a save written before the Game Boy existed is
    // a misparse rather than a very old save.
    let plausible_instant = clock.written_at >= 0;
    (sane(&clock) && plausible_instant).then_some(clock)
}

impl Split {
    /// The extension keys this split contributes to a bundle's header.
    ///
    /// The chip's own state always goes on, since it is what puts the file back the way the
    /// emulator wrote it. The portable instant joins it only when the reading could be computed:
    /// a chip whose components are not a date leaves the key out rather than carrying a guess.
    #[must_use]
    pub fn extensions(&self) -> Extensions {
        let mut extensions = Extensions::new();
        if let Some(reading) = self.reading {
            extensions.insert(reading);
        }
        match self.clock {
            Clock::Mbc3(clock) => extensions.insert(RtcMbc3::from(clock)),
            Clock::S3511a(clock) => extensions.insert(RtcS3511a::from(clock)),
        }
        extensions
    }
}

/// Which on-disk shape a clock is written back in.
///
/// The bundle carries the clock, not the encoding, because the encoding is the producer's rather
/// than the cartridge's — three of these hold the same reading and differ only in how they lay it
/// out. So writing one back is a choice, the way a card format is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockForm {
    /// Appended to the save: ten little-endian registers and an instant. VBA, BGB, mGBA, SameBoy.
    Mbc3Appended,
    /// Appended to the save: an instant and the registers bit-packed, padded to a reserved size.
    /// The MiSTer Gameboy core and its openFPGA ports.
    Packed(usize),
    /// A separate file holding the instant the clock counts from. Gambatte.
    Sidecar,
    /// Appended to the save: latched BCD and a control register. A GBA cartridge's Seiko chip.
    S3511a,
    /// A separate file holding an instant and the Seiko chip's calendar, bit-packed. mincer_ray's
    /// GBA core.
    S3511aSidecar,
    /// Appended to the save: the Seiko chip's calendar and an instant, bit-packed behind a magic
    /// word and repeated to fill a reserved size. GBA_MiSTer.
    S3511aPacked(usize),
}

/// The shape a bundle's producer writes, from what its `source` says.
///
/// `source.core` names the engine that wrote the bytes, so it is what says which of the shapes
/// to expect — not `source.app`, since RetroArch writes whatever its core does. The layout each
/// core writes is registry data — see [`ClockLayout`] — rather than
/// anything this module reads out of the slug: a core is not obliged to spell its own name in a
/// way that gives its clock away, and two that do spell it the same way need not agree.
///
/// A bundle naming nobody, or naming a core the registry records no clock for, gets the form most
/// producers use.
#[must_use]
pub fn form_of(bundle: &one_saves::Bundle) -> ClockForm {
    let core = bundle.header.source.as_ref().and_then(|source| source.core.as_ref());
    // `source.core` carries the slug for a listed core, which is what the registry is keyed by.
    let layout = core.map(one_saves::Name::as_str).and_then(one_saves_registry::core).and_then(|c| c.clock);
    // Which chip wrote the state is the save's own business — a Seiko S-3511A is a Game Boy
    // Advance part and an MBC3 a Game Boy one — so the key settles it, and what the producer
    // settles is the layout. mincer_ray keeps the chip in a file of its own and GBA_MiSTer packs
    // it; every other GBA core appends it the way mGBA does.
    if bundle.header.extensions.contains_key(&RtcS3511a::KEY) {
        return match layout {
            Some(ClockLayout::Sidecar) => ClockForm::S3511aSidecar,
            Some(ClockLayout::Packed { reserved }) => ClockForm::S3511aPacked(reserved),
            Some(ClockLayout::Appended) | None => ClockForm::S3511a,
        };
    }
    match layout {
        Some(ClockLayout::Sidecar) => ClockForm::Sidecar,
        Some(ClockLayout::Packed { reserved }) => ClockForm::Packed(reserved),
        Some(ClockLayout::Appended) | None => ClockForm::Mbc3Appended,
    }
}

/// Rebuilds the footer a bundle's header describes, in the shape its producer writes.
///
/// Returns `None` for a bundle that carried no clock, which is most of them.
#[must_use]
pub fn footer_of(bundle: &one_saves::Bundle) -> Option<Vec<u8>> {
    footer_of_form(bundle, form_of(bundle))
}

/// Rebuilds the footer in a chosen shape.
#[must_use]
pub fn footer_of_form(bundle: &one_saves::Bundle, form: ClockForm) -> Option<Vec<u8>> {
    // A sidecar is not appended at all, so there is no footer for the save to carry.
    if matches!(form, ClockForm::Sidecar | ClockForm::S3511aSidecar) {
        return None;
    }
    if let ClockForm::S3511aPacked(reserved) = form {
        let (chip, anchor) = s3511a_of(bundle)?;
        return Some(build_s3511a_packed(&chip.components, chip.control, anchor, reserved));
    }
    let raw = footer_of_mbc3(bundle)?;
    match form {
        ClockForm::Packed(reserved) => parse_mbc3(&raw).as_ref().map(|clock| build_packed(clock, reserved)),
        _ => Some(raw),
    }
}

/// Rebuilds the appended MBC3 footer, whatever shape the producer actually writes.
#[must_use]
fn footer_of_mbc3(bundle: &one_saves::Bundle) -> Option<Vec<u8>> {
    let extensions = &bundle.header.extensions;
    if let Some((chip, anchor)) = s3511a_of(bundle) {
        return Some(build_s3511a(&chip.components, chip.control, anchor));
    }
    let clock = extensions.get::<RtcMbc3>()?;
    Some(build_mbc3(&clock.live, &clock.latched, clock.written_at, usize::from(clock.timestamp_width)))
}

/// Reads the footer mGBA appends for a GBA cartridge clock.
///
/// Every field is range-checked as BCD before it is believed. Sixteen trailing bytes is a weak
/// signal on its own, and a misparse would put a fictional date in the header.
#[must_use]
pub fn parse_s3511a(footer: &[u8]) -> Option<S3511aClock> {
    if footer.len() != S3511A_LEN {
        return None;
    }
    let components: [u8; 7] = footer[..7].try_into().ok()?;
    let year = from_bcd(components[0])?;
    let month = from_bcd(components[1])?;
    let day = from_bcd(components[2])?;
    // The weekday is a plain value rather than BCD in practice, and the chip's own idea of which
    // day is which, so it is range-checked and not otherwise interpreted.
    let weekday = components[3];
    let hour = from_bcd(components[4] & 0x3f)?;
    let minute = from_bcd(components[5])?;
    let second = from_bcd(components[6])?;

    let control = footer[7];
    let latched_at = i64::from_le_bytes(footer[8..16].try_into().ok()?);

    let sane = year <= 99
        && (1..=12).contains(&month)
        && (1..=31).contains(&day)
        && weekday <= 6
        && hour <= 23
        && minute <= 59
        && second <= 59
        && latched_at >= 0;
    sane.then_some(S3511aClock { components, control, latched_at })
}

/// Writes the S-3511A footer back out: the latched components, the control register, then the
/// instant as eight little-endian bytes.
#[must_use]
pub fn build_s3511a(components: &[u8; 7], control: u8, latched_at: i64) -> Vec<u8> {
    let mut footer = Vec::with_capacity(S3511A_LEN);
    footer.extend_from_slice(components);
    footer.push(control);
    footer.extend_from_slice(&latched_at.to_le_bytes());
    footer
}

/// A BCD byte's value, when both its digits are digits.
fn from_bcd(byte: u8) -> Option<u8> {
    let (high, low) = (byte >> 4, byte & 0x0f);
    (high <= 9 && low <= 9).then_some(high * 10 + low)
}

/// The reserved region an Analogue Pocket openFPGA Game Boy core appends.
pub const PACKED_LEN_POCKET: usize = 16;

/// The reserved region the MiSTer Gameboy core appends: one SD block.
pub const PACKED_LEN_MISTER: usize = 512;

/// Reads the footer the MiSTer Gameboy core and its openFPGA ports append.
///
/// One format, two reserved sizes: MiSTer writes a whole 512-byte SD block and the Analogue
/// Pocket port writes 16 bytes. Both use ten of them, and the ports share `mbc3.v` with the
/// original, which is why the packing is identical.
///
/// The layout is taken from the cores themselves rather than inferred. Both write five 16-bit
/// words past the save memory:
///
/// ```text
/// 0x00 : RTC_timestampOut[15:0]     bytes 0..2   when the save was written
/// 0x01 : RTC_timestampOut[31:16]    bytes 2..4
/// 0x02 : RTC_savedtimeOut[15:0]     bytes 4..6   the registers, bit-packed
/// 0x03 : RTC_savedtimeOut[31:16]    bytes 6..8
/// 0x04 : RTC_savedtimeOut[47:32]    bytes 8..10  always zero; only 29 bits are used
/// ```
///
/// and `mbc3.v` packs the registers into the low 29 bits:
///
/// ```text
/// {rtc_halt, rtc_overflow, rtc_days[9:0], rtc_hours[4:0], rtc_minutes[5:0], rtc_seconds[5:0]}
/// ```
///
/// Note the day counter is **ten** bits here, not the nine the hardware has, and the overflow
/// flag is a bit of its own rather than the top of the day register. So a clock read from this
/// core can hold a day count the appended MBC3 footer cannot, and converting between the two is
/// lossy past 511 days. The six bytes past the tenth are never written and read back as `0xFF`,
/// which the cores write explicitly as `16'hFFFF` rather than leaving erased.
#[must_use]
pub fn parse_packed(footer: &[u8]) -> Option<Mbc3Clock> {
    if footer.len() != PACKED_LEN_POCKET && footer.len() != PACKED_LEN_MISTER {
        return None;
    }
    let written_at = i64::from(u32::from_le_bytes(footer[0..4].try_into().ok()?));
    let mut packed = [0u8; 8];
    packed[..6].copy_from_slice(&footer[4..10]);
    let packed = u64::from_le_bytes(packed);

    let seconds = u32::try_from(packed & 0x3F).ok()?;
    let minutes = u32::try_from(packed >> 6 & 0x3F).ok()?;
    let hours = u32::try_from(packed >> 12 & 0x1F).ok()?;
    let days = u32::try_from(packed >> 17 & 0x3FF).ok()?;
    let overflow = packed >> 27 & 1 != 0;
    let halt = packed >> 28 & 1 != 0;

    // The core keeps registers rather than a counter, so these are already ranged — but a footer
    // that says 97 minutes past the hour is not this core's output whatever its length.
    if seconds > 59 || minutes > 59 || hours > 23 || written_at < 0 {
        return None;
    }

    let mut days_high = days >> 8 & 1;
    if halt {
        days_high |= 0x40;
    }
    if overflow {
        days_high |= 0x80;
    }
    let live = [seconds, minutes, hours, days & 0xFF, days_high];
    Some(Mbc3Clock {
        seconds,
        minutes,
        hours,
        days_low: days & 0xFF,
        days_high,
        latched: live,
        written_at,
        timestamp_width: 8,
    })
}

/// Writes the footer back out, padded to the reserved size its producer uses.
#[must_use]
pub fn build_packed(clock: &Mbc3Clock, reserved: usize) -> Vec<u8> {
    let packed = u64::from(clock.seconds & 0x3F)
        | u64::from(clock.minutes & 0x3F) << 6
        | u64::from(clock.hours & 0x1F) << 12
        | u64::from(clock.days() & 0x3FF) << 17
        | u64::from(clock.overflowed()) << 27
        | u64::from(clock.halted()) << 28;

    let mut footer = Vec::with_capacity(reserved);
    footer.extend_from_slice(&u32::try_from(clock.written_at).unwrap_or(0).to_le_bytes());
    footer.extend_from_slice(&packed.to_le_bytes()[..6]);
    footer.resize(reserved, 0xFF);
    footer
}

/// The length of the sidecar clock file Gambatte writes beside a save.
pub const GAMBATTE_RTC_LEN: usize = 4;

/// Reads Gambatte's sidecar clock file, which stores an **origin** rather than a reading.
///
/// The four bytes are a big-endian Unix instant, and Gambatte derives the cartridge clock as
/// `now - base`. That is the other operand of a reading, not a reading: on its own it says
/// nothing about what time the game thinks it is.
///
/// `captured_at` is what supplies the missing half — the instant the file was written, which the
/// file itself does not record. Pass the file's modification time, or a known instant if you have
/// a better one. The elapsed time is then turned into the register set an MBC3 would have held at
/// that moment, so the bundle carries a real reading like every other producer's.
///
/// Whatever `captured_at` is, the round trip is exact: writing the sidecar back computes
/// `captured_at - elapsed`, which returns the original base however the instant was chosen.
#[must_use]
pub fn parse_gambatte_sidecar(bytes: &[u8], captured_at: i64) -> Option<Mbc3Clock> {
    if bytes.len() != GAMBATTE_RTC_LEN {
        return None;
    }
    let base = i64::from(u32::from_be_bytes(bytes.try_into().ok()?));
    let elapsed = captured_at.checked_sub(base)?;
    // A clock that has not started yet is not a clock; a base in the future means the instant is
    // wrong rather than that the cartridge ran backwards.
    if elapsed < 0 {
        return None;
    }

    let days = elapsed / 86_400;
    let seconds = u32::try_from(elapsed % 60).ok()?;
    let minutes = u32::try_from(elapsed / 60 % 60).ok()?;
    let hours = u32::try_from(elapsed / 3_600 % 24).ok()?;
    // The day counter is nine bits with a carry flag above it, exactly as the chip holds it.
    let days_low = u32::try_from(days & 0xFF).ok()?;
    let mut days_high = u32::try_from(days >> 8 & 1).ok()?;
    if days > 511 {
        days_high |= 0x80;
    }

    let live = [seconds, minutes, hours, days_low, days_high];
    Some(Mbc3Clock {
        seconds,
        minutes,
        hours,
        days_low,
        days_high,
        // Nothing was mid-read, so the latched copy is the live one.
        latched: live,
        written_at: captured_at,
        timestamp_width: 8,
    })
}

/// Reads a sidecar clock file, whichever producer wrote it.
///
/// The two that exist share an extension and nothing else, and their lengths tell them apart:
/// Gambatte's four bytes are an MBC3 origin, mincer_ray's sixteen an S-3511A reading. Only the
/// first needs `captured_at` — see [`parse_gambatte_sidecar`] — and the second ignores it, since
/// it records its own instant.
#[must_use]
pub fn parse_sidecar(bytes: &[u8], captured_at: i64) -> Option<Clock> {
    match bytes.len() {
        GAMBATTE_RTC_LEN => parse_gambatte_sidecar(bytes, captured_at).map(Clock::Mbc3),
        MINCER_RTC_LEN => parse_mincer_sidecar(bytes).map(Clock::S3511a),
        _ => None,
    }
}

/// Rebuilds the sidecar file a bundle's clock goes in.
///
/// Which file that is follows from the chip, since one producer keeps each in a file of its own:
/// an MBC3 goes back as Gambatte's origin and an S-3511A as mincer_ray's record.
#[must_use]
pub fn sidecar_of(bundle: &one_saves::Bundle) -> Option<Vec<u8>> {
    if let Some((chip, anchor)) = s3511a_of(bundle) {
        return Some(build_mincer_sidecar(&chip.components, chip.control, anchor));
    }
    let footer = footer_of_mbc3(bundle)?;
    // `footer_of` rebuilds the appended form; reading it back gives the registers to work from.
    let clock = parse_mbc3(&footer)?;
    Some(build_gambatte_sidecar(&clock))
}

/// Writes Gambatte's sidecar back out: the origin the clock is derived from, big-endian.
#[must_use]
pub fn build_gambatte_sidecar(clock: &Mbc3Clock) -> Vec<u8> {
    let elapsed = i64::from(clock.days()) * 86_400
        + i64::from(clock.hours) * 3_600
        + i64::from(clock.minutes) * 60
        + i64::from(clock.seconds);
    let base = clock.written_at.saturating_sub(elapsed);
    u32::try_from(base).unwrap_or(0).to_be_bytes().to_vec()
}

/// The control register the MiSTer GBA core and its Pocket port hold after loading a clock:
/// 24-hour mode.
///
/// Neither core's record has room for the register, so neither stores it, and both reset it to
/// this on every load. It is what the chip holds whenever a game first reads a restored clock.
const MISTER_GBA_CONTROL: u8 = 0x40;

/// Unpacks the calendar the MiSTer GBA core keeps beside a clock, `RTC_savedtimeOut`: the
/// S-3511A's seven BCD components at their natural widths, in 42 bits.
///
/// ```text
/// {year[7:0], month[4:0], day[5:0], weekday[2:0], hour[5:0], minute[6:0], second[6:0]}
/// ```
///
/// Checked the way mincer_ray's port checks one before believing it: every component BCD, and the
/// day one the month has. The hour is on the 24-hour clock, with no PM bit, since the control
/// register that would say otherwise is not kept.
fn unpack_calendar(packed: u64) -> Option<[u8; 7]> {
    if packed >> 42 != 0 {
        return None;
    }
    let field = |shift: u32, width: u32| u8::try_from(packed >> shift & ((1 << width) - 1)).ok();
    let components = [
        field(34, 8)?,
        field(29, 5)?,
        field(23, 6)?,
        field(20, 3)?,
        field(14, 6)?,
        field(7, 7)?,
        field(0, 7)?,
    ];
    let year = from_bcd(components[0])?;
    let month = from_bcd(components[1])?;
    let day = from_bcd(components[2])?;
    let hour = from_bcd(components[4])?;
    let minute = from_bcd(components[5])?;
    let second = from_bcd(components[6])?;

    let sane = (1..=12).contains(&month)
        && (1..=days_in_month(year, month)).contains(&day)
        && components[3] <= 6
        && hour <= 23
        && minute <= 59
        && second <= 59;
    sane.then_some(components)
}

/// Packs the chip's components back into the 42 bits the MiSTer GBA core keeps.
///
/// There is no control register beside them and the hour is on the 24-hour clock, so a clock read
/// in 12-hour mode has its PM flag folded into the hour, and the register itself goes.
fn pack_calendar(components: [u8; 7], control: u8) -> u64 {
    u64::from(components[0]) << 34
        | u64::from(components[1] & 0x1F) << 29
        | u64::from(components[2] & 0x3F) << 23
        | u64::from(components[3] & 0x07) << 20
        | u64::from(hour24(components[4], control)) << 14
        | u64::from(components[5] & 0x7F) << 7
        | u64::from(components[6] & 0x7F)
}

/// An S-3511A hour byte on the 24-hour clock, as BCD.
///
/// In 24-hour mode the byte already is one, less the PM flag the chip may set beside it. In
/// 12-hour mode that flag is what says which half of the day the hour falls in.
fn hour24(hour: u8, control: u8) -> u8 {
    let digits = hour & 0x3F;
    if control & 0x40 != 0 || hour & 0x80 == 0 {
        return digits;
    }
    let Some(value) = from_bcd(digits) else { return digits };
    let value = value % 12 + 12;
    ((value / 10) << 4) | (value % 10)
}

/// The length of the sidecar clock file mincer_ray's GBA core writes beside a save.
pub const MINCER_RTC_LEN: usize = 16;

/// Reads the sidecar clock file mincer_ray's GBA core writes, which unlike Gambatte's holds a
/// reading rather than an origin.
///
/// It holds what the MiSTer GBA core it was ported from keeps — `RTC_timestampOut` and
/// `RTC_savedtimeOut` — in an order of its own, and in a file of its own as of the core's 0.8.0,
/// so that the save is the size a cartridge's is. Five little-endian 16-bit words, then padding:
///
/// ```text
/// bytes 0..4    the host clock when the record was taken, in epoch seconds
/// bytes 4..10   the calendar, bit-packed into the low 42 bits
/// bytes 10..16  zero
/// ```
///
/// The two halves describe the same moment — the core only exports a pair that has held still —
/// and the timestamp is the anchor the core advances the calendar from on the next load. So the
/// record needs no instant supplying: the calendar is the reading, as [`S3511aClock::reading`]
/// says, and the timestamp is what rebuilding the file needs. What the record does not hold is
/// the control register, which the core resets to 24-hour mode on every load, so that is what is
/// read back.
///
/// Everything is checked the way the core checks it before believing it, since a record the
/// core refuses is one it replaces with the Pocket's own clock: the timestamp neither zero nor
/// erased, the padding zero, and the calendar a date.
#[must_use]
pub fn parse_mincer_sidecar(bytes: &[u8]) -> Option<S3511aClock> {
    if bytes.len() != MINCER_RTC_LEN {
        return None;
    }
    let timestamp = u32::from_le_bytes(bytes[0..4].try_into().ok()?);
    if timestamp == 0 || timestamp == u32::MAX || bytes[10..].iter().any(|&b| b != 0) {
        return None;
    }
    let mut packed = [0u8; 8];
    packed[..6].copy_from_slice(&bytes[4..10]);
    let components = unpack_calendar(u64::from_le_bytes(packed))?;
    Some(S3511aClock { components, control: MISTER_GBA_CONTROL, latched_at: i64::from(timestamp) })
}

/// Writes mincer_ray's sidecar back out, from the chip's components and the instant they show.
///
/// The record keeps the hour on the 24-hour clock and has no control register, so a clock read in
/// 12-hour mode has its PM flag folded into the hour, and the register itself goes. An instant
/// outside the record's 32 bits is written as zero, which the core refuses and replaces with the
/// Pocket's own clock rather than misreading.
#[must_use]
pub fn build_mincer_sidecar(components: &[u8; 7], control: u8, latched_at: i64) -> Vec<u8> {
    let mut record = Vec::with_capacity(MINCER_RTC_LEN);
    record.extend_from_slice(&u32::try_from(latched_at).unwrap_or(0).to_le_bytes());
    record.extend_from_slice(&pack_calendar(*components, control).to_le_bytes()[..6]);
    record.resize(MINCER_RTC_LEN, 0);
    record
}

/// The first word of every record GBA_MiSTer writes past a save: `"RT"`, little-endian.
const MISTER_GBA_MAGIC: [u8; 2] = *b"TR";

/// The length of the record GBA_MiSTer repeats across the block it appends.
const MISTER_GBA_RECORD_LEN: usize = 16;

/// Reads the block GBA_MiSTer appends to a save for a cartridge clock.
///
/// The core writes one 512-byte SD block past the save memory and fills it from eight 16-bit
/// words, indexed by the low three bits of the word address, so the block is 32 copies of one
/// 16-byte record:
///
/// ```text
/// bytes 0..2    the word "RT", which is how the core knows the block is a clock
/// bytes 2..12   80 bits, little-endian: the calendar in bits 0..42, the host clock when the
///               record was taken in bits 42..74, and zero above
/// bytes 12..16  zero
/// ```
///
/// Both halves are the ones mincer_ray's port keeps, so the calendar unpacks the same way and is
/// the reading, and the timestamp is the anchor. The order is the other way round, though, and the
/// two cores' records are not interchangeable. The timestamp is not UTC either: MiSTer hands
/// nearly all of its cores local standard time counted as though it were, eight hours behind the
/// instant in Pacific time, which is one more reason it is carried as the anchor and never read
/// as the reading.
///
/// The core checks the word and nothing else, and this checks more: every copy has to agree and
/// the calendar has to be a date. A block that is anything else is not this core's output, could
/// not be written back as it was, and stays in the payload.
#[must_use]
pub fn parse_s3511a_packed(footer: &[u8]) -> Option<S3511aClock> {
    if footer.len() != PACKED_LEN_MISTER {
        return None;
    }
    let record = &footer[..MISTER_GBA_RECORD_LEN];
    if record[..2] != MISTER_GBA_MAGIC
        || record[12..].iter().any(|&b| b != 0)
        || footer.chunks(MISTER_GBA_RECORD_LEN).any(|copy| copy != record)
    {
        return None;
    }
    let mut value = [0u8; 16];
    value[..10].copy_from_slice(&record[2..12]);
    let value = u128::from_le_bytes(value);
    if value >> 74 != 0 {
        return None;
    }
    let components = unpack_calendar(u64::try_from(value & ((1 << 42) - 1)).ok()?)?;
    let latched_at = i64::try_from(value >> 42).ok()?;
    Some(S3511aClock { components, control: MISTER_GBA_CONTROL, latched_at })
}

/// Writes GBA_MiSTer's block back out: the record, repeated to fill the reserved size.
///
/// The hour and the control register go the way they do in [`build_mincer_sidecar`], since the
/// calendar is the same. An instant outside the record's 32 bits is written as zero, which the
/// core does not check and runs the clock forward from.
#[must_use]
pub fn build_s3511a_packed(components: &[u8; 7], control: u8, latched_at: i64, reserved: usize) -> Vec<u8> {
    let timestamp = u32::try_from(latched_at).unwrap_or(0);
    let value = u128::from(pack_calendar(*components, control)) | u128::from(timestamp) << 42;
    let mut record = Vec::with_capacity(MISTER_GBA_RECORD_LEN);
    record.extend_from_slice(&MISTER_GBA_MAGIC);
    record.extend_from_slice(&value.to_le_bytes()[..10]);
    record.resize(MISTER_GBA_RECORD_LEN, 0);
    record.iter().copied().cycle().take(reserved).collect()
}

/// Writes the MBC3 footer back out: ten little-endian registers, then the timestamp.
#[must_use]
pub fn build_mbc3(live: &[u32; 5], latched: &[u32; 5], written_at: i64, timestamp_width: usize) -> Vec<u8> {
    let mut footer = Vec::with_capacity(40 + timestamp_width);
    for value in live.iter().chain(latched) {
        footer.extend_from_slice(&value.to_le_bytes());
    }
    if timestamp_width == 8 {
        footer.extend_from_slice(&written_at.to_le_bytes());
    } else {
        // The 32-bit form some emulators still write. A negative or out-of-range instant never
        // reaches here: `parse_mbc3` refuses one.
        footer.extend_from_slice(&u32::try_from(written_at).unwrap_or(0).to_le_bytes());
    }
    footer
}

#[cfg(test)]
mod tests {
    use super::*;
    use one_saves::dcbor::CBOR;

    /// Builds the footer an emulator writes for a Game Boy cartridge with a clock.
    fn mbc3_footer(seconds: u32, minutes: u32, hours: u32, days: u32, written_at: i64) -> Vec<u8> {
        let mut footer = Vec::with_capacity(MBC3_LEN_64);
        for value in [seconds, minutes, hours, days & 0xFF, (days >> 8) & 1] {
            footer.extend_from_slice(&value.to_le_bytes());
        }
        // The latched copy the game reads, here the same as the live clock.
        for value in [seconds, minutes, hours, days & 0xFF, (days >> 8) & 1] {
            footer.extend_from_slice(&value.to_le_bytes());
        }
        footer.extend_from_slice(&written_at.to_le_bytes());
        footer
    }

    fn crystal_save(written_at: i64) -> Vec<u8> {
        let mut save = vec![0x11u8; 32768];
        save.extend_from_slice(&mbc3_footer(30, 45, 13, 300, written_at));
        save
    }

    /// The trailing 48 bytes of a real Pokémon Crystal save written by SameBoy, whose cartridge
    /// clock had been running 2 minutes 13 seconds. Ten little-endian registers, then an
    /// eight-byte instant — the shape this module infers, here confirmed against a real emulator.
    const SAMEBOY_FOOTER: [u8; 48] = [
        0x0d, 0, 0, 0, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // live
        0x0d, 0, 0, 0, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // latched
        0x62, 0xba, 0x7c, 0x6a, 0, 0, 0, 0, // written_at, 64-bit
    ];

    #[test]
    fn reads_a_real_sameboy_clock() {
        let clock = parse_mbc3(&SAMEBOY_FOOTER).expect("an MBC3 clock");
        assert_eq!((clock.seconds, clock.minutes, clock.hours), (13, 2, 0));
        assert_eq!(clock.days(), 0);
        assert!(!clock.halted() && !clock.overflowed());
        // SameBoy writes the 64-bit form, so the footer is 48 bytes rather than 44.
        assert_eq!(clock.timestamp_width, 8);
        // The instant agrees with when the emulator wrote the file, to the second.
        assert_eq!(clock.written_at, 1_786_559_074);
        // The latched copy is the live one here, which is what a game that is not mid-read leaves.
        assert_eq!(clock.latched, clock.live());
    }

    #[test]
    fn a_real_sameboy_save_splits_and_rebuilds() {
        let mut save: Vec<u8> = (0..32_768u32).map(|i| (i % 251) as u8).collect();
        save.extend_from_slice(&SAMEBOY_FOOTER);

        let split = split(&save, Some("gb")).expect("has a clock");
        assert_eq!(split.sram.len(), 32_768);

        let Clock::Mbc3(clock) = split.clock else { panic!("wrong chip") };
        let rebuilt = build_mbc3(&clock.live(), &clock.latched, clock.written_at, clock.timestamp_width);
        assert_eq!(rebuilt, SAMEBOY_FOOTER, "regenerated byte for byte");
    }

    #[test]
    fn a_save_with_no_footer_is_left_alone() {
        // Most cartridges have no clock, and a plain save must not be split.
        for size in GB_SRAM_SIZES {
            assert_eq!(split(&vec![0u8; *size], Some("gb")), None, "{size} bytes");
        }
    }

    #[test]
    fn splits_a_pokemon_crystal_save() {
        // 32 KiB of save memory plus a 48-byte clock footer.
        let save = crystal_save(1_700_000_000);
        assert_eq!(save.len(), 32_816);

        let split = split(&save, Some("gb")).expect("has a footer");
        assert_eq!(split.sram.len(), 32_768);
        assert_eq!(split.footer.len(), MBC3_LEN_64);
        // The anchor plus 300 days, 13:45:30 of elapsed cartridge time.
        let elapsed = 300 * 86_400 + 13 * 3_600 + 45 * 60 + 30;
        assert_eq!(split.reading.unwrap().reading, 1_700_000_000 + elapsed);
    }

    #[test]
    fn reads_the_registers_and_the_day_counter() {
        let clock = parse_mbc3(&mbc3_footer(30, 45, 13, 300, 1_700_000_000)).expect("parses");
        assert_eq!((clock.seconds, clock.minutes, clock.hours), (30, 45, 13));
        // 300 days needs bit 8, which lives in the high register.
        assert_eq!(clock.days(), 300);
        assert!(!clock.halted());
        assert!(!clock.overflowed());
    }

    #[test]
    fn accepts_the_shorter_footer_some_emulators_write() {
        let mut footer = mbc3_footer(1, 2, 3, 4, 0);
        footer.truncate(MBC3_LEN_32);
        footer[40..44].copy_from_slice(&1_600_000_000u32.to_le_bytes());

        let clock = parse_mbc3(&footer).expect("44 bytes is the 32-bit timestamp form");
        assert_eq!(clock.written_at, 1_600_000_000);
    }

    #[test]
    fn a_tail_of_the_right_length_that_is_not_a_clock_is_left_alone() {
        // The length alone is a weak signal: this one says 97 minutes past the hour. Splitting it
        // would stabilise the save's bytes, but nothing says it is a clock, and filing
        // unidentified bytes under a chip's key would claim more than is known.
        let mut save = vec![0u8; 32_768];
        let mut footer = mbc3_footer(30, 97, 13, 300, 1_700_000_000);
        footer[4..8].copy_from_slice(&97u32.to_le_bytes());
        save.extend_from_slice(&footer);

        assert_eq!(split(&save, Some("gb")), None);
    }

    #[test]
    fn a_tail_running_into_the_kilobytes_is_not_a_footer() {
        // That is a save whose size this module does not know, and splitting it would corrupt it.
        let mut save = vec![0u8; 32_768];
        save.extend_from_slice(&vec![0u8; 4096]);
        assert_eq!(split(&save, Some("gb")), None);
    }

    #[test]
    fn a_system_with_no_footer_convention_is_never_split() {
        let mut save = vec![0u8; 32_768];
        save.extend_from_slice(&mbc3_footer(1, 2, 3, 4, 5));
        assert_eq!(split(&save, Some("snes")), None);
        assert_eq!(split(&save, Some("psx")), None);
    }

    #[test]
    fn the_reading_encodes_as_the_extension_schema_says() {
        let encoded = CBOR::from(Rtc::new(1_700_000_000)).to_cbor_data();
        // { 0: 1(1700000000) } — one key, tagged whole seconds, and nothing else. Which clock
        // produced it is not a field: the chip key beside this one names it.
        assert_eq!(encoded, [0xa1, 0x00, 0xc1, 0x1a, 0x65, 0x53, 0xf1, 0x00]);
    }

    #[test]
    fn a_read_footer_contributes_both_keys() {
        // The specification's rule: the portable instant anyone can read, and the chip's own
        // state only something that knows the layout can rebuild.
        let split = split(&crystal_save(1_700_000_000), Some("gb")).expect("has a footer");
        let extensions = split.extensions();

        assert!(extensions.get::<Rtc>().is_some(), "the portable reading");
        let native = extensions.get::<RtcMbc3>().expect("the chip state");

        // Read out as fields, not kept as a blob, so a consumer can see what time it is.
        assert_eq!(native.live, [30, 45, 13, 44, 1]);
        assert_eq!(native.latched, [30, 45, 13, 44, 1]);
        assert_eq!(native.timestamp_width, 8);
        // Keys 0 to 3, exactly as the published schema numbers them, and no layout discriminator:
        // the key name says which chip this is.
        let map = extensions.cbor(&RtcMbc3::KEY).unwrap();
        assert_eq!(map.as_map().unwrap().len(), 4);
    }

    #[test]
    fn the_footer_is_regenerated_byte_for_byte() {
        // This is what makes storing fields rather than a blob safe: every byte of the footer is
        // accounted for, so rebuilding it reproduces the emulator's file exactly.
        for width in [MBC3_LEN_32, MBC3_LEN_64] {
            let mut original = mbc3_footer(30, 45, 13, 300, 1_600_000_000);
            if width == MBC3_LEN_32 {
                original.truncate(MBC3_LEN_32);
                original[40..44].copy_from_slice(&1_600_000_000u32.to_le_bytes());
            }

            let clock = parse_mbc3(&original).expect("parses");
            let rebuilt = build_mbc3(&clock.live(), &clock.latched, clock.written_at, clock.timestamp_width);
            assert_eq!(rebuilt, original, "{width}-byte footer should rebuild exactly");
        }
    }

    #[test]
    fn the_timestamp_width_is_what_keeps_it_exact() {
        // The same clock written by two emulators gives two different files, and the width is the
        // only thing that says which. Dropping it would silently turn a 44-byte footer into 48.
        let clock = Mbc3Clock {
            seconds: 1,
            minutes: 2,
            hours: 3,
            days_low: 4,
            days_high: 0,
            latched: [1, 2, 3, 4, 0],
            written_at: 1_600_000_000,
            timestamp_width: 4,
        };
        assert_eq!(build_mbc3(&clock.live(), &clock.latched, clock.written_at, 4).len(), MBC3_LEN_32);
        assert_eq!(build_mbc3(&clock.live(), &clock.latched, clock.written_at, 8).len(), MBC3_LEN_64);
    }

    /// The trailing sixteen bytes of a real Pokémon Ruby save written by mGBA, whose in-game
    /// clock read 10:39 on 2026-08-12. Every field below is checked against something known
    /// independently of the file: the date, the weekday, the time on screen, and the fact that
    /// the emulator wrote the file 22 seconds later.
    const RUBY_FOOTER: [u8; 16] =
        [0x26, 0x08, 0x12, 0x03, 0x10, 0x39, 0x43, 0x40, 0xdf, 0xaf, 0x7c, 0x6a, 0, 0, 0, 0];

    #[test]
    fn reads_a_real_gba_clock() {
        let clock = parse_s3511a(&RUBY_FOOTER).expect("a Seiko S-3511A");

        // BCD, in the chip's order: year since 2000, month, day, weekday, hour, minute, second.
        assert_eq!(clock.components, [0x26, 0x08, 0x12, 0x03, 0x10, 0x39, 0x43]);
        // 2026-08-12 really was a Wednesday, and the chip counts Sunday as 0.
        assert_eq!(clock.components[3], 3);
        // The clock on screen said 10:39.
        assert_eq!((clock.components[4], clock.components[5]), (0x10, 0x39));
        // Bit 6 of the control register is 24-hour mode, which is why the schema has no separate
        // flag for it: the two would be one value with two spellings.
        assert_eq!(clock.control, 0x40);
        assert!(clock.hour24());
        // The latch instant agrees with the components rather than merely being plausible.
        assert_eq!(clock.latched_at, 1_786_556_383);
    }

    #[test]
    fn a_real_gba_save_splits_and_rebuilds() {
        // 128 KiB of flash, then the clock.
        let mut save: Vec<u8> = (0..131_072u32).map(|i| (i % 251) as u8).collect();
        save.extend_from_slice(&RUBY_FOOTER);

        let split = split(&save, Some("gba")).expect("has a clock");
        assert_eq!(split.sram.len(), 131_072);
        // The reading is the components, 10:39:43 on 2026-08-12, read as though they were UTC. mGBA's
        // chip follows the host, so it sits exactly the 7-hour offset of mGBA's zone from the anchor.
        assert_eq!(split.reading.unwrap().reading, 1_786_531_183);

        // Both keys go on, and the chip key carries only what the chip holds.
        let extensions = split.extensions();
        assert!(extensions.get::<Rtc>().is_some());
        assert_eq!(extensions.get::<RtcS3511a>().expect("chip state").control, 0x40);
        // The anchor rides in the chip key as key 2, since it is not the reading.
        let native = extensions.cbor(&RtcS3511a::KEY).unwrap();
        assert_eq!(native.as_map().unwrap().len(), 3);
        assert_eq!(extensions.get::<RtcS3511a>().unwrap().taken_at, Some(1_786_556_383));

        let Clock::S3511a(clock) = split.clock else { panic!("wrong chip") };
        assert_eq!(build_s3511a(&clock.components, clock.control, clock.latched_at), RUBY_FOOTER);
    }

    #[test]
    fn a_gba_tail_that_is_not_bcd_is_not_a_clock() {
        // Sixteen trailing bytes is a weak signal, so every field is range-checked. Month 19 is
        // not a month, and a misparse would put a fictional date in the header.
        let mut bad = RUBY_FOOTER;
        bad[1] = 0x19;
        assert_eq!(parse_s3511a(&bad), None);

        let mut nibble = RUBY_FOOTER;
        nibble[5] = 0x3f; // 0x0f is not a BCD digit
        assert_eq!(parse_s3511a(&nibble), None);
    }

    /// The sidecar Gambatte wrote beside a real Pokémon Crystal save, and the moment it wrote it.
    /// Big-endian, unlike every appended footer, which is the detail most likely to be got wrong.
    const GAMBATTE_SIDECAR: [u8; 4] = [0x6a, 0x7c, 0xb7, 0x78];
    const GAMBATTE_WRITTEN_AT: i64 = 1_786_558_622;

    #[test]
    fn reads_a_real_gambatte_sidecar() {
        // base = 1786558328 (2026-08-12 11:12:08), captured 294 s later.
        let clock = parse_gambatte_sidecar(&GAMBATTE_SIDECAR, GAMBATTE_WRITTEN_AT).expect("a clock");
        assert_eq!((clock.hours, clock.minutes, clock.seconds), (0, 4, 54));
        assert_eq!(clock.days(), 0);
        // The reading is what the cartridge clock said, not the origin it is derived from.
        assert_eq!(clock.written_at, GAMBATTE_WRITTEN_AT);
        assert_eq!(clock.latched, clock.live(), "nothing was mid-read");
    }

    #[test]
    fn the_sidecar_round_trips_whatever_instant_is_chosen() {
        // The captured instant is a judgement — the file does not record it — so what matters is
        // that the origin comes back unchanged however it was chosen. Writing back computes
        // `captured_at - elapsed`, which cancels it out.
        for captured_at in [GAMBATTE_WRITTEN_AT, GAMBATTE_WRITTEN_AT + 3600, 1_800_000_000] {
            let clock = parse_gambatte_sidecar(&GAMBATTE_SIDECAR, captured_at).expect("a clock");
            assert_eq!(
                build_gambatte_sidecar(&clock),
                GAMBATTE_SIDECAR,
                "origin should survive captured_at = {captured_at}"
            );
        }
    }

    #[test]
    fn a_sidecar_from_the_future_is_refused() {
        // A base later than the capture instant means the instant is wrong, not that the
        // cartridge ran backwards. Inventing a negative elapsed would put nonsense in the header.
        assert_eq!(parse_gambatte_sidecar(&GAMBATTE_SIDECAR, 1_000_000_000), None);
        assert_eq!(parse_gambatte_sidecar(&[1, 2, 3], GAMBATTE_WRITTEN_AT), None);
    }

    /// mincer_ray's sidecar for the moment `RUBY_FOOTER` holds — 10:39:43 on Wednesday 2026-08-12,
    /// taken at 1786556383 — packed the way the core's author packs one in the repair tool
    /// published beside the core, rather than by this module's own builder.
    const MINCER_SIDECAR: [u8; 16] =
        [0xdf, 0xaf, 0x7c, 0x6a, 0xc3, 0x1c, 0x34, 0x09, 0x99, 0x00, 0, 0, 0, 0, 0, 0];

    #[test]
    fn reads_a_mincer_sidecar() {
        let clock = parse_mincer_sidecar(&MINCER_SIDECAR).expect("a Seiko S-3511A");
        // The seven components mGBA latched for the same moment, out of a different layout.
        assert_eq!(clock.components, RUBY_FOOTER[..7]);
        // The core never stores the register, and comes up in 24-hour mode.
        assert_eq!(clock.control, 0x40);
        // The calendar is the reading and the timestamp the anchor, so no instant needs supplying.
        assert_eq!(clock.reading().expect("a date").reading, 1_786_531_183);
        assert_eq!(clock.latched_at, 1_786_556_383);
        assert_eq!(build_mincer_sidecar(&clock.components, clock.control, clock.latched_at), MINCER_SIDECAR);
    }

    #[test]
    fn the_two_sidecars_are_told_apart_by_length() {
        assert!(matches!(parse_sidecar(&MINCER_SIDECAR, 0), Some(Clock::S3511a(_))));
        assert!(matches!(parse_sidecar(&GAMBATTE_SIDECAR, GAMBATTE_WRITTEN_AT), Some(Clock::Mbc3(_))));
        assert_eq!(parse_sidecar(&MINCER_SIDECAR[..8], 0), None);
    }

    #[test]
    fn a_mincer_record_the_core_would_refuse_is_not_a_clock() {
        // The core replaces each of these with the Pocket's own clock, so none is the clock it ran.
        let mut erased = MINCER_SIDECAR;
        erased[..4].copy_from_slice(&[0xff; 4]);
        let mut unset = MINCER_SIDECAR;
        unset[..4].copy_from_slice(&[0; 4]);
        let mut padded = MINCER_SIDECAR;
        padded[15] = 0xff;
        let mut wide = MINCER_SIDECAR;
        wide[9] = 0x04; // bit 42, past the calendar
        for bad in [erased, unset, padded, wide] {
            assert_eq!(parse_mincer_sidecar(&bad), None, "{bad:02x?}");
        }

        // The day has to be one the month has, leap years included.
        let on = |components: [u8; 7]| build_mincer_sidecar(&components, 0x40, 1_786_556_383);
        assert_eq!(parse_mincer_sidecar(&on([0x27, 0x02, 0x29, 0x01, 0x10, 0, 0])), None);
        assert!(parse_mincer_sidecar(&on([0x28, 0x02, 0x29, 0x02, 0x10, 0, 0])).is_some());
        assert_eq!(parse_mincer_sidecar(&on([0x26, 0x04, 0x31, 0x04, 0x10, 0, 0])), None);
    }

    #[test]
    fn a_twelve_hour_clock_goes_into_the_record_on_the_twenty_four_hour_one() {
        // In 12-hour mode the chip latches 10 PM as 10 with the PM flag. The record has no flag.
        let record =
            build_mincer_sidecar(&[0x26, 0x08, 0x12, 0x03, 0x80 | 0x10, 0x39, 0x43], 0, 1_786_556_383);
        assert_eq!(parse_mincer_sidecar(&record).expect("a clock").components[4], 0x22);
        // In 24-hour mode the hour already is one, and a PM flag beside it is dropped.
        let record =
            build_mincer_sidecar(&[0x26, 0x08, 0x12, 0x03, 0x80 | 0x22, 0x39, 0x43], 0x40, 1_786_556_383);
        assert_eq!(parse_mincer_sidecar(&record).expect("a clock").components[4], 0x22);
    }

    #[test]
    fn the_seiko_chip_goes_back_in_the_shape_its_producer_writes() {
        use one_saves::{Header, Name, Part, Source};
        let clock = parse_mincer_sidecar(&MINCER_SIDECAR).expect("a clock");
        let split = Split {
            sram: Vec::new(),
            footer: Vec::new(),
            reading: clock.reading(),
            clock: Clock::S3511a(clock),
        };
        let bundle = |core: &str| {
            let header = Header::default()
                .source(Source::default().core(Name::parse(core).unwrap()))
                .extensions(split.extensions());
            one_saves::Bundle::new(header, vec![Part::new(0, *b"SAVE")])
        };

        let mincer = bundle("mincer-ray-gba");
        assert_eq!(form_of(&mincer), ClockForm::S3511aSidecar);
        // Kept in a file of its own, so the save carries nothing.
        assert_eq!(footer_of(&mincer), None);
        assert_eq!(sidecar_of(&mincer).as_deref(), Some(&MINCER_SIDECAR[..]));

        // The same clock handed to mGBA is mGBA's own footer for that moment, byte for byte.
        let mgba = bundle("mgba");
        assert_eq!(form_of(&mgba), ClockForm::S3511a);
        assert_eq!(footer_of(&mgba).as_deref(), Some(&RUBY_FOOTER[..]));
    }

    /// One record of the block GBA_MiSTer appends, for the moment `RUBY_FOOTER` holds: the word
    /// `"RT"`, then the calendar and the instant as `GBA.sv` lays them into `time_din_h`, encoded
    /// from the core's source rather than by this module's own builder.
    const MISTER_GBA_RECORD: [u8; 16] =
        [0x54, 0x52, 0xc3, 0x1c, 0x34, 0x09, 0x99, 0x7c, 0xbf, 0xf2, 0xa9, 0x01, 0, 0, 0, 0];

    /// The whole block: one SD block, which the core fills with copies of the record.
    fn mister_gba_block() -> Vec<u8> {
        MISTER_GBA_RECORD.repeat(PACKED_LEN_MISTER / MISTER_GBA_RECORD.len())
    }

    #[test]
    fn reads_a_mister_gba_block() {
        let clock = parse_s3511a_packed(&mister_gba_block()).expect("a Seiko S-3511A");
        // The calendar mincer_ray's port keeps, with the instant after it rather than before.
        assert_eq!(clock.components, RUBY_FOOTER[..7]);
        assert_eq!(clock.control, 0x40);
        assert_eq!(clock.reading().expect("a date").reading, 1_786_531_183);
        assert_eq!(clock.latched_at, 1_786_556_383);
        assert_eq!(
            build_s3511a_packed(&clock.components, clock.control, clock.latched_at, PACKED_LEN_MISTER),
            mister_gba_block()
        );
    }

    #[test]
    fn a_mister_gba_save_goes_back_as_its_core_wrote_it() {
        use one_saves::{Header, Name, Part, Source};
        let mut save: Vec<u8> = (0..131_072u32).map(|i| (i % 251) as u8).collect();
        save.extend_from_slice(&mister_gba_block());

        let split = split(&save, Some("gba")).expect("has a clock");
        assert_eq!(split.sram.len(), 131_072);
        assert!(matches!(split.clock, Clock::S3511a(_)));
        let bundle = |core: &str| {
            let header = Header::default()
                .source(Source::default().core(Name::parse(core).unwrap()))
                .extensions(split.extensions());
            one_saves::Bundle::new(header, vec![Part::new(0, *b"SAVE")])
        };

        let mister = bundle("gba-mister");
        assert_eq!(form_of(&mister), ClockForm::S3511aPacked(PACKED_LEN_MISTER));
        assert_eq!(footer_of(&mister), Some(mister_gba_block()));
        // One moment, three producers' files for it, each byte for byte what that producer writes.
        assert_eq!(footer_of(&bundle("mgba")).as_deref(), Some(&RUBY_FOOTER[..]));
        assert_eq!(sidecar_of(&mister).as_deref(), Some(&MINCER_SIDECAR[..]));
    }

    #[test]
    fn a_gba_save_never_carries_a_game_boy_clock() {
        // GBA_MiSTer's block is 512 bytes, as the MiSTer Gameboy core's is, and this used to be
        // read as that core's MBC3 — so a GBA save got a Game Boy clock in its header and a
        // 48-byte Game Boy footer written back where the block had been. The tail here is the
        // placeholder save-file-converter's test data uses: not a clock in either layout.
        let mut placeholder = vec![0u8; 131_072];
        placeholder.extend_from_slice(&[0x12, 0x34, 0x56, 0x78].repeat(128));
        assert_eq!(split(&placeholder, Some("gba")), None);

        // A real Game Boy footer on a GBA save is not a GBA clock either, at either length.
        let mut gb_block = vec![0u8; 32_768];
        gb_block.extend_from_slice(&mister_footer());
        assert_eq!(split(&gb_block, Some("gba")), None);
        assert!(matches!(split(&gb_block, Some("gb")).unwrap().clock, Clock::Mbc3(_)));
        let mut mbc3 = vec![0u8; 32_768];
        mbc3.extend_from_slice(&SAMEBOY_FOOTER);
        assert_eq!(split(&mbc3, Some("gba")), None);
    }

    #[test]
    fn a_block_that_is_not_the_cores_output_is_left_alone() {
        // The core checks the word and nothing else. Anything this module could not write back as
        // it found it stays in the payload.
        let mut unmarked = mister_gba_block();
        unmarked[..2].copy_from_slice(b"RT"); // the word the wrong way round
        let mut disagreeing = mister_gba_block();
        disagreeing[300] ^= 1;
        let mut wide = mister_gba_block();
        for copy in wide.chunks_mut(MISTER_GBA_RECORD.len()) {
            copy[11] |= 0x04; // bit 74, past the instant
        }
        for bad in [unmarked, disagreeing, wide] {
            assert_eq!(parse_s3511a_packed(&bad), None);
        }
    }

    #[test]
    fn an_unstated_system_tells_the_two_blocks_apart() {
        // The GBA core's record opens with a word the Game Boy core never writes there, so with
        // no system named each block is still read as the chip that wrote it.
        let mut gba = vec![0u8; 131_072];
        gba.extend_from_slice(&mister_gba_block());
        assert!(matches!(split(&gba, None).unwrap().clock, Clock::S3511a(_)));
        let mut gb = vec![0u8; 32_768];
        gb.extend_from_slice(&mister_footer());
        assert!(matches!(split(&gb, None).unwrap().clock, Clock::Mbc3(_)));
    }

    #[test]
    fn the_seiko_reading_is_the_calendar_and_not_the_anchor() {
        // GBA_MiSTer powers the chip on at 2009-12-31 23:59:45, and nine minutes of a game that
        // never set it later, it reads 2010-01-01 00:08:47 while the host says 2026.
        let clock = S3511aClock {
            components: [0x10, 0x01, 0x01, 0x00, 0x00, 0x08, 0x47],
            control: 0x40,
            latched_at: 1_792_576_838,
        };
        assert_eq!(clock.reading().expect("a date").reading, 1_262_304_527);
        // In 12-hour mode the PM flag is what puts the hour after noon.
        let pm = S3511aClock {
            components: [0x26, 0x08, 0x12, 0x03, 0x80 | 0x10, 0x39, 0x43],
            control: 0,
            ..clock
        };
        assert_eq!(pm.reading().expect("a date").reading, 1_786_531_183 + 12 * 3_600);
        // A day the month does not have is not a date, so there is no reading to carry.
        let no_date = S3511aClock { components: [0x26, 0x02, 0x30, 0x01, 0x10, 0, 0], ..clock };
        assert_eq!(no_date.reading(), None);
        assert!(
            !Split { sram: Vec::new(), footer: Vec::new(), reading: None, clock: Clock::S3511a(no_date) }
                .extensions()
                .contains_key(&Rtc::KEY)
        );
    }

    #[test]
    fn a_bundle_from_before_the_anchor_had_a_key_still_rebuilds() {
        // One-saves 0.4.0 wrote the anchor as the reading and left key 2 out. Its bundles still
        // hand mGBA back the footer it wrote.
        use one_saves::{Header, Name, Part, Source};
        let mut extensions = one_saves::Extensions::new();
        extensions.insert(Rtc::new(1_786_556_383));
        extensions.insert(RtcS3511a {
            components: RUBY_FOOTER[..7].try_into().unwrap(),
            control: 0x40,
            taken_at: None,
        });
        let header = Header::default()
            .source(Source::default().core(Name::parse("mgba").unwrap()))
            .extensions(extensions);
        let bundle = one_saves::Bundle::new(header, vec![Part::new(0, *b"SAVE")]);
        assert_eq!(footer_of(&bundle).as_deref(), Some(&RUBY_FOOTER[..]));
    }

    #[test]
    fn a_long_running_clock_keeps_its_day_counter_and_carry() {
        // 600 days is past the nine bits the counter holds, which is what the carry flag is for.
        let base = 1_700_000_000i64;
        let captured = base + 600 * 86_400 + 3 * 3_600 + 4 * 60 + 5;
        let clock =
            parse_gambatte_sidecar(&u32::try_from(base).unwrap().to_be_bytes(), captured).expect("a clock");
        assert_eq!((clock.hours, clock.minutes, clock.seconds), (3, 4, 5));
        assert_eq!(clock.days(), 0x258 & 0x1FF, "600 days wraps to the low nine bits");
        assert!(clock.overflowed(), "and the carry flag says it did");
    }

    /// The two real Pokémon Crystal footers written by budude2's openFPGA core, 2673 seconds
    /// apart. Both fields advance by exactly that, which is what confirms the bit packing: read
    /// as a plain seconds counter they appear to disagree by 432 s.
    const POCKET_FIRST: [u8; 16] =
        [0x75, 0x52, 0x7c, 0x6a, 0x47, 0x3c, 0x05, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
    const POCKET_SECOND: [u8; 16] =
        [0xe6, 0x5c, 0x7c, 0x6a, 0x68, 0x48, 0x05, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];

    #[test]
    fn reads_a_real_pocket_clock() {
        let first = parse_packed(&POCKET_FIRST).expect("an MBC3 clock");
        assert_eq!((first.hours, first.minutes, first.seconds), (19, 49, 7));
        assert_eq!(first.days(), 2);
        assert!(!first.halted() && !first.overflowed());
        assert_eq!(first.written_at, 1_786_532_469);

        let second = parse_packed(&POCKET_SECOND).expect("an MBC3 clock");
        assert_eq!((second.hours, second.minutes, second.seconds), (20, 33, 40));
    }

    #[test]
    fn both_pocket_fields_advance_together() {
        // The check that settles the bit packing. A save taken later has both its timestamp and
        // its clock advanced by the same wall-clock interval, because an MBC3 counts real time.
        // Misreading the registers as a seconds counter breaks this by 432 s.
        let a = parse_packed(&POCKET_FIRST).expect("a clock");
        let b = parse_packed(&POCKET_SECOND).expect("a clock");
        let elapsed = |c: &Mbc3Clock| {
            i64::from(c.days()) * 86_400
                + i64::from(c.hours) * 3_600
                + i64::from(c.minutes) * 60
                + i64::from(c.seconds)
        };
        assert_eq!(b.written_at - a.written_at, 2673);
        assert_eq!(elapsed(&b) - elapsed(&a), 2673);
    }

    #[test]
    fn a_pocket_footer_rebuilds_byte_for_byte() {
        for original in [POCKET_FIRST, POCKET_SECOND] {
            let clock = parse_packed(&original).expect("a clock");
            assert_eq!(build_packed(&clock, PACKED_LEN_POCKET), original);
        }
    }

    #[test]
    fn the_pocket_day_counter_is_wider_than_the_hardware_register() {
        // The core keeps ten bits of days and a separate overflow flag, where the appended MBC3
        // footer has nine bits with the carry above them. Past 511 days the two disagree, and
        // this is the boundary where that starts.
        let mut footer = POCKET_FIRST;
        let packed = 600u64 << 17 | 1 << 27; // 600 days, overflow set
        footer[4..10].copy_from_slice(&packed.to_le_bytes()[..6]);

        let clock = parse_packed(&footer).expect("a clock");
        assert_eq!(clock.days(), 0x258 & 0x1FF, "600 days wraps into the nine-bit register");
        assert!(clock.overflowed(), "and the carry flag survives");
        // Rebuilding recovers what the register can hold, which is not the same as what went in.
        assert_ne!(build_packed(&clock, PACKED_LEN_POCKET), footer, "past 511 days the two forms differ");
    }

    /// The trailing block of a real Pokémon Crystal save from the MiSTer Gameboy core: the same
    /// ten bytes as the Pocket port, padded to a whole 512-byte SD block with `0xFFFF`.
    fn mister_footer() -> Vec<u8> {
        let mut footer = vec![0x38, 0x57, 0x7c, 0x6a, 0x91, 0, 0, 0, 0, 0];
        footer.resize(PACKED_LEN_MISTER, 0xFF);
        footer
    }

    #[test]
    fn reads_a_real_mister_clock() {
        let clock = parse_packed(&mister_footer()).expect("an MBC3 clock");
        assert_eq!((clock.hours, clock.minutes, clock.seconds), (0, 2, 17));
        assert_eq!(clock.days(), 0);
        assert_eq!(clock.written_at, 1_786_533_688);
        // One format, two reserved sizes: the ten written bytes are laid out identically.
        assert_eq!(build_packed(&clock, PACKED_LEN_MISTER), mister_footer());
        assert_eq!(&build_packed(&clock, PACKED_LEN_POCKET)[..10], &mister_footer()[..10]);
    }

    #[test]
    fn the_reserved_size_is_the_producers_and_round_trips() {
        // MiSTer writes a whole SD block, its openFPGA ports write sixteen bytes, and the clock
        // inside is the same. Writing back in the wrong one gives a file the core would not.
        let clock = parse_packed(&mister_footer()).expect("a clock");
        assert_eq!(build_packed(&clock, PACKED_LEN_MISTER).len(), 512);
        assert_eq!(build_packed(&clock, PACKED_LEN_POCKET).len(), 16);
        assert!(build_packed(&clock, PACKED_LEN_MISTER)[10..].iter().all(|&b| b == 0xFF));
    }

    #[test]
    fn the_producer_picks_the_reserved_size() {
        use one_saves::{Header, Name, Part, Source};
        let clock = parse_packed(&mister_footer()).expect("a clock");
        let split = Split {
            sram: Vec::new(),
            footer: Vec::new(),
            reading: Some(clock.reading()),
            clock: Clock::Mbc3(clock),
        };
        for (core, want) in [
            ("gameboy-mister", PACKED_LEN_MISTER),
            ("sgb-mister", PACKED_LEN_MISTER),
            ("budude2-gbc", PACKED_LEN_POCKET),
            ("spiritualized-gb", PACKED_LEN_POCKET),
        ] {
            let header = Header::default()
                .source(Source::default().core(Name::parse(core).unwrap()))
                .extensions(split.extensions());
            let bundle = one_saves::Bundle::new(header, vec![Part::new(0, *b"SAVE")]);
            assert_eq!(form_of(&bundle), ClockForm::Packed(want), "{core}");
            assert_eq!(footer_of(&bundle).map(|f| f.len()), Some(want), "{core}");
        }
    }

    #[test]
    fn a_gba_save_of_the_same_length_is_read_as_the_seiko_chip() {
        // Both producers write sixteen bytes; the system is what tells them apart.
        let mut gba: Vec<u8> = vec![0u8; 131_072];
        gba.extend_from_slice(&RUBY_FOOTER);
        assert!(matches!(split(&gba, Some("gba")).unwrap().clock, Clock::S3511a(_)));

        let mut gb: Vec<u8> = vec![0u8; 32_768];
        gb.extend_from_slice(&POCKET_FIRST);
        assert!(matches!(split(&gb, Some("gb")).unwrap().clock, Clock::Mbc3(_)));
    }

    #[test]
    fn the_producer_decides_which_shape_is_written_back() {
        // Four producers, one clock, four encodings. The bundle records the clock; `source.core`
        // is what says which shape to put it back in, so a round trip returns the file the
        // emulator would have written rather than some other emulator's idea of it.
        use one_saves::{Header, Name, Part, Source};
        let clock = parse_packed(&POCKET_FIRST).expect("a clock");
        let split = Split {
            sram: Vec::new(),
            footer: Vec::new(),
            reading: Some(clock.reading()),
            clock: Clock::Mbc3(clock),
        };

        for (core, want) in [
            ("budude2-gbc", ClockForm::Packed(PACKED_LEN_POCKET)),
            ("gambatte", ClockForm::Sidecar),
            ("sameboy", ClockForm::Mbc3Appended),
            ("mgba", ClockForm::Mbc3Appended),
        ] {
            let header = Header::default()
                .source(Source::default().core(Name::parse(core).unwrap()))
                .extensions(split.extensions());
            let bundle = one_saves::Bundle::new(header, vec![Part::new(0, *b"SAVE")]);
            assert_eq!(form_of(&bundle), want, "{core}");

            match want {
                // A sidecar is not appended, so there is nothing for the save to carry.
                ClockForm::Sidecar => {
                    assert_eq!(footer_of(&bundle), None);
                    assert!(sidecar_of(&bundle).is_some());
                }
                ClockForm::Packed(PACKED_LEN_POCKET) => {
                    assert_eq!(footer_of(&bundle).as_deref(), Some(&POCKET_FIRST[..]));
                }
                _ => assert_eq!(footer_of(&bundle).map(|f| f.len()), Some(MBC3_LEN_64)),
            }
        }
    }

    #[test]
    fn the_core_picks_the_layout_and_the_app_around_it_does_not() {
        // RetroArch writes whatever its core writes, so the same app lays a clock down two ways.
        // And an `app` of `gambatte` is a 0.3 spelling that now names an app nobody listed: read
        // as written, it says nothing about a core, so it gets the common form.
        use one_saves::{Header, Name, Part, Source};
        let clock = parse_packed(&POCKET_FIRST).expect("a clock");
        let split = Split {
            sram: Vec::new(),
            footer: Vec::new(),
            reading: Some(clock.reading()),
            clock: Clock::Mbc3(clock),
        };
        let name = |text| Name::parse(text).ok();
        for (source, want) in [
            (Source::default().app(name("retroarch")).core(name("gambatte")), ClockForm::Sidecar),
            (Source::default().app(name("retroarch")).core(name("sameboy")), ClockForm::Mbc3Appended),
            (Source::default().app(name("gambatte")), ClockForm::Mbc3Appended),
        ] {
            let header = Header::default().source(source.clone()).extensions(split.extensions());
            let bundle = one_saves::Bundle::new(header, vec![Part::new(0, *b"SAVE")]);
            assert_eq!(form_of(&bundle), want, "{source:?}");
        }
    }

    #[test]
    fn the_layout_comes_from_the_registry_and_not_from_the_shape_of_the_slug() {
        // This used to match `-mister` and `-gb` as suffixes, which got two things wrong. A core
        // whose name ends some other way was read as an appended footer however it actually
        // writes, and every MiSTer core was read as the Game Boy one however little it has to do
        // with a clock. Both are registry questions, and the registry now answers them.
        use one_saves::{Header, Name, Part, Source};
        let clock = parse_packed(&POCKET_FIRST).expect("a clock");
        let split = Split {
            sram: Vec::new(),
            footer: Vec::new(),
            reading: Some(clock.reading()),
            clock: Clock::Mbc3(clock),
        };

        for (core, want) in [
            // An openFPGA Game Boy port whose slug does not end in `-gb`. The suffix rule read
            // this as an appended footer and wrote 48 bytes where the core wants 16.
            ("spiritualized-supergb", ClockForm::Packed(PACKED_LEN_POCKET)),
            // MiSTer cores for systems with no MBC3 anywhere near them. The suffix rule gave each
            // of these the Game Boy core's 512-byte block.
            ("psx-mister", ClockForm::Mbc3Appended),
            ("n64-mister", ClockForm::Mbc3Appended),
            ("saturn-mister", ClockForm::Mbc3Appended),
            // A producer the registry does not list still gets the common form.
            ("pcsx2", ClockForm::Mbc3Appended),
        ] {
            let header = Header::default()
                .source(Source::default().core(Name::parse(core).unwrap()))
                .extensions(split.extensions());
            let bundle = one_saves::Bundle::new(header, vec![Part::new(0, *b"SAVE")]);
            assert_eq!(form_of(&bundle), want, "{core}");
        }
    }

    #[test]
    fn an_unrecognised_tail_is_not_filed_under_a_chip_key() {
        // The chip key is chip state and nothing else. Bytes this module cannot identify stay in
        // the payload rather than being labelled as a clock they may not be.
        let mut save = vec![0u8; 32_768];
        save.extend_from_slice(&[0xAB; 12]);
        assert_eq!(split(&save, Some("gb")), None);
    }
}
