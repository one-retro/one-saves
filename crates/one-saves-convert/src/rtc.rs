//! Real-time clock state that emulators append to a save file.
//!
//! A cartridge with a clock chip — Pokémon Crystal, Pokémon Ruby and friends — has state the save
//! memory does not hold, and emulators disagree about where to put it. These arrangements are
//! handled here, all confirmed against files real emulators wrote:
//!
//! | producer | where | what it holds |
//! | -------- | ----- | ------------- |
//! | the mGBA app, GBA | appended, 16 bytes | latched BCD, a control register, a latch instant |
//! | the mGBA app, standalone SameBoy and VBA-M, Game Boy | appended, 48 bytes | ten MBC3 registers and a write time |
//! | MiSTer Gameboy, SGB | appended, 512 bytes | a 4-byte host clock, **bit-packed** registers, an MBC3's or a HuC-3's |
//! | budude2's Pocket port | appended, 16 bytes | the same ten bytes, a smaller reserved region |
//! | mGBA and VBA-M under libretro, Game Boy | a `.rtc` file, 48 bytes | the ten registers and the write time the app appends |
//! | SameBoy under libretro, Game Boy | a `.rtc` file, 32 bytes | the registers a byte each, a write time, the cycles into the second |
//! | Gambatte under libretro, Game Boy | a `.rtc` file, 8 bytes | an **origin**, little-endian |
//! | Gambatte in OpenEmu, Game Boy | a `.rtc` file, 4 bytes | an **origin**, big-endian |
//! | mincer_ray, GBA | a `.rtc` file, 16 bytes | an instant and the calendar **bit-packed** |
//! | GBA_MiSTer, GBA | appended, 512 bytes | the same two, the other way round, **repeated** |
//! | Gambatte and VBA-M under libretro, a HuC-3 | a `.rtc` file, 8 bytes | an **origin**, as Gambatte keeps for an MBC3 |
//! | the mGBA app, a HuC-3 | appended, 136 bytes | the controller's 256 nybbles and a latch instant |
//!
//! mGBA's and SameBoy's libretro cores keep no HuC-3 clock at all: mGBA writes no `.rtc` for one, and
//! SameBoy's is its MBC3 section with nothing in it.
//!
//! Two of the appended forms are sixteen bytes and two are 512, and each pair means entirely
//! different things by them, so the system is what tells them apart. Which shape a clock goes back
//! into is decided by [`form_of`] from the bundle's `source`, since the encoding belongs to the
//! producer rather than to the cartridge: and to the app as well as the core, because a libretro
//! frontend writes the clock its core exposes to a file of its own, where the same engine run as an
//! app of its own appends it. [`form_for`] makes the same choice for a core about to read the save.
//!
//! The appended forms say nothing about where the save memory ends, so they are found by length.
//! The sidecars are told apart by the record the producer's registry entry names, and by length
//! where nobody named the producer. Gambatte's is a different problem from the rest: it stores the
//! instant the clock counts *from*, which is the other operand of a reading rather than a reading,
//! and turning one into the other needs an instant the file does not record. See [`parse_origin`].
//! The others record their own instant; see [`parse_sidecar_as`].
//!
//! Left inline, that footer is a problem rather than a curiosity: it carries a **live timestamp**,
//! so two dumps of the same save minutes apart hash differently, and a
//! [content hash](one_saves::Bundle::content_hash) that moves on its own is one a store cannot
//! deduplicate on. Splitting it out is what makes a save's bytes stable.
//!
//! The shape follows what [`x.1sav.rtc`] prescribes, which is to carry two keys rather than one:
//!
//! - the save part's payload is the bare save memory,
//! - the portable reading goes in [`x.1sav.rtc`] as an [`Rtc`], on the Unix scale, so a consumer
//!   can read it without knowing which chip produced it,
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
//! Neither belongs to an emulator. The MBC3 footer is written identically by VBA, BGB, mGBA and
//! SameBoy, apart from the width of its timestamp, which is what makes it readable at all, so both
//! keys sit under `x.1sav.rtc` in the
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
//! That includes most other cartridge clocks. The specification gives each one a key — the HuC-3,
//! TPP1 and TAMA5 on the Game Boy, the two Super Famicom chips, the N64's Joybus clock — and
//! [`one_saves::extension`] has a type for each. Of those, only the HuC-3 is read here, and only
//! when the caller says the cartridge has one, since nothing in any producer's bytes tells it from
//! an MBC3: the MiSTer cores' packed footer, the mGBA app's, and the origin Gambatte and VBA-M keep
//! beside the save (see [`split_for`] and [`parse_huc3_sidecar`]). Standalone SameBoy and VBA-M
//! append records of their own for one, which are not read, since the registry lists neither as an
//! app. No producer's file for the other chips is read here yet.
//!
//! [`x.1sav.rtc`]: https://docs.1retro.com/specifications/extensions/x.1sav.rtc/
//! [`io.mgba.rtc`]: https://docs.1retro.com/specifications/extensions/io.mgba.rtc/
//! [`x.1sav.rtc.s3511a`]: https://docs.1retro.com/specifications/extensions/x.1sav.rtc.s3511a/

use one_saves::extension::{Rtc, RtcHuc3, RtcMbc3, RtcS3511a, S3511aAlarm};
use one_saves::{Extension, Extensions};
use one_saves_registry::ClockLayout;
/// How a core keeps a HuC-3's clock, as the registry names it.
pub use one_saves_registry::Huc3Layout;
/// What a clock kept in a file of its own holds, as the registry names it.
pub use one_saves_registry::SidecarRecord;

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
#[allow(
    clippy::large_enum_variant,
    reason = "a HuC-3 carries the controller's whole window where the producer keeps it; one clock is read per save, and copying it is cheaper than the boxing that would cost every caller"
)]
pub enum Clock {
    /// A Game Boy MBC3 cartridge's clock.
    Mbc3(Mbc3Clock),
    /// A GBA cartridge's Seiko S-3511A.
    S3511a(S3511aClock),
    /// A Game Boy HuC-3 cartridge's clock, as the MiSTer cores keep it.
    Huc3(Huc3Clock),
}

impl Clock {
    /// What this clock showed, on the Unix scale, when that can be computed.
    ///
    /// An MBC3's and a HuC-3's always can. A Seiko chip's cannot when its components are not a
    /// date.
    #[must_use]
    pub fn reading(self) -> Option<Rtc> {
        match self {
            Clock::Mbc3(clock) => Some(clock.reading()),
            Clock::S3511a(clock) => clock.reading(),
            Clock::Huc3(clock) => Some(clock.reading()),
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
    pub taken_at: i64,
}

impl S3511aClock {
    /// Whether the chip is in 24-hour mode, which is bit 6 of the control register.
    #[must_use]
    pub fn hour24(self) -> bool {
        self.control & 0x40 != 0
    }

    /// What this chip showed, on the Unix scale: its components, read as though they were UTC,
    /// as [`RtcS3511a::reading`] reads them.
    ///
    /// `taken_at` is not the answer. It is when the components were taken rather than what they
    /// say, and the two can be years apart. Where the chip does follow the host, as mGBA's does,
    /// the reading sits the host's zone offset from the anchor, which is one wall clock on two
    /// scales.
    ///
    /// `None` when the components are not a date, since a reading that cannot be computed is
    /// omitted rather than approximated.
    #[must_use]
    pub fn reading(self) -> Option<Rtc> {
        RtcS3511a::from(self).reading()
    }
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

impl From<S3511aClock> for RtcS3511a {
    /// The chip's own key carries the components, the control register and the moment the
    /// producer took them, which between them are everything its file is rebuilt from.
    ///
    /// The alarm is unknown. No file read here keeps the register, and nothing here knows which
    /// game wrote the save, so nothing can say the game never set it.
    fn from(clock: S3511aClock) -> Self {
        RtcS3511a {
            components: clock.components,
            control: clock.control,
            taken_at: Some(clock.taken_at),
            alarm: S3511aAlarm::Unknown,
        }
    }
}

/// The Seiko chip a bundle carries, and the anchor to rebuild its producer's file from.
///
/// The anchor is key 2. Where a bundle has none, the chip resumes from its components with no
/// time passed, so the anchor is the moment of rebuilding, which is what `x.1sav.rtc` says a
/// consumer should use. Neither the reading nor `bundled_at` stands in for it: the one is what the
/// chip says rather than when it was taken, and the other is when the bundle was assembled.
fn s3511a_of(bundle: &one_saves::Bundle) -> Option<(RtcS3511a, i64)> {
    let chip = bundle.header.extensions.get::<RtcS3511a>()?;
    Some((*chip, chip.taken_at.unwrap_or_else(now)))
}

/// The host clock, in epoch seconds.
pub(crate) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|since| i64::try_from(since.as_secs()).ok())
        .unwrap_or(0)
}

/// The MBC3 clock, as the de-facto save footer stores it.
///
/// Ten little-endian 32-bit registers — the live clock, then the latched copy a game reads — and
/// then the host clock the emulator advances them from. The other sources keep less, and say so
/// with `None`.
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
    /// The latched copy the game reads, in the same order, or `None` where the source keeps none,
    /// as Gambatte's sidecar and the MiSTer core's packed footer do not.
    pub latched: Option<[u32; 5]>,
    /// The host clock the emulator advances the counters from, in epoch seconds: the anchor,
    /// which is key 2 of `x.1sav.rtc.mbc3`. SameBoy takes it when it writes the save, VBA-M and
    /// mGBA when the game last latched the clock.
    pub taken_at: i64,
    /// How many bytes that timestamp occupied in the footer: 4 or 8, depending on the emulator.
    /// The MiSTer core and its Pocket ports write 4, whatever length they pad the footer to.
    ///
    /// Carried because the footer is regenerated rather than stored, and the two widths give
    /// different files for the same clock. `None` for a clock kept outside a footer, as
    /// Gambatte's sidecar is.
    pub timestamp_width: Option<usize>,
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

    /// What this clock showed: the counters, put on the Unix scale as that long after 1970-01-01
    /// UTC, as [`RtcMbc3::reading`] reads them.
    ///
    /// Not the anchor, which is a host time rather than anything the cartridge counted, and not
    /// the anchor plus the counters either. What the game displays is this plus an offset of its
    /// own, in save memory where only something that knows the game can find it.
    #[must_use]
    pub fn reading(self) -> Rtc {
        RtcMbc3::from(self).reading()
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
            taken_at: clock.taken_at,
            timestamp_width: clock.timestamp_width.map(|width| u8::try_from(width).expect("4 or 8")),
        }
    }
}

/// Where a HuC-3's two counters sit in its controller's window: three nybbles of minutes, then
/// four of days, least significant first.
const HUC3_COUNTERS: usize = 0x10;

/// Where a HuC-3 keeps the time its alarm goes off at, laid out as the counters are, and the
/// address after it whose low bit turns the alarm on.
const HUC3_ALARM: usize = 0x58;

/// A HuC-3's alarm, where a producer keeps it apart from the rest of the controller: the time it
/// goes off, as a minute of the day and a day count, and whether it is on. SameBoy keeps it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Huc3Alarm {
    /// The minute of the day, three nybbles at 0x58.
    pub minutes: u16,
    /// The day count, four nybbles at 0x5B.
    pub days: u16,
    /// Bit 0 of 0x5F.
    pub enabled: bool,
}

impl Huc3Alarm {
    /// The eight nybbles it occupies from 0x58, least significant first.
    fn nybbles(self) -> [u8; 8] {
        let nybble = |value: u16, at: u16| u8::try_from((value >> (4 * at)) & 0x0F).expect("a nybble");
        [
            nybble(self.minutes, 0),
            nybble(self.minutes, 1),
            nybble(self.minutes, 2),
            nybble(self.days, 0),
            nybble(self.days, 1),
            nybble(self.days, 2),
            nybble(self.days, 3),
            u8::from(self.enabled),
        ]
    }

    /// The alarm out of a whole window, filled as the specification fills a null.
    fn of(memory: &[u8; 256]) -> Self {
        let value = |at: usize, len: usize| {
            memory[at..at + len].iter().rev().fold(0u16, |value, &nybble| value << 4 | u16::from(nybble))
        };
        Huc3Alarm {
            minutes: value(HUC3_ALARM, 3),
            days: value(HUC3_ALARM + 3, 4),
            enabled: memory[HUC3_ALARM + 7] & 1 != 0,
        }
    }
}

/// A HuC-3's clock: its two counters and the seconds counted into the minute, with the host clock
/// they were taken at, and the rest of the controller where the producer keeps it.
///
/// The cartridge's controller has 256 addresses a game reads a nybble at a time. The MiSTer Game
/// Boy cores keep the counters and the seconds and nothing else of them, Gambatte and VBA-M the
/// instant the counters count from, and the mGBA app every address, so the key has the addresses
/// the producer did not keep null.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Huc3Clock {
    /// Seconds counted into the current minute, 0 to 59: key 2 of `x.1sav.rtc.huc3`.
    pub seconds: u8,
    /// The minute counter, the minute of the day, twelve bits.
    pub minutes: u16,
    /// The day counter, sixteen bits.
    pub days: u16,
    /// The host clock the core advances the counters from, in epoch seconds: key 1.
    pub taken_at: i64,
    /// The controller's whole window, a nybble to an element, where the producer keeps it. Its
    /// counters are the ones above.
    pub memory: Option<[u8; 256]>,
    /// The alarm, where the producer keeps it and not the whole window.
    pub alarm: Option<Huc3Alarm>,
}

impl Huc3Clock {
    /// What this clock showed: its days and minutes after 1970-01-01 UTC, as
    /// [`RtcHuc3::reading`] reads them. The seconds are not part of it, since no game can read
    /// them.
    #[must_use]
    pub fn reading(self) -> Rtc {
        RtcHuc3::from(self).reading().expect("both counters are kept")
    }
}

impl From<Huc3Clock> for RtcHuc3 {
    /// The whole window where the producer kept it. Otherwise the counters go at their addresses,
    /// and every other address is null, since the producer did not keep it.
    fn from(clock: Huc3Clock) -> Self {
        if let Some(memory) = clock.memory {
            return RtcHuc3 {
                memory: memory.map(Some),
                taken_at: Some(clock.taken_at),
                seconds: Some(clock.seconds),
            };
        }
        let minutes = (0..3).map(|at| clock.minutes >> (4 * at));
        let days = (0..4).map(|at| clock.days >> (4 * at));
        let mut memory = [None; 256];
        for (slot, nybble) in memory[HUC3_COUNTERS..].iter_mut().zip(minutes.chain(days)) {
            *slot = Some(u8::try_from(nybble & 0x0F).expect("a nybble"));
        }
        if let Some(alarm) = clock.alarm {
            for (slot, nybble) in memory[HUC3_ALARM..].iter_mut().zip(alarm.nybbles()) {
                *slot = Some(nybble);
            }
        }
        RtcHuc3 { memory, taken_at: Some(clock.taken_at), seconds: Some(clock.seconds) }
    }
}

/// The HuC-3 a bundle carries, filled where its key is null the way the specification fills it:
/// 0 for an address and for the seconds, and the moment of rebuilding for the anchor.
fn huc3_of(bundle: &one_saves::Bundle) -> Option<Huc3Clock> {
    let chip = bundle.header.extensions.get::<RtcHuc3>()?;
    let memory = chip.filled();
    let value = |at: usize, len: usize| {
        memory[at..at + len].iter().rev().fold(0u16, |value, &nybble| value << 4 | u16::from(nybble))
    };
    Some(Huc3Clock {
        seconds: chip.seconds.unwrap_or(0),
        minutes: value(HUC3_COUNTERS, 3),
        days: value(HUC3_COUNTERS + 3, 4),
        taken_at: chip.taken_at.unwrap_or_else(now),
        memory: chip.memory.iter().all(Option::is_some).then_some(memory),
        alarm: chip.memory[HUC3_ALARM..HUC3_ALARM + 8]
            .iter()
            .all(Option::is_some)
            .then(|| Huc3Alarm::of(&memory)),
    })
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
    split_for(bytes, system, None)
}

/// [`split`], for a cartridge whose clock chip is known: `chip` is that chip's key, such as
/// `x.1sav.rtc.huc3`, as its ROM's header says. `rom_id`'s cartridge clock gives it.
///
/// It matters where two chips share a footer. The MiSTer Game Boy cores keep a HuC-3 in the
/// footer they keep an MBC3 in, packed another way, and nothing in the bytes says which. So a
/// packed footer is read as a HuC-3 only when `chip` says the cartridge has one, and then as
/// nothing else. The mGBA app's footer for one, the controller's whole window, is read the same
/// way: no other footer is its length, but nothing in its bytes is checkable enough to find it by
/// without the cartridge saying. A footer of any other length is left in the save, since no other
/// producer's HuC-3 is read here. With no chip, or another one, this is [`split`].
#[must_use]
pub fn split_for(bytes: &[u8], system: Option<&str>, chip: Option<&str>) -> Option<Split> {
    split_at(bytes, system, chip, now())
}

/// [`split_for`], reading a footer that holds an origin rather than the counters at `captured_at`:
/// the VisualBoyAdvance-M app's for a HuC-3, which counts from an instant as Gambatte does. Every
/// other footer records its own instant and ignores this one.
#[must_use]
pub fn split_at(bytes: &[u8], system: Option<&str>, chip: Option<&str>, captured_at: i64) -> Option<Split> {
    let huc3 = chip == Some(RtcHuc3::KEY.as_str());
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
            PACKED_LEN_POCKET | PACKED_LEN_MISTER if huc3 => parse_packed_huc3(footer).map(Clock::Huc3),
            HUC3_MEMORY_LEN if huc3 => parse_huc3_memory(footer).map(Clock::Huc3),
            SAMEBOY_HUC3_LEN if huc3 => parse_sameboy_huc3(footer).map(Clock::Huc3),
            VBAM_HUC3_LEN if huc3 => parse_vbam_huc3(footer, captured_at).map(Clock::Huc3),
            _ if huc3 => None,
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
        // The chip's own clock is the reading: an MBC3's counters as that long after 1970, a
        // Seiko chip's calendar as though it were UTC. The host time stored beside them is the
        // anchor the producer advances them from on the next load, which stays with the chip.
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

    let live = [word(0), word(1), word(2), word(3), word(4)];
    let latched = [word(5), word(6), word(7), word(8), word(9)];
    let taken_at = if footer.len() == MBC3_LEN_64 {
        i64::from_le_bytes(footer[40..48].try_into().ok()?)
    } else {
        i64::from(u32::from_le_bytes(footer[40..44].try_into().ok()?))
    };

    let sane = |[seconds, minutes, hours, days_low, _]: [u32; 5]| {
        seconds < 60 && minutes < 60 && hours < 24 && days_low < 256
    };
    // A negative or absurd host time is not a time; a save written before the Game Boy existed is
    // a misparse rather than a very old save.
    let plausible_instant = taken_at >= 0;
    (sane(live) && sane(latched) && plausible_instant).then_some(Mbc3Clock {
        seconds: live[0],
        minutes: live[1],
        hours: live[2],
        days_low: live[3],
        days_high: live[4],
        latched: Some(latched),
        taken_at,
        timestamp_width: Some(footer.len() - 40),
    })
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
            Clock::Huc3(clock) => extensions.insert(RtcHuc3::from(clock)),
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
    /// Appended to the save: ten little-endian registers and an instant this many bytes wide.
    /// VBA, BGB, the mGBA app, standalone SameBoy.
    Mbc3Appended(u8),
    /// Appended to the save: an instant four bytes wide and the registers bit-packed, padded to a
    /// reserved size. The MiSTer Gameboy and SGB cores and the Gameboy core's openFPGA ports.
    Packed(usize),
    /// Nothing appended, and a file of its own beside the save holding the record: every libretro
    /// core that keeps a clock, and mincer_ray's GBA core.
    ///
    /// A clock of a chip the record does not hold writes no file, since the core keeps no clock
    /// for it: mGBA's libretro core keeps an MBC3's and none for a GBA cartridge.
    Sidecar(SidecarRecord),
    /// Appended to the save: latched BCD and a control register. A GBA cartridge's Seiko chip.
    S3511a,
    /// Appended to the save: the Seiko chip's calendar and an instant, bit-packed behind a magic
    /// word and repeated to fill a reserved size, or cut to it. GBA_MiSTer.
    S3511aPacked(usize),
    /// Appended to the save: an instant four bytes wide and a HuC-3's counters and seconds
    /// bit-packed, padded to a reserved size. The MiSTer Gameboy and SGB cores and the Gameboy
    /// core's openFPGA ports, for a HuC-3 cartridge.
    Huc3Packed(usize),
    /// Appended to the save: a HuC-3 controller's whole window, two nybbles to a byte, and the
    /// host clock it was latched at. The mGBA app, for a HuC-3 cartridge.
    Huc3Memory,
    /// Appended to the save: SameBoy's seventeen bytes, the anchor, the counters and the alarm.
    /// The SameBoy app, for a HuC-3 cartridge.
    Huc3SameBoy,
    /// Appended to the save: VBA-M's 24 bytes, the origin and the controller's state. The
    /// VisualBoyAdvance-M app, for a HuC-3 cartridge.
    Huc3Vbam,
    /// Nothing written anywhere: the reader keeps no clock for this cartridge, and a game run there
    /// starts its clock over. mGBA's and SameBoy's libretro cores, for a HuC-3 cartridge.
    None,
}

/// Whether a core stamps the anchors of the clocks it keeps in its host's local standard time,
/// counted as though it were UTC, rather than in UTC: every MiSTer core. Main_MiSTer hands them all
/// one timestamp, `t + (t - mktime(gmtime(t)))` (`user_io.cpp`, `send_rtc`), which is the zone's
/// standard offset in summer as in winter. Its MBC3, HuC-3 and Seiko anchors are all on that scale.
#[must_use]
pub fn stamps_local_time(core: &one_saves_registry::Core) -> bool {
    core.kind == one_saves_registry::CoreKind::MiSTer
}

/// The offset from UTC of the scale a bundle's clock anchors are on, as far as it is known: what
/// its `source` says, else zero for a producer that stamps UTC. `None` for a producer that stamps
/// local time and did not say which, whose anchors cannot be moved to another scale.
#[must_use]
pub fn scale_of(bundle: &one_saves::Bundle) -> Option<i32> {
    let source = bundle.header.source.as_ref();
    if let Some(offset) = source.and_then(|source| source.utc_offset) {
        return Some(offset);
    }
    match producer_of(source) {
        Some((core, _)) if stamps_local_time(core) => None,
        _ => Some(0),
    }
}

/// Moves every clock anchor a bundle carries by `seconds`: from one producer's scale to another's,
/// `seconds` being the difference between their offsets from UTC.
///
/// Only anchors move. A clock's reading is its counters or its calendar, which no scale changes,
/// and an anchor is only ever the host time the producer advances them from. So a clock moved from
/// a MiSTer in Pacific time to RetroArch has its anchor put eight hours later, and RetroArch counts
/// the time since then as the MiSTer would have.
pub fn rescale(bundle: &mut one_saves::Bundle, seconds: i64) {
    use one_saves::extension::{RtcJoybus, RtcRtc4513, RtcSrtc, RtcTc8521, RtcTpp1};
    if seconds == 0 {
        return;
    }
    let extensions = &mut bundle.header.extensions;
    let moved = |at: i64| at.saturating_add(seconds);
    if let Some(&chip) = extensions.get::<RtcMbc3>() {
        extensions.insert(RtcMbc3 { taken_at: moved(chip.taken_at), ..chip });
    }
    if let Some(&chip) = extensions.get::<RtcS3511a>() {
        extensions.insert(RtcS3511a { taken_at: chip.taken_at.map(moved), ..chip });
    }
    if let Some(chip) = extensions.get::<RtcHuc3>().cloned() {
        extensions.insert(RtcHuc3 { taken_at: chip.taken_at.map(moved), ..chip });
    }
    if let Some(&chip) = extensions.get::<RtcJoybus>() {
        extensions.insert(RtcJoybus { taken_at: chip.taken_at.map(moved), ..chip });
    }
    if let Some(&chip) = extensions.get::<RtcRtc4513>() {
        extensions.insert(RtcRtc4513 { taken_at: chip.taken_at.map(moved), ..chip });
    }
    if let Some(&chip) = extensions.get::<RtcSrtc>() {
        extensions.insert(RtcSrtc { taken_at: chip.taken_at.map(moved), ..chip });
    }
    if let Some(&chip) = extensions.get::<RtcTc8521>() {
        extensions.insert(RtcTc8521 { taken_at: chip.taken_at.map(moved), ..chip });
    }
    if let Some(&chip) = extensions.get::<RtcTpp1>() {
        extensions.insert(RtcTpp1 { taken_at: chip.taken_at.map(moved), ..chip });
    }
}

/// The core and app a `source` names, where the registry lists them. A listed core and app are
/// carried as their slugs, which is what the registry is keyed by.
fn producer_of(
    source: Option<&one_saves::Source>,
) -> Option<(&'static one_saves_registry::Core, Option<&'static one_saves_registry::App>)> {
    let source = source?;
    let core = one_saves_registry::core(source.core.as_ref()?.as_str())?;
    let app = source.app.as_ref().and_then(|app| one_saves_registry::app(app.as_str()));
    Some((core, app))
}

/// The layout the producer a `source` names writes an MBC3's or a Seiko chip's clock in, as the
/// registry records it.
///
/// `source.core` names the engine that wrote the bytes, and `source.app` what ran it, which
/// matters as much: RetroArch writes what its core exposes to a file of its own, and the mGBA app
/// appends the same record to the save. See [`Core::clock_in`](one_saves_registry::Core::clock_in).
/// `None` for a core nobody listed or one the registry records no clock for, and for an app's own
/// build nobody has sampled.
#[must_use]
pub fn layout_of(source: Option<&one_saves::Source>) -> Option<ClockLayout> {
    let (core, app) = producer_of(source)?;
    core.clock_in(app)
}

/// How the producer a `source` names keeps a HuC-3's clock, as the registry records it, by the
/// same rule as [`layout_of`]. See [`Core::huc3_in`](one_saves_registry::Core::huc3_in).
#[must_use]
pub fn huc3_layout_of(source: Option<&one_saves::Source>) -> Option<Huc3Layout> {
    let (core, app) = producer_of(source)?;
    core.huc3_in(app)
}

/// The shape a bundle's producer writes, from what its `source` says.
///
/// The layout each core writes, in the app that ran it, is registry data (see [`layout_of`])
/// rather than anything this module reads out of a slug: a core is not obliged to spell its own
/// name in a way that gives its clock away, and two that do spell it the same way need not agree.
///
/// A bundle naming nobody, or naming a core the registry records no clock for, gets the form most
/// producers use. An appended footer goes back at the width key 3 says it had, which an older
/// build of a core may have written narrower than the registry's entry for it.
///
/// Where the producer as named keeps the clock differently from how this one was read, the clock
/// is what was written. An MBC3 records the width of the footer it was read from as key 3, and
/// leaves the key out for one read from a file of its own, so a footer goes back as one whatever
/// the registry says. And a bundle naming a core and no app means the core as its kind hosts it,
/// or the app that ships the engine as its own: `--from mgba` is said of the mGBA app's `.sav` as
/// much as of RetroArch's `.srm`. So where the core as its kind hosts it keeps no clock of the
/// bundle's chip (mGBA's libretro core keeps neither a Seiko chip's nor a HuC-3's), the clock was
/// read from the app's file, and goes back in the app's shape.
#[must_use]
pub fn form_of(bundle: &one_saves::Bundle) -> ClockForm {
    let width = bundle.header.extensions.get::<RtcMbc3>().and_then(|clock| clock.timestamp_width);
    let as_read = |form| match (form, width) {
        (ClockForm::Mbc3Appended(default), width) => ClockForm::Mbc3Appended(width.unwrap_or(default)),
        (ClockForm::Sidecar(_), Some(width)) => ClockForm::Mbc3Appended(width),
        (form, _) => form,
    };
    let Some((core, app)) = producer_of(bundle.header.source.as_ref()) else {
        return as_read(form_for(bundle, None));
    };
    let form = as_read(form_in(bundle, core, app));
    if app.is_some() || writes_a_clock(bundle, form) {
        return form;
    }
    match one_saves_registry::app(core.slug).filter(|own| own.cores.builds.contains(&core.slug)) {
        Some(own) => match as_read(form_in(bundle, core, Some(own))) {
            own_form if writes_a_clock(bundle, own_form) => own_form,
            _ => form,
        },
        None => form,
    }
}

/// Whether writing `bundle` in `form` puts its clock anywhere: on the end of the save or beside it.
fn writes_a_clock(bundle: &one_saves::Bundle, form: ClockForm) -> bool {
    footer_of_form(bundle, form).is_some() || sidecar_of_form(bundle, form).is_some()
}

/// Whether a sidecar's record holds a Seiko S-3511A. Every other one holds an MBC3, and an origin a
/// HuC-3 as well.
fn holds_seiko(record: SidecarRecord) -> bool {
    matches!(record, SidecarRecord::PackedSeiko)
}

/// The shape `core` writes this bundle's clock in when `app` runs it: what writing a save for a
/// chosen reader takes, from its registry entries for every chip.
///
/// See [`form_with`], and [`Core::clock_in`](one_saves_registry::Core::clock_in) for what the app
/// changes.
#[must_use]
pub fn form_in(
    bundle: &one_saves::Bundle,
    core: &one_saves_registry::Core,
    app: Option<&one_saves_registry::App>,
) -> ClockForm {
    form_with(bundle, core.clock_in(app), core.huc3_in(app))
}

/// The shape a core with `layout` writes this bundle's clock in, from its [`ClockLayout`] alone.
///
/// A HuC-3 follows a layout of its own, which this does not know; it is [`form_with`] with none,
/// whose rule for that case is the one a packed footer implies. [`form_in`] knows both.
#[must_use]
pub fn form_for(bundle: &one_saves::Bundle, layout: Option<ClockLayout>) -> ClockForm {
    form_with(bundle, layout, None)
}

/// The shape a core with `layout`, keeping a HuC-3 as `huc3` says, writes this bundle's clock in.
///
/// Which chip wrote the state is the save's own business — a Seiko S-3511A is a Game Boy Advance
/// part and an MBC3 a Game Boy one — so the bundle's key settles it, and what the core settles is
/// the layout. mincer_ray keeps the chip in a file of its own and GBA_MiSTer packs it; every
/// other GBA core that keeps one appends it the way the mGBA app does.
///
/// An appended footer takes the width the core gives its timestamp. With no layout, it is the
/// form most producers use, at the width key 3 says the clock's own footer had.
///
/// A sidecar is the record the core keeps, whichever chip the bundle holds. Writing it for a chip
/// the record does not hold writes nothing, which is right: mGBA's libretro core keeps no Seiko
/// clock, and a GBA game run there reads the host's.
///
/// A HuC-3 goes where `huc3` says: packed into the footer, an origin beside the save, the
/// controller's window on the end of it, or nowhere. With nothing recorded it goes into the packed
/// footer for a core that packs its MBC3, and at MiSTer's size for no core at all, as it always
/// has; any other core is not known to keep one, and gets nothing.
#[must_use]
pub fn form_with(
    bundle: &one_saves::Bundle,
    layout: Option<ClockLayout>,
    huc3: Option<Huc3Layout>,
) -> ClockForm {
    if bundle.header.extensions.contains_key(&RtcHuc3::KEY) {
        return match (huc3, layout) {
            (Some(Huc3Layout::Packed { reserved }), _)
            | (None, Some(ClockLayout::Packed { reserved, .. })) => ClockForm::Huc3Packed(reserved),
            (Some(Huc3Layout::Origin { timestamp_width, big_endian }), _) => {
                ClockForm::Sidecar(SidecarRecord::Origin { timestamp_width, big_endian })
            }
            (Some(Huc3Layout::Memory), _) => ClockForm::Huc3Memory,
            (Some(Huc3Layout::SameBoy), _) => ClockForm::Huc3SameBoy,
            (Some(Huc3Layout::Vbam), _) => ClockForm::Huc3Vbam,
            (None, None) => ClockForm::Huc3Packed(PACKED_LEN_MISTER),
            (Some(Huc3Layout::NotKept), _) | (None, Some(_)) => ClockForm::None,
        };
    }
    if bundle.header.extensions.contains_key(&RtcS3511a::KEY) {
        return match layout {
            Some(ClockLayout::Sidecar(record)) => ClockForm::Sidecar(record),
            Some(ClockLayout::Packed { reserved, .. }) => ClockForm::S3511aPacked(reserved),
            Some(ClockLayout::Appended { .. }) | None => ClockForm::S3511a,
        };
    }
    match layout {
        Some(ClockLayout::Sidecar(record)) => ClockForm::Sidecar(record),
        Some(ClockLayout::Packed { reserved, .. }) => ClockForm::Packed(reserved),
        Some(ClockLayout::Appended { timestamp_width }) => ClockForm::Mbc3Appended(timestamp_width),
        None => ClockForm::Mbc3Appended(
            bundle
                .header
                .extensions
                .get::<RtcMbc3>()
                .map_or(RtcMbc3::DEFAULT_TIMESTAMP_WIDTH, RtcMbc3::footer_width),
        ),
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
    // A sidecar is not appended at all, so there is no footer for the save to carry, and a
    // reader that keeps no clock has none anywhere.
    if matches!(form, ClockForm::Sidecar(_) | ClockForm::None) {
        return None;
    }
    if form == ClockForm::Huc3Memory {
        let chip = bundle.header.extensions.get::<RtcHuc3>()?;
        return Some(build_huc3_memory(&chip.filled(), chip.taken_at.unwrap_or_else(now)));
    }
    if let ClockForm::S3511aPacked(reserved) = form {
        let (chip, anchor) = s3511a_of(bundle)?;
        return Some(build_s3511a_packed(&chip.components, chip.control, anchor, reserved));
    }
    if let ClockForm::Huc3Packed(reserved) = form {
        return huc3_of(bundle).map(|clock| build_packed_huc3(&clock, reserved));
    }
    if form == ClockForm::Huc3SameBoy {
        return huc3_of(bundle).map(|clock| build_sameboy_huc3(&clock));
    }
    if form == ClockForm::Huc3Vbam {
        return huc3_of(bundle).map(|clock| build_vbam_huc3(&clock));
    }
    // A packed footer is built from an appended one, and keeps none of its width.
    let width = match form {
        ClockForm::Mbc3Appended(width) => width,
        _ => RtcMbc3::DEFAULT_TIMESTAMP_WIDTH,
    };
    let raw = footer_of_mbc3(bundle, width)?;
    match form {
        ClockForm::Packed(reserved) => parse_mbc3(&raw).as_ref().map(|clock| build_packed(clock, reserved)),
        _ => Some(raw),
    }
}

/// Rebuilds the appended MBC3 footer, with a timestamp `width` bytes wide, whatever shape the
/// producer actually writes.
#[must_use]
fn footer_of_mbc3(bundle: &one_saves::Bundle, width: u8) -> Option<Vec<u8>> {
    let extensions = &bundle.header.extensions;
    if let Some((chip, anchor)) = s3511a_of(bundle) {
        return Some(build_s3511a(&chip.components, chip.control, anchor));
    }
    // A source that kept no latched copy gets what the specification fills it with: the live
    // registers.
    let clock = extensions.get::<RtcMbc3>()?;
    Some(build_mbc3(&clock.live, &clock.latched_or_live(), clock.taken_at, usize::from(width)))
}

/// The MBC3 a bundle carries, and nothing for any other chip: a record that holds an MBC3 is no
/// place for a Seiko chip's calendar.
fn mbc3_of(bundle: &one_saves::Bundle) -> Option<Mbc3Clock> {
    let clock = bundle.header.extensions.get::<RtcMbc3>()?;
    let [seconds, minutes, hours, days_low, days_high] = clock.live;
    Some(Mbc3Clock {
        seconds,
        minutes,
        hours,
        days_low,
        days_high,
        latched: clock.latched,
        taken_at: clock.taken_at,
        timestamp_width: clock.timestamp_width.map(usize::from),
    })
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
    let taken_at = i64::from_le_bytes(footer[8..16].try_into().ok()?);

    let sane = year <= 99
        && (1..=12).contains(&month)
        && (1..=31).contains(&day)
        && weekday <= 6
        && hour <= 23
        && minute <= 59
        && second <= 59
        && taken_at >= 0;
    sane.then_some(S3511aClock { components, control, taken_at })
}

/// Writes the S-3511A footer back out: the latched components, the control register, then the
/// instant as eight little-endian bytes.
#[must_use]
pub fn build_s3511a(components: &[u8; 7], control: u8, taken_at: i64) -> Vec<u8> {
    let mut footer = Vec::with_capacity(S3511A_LEN);
    footer.extend_from_slice(components);
    footer.push(control);
    footer.extend_from_slice(&taken_at.to_le_bytes());
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

/// Reads the footer the MiSTer Gameboy and SGB cores and the Gameboy core's openFPGA ports
/// append.
///
/// One format, two reserved sizes: MiSTer writes a whole 512-byte SD block and budude2's Pocket
/// port writes 16 bytes. Both use ten of them, and the cores share `mbc3.v`, which is why the
/// packing is identical. The two sizes are not interchangeable: the Pocket port wraps a block it
/// is given at sixteen words, and MiSTer reads the footer only from a file whose size has bit 9
/// set.
///
/// The layout is taken from the cores themselves rather than inferred. Both write five 16-bit
/// little-endian words past the save memory:
///
/// ```text
/// 0x00 : RTC_timestampOut[15:0]     bytes 0..2   the host clock, four bytes wide
/// 0x01 : RTC_timestampOut[31:16]    bytes 2..4
/// 0x02 : RTC_savedtimeOut[15:0]     bytes 4..6   the live registers, bit-packed
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
/// The host clock is a counter the core keeps: seeded once, when it loads, from the firmware's
/// time, and counted up from there, so it is read live when the save is written. MiSTer seeds it
/// with local standard time counted as though it were UTC. On an MBC3 the counter stands still
/// while the game has the clock halted.
///
/// The day counter has room for **ten** bits here, not the nine the hardware has, and the
/// overflow flag is a bit of its own rather than the top of the day register. The core never sets
/// the tenth bit — the days wrap at 511 and set the flag — so only a footer something else wrote
/// holds one. The bytes past the tenth are never written and read back as `0xFF`, which the cores
/// write explicitly as `16'hFFFF` rather than leaving erased: 502 of them on MiSTer, six on the
/// Pocket.
///
/// The cores keep a HuC-3 in the same footer, packed its own way, and nothing in the bytes says
/// which chip wrote them. Bits above the 29th are refused, since the core never sets them and the
/// footer would not come back as it was, which turns away a HuC-3 that has counted 2048 days. One
/// that has not counted that far reads as an MBC3 where its minute field allows, which is why a
/// caller that knows the cartridge reads it with [`parse_packed_huc3`] instead, as [`split_for`]
/// does when told.
#[must_use]
pub fn parse_packed(footer: &[u8]) -> Option<Mbc3Clock> {
    let (taken_at, packed) = packed_frame(footer)?;

    let seconds = u32::try_from(packed & 0x3F).ok()?;
    let minutes = u32::try_from(packed >> 6 & 0x3F).ok()?;
    let hours = u32::try_from(packed >> 12 & 0x1F).ok()?;
    let days = u32::try_from(packed >> 17 & 0x3FF).ok()?;
    let overflow = packed >> 27 & 1 != 0;
    let halt = packed >> 28 & 1 != 0;

    // The core keeps registers rather than a counter, so these are already ranged — but a footer
    // that says 97 minutes past the hour is not this core's output whatever its length.
    if seconds > 59 || minutes > 59 || hours > 23 || packed >> 29 != 0 || taken_at < 0 {
        return None;
    }

    let mut days_high = days >> 8 & 1;
    if halt {
        days_high |= 0x40;
    }
    if overflow {
        days_high |= 0x80;
    }
    // The core keeps one copy of the registers, and its timestamp is the first four bytes, as the
    // registry's entry for every core that writes this says.
    Some(Mbc3Clock {
        seconds,
        minutes,
        hours,
        days_low: days & 0xFF,
        days_high,
        latched: None,
        taken_at,
        timestamp_width: Some(PACKED_TIMESTAMP_WIDTH),
    })
}

/// Reads the footer the MiSTer Game Boy cores and their openFPGA ports append for a HuC-3: the
/// frame [`parse_packed`] reads, with `huc3.v`'s packing in place of `mbc3.v`'s.
///
/// ```text
/// RTC_savedtimeOut[33:0] = {rtc_days[15:0], rtc_minutes[11:0], rtc_seconds[5:0]}
/// ```
///
/// The minute counter is the minute of the day and the day counter sixteen bits, and the bits
/// above them are written as zero. The core restores all three counters when it loads the save,
/// and counts the time since the host clock on from them.
///
/// Nothing in the bytes says this is a HuC-3 rather than an MBC3, so it is for a footer whose
/// cartridge is known to be one, as its ROM's header says.
#[must_use]
pub fn parse_packed_huc3(footer: &[u8]) -> Option<Huc3Clock> {
    let (taken_at, packed) = packed_frame(footer)?;
    let seconds = u8::try_from(packed & 0x3F).ok()?;
    let minutes = u16::try_from(packed >> 6 & 0xFFF).ok()?;
    let days = u16::try_from(packed >> 18 & 0xFFFF).ok()?;
    // The seconds only ever count up to 59 and wrap, and a game that writes the minutes clears
    // them, so 60 is not this core's. The minutes are left alone: a game may set them past 1439,
    // and the counter runs on from there.
    if seconds > 59 || packed >> 34 != 0 {
        return None;
    }
    Some(Huc3Clock { seconds, minutes, days, taken_at, memory: None, alarm: None })
}

/// The host clock and the 48-bit word both packed footers open with, from one of the two sizes
/// the cores reserve.
fn packed_frame(footer: &[u8]) -> Option<(i64, u64)> {
    if footer.len() != PACKED_LEN_POCKET && footer.len() != PACKED_LEN_MISTER {
        return None;
    }
    let taken_at = i64::from(u32::from_le_bytes(footer[0..4].try_into().ok()?));
    let mut packed = [0u8; 8];
    packed[..6].copy_from_slice(&footer[4..10]);
    Some((taken_at, u64::from_le_bytes(packed)))
}

/// How many bytes the packed footers give the host clock: the MiSTer Game Boy one and the
/// GBA_MiSTer one alike.
const PACKED_TIMESTAMP_WIDTH: usize = 4;

/// Writes the footer back out, padded to the reserved size its producer uses.
#[must_use]
pub fn build_packed(clock: &Mbc3Clock, reserved: usize) -> Vec<u8> {
    let packed = u64::from(clock.seconds & 0x3F)
        | u64::from(clock.minutes & 0x3F) << 6
        | u64::from(clock.hours & 0x1F) << 12
        | u64::from(clock.days() & 0x3FF) << 17
        | u64::from(clock.overflowed()) << 27
        | u64::from(clock.halted()) << 28;
    packed_footer(clock.taken_at, packed, reserved)
}

/// Writes a HuC-3's packed footer back out, padded to the reserved size its producer uses.
#[must_use]
pub fn build_packed_huc3(clock: &Huc3Clock, reserved: usize) -> Vec<u8> {
    let packed =
        u64::from(clock.seconds & 0x3F) | u64::from(clock.minutes & 0xFFF) << 6 | u64::from(clock.days) << 18;
    packed_footer(clock.taken_at, packed, reserved)
}

/// The host clock and a packed word, then the `0xFFFF` words the cores write past them.
fn packed_footer(taken_at: i64, packed: u64, reserved: usize) -> Vec<u8> {
    let mut footer = Vec::with_capacity(reserved);
    footer.extend_from_slice(&u32::try_from(taken_at).unwrap_or(0).to_le_bytes());
    footer.extend_from_slice(&packed.to_le_bytes()[..6]);
    footer.resize(reserved, 0xFF);
    footer
}

/// The length of the sidecar clock file OpenEmu's Gambatte writes beside a save: its origin, four
/// bytes big-endian.
pub const GAMBATTE_RTC_LEN: usize = 4;

/// The length of the sidecar clock file Gambatte's libretro core has a libretro frontend write on a
/// 64-bit host: its origin as a `time_t`, eight bytes little-endian.
pub const GAMBATTE_LIBRETRO_RTC_LEN: usize = 8;

/// The length of the sidecar clock file SameBoy's libretro core has a libretro frontend write: the
/// clock section of its machine state.
pub const SAMEBOY_RTC_LEN: usize = 32;

/// Where SameBoy's record keeps the host clock, after two five-byte register sets padded out to
/// eight-byte alignment.
const SAMEBOY_TAKEN_AT: usize = 16;

/// Reads Gambatte's sidecar clock file as OpenEmu's build writes it: an origin in four big-endian
/// bytes. See [`parse_origin`].
#[must_use]
pub fn parse_gambatte_sidecar(bytes: &[u8], captured_at: i64) -> Option<Mbc3Clock> {
    if bytes.len() != GAMBATTE_RTC_LEN {
        return None;
    }
    parse_origin(bytes, true, captured_at)
}

/// Reads a sidecar clock file that stores an **origin** rather than a reading, as Gambatte's does.
///
/// The bytes are a Unix instant, four or eight of them in the byte order given, and Gambatte
/// derives the cartridge clock as `now - base`. That is the other operand of a reading, not a
/// reading: on its own it says nothing about what time the game thinks it is. OpenEmu's build
/// writes four bytes big-endian; the libretro core hands its frontend the base itself, a
/// `uint64_t`, which is eight bytes little-endian on every host it runs on.
///
/// `captured_at` is what supplies the missing half: the moment the counters are taken at, which
/// is the moment the producer reads them. The cartridge clock follows the host's, so the counters
/// then are `captured_at - base`, and the bundle carries a reading like every other producer's,
/// with `captured_at` as its anchor. Pass the current time unless you mean another moment.
///
/// Whatever `captured_at` is, the round trip is exact: writing the sidecar back computes
/// `captured_at - elapsed`, which returns the original base however the instant was chosen.
///
/// The file keeps no latched copy and is no footer, so the clock has neither a latched copy nor a
/// timestamp width.
#[must_use]
pub fn parse_origin(bytes: &[u8], big_endian: bool, captured_at: i64) -> Option<Mbc3Clock> {
    let elapsed = origin_elapsed(bytes, big_endian, captured_at)?;
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

    Some(Mbc3Clock {
        seconds,
        minutes,
        hours,
        days_low,
        days_high,
        latched: None,
        taken_at: captured_at,
        timestamp_width: None,
    })
}

/// How long a clock read from an origin had been running at `captured_at`: the instant in `bytes`,
/// four or eight of them in the byte order given, taken from it.
///
/// A clock that has not started yet is not a clock, so a base in the future, or before 1970, means
/// the instant is wrong rather than that the cartridge ran backwards.
fn origin_elapsed(bytes: &[u8], big_endian: bool, captured_at: i64) -> Option<i64> {
    let base = match (bytes.len(), big_endian) {
        (4, true) => i64::from(u32::from_be_bytes(bytes.try_into().ok()?)),
        (4, false) => i64::from(u32::from_le_bytes(bytes.try_into().ok()?)),
        (8, true) => i64::from_be_bytes(bytes.try_into().ok()?),
        (8, false) => i64::from_le_bytes(bytes.try_into().ok()?),
        _ => return None,
    };
    let elapsed = captured_at.checked_sub(base)?;
    (base >= 0 && elapsed >= 0).then_some(elapsed)
}

/// Reads a HuC-3's origin, as Gambatte and VBA-M keep one beside a save: the same instant they keep
/// for an MBC3, from which they derive the minute of the day and a twelve-bit day count. See
/// [`parse_origin`] for the instant.
///
/// The seconds into the minute are the remainder, which neither core shows a game but which says
/// when the minute counter next moves. Nothing else of the controller is kept.
#[must_use]
pub fn parse_huc3_origin(bytes: &[u8], big_endian: bool, captured_at: i64) -> Option<Huc3Clock> {
    let elapsed = origin_elapsed(bytes, big_endian, captured_at)?;
    Some(Huc3Clock {
        seconds: u8::try_from(elapsed % 60).ok()?,
        minutes: u16::try_from(elapsed / 60 % 1440).ok()?,
        days: u16::try_from((elapsed / 86_400) & 0xFFF).ok()?,
        taken_at: captured_at,
        memory: None,
        alarm: None,
    })
}

/// Writes a HuC-3's origin back out, `timestamp_width` bytes in the byte order given: the instant
/// that many days, minutes and seconds before the anchor.
///
/// The cores keeping one count twelve bits of days, so a day counter past them is taken as they
/// would take it, and the minute as the minute of the day.
#[must_use]
pub fn build_huc3_origin(clock: &Huc3Clock, timestamp_width: u8, big_endian: bool) -> Vec<u8> {
    let elapsed = i64::from(clock.days & 0xFFF) * 86_400
        + i64::from(clock.minutes % 1440) * 60
        + i64::from(clock.seconds.min(59));
    write_instant(clock.taken_at.saturating_sub(elapsed), timestamp_width, big_endian)
}

/// The length of the record the mGBA app appends to a save for a HuC-3: the controller's 256
/// nybbles two to a byte, then the instant it was last latched, eight bytes.
pub const HUC3_MEMORY_LEN: usize = 0x80 + 8;

/// Reads the record the mGBA app appends for a HuC-3, `GBMBCHuC3SaveBuffer`:
///
/// ```text
/// bytes 0..128    the controller's window, address 2n in the low nybble of byte n and 2n + 1 in
///                 the high one
/// bytes 128..136  the host clock it last latched the counters at, epoch seconds, little-endian
/// ```
///
/// The counters are where every HuC-3 keeps them, and the rest of the window is the game's and
/// kept with them. mGBA moves its latch a whole minute at a time, so no seconds are counted into
/// the minute. Nothing in the bytes says it is a HuC-3's, which is why this is for a footer whose
/// cartridge is known to have one; the instant is checked to be a time.
#[must_use]
pub fn parse_huc3_memory(footer: &[u8]) -> Option<Huc3Clock> {
    if footer.len() != HUC3_MEMORY_LEN {
        return None;
    }
    let mut memory = [0u8; 256];
    for (at, byte) in footer[..0x80].iter().enumerate() {
        memory[2 * at] = byte & 0x0F;
        memory[2 * at + 1] = byte >> 4;
    }
    let taken_at = i64::from_le_bytes(footer[0x80..].try_into().ok()?);
    let value = |at: usize, len: usize| {
        memory[at..at + len].iter().rev().fold(0u16, |value, &nybble| value << 4 | u16::from(nybble))
    };
    (taken_at >= 0).then_some(Huc3Clock {
        seconds: 0,
        minutes: value(HUC3_COUNTERS, 3),
        days: value(HUC3_COUNTERS + 3, 4),
        taken_at,
        memory: Some(memory),
        alarm: None,
    })
}

/// Writes the mGBA app's HuC-3 record back out, from the controller's window and the instant it
/// was latched at. See [`parse_huc3_memory`].
#[must_use]
pub fn build_huc3_memory(memory: &[u8; 256], taken_at: i64) -> Vec<u8> {
    let mut record: Vec<u8> = memory.chunks(2).map(|pair| (pair[0] & 0x0F) | (pair[1] & 0x0F) << 4).collect();
    record.extend_from_slice(&taken_at.to_le_bytes());
    record
}

/// The length of the record the SameBoy app appends to a save for a HuC-3.
pub const SAMEBOY_HUC3_LEN: usize = 17;

/// Reads the record the SameBoy app appends for a HuC-3, `GB_huc3_rtc_time_t`, packed:
///
/// ```text
/// bytes 0..8    the host clock the counters were last moved at, epoch seconds, little-endian
/// bytes 8..10   the minute counter
/// bytes 10..12  the day counter
/// bytes 12..14  the alarm's minute
/// bytes 14..16  the alarm's day
/// byte  16      whether the alarm is on
/// ```
///
/// SameBoy moves the minute counter when the host's minute turns, so the seconds counted into the
/// minute are its anchor's. It keeps the counters, the alarm and nothing else of the controller.
/// The instant is checked to be a time and the counters to be a clock's, since nothing else in the
/// bytes is checkable.
#[must_use]
pub fn parse_sameboy_huc3(footer: &[u8]) -> Option<Huc3Clock> {
    if footer.len() != SAMEBOY_HUC3_LEN {
        return None;
    }
    let word = |at: usize| u16::from_le_bytes([footer[at], footer[at + 1]]);
    let taken_at = i64::from_le_bytes(footer[..8].try_into().ok()?);
    let (minutes, days) = (word(8), word(10));
    let alarm = Huc3Alarm { minutes: word(12), days: word(14), enabled: footer[16] & 1 != 0 };
    (taken_at >= 0 && minutes < 1440 && alarm.minutes < 1440 && footer[16] <= 1).then_some(Huc3Clock {
        seconds: u8::try_from(taken_at % 60).ok()?,
        minutes,
        days,
        taken_at,
        memory: None,
        alarm: Some(alarm),
    })
}

/// Writes the SameBoy app's HuC-3 record back out. See [`parse_sameboy_huc3`].
///
/// SameBoy keeps no seconds of its own, only its anchor's, so a clock counted some other number of
/// seconds into its minute starts that minute at the anchor's second. An alarm the clock does not
/// carry is the one the specification fills a null with: all zero, and off.
#[must_use]
pub fn build_sameboy_huc3(clock: &Huc3Clock) -> Vec<u8> {
    let alarm = clock.alarm.or_else(|| clock.memory.as_ref().map(Huc3Alarm::of)).unwrap_or(Huc3Alarm {
        minutes: 0,
        days: 0,
        enabled: false,
    });
    let mut record = Vec::with_capacity(SAMEBOY_HUC3_LEN);
    record.extend_from_slice(&clock.taken_at.to_le_bytes());
    for value in [clock.minutes, clock.days, alarm.minutes, alarm.days] {
        record.extend_from_slice(&value.to_le_bytes());
    }
    record.push(u8::from(alarm.enabled));
    record
}

/// The length of the record the VisualBoyAdvance-M app appends to a save for a HuC-3.
pub const VBAM_HUC3_LEN: usize = 24;

/// Reads the record the VisualBoyAdvance-M app appends for a HuC-3, `mapperHuC3RTC` from its
/// `mapperLastTime`:
///
/// ```text
/// bytes 0..8    the instant the counters count from, epoch seconds, little-endian
/// bytes 8..12   the counters as a game last read them, the day count above the minute's twelve bits
/// bytes 12..16  the counters as a game is writing them
/// bytes 16..20  whether the controller is in its reading mode
/// bytes 20..24  how far into the counters a game has read or written
/// ```
///
/// The first is VBA-M's clock, the same origin its libretro core keeps beside the save, and the
/// rest is where a game had got to talking to the controller; the counters come from the origin at
/// `captured_at`, as [`parse_huc3_origin`] reads it.
#[must_use]
pub fn parse_vbam_huc3(footer: &[u8], captured_at: i64) -> Option<Huc3Clock> {
    if footer.len() != VBAM_HUC3_LEN {
        return None;
    }
    let word = |at: usize| u32::from_le_bytes(footer[at..at + 4].try_into().expect("four bytes"));
    // A mode is a flag and a shift steps four bits at a time to 24, so anything else is not this.
    let plausible = word(16) <= 1 && word(20) <= 24 && word(20) % 4 == 0;
    plausible.then(|| parse_huc3_origin(&footer[..8], false, captured_at)).flatten()
}

/// Writes the VisualBoyAdvance-M app's HuC-3 record back out: the origin, then the controller as
/// VBA-M leaves it between a game's conversations with it, reading mode on and the counters latched
/// as they stand at the anchor. See [`parse_vbam_huc3`].
#[must_use]
pub fn build_vbam_huc3(clock: &Huc3Clock) -> Vec<u8> {
    let mut record = build_huc3_origin(clock, 8, false);
    let latched = u32::from(clock.days & 0xFFF) << 12 | u32::from(clock.minutes % 1440);
    for word in [latched, 0, 1, 0] {
        record.extend_from_slice(&u32::to_le_bytes(word));
    }
    record
}

/// Reads a sidecar clock file as a HuC-3's, as the producer keeps one (`layout`), or by its length
/// where it is not known which.
///
/// Only an origin is ever kept beside the save for a HuC-3, so that is what a known producer's file
/// is read as, at its width and byte order, and an unknown one's at the width the file has: four
/// bytes big-endian as OpenEmu's Gambatte writes an origin, eight little-endian as a libretro core
/// writes one. A producer that keeps a HuC-3 some other way, or not at all, keeps none here.
#[must_use]
pub fn parse_huc3_sidecar(bytes: &[u8], layout: Option<Huc3Layout>, captured_at: i64) -> Option<Huc3Clock> {
    match layout {
        Some(Huc3Layout::Origin { timestamp_width, big_endian })
            if bytes.len() == usize::from(timestamp_width) =>
        {
            parse_huc3_origin(bytes, big_endian, captured_at)
        }
        Some(_) => None,
        None => match bytes.len() {
            GAMBATTE_RTC_LEN => parse_huc3_origin(bytes, true, captured_at),
            GAMBATTE_LIBRETRO_RTC_LEN => parse_huc3_origin(bytes, false, captured_at),
            _ => None,
        },
    }
}

/// Reads the record mGBA's and VBA-M's libretro cores keep beside a save: the MBC3 footer their
/// standalone builds append, alone in a file.
///
/// Checked as [`parse_mbc3`] checks a footer. Read from a file of its own, it is no footer, so the
/// clock carries no width, which is what says so in key 3.
#[must_use]
pub fn parse_registers_sidecar(bytes: &[u8]) -> Option<Mbc3Clock> {
    parse_mbc3(bytes).map(|clock| Mbc3Clock { timestamp_width: None, ..clock })
}

/// Reads the record SameBoy's libretro core keeps beside a save, the clock section of its machine
/// state, laid out by its C compiler on a little-endian host:
///
/// ```text
/// bytes 0..5    the live registers, a byte each: seconds, minutes, hours, days, day high
/// bytes 5..10   the latched copy, the same way
/// bytes 10..16  padding to the next field's alignment
/// bytes 16..24  the host clock when the registers were last brought up to date, epoch seconds
/// bytes 24..28  the CPU cycles counted into the current second
/// bytes 28      a TPP1 cartridge's fourth mode register, zero for an MBC3
/// bytes 29..32  padding
/// ```
///
/// The registers are checked as a footer's are, and the host clock has to be a time. The cycle
/// count is less than a second of the clock and nothing here keeps it, so writing the record back
/// starts that second over.
#[must_use]
pub fn parse_sameboy_sidecar(bytes: &[u8]) -> Option<Mbc3Clock> {
    if bytes.len() != SAMEBOY_RTC_LEN {
        return None;
    }
    let registers = |at: usize| -> [u32; 5] { std::array::from_fn(|i| u32::from(bytes[at + i])) };
    let live = registers(0);
    let latched = registers(5);
    let taken_at = i64::from_le_bytes(bytes[SAMEBOY_TAKEN_AT..SAMEBOY_TAKEN_AT + 8].try_into().ok()?);
    let sane = |[seconds, minutes, hours, ..]: [u32; 5]| seconds < 60 && minutes < 60 && hours < 24;
    (sane(live) && sane(latched) && taken_at >= 0).then_some(Mbc3Clock {
        seconds: live[0],
        minutes: live[1],
        hours: live[2],
        days_low: live[3],
        days_high: live[4],
        latched: Some(latched),
        taken_at,
        timestamp_width: None,
    })
}

/// Reads a sidecar clock file as `record` lays it out, and nothing else: a file of another length
/// is not this record.
///
/// Only an origin needs `captured_at` (see [`parse_origin`]); every other record holds its own
/// instant and ignores it.
#[must_use]
pub fn parse_sidecar_as(bytes: &[u8], record: SidecarRecord, captured_at: i64) -> Option<Clock> {
    match record {
        SidecarRecord::Origin { timestamp_width, big_endian }
            if bytes.len() == usize::from(timestamp_width) =>
        {
            parse_origin(bytes, big_endian, captured_at).map(Clock::Mbc3)
        }
        SidecarRecord::Origin { .. } => None,
        SidecarRecord::Registers { .. } => parse_registers_sidecar(bytes).map(Clock::Mbc3),
        SidecarRecord::SameBoy => parse_sameboy_sidecar(bytes).map(Clock::Mbc3),
        SidecarRecord::PackedSeiko => parse_mincer_sidecar(bytes).map(Clock::S3511a),
    }
}

/// Reads a sidecar clock file whose producer nobody named, by its length.
///
/// The files share an extension and nothing else, and among the producers sampled the lengths
/// tell them apart: four bytes are OpenEmu's Gambatte origin and eight the libretro core's, sixteen
/// are mincer_ray's Seiko record, thirty-two SameBoy's and forty-eight mGBA's or VBA-M's
/// registers. A producer that is known is read as its record says instead, by [`parse_sidecar_as`],
/// since a length is only a guess at who wrote the file.
#[must_use]
pub fn parse_sidecar(bytes: &[u8], captured_at: i64) -> Option<Clock> {
    let record = match bytes.len() {
        GAMBATTE_RTC_LEN => SidecarRecord::Origin { timestamp_width: 4, big_endian: true },
        GAMBATTE_LIBRETRO_RTC_LEN => SidecarRecord::Origin { timestamp_width: 8, big_endian: false },
        MINCER_RTC_LEN => SidecarRecord::PackedSeiko,
        SAMEBOY_RTC_LEN => SidecarRecord::SameBoy,
        MBC3_LEN_32 | MBC3_LEN_64 => SidecarRecord::Registers { timestamp_width: 8 },
        _ => return None,
    };
    parse_sidecar_as(bytes, record, captured_at)
}

/// Whether reading a sidecar `len` bytes long takes the instant it is read at: true of an origin,
/// which is no reading until one is supplied, and of nothing else.
///
/// `record` is the producer's, where it is known. A file that is not that record is read by its
/// length, as [`parse_sidecar`] reads one whose producer nobody named, so its length can make it an
/// origin too.
#[must_use]
pub fn sidecar_takes_instant(record: Option<SidecarRecord>, len: usize) -> bool {
    matches!(record, Some(SidecarRecord::Origin { .. }))
        || matches!(len, GAMBATTE_RTC_LEN | GAMBATTE_LIBRETRO_RTC_LEN)
}

/// The record the producer a `source` names keeps its clock in beside the save, when it keeps one
/// there.
#[must_use]
pub fn sidecar_record_of(source: Option<&one_saves::Source>) -> Option<SidecarRecord> {
    match layout_of(source)? {
        ClockLayout::Sidecar(record) => Some(record),
        _ => None,
    }
}

/// Rebuilds the sidecar file a bundle's clock goes in, in a chosen shape.
///
/// `None` for a shape that is not a sidecar, for a bundle with no clock, and for a clock of a chip
/// the record does not hold, which the core keeps no file for.
#[must_use]
pub fn sidecar_of_form(bundle: &one_saves::Bundle, form: ClockForm) -> Option<Vec<u8>> {
    let ClockForm::Sidecar(record) = form else { return None };
    if holds_seiko(record) {
        let (chip, anchor) = s3511a_of(bundle)?;
        return Some(build_mincer_sidecar(&chip.components, chip.control, anchor));
    }
    // An origin is the one record kept for a HuC-3 as well as an MBC3.
    if let SidecarRecord::Origin { timestamp_width, big_endian } = record
        && let Some(clock) = huc3_of(bundle)
    {
        return Some(build_huc3_origin(&clock, timestamp_width, big_endian));
    }
    let clock = mbc3_of(bundle)?;
    Some(match record {
        SidecarRecord::Origin { timestamp_width, big_endian } => {
            build_origin(&clock, timestamp_width, big_endian)
        }
        SidecarRecord::Registers { timestamp_width } => build_mbc3(
            &clock.live(),
            &clock.latched.unwrap_or(clock.live()),
            clock.taken_at,
            usize::from(timestamp_width),
        ),
        SidecarRecord::SameBoy => build_sameboy_sidecar(&clock),
        SidecarRecord::PackedSeiko => unreachable!("a Seiko record was written above"),
    })
}

/// Rebuilds the sidecar file a bundle's clock goes in.
///
/// The file the bundle's producer keeps, where its producer keeps one (see [`form_of`]). Otherwise
/// the one a core keeping such a file for this chip writes: Gambatte's libretro origin for an MBC3
/// or a HuC-3, and mincer_ray's record for a Seiko chip. Which core will read it is the better
/// question, and [`sidecar_of_form`] answers it.
#[must_use]
pub fn sidecar_of(bundle: &one_saves::Bundle) -> Option<Vec<u8>> {
    use one_saves_registry::cores::{GAMBATTE, MINCER_RAY_GBA};
    let form = match form_of(bundle) {
        form @ ClockForm::Sidecar(_) => form,
        _ if s3511a_of(bundle).is_some() => form_in(bundle, &MINCER_RAY_GBA, None),
        _ => form_in(bundle, &GAMBATTE, None),
    };
    sidecar_of_form(bundle, form)
}

/// Writes Gambatte's sidecar back out as OpenEmu's build writes it: the origin the clock is
/// derived from, four bytes big-endian.
#[must_use]
pub fn build_gambatte_sidecar(clock: &Mbc3Clock) -> Vec<u8> {
    build_origin(clock, 4, true)
}

/// Writes an origin back out: the instant the clock is derived from, `timestamp_width` bytes in
/// the byte order given. See [`parse_origin`].
///
/// An instant four bytes cannot hold is written as zero, which reads back as a clock that has been
/// running since 1970 rather than as some other plausible time.
#[must_use]
pub fn build_origin(clock: &Mbc3Clock, timestamp_width: u8, big_endian: bool) -> Vec<u8> {
    write_instant(clock.taken_at.saturating_sub(clock.elapsed_seconds()), timestamp_width, big_endian)
}

/// An instant as an origin keeps one, `timestamp_width` bytes in the byte order given, and zero
/// where four bytes cannot hold it.
fn write_instant(base: i64, timestamp_width: u8, big_endian: bool) -> Vec<u8> {
    match (timestamp_width, big_endian) {
        (4, true) => u32::try_from(base).unwrap_or(0).to_be_bytes().to_vec(),
        (4, false) => u32::try_from(base).unwrap_or(0).to_le_bytes().to_vec(),
        (_, true) => base.to_be_bytes().to_vec(),
        (_, false) => base.to_le_bytes().to_vec(),
    }
}

/// Writes SameBoy's record back out. See [`parse_sameboy_sidecar`] for the layout.
///
/// A source that kept no latched copy gets the live registers in its place, and the cycles into
/// the current second, which no bundle keeps, start from zero.
#[must_use]
pub fn build_sameboy_sidecar(clock: &Mbc3Clock) -> Vec<u8> {
    let byte = |value: u32| u8::try_from(value & 0xFF).expect("masked to a byte");
    let mut record = Vec::with_capacity(SAMEBOY_RTC_LEN);
    record.extend(clock.live().map(byte));
    record.extend(clock.latched.unwrap_or(clock.live()).map(byte));
    record.resize(SAMEBOY_TAKEN_AT, 0);
    record.extend_from_slice(&clock.taken_at.to_le_bytes());
    record.resize(SAMEBOY_RTC_LEN, 0);
    record
}

/// The control register the MiSTer GBA core and its Pocket port start with: 24-hour mode.
///
/// Neither core's record has room for the register, so neither stores it. It is the value the
/// register powers up with, and loading a clock leaves it alone, so it is what a game first reads
/// from a core just started. A game that writes another keeps it on MiSTer until the core itself
/// is reloaded, but no save says so. Neither core counts hours in 12-hour mode in any case.
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
    Some(S3511aClock { components, control: MISTER_GBA_CONTROL, taken_at: i64::from(timestamp) })
}

/// Writes mincer_ray's sidecar back out, from the chip's components and the instant they show.
///
/// The record keeps the hour on the 24-hour clock and has no control register, so a clock read in
/// 12-hour mode has its PM flag folded into the hour, and the register itself goes. An instant
/// outside the record's 32 bits is written as zero, which the core refuses and replaces with the
/// Pocket's own clock rather than misreading.
#[must_use]
pub fn build_mincer_sidecar(components: &[u8; 7], control: u8, taken_at: i64) -> Vec<u8> {
    let mut record = Vec::with_capacity(MINCER_RTC_LEN);
    record.extend_from_slice(&u32::try_from(taken_at).unwrap_or(0).to_le_bytes());
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
    let taken_at = i64::try_from(value >> 42).ok()?;
    Some(S3511aClock { components, control: MISTER_GBA_CONTROL, taken_at })
}

/// Writes GBA_MiSTer's block back out: the record, repeated to fill the reserved size.
///
/// The hour and the control register go the way they do in [`build_mincer_sidecar`], since the
/// calendar is the same. An instant outside the record's 32 bits is written as zero, which the
/// core does not check and runs the clock forward from.
#[must_use]
pub fn build_s3511a_packed(components: &[u8; 7], control: u8, taken_at: i64, reserved: usize) -> Vec<u8> {
    let timestamp = u32::try_from(taken_at).unwrap_or(0);
    let value = u128::from(pack_calendar(*components, control)) | u128::from(timestamp) << 42;
    let mut record = Vec::with_capacity(MISTER_GBA_RECORD_LEN);
    record.extend_from_slice(&MISTER_GBA_MAGIC);
    record.extend_from_slice(&value.to_le_bytes()[..10]);
    record.resize(MISTER_GBA_RECORD_LEN, 0);
    record.iter().copied().cycle().take(reserved).collect()
}

/// Writes the MBC3 footer back out: ten little-endian registers, then the timestamp.
#[must_use]
pub fn build_mbc3(live: &[u32; 5], latched: &[u32; 5], taken_at: i64, timestamp_width: usize) -> Vec<u8> {
    let mut footer = Vec::with_capacity(40 + timestamp_width);
    for value in live.iter().chain(latched) {
        footer.extend_from_slice(&value.to_le_bytes());
    }
    if timestamp_width == 8 {
        footer.extend_from_slice(&taken_at.to_le_bytes());
    } else {
        // The 32-bit form some emulators still write. A negative or out-of-range instant never
        // reaches here: `parse_mbc3` refuses one.
        footer.extend_from_slice(&u32::try_from(taken_at).unwrap_or(0).to_le_bytes());
    }
    footer
}

#[cfg(test)]
mod tests {
    use super::*;
    use one_saves::dcbor::CBOR;

    /// Builds the footer an emulator writes for a Game Boy cartridge with a clock.
    fn mbc3_footer(seconds: u32, minutes: u32, hours: u32, days: u32, taken_at: i64) -> Vec<u8> {
        let mut footer = Vec::with_capacity(MBC3_LEN_64);
        for value in [seconds, minutes, hours, days & 0xFF, (days >> 8) & 1] {
            footer.extend_from_slice(&value.to_le_bytes());
        }
        // The latched copy the game reads, here the same as the live clock.
        for value in [seconds, minutes, hours, days & 0xFF, (days >> 8) & 1] {
            footer.extend_from_slice(&value.to_le_bytes());
        }
        footer.extend_from_slice(&taken_at.to_le_bytes());
        footer
    }

    fn crystal_save(taken_at: i64) -> Vec<u8> {
        let mut save = vec![0x11u8; 32768];
        save.extend_from_slice(&mbc3_footer(30, 45, 13, 300, taken_at));
        save
    }

    /// The trailing 48 bytes of a real Pokémon Crystal save written by SameBoy, whose cartridge
    /// clock had been running 2 minutes 13 seconds. Ten little-endian registers, then an
    /// eight-byte instant — the shape this module infers, here confirmed against a real emulator.
    const SAMEBOY_FOOTER: [u8; 48] = [
        0x0d, 0, 0, 0, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // live
        0x0d, 0, 0, 0, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // latched
        0x62, 0xba, 0x7c, 0x6a, 0, 0, 0, 0, // taken_at, 64-bit
    ];

    #[test]
    fn reads_a_real_sameboy_clock() {
        let clock = parse_mbc3(&SAMEBOY_FOOTER).expect("an MBC3 clock");
        assert_eq!((clock.seconds, clock.minutes, clock.hours), (13, 2, 0));
        assert_eq!(clock.days(), 0);
        assert!(!clock.halted() && !clock.overflowed());
        // SameBoy writes the 64-bit form, so the footer is 48 bytes rather than 44.
        assert_eq!(clock.timestamp_width, Some(8));
        // SameBoy takes the anchor when it writes the save, and it agrees with the file to the
        // second.
        assert_eq!(clock.taken_at, 1_786_559_074);
        // The latched copy is the live one here, which is what a game that is not mid-read leaves.
        assert_eq!(clock.latched, Some(clock.live()));
    }

    #[test]
    fn a_real_sameboy_save_splits_and_rebuilds() {
        let mut save: Vec<u8> = (0..32_768u32).map(|i| (i % 251) as u8).collect();
        save.extend_from_slice(&SAMEBOY_FOOTER);

        let split = split(&save, Some("gb")).expect("has a clock");
        assert_eq!(split.sram.len(), 32_768);

        let Clock::Mbc3(clock) = split.clock else { panic!("wrong chip") };
        let rebuilt = build_mbc3(
            &clock.live(),
            &clock.latched.unwrap(),
            clock.taken_at,
            clock.timestamp_width.unwrap(),
        );
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
        // 300 days, 13:45:30 of cartridge time, on the Unix scale. The anchor plays no part, so a
        // save written at another moment with the same counters reads the same.
        let elapsed = 300 * 86_400 + 13 * 3_600 + 45 * 60 + 30;
        assert_eq!(split.reading.unwrap().reading, elapsed);
        let later = super::split(&crystal_save(1_800_000_000), Some("gb")).expect("has a footer");
        assert_eq!(later.reading, split.reading);
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
        assert_eq!(clock.taken_at, 1_600_000_000);
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
        assert_eq!(native.latched, Some([30, 45, 13, 44, 1]));
        assert_eq!(native.timestamp_width, Some(8));
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
            let rebuilt = build_mbc3(
                &clock.live(),
                &clock.latched.unwrap(),
                clock.taken_at,
                clock.timestamp_width.unwrap(),
            );
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
            latched: Some([1, 2, 3, 4, 0]),
            taken_at: 1_600_000_000,
            timestamp_width: Some(4),
        };
        assert_eq!(build_mbc3(&clock.live(), &clock.live(), clock.taken_at, 4).len(), MBC3_LEN_32);
        assert_eq!(build_mbc3(&clock.live(), &clock.live(), clock.taken_at, 8).len(), MBC3_LEN_64);
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
        assert_eq!(clock.taken_at, 1_786_556_383);
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
        // The anchor rides in the chip key as key 2, since it is not the reading. Key 3 is null:
        // mGBA's footer has no room for the alarm, so a game that set one lost it.
        let native = extensions.cbor(&RtcS3511a::KEY).unwrap();
        assert_eq!(native.as_map().unwrap().len(), 4);
        let chip = extensions.get::<RtcS3511a>().unwrap();
        assert_eq!(chip.taken_at, Some(1_786_556_383));
        assert_eq!(chip.alarm, S3511aAlarm::Unknown);

        let Clock::S3511a(clock) = split.clock else { panic!("wrong chip") };
        assert_eq!(build_s3511a(&clock.components, clock.control, clock.taken_at), RUBY_FOOTER);
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
        assert_eq!(clock.reading().reading, 294);
        assert_eq!(clock.taken_at, GAMBATTE_WRITTEN_AT);
        // The file keeps one copy of the clock and is no footer, and the chip key says both.
        assert_eq!((clock.latched, clock.timestamp_width), (None, None));
        let chip = CBOR::from(RtcMbc3::from(clock));
        let map = chip.as_map().unwrap();
        assert!(map.get::<u64, CBOR>(1).unwrap().is_null(), "the latched copy is null");
        assert!(map.get::<u64, CBOR>(3).is_none(), "and the width is absent");
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
        assert_eq!(clock.taken_at, 1_786_556_383);
        assert_eq!(build_mincer_sidecar(&clock.components, clock.control, clock.taken_at), MINCER_SIDECAR);
    }

    /// The `.rtc` mGBA's libretro core had RetroArch write beside a real Pokémon Crystal save: the
    /// clock one second in, latched, at 2026-11-04 07:31:29 UTC. The same forty-eight bytes the
    /// mGBA app appends to its `.sav`.
    const MGBA_LIBRETRO_SIDECAR: [u8; 48] = [
        1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // live
        1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // latched
        0x51, 0xdf, 0xea, 0x6a, 0, 0, 0, 0, // taken_at, 64-bit
    ];

    /// The `.rtc` Gambatte's libretro core had RetroArch write beside the same save: its origin as
    /// the core's `uint64_t`, little-endian.
    const GAMBATTE_LIBRETRO_SIDECAR: [u8; 8] = [0x4f, 0xdf, 0xea, 0x6a, 0, 0, 0, 0];

    /// The `.rtc` SameBoy's libretro core had RetroArch write beside the same save: nine seconds
    /// in, latched, the host clock at 16 and 242448 cycles into the second at 24.
    const SAMEBOY_LIBRETRO_SIDECAR: [u8; 32] = [
        0x09, 0, 0, 0, 0, 0x09, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // live, latched, padding
        0x5a, 0xdf, 0xea, 0x6a, 0, 0, 0, 0, // taken_at
        0x10, 0xb3, 0x03, 0, 0, 0, 0, 0, // cycles, the TPP1 register, padding
    ];

    #[test]
    fn the_sidecars_are_told_apart_by_length() {
        assert!(matches!(parse_sidecar(&MINCER_SIDECAR, 0), Some(Clock::S3511a(_))));
        assert!(matches!(parse_sidecar(&GAMBATTE_SIDECAR, GAMBATTE_WRITTEN_AT), Some(Clock::Mbc3(_))));
        let at = 1_793_777_500;
        assert!(matches!(parse_sidecar(&GAMBATTE_LIBRETRO_SIDECAR, at), Some(Clock::Mbc3(_))));
        assert!(matches!(parse_sidecar(&SAMEBOY_LIBRETRO_SIDECAR, 0), Some(Clock::Mbc3(_))));
        assert!(matches!(parse_sidecar(&MGBA_LIBRETRO_SIDECAR, 0), Some(Clock::Mbc3(_))));
        // Half a Seiko record is eight bytes, the length of an origin, and an origin that far in
        // the future is no clock.
        assert_eq!(parse_sidecar(&MINCER_SIDECAR[..8], 0), None);
        assert_eq!(parse_sidecar(&MINCER_SIDECAR[..12], 0), None);
    }

    #[test]
    fn a_named_record_reads_only_its_own_shape() {
        let origin = |width, big_endian| SidecarRecord::Origin { timestamp_width: width, big_endian };
        let at = 1_793_777_500;
        assert!(parse_sidecar_as(&GAMBATTE_LIBRETRO_SIDECAR, origin(8, false), at).is_some());
        // OpenEmu's four big-endian bytes are not the libretro core's eight little-endian ones.
        assert_eq!(parse_sidecar_as(&GAMBATTE_SIDECAR, origin(8, false), GAMBATTE_WRITTEN_AT), None);
        assert!(parse_sidecar_as(&GAMBATTE_SIDECAR, origin(4, true), GAMBATTE_WRITTEN_AT).is_some());
        // Nor is anything else the record of another.
        let registers = SidecarRecord::Registers { timestamp_width: 8 };
        assert_eq!(parse_sidecar_as(&SAMEBOY_LIBRETRO_SIDECAR, registers, 0), None);
        assert_eq!(parse_sidecar_as(&MGBA_LIBRETRO_SIDECAR, SidecarRecord::SameBoy, 0), None);
        assert_eq!(parse_sidecar_as(&MGBA_LIBRETRO_SIDECAR, SidecarRecord::PackedSeiko, 0), None);
    }

    #[test]
    fn reads_a_real_gambatte_libretro_sidecar_and_round_trips_it() {
        // The base is 2026-11-04 07:31:27 UTC; read 73 seconds later, the clock has run that long.
        let at = 1_793_777_487 + 73;
        let clock = parse_origin(&GAMBATTE_LIBRETRO_SIDECAR, false, at).expect("a clock");
        assert_eq!((clock.days(), clock.hours, clock.minutes, clock.seconds), (0, 0, 1, 13));
        assert_eq!((clock.taken_at, clock.latched, clock.timestamp_width), (at, None, None));
        assert_eq!(build_origin(&clock, 8, false), GAMBATTE_LIBRETRO_SIDECAR);
        // The same origin as OpenEmu writes it.
        assert_eq!(build_origin(&clock, 4, true), 1_793_777_487u32.to_be_bytes());
    }

    #[test]
    fn reads_a_real_mgba_libretro_sidecar_as_no_footer() {
        let clock = parse_registers_sidecar(&MGBA_LIBRETRO_SIDECAR).expect("a clock");
        assert_eq!(clock.live(), [1, 0, 0, 0, 0]);
        assert_eq!(clock.latched, Some([1, 0, 0, 0, 0]));
        assert_eq!(clock.taken_at, 1_793_777_489);
        // Read from a file of its own, it had no footer to have a width, and key 3 says so.
        assert_eq!(clock.timestamp_width, None);
        assert_eq!(
            build_mbc3(&clock.live(), &clock.latched.unwrap(), clock.taken_at, 8),
            MGBA_LIBRETRO_SIDECAR
        );
    }

    #[test]
    fn reads_a_real_sameboy_libretro_sidecar_and_rebuilds_all_but_its_cycles() {
        let clock = parse_sameboy_sidecar(&SAMEBOY_LIBRETRO_SIDECAR).expect("a clock");
        assert_eq!(clock.live(), [9, 0, 0, 0, 0]);
        assert_eq!(clock.latched, Some([9, 0, 0, 0, 0]));
        assert_eq!((clock.taken_at, clock.timestamp_width), (1_793_777_498, None));
        // The cycles into the second are the one thing no bundle keeps.
        let mut expected = SAMEBOY_LIBRETRO_SIDECAR;
        expected[24..28].fill(0);
        assert_eq!(build_sameboy_sidecar(&clock), expected);

        // A register out of range is not SameBoy's clock.
        let mut bad = SAMEBOY_LIBRETRO_SIDECAR;
        bad[1] = 60;
        assert_eq!(parse_sameboy_sidecar(&bad), None);
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
        let in_app = |app: Option<&str>, core: &str| {
            let source = Source::default()
                .app(app.map(|app| Name::parse(app).unwrap()))
                .core(Name::parse(core).unwrap());
            let header = Header::default().source(source).extensions(split.extensions());
            one_saves::Bundle::new(header, vec![Part::new(0, *b"SAVE")])
        };
        let bundle = |core: &str| in_app(None, core);

        let mincer = bundle("mincer-ray-gba");
        assert_eq!(form_of(&mincer), ClockForm::Sidecar(SidecarRecord::PackedSeiko));
        // Kept in a file of its own, so the save carries nothing.
        assert_eq!(footer_of(&mincer), None);
        assert_eq!(sidecar_of(&mincer).as_deref(), Some(&MINCER_SIDECAR[..]));

        // The same clock handed to the mGBA app is the app's own footer for that moment, byte for
        // byte. And to mGBA with no app named: its libretro core keeps no Seiko clock, so a Seiko
        // clock under its name was read off the end of the app's file.
        for mgba in [in_app(Some("mgba"), "mgba"), bundle("mgba")] {
            assert_eq!(form_of(&mgba), ClockForm::S3511a);
            assert_eq!(footer_of(&mgba).as_deref(), Some(&RUBY_FOOTER[..]));
        }

        // Written for RetroArch's mGBA there is nothing to write at all: a GBA game there reads
        // the host's clock.
        let form = form_for(&mincer, one_saves_registry::cores::MGBA.clock);
        assert_eq!(form, ClockForm::Sidecar(SidecarRecord::Registers { timestamp_width: 8 }));
        assert_eq!((footer_of_form(&mincer, form), sidecar_of_form(&mincer, form)), (None, None));
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
        assert_eq!(clock.taken_at, 1_786_556_383);
        assert_eq!(
            build_s3511a_packed(&clock.components, clock.control, clock.taken_at, PACKED_LEN_MISTER),
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
            taken_at: 1_792_576_838,
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
    fn a_chip_without_an_anchor_resumes_from_the_moment_it_is_rebuilt() {
        // A producer that keeps no anchor leaves key 2 out, as one-saves 0.4.0 did. The chip then
        // resumes from its components with no time passed, which is the moment of rebuilding.
        // The reading is not the anchor, and neither is `bundled_at`, however close either is.
        use one_saves::{Header, Name, Part, Source};
        let mut extensions = one_saves::Extensions::new();
        extensions.insert(Rtc::new(1_786_556_383));
        extensions.insert(RtcS3511a {
            components: RUBY_FOOTER[..7].try_into().unwrap(),
            control: 0x40,
            taken_at: None,
            alarm: S3511aAlarm::NeverSet,
        });
        let header = Header::default()
            .source(Source::default().core(Name::parse("mgba").unwrap()))
            .extensions(extensions)
            .bundled_at(1_786_556_383);
        let bundle = one_saves::Bundle::new(header, vec![Part::new(0, *b"SAVE")]);

        let before = now();
        let footer = footer_of(&bundle).expect("a footer");
        let anchor = i64::from_le_bytes(footer[8..16].try_into().unwrap());
        assert_eq!(footer[..8], RUBY_FOOTER[..8], "the components and the control, as they were");
        assert!((before..=now()).contains(&anchor), "anchored at {anchor}, not when the rebuild ran");
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
        assert_eq!(first.taken_at, 1_786_532_469);
        // The core keeps one copy of the registers, behind a timestamp four bytes wide.
        assert_eq!((first.latched, first.timestamp_width), (None, Some(4)));

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
        assert_eq!(b.taken_at - a.taken_at, 2673);
        assert_eq!(elapsed(&b) - elapsed(&a), 2673);
        // So the reading moves at the rate a clock does. Adding the anchor to the counters, which
        // is what this crate used to write, moved it 5346 s between these two saves.
        assert_eq!(b.reading().reading - a.reading().reading, 2673);
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
        assert_eq!(clock.taken_at, 1_786_533_688);
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
            ("budude2-gb", PACKED_LEN_POCKET),
            ("budude2-gbc", PACKED_LEN_POCKET),
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
    fn every_packed_layout_gives_the_host_clock_the_width_read_here() {
        // The registry records the width for each core, read from its source; this module reads
        // and writes the packed footers at one width. The two have to agree, or key 3 would say
        // one thing about a footer and the core's own entry another.
        for core in one_saves_registry::cores() {
            if let Some(ClockLayout::Packed { timestamp_width, .. }) = core.clock {
                assert_eq!(usize::from(timestamp_width), PACKED_TIMESTAMP_WIDTH, "{}", core.slug);
            }
        }
    }

    #[test]
    fn bits_the_core_never_sets_are_not_read_as_an_mbc3() {
        // A HuC-3 that has counted 2048 days sets bit 29, which `mbc3.v` never does. Splitting
        // it would drop the bit on the way back, so the footer stays in the save.
        let mut footer = POCKET_FIRST;
        footer[7] |= 0x20;
        assert_eq!(parse_packed(&footer), None);
    }

    /// A HuC-3 footer as `huc3.v` packs it, padded to `reserved` as the cores pad it.
    fn huc3_footer(days: u16, minutes: u16, seconds: u8, reserved: usize) -> Vec<u8> {
        let packed = u64::from(seconds) | u64::from(minutes) << 6 | u64::from(days) << 18;
        let mut footer = 1_786_533_688u32.to_le_bytes().to_vec();
        footer.extend_from_slice(&packed.to_le_bytes()[..6]);
        footer.resize(reserved, 0xFF);
        footer
    }

    #[test]
    fn a_huc3_footer_is_read_as_one_when_the_rom_says_so() {
        let footer = huc3_footer(3000, 13 * 60 + 45, 37, PACKED_LEN_MISTER);
        let clock = parse_packed_huc3(&footer).expect("a HuC-3");
        assert_eq!((clock.days, clock.minutes, clock.seconds), (3000, 825, 37));
        assert_eq!(clock.taken_at, 1_786_533_688);
        assert_eq!(build_packed_huc3(&clock, PACKED_LEN_MISTER), footer, "rebuilt byte for byte");
        // The reading is the counters alone: no game can read the seconds.
        assert_eq!(clock.reading(), Rtc::new(3000 * 86_400 + 825 * 60));

        let mut save = vec![0x22u8; 32_768];
        save.extend_from_slice(&footer);
        // Past 2047 days it is no MBC3's footer, so without the ROM's word it stays in the save.
        assert_eq!(split(&save, Some("gb")), None);
        let split = split_for(&save, Some("gb"), Some("x.1sav.rtc.huc3")).expect("a clock");
        assert_eq!((split.sram.len(), split.clock), (32_768, Clock::Huc3(clock)));
        let extensions = split.extensions();
        let key = extensions.get::<RtcHuc3>().expect("the chip's key");
        assert_eq!((key.minutes(), key.days(), key.seconds), (Some(825), Some(3000), Some(37)));
        assert_eq!(key.memory.iter().flatten().count(), 7, "the two counters and nothing else");
        assert_eq!(extensions.get::<Rtc>(), Some(&clock.reading()));
    }

    #[test]
    fn a_young_huc3_is_not_taken_for_an_mbc3_when_the_rom_says_otherwise() {
        // Ten days in, a HuC-3's bits fit `mbc3.v`'s, and read as an MBC3 its minutes are some
        // clock's twenty minutes and its days twenty. Only the ROM can say which it is.
        let mut save = vec![0u8; 8192];
        save.extend_from_slice(&huc3_footer(10, 20, 5, PACKED_LEN_POCKET));
        let misread = split(&save, Some("gb")).expect("a clock").clock;
        assert!(matches!(misread, Clock::Mbc3(Mbc3Clock { minutes: 20, days_low: 20, .. })));
        let clock = split_for(&save, Some("gb"), Some("x.1sav.rtc.huc3")).expect("a clock").clock;
        assert!(matches!(clock, Clock::Huc3(Huc3Clock { days: 10, minutes: 20, seconds: 5, .. })));

        // An MBC3's footer is no HuC-3's, so one stays in the save.
        let mut appended = vec![0u8; 8192];
        appended.extend_from_slice(&mbc3_footer(1, 2, 3, 4, 1_700_000_000));
        assert_eq!(split_for(&appended, Some("gb"), Some("x.1sav.rtc.huc3")), None);
        // And another chip's word changes nothing.
        assert_eq!(split_for(&save, Some("gb"), Some("x.1sav.rtc.mbc3")), split(&save, Some("gb")));
    }

    #[test]
    fn a_huc3_goes_back_into_the_footer_its_core_packs() {
        use one_saves::{Header, Name, Part, Source};
        let original = huc3_footer(3000, 825, 37, PACKED_LEN_MISTER);
        let clock = parse_packed_huc3(&original).expect("a HuC-3");
        let split = Split {
            sram: Vec::new(),
            footer: Vec::new(),
            reading: Some(clock.reading()),
            clock: Clock::Huc3(clock),
        };
        let bundle_from = |core: Option<&str>, extensions: one_saves::Extensions| {
            let source = core.map(|core| Source::default().core(Name::parse(core).unwrap()));
            let header = Header::default().source(source).extensions(extensions);
            one_saves::Bundle::new(header, vec![Part::new(0, *b"SAVE")])
        };
        let footer = |core| footer_of(&bundle_from(core, split.extensions()));
        assert_eq!(footer(Some("gameboy-mister")), Some(original.clone()));
        assert_eq!(
            footer(Some("budude2-gbc")).as_deref(),
            Some(&huc3_footer(3000, 825, 37, PACKED_LEN_POCKET)[..])
        );
        assert_eq!(footer(None), Some(original), "naming no core, MiSTer's");
        // mGBA's libretro core keeps none, so a HuC-3 under its name came off the app's file, and
        // goes back as the app keeps it: the counters at their addresses in the whole window.
        let app = footer(Some("mgba")).expect("the app's footer");
        let back = parse_huc3_memory(&app).expect("the app's record");
        assert_eq!((back.days, back.minutes, back.taken_at), (3000, 825, 1_786_533_688));
        // SameBoy's libretro core keeps none either, so the same goes for the SameBoy app's record:
        // the anchor, the two counters and an alarm nobody set.
        let app = footer(Some("sameboy")).expect("the app's footer");
        assert_eq!(app.len(), SAMEBOY_HUC3_LEN);
        let back = parse_sameboy_huc3(&app).expect("the app's record");
        assert_eq!((back.days, back.minutes, back.taken_at), (3000, 825, 1_786_533_688));
        assert_eq!(back.alarm, Some(Huc3Alarm { minutes: 0, days: 0, enabled: false }));

        // A key from a source that kept less is filled the way the specification fills it.
        let mut sparse = RtcHuc3::from(clock);
        sparse.seconds = None;
        sparse.memory[HUC3_COUNTERS] = None;
        let mut extensions = one_saves::Extensions::new();
        extensions.insert(sparse);
        let rebuilt = footer_of(&bundle_from(Some("gameboy-mister"), extensions)).expect("a footer");
        let back = parse_packed_huc3(&rebuilt).expect("a HuC-3");
        assert_eq!(
            (back.seconds, back.minutes, back.days),
            (0, 0x330, 3000),
            "0 for each null: 825 is 0x339"
        );
    }

    /// The `.rtc` Gambatte's libretro core had RetroArch write beside a real Robopon Sun save: the
    /// HuC-3's origin, the same eight bytes it keeps for an MBC3.
    const GAMBATTE_HUC3_SIDECAR: [u8; 8] = [0x55, 0xdf, 0xea, 0x6a, 0, 0, 0, 0];

    #[test]
    fn a_huc3_origin_reads_as_gambatte_and_vba_m_count_from_it() {
        // Three days, two hours, five minutes and seven seconds after the origin.
        let base = 1_793_777_493;
        let at = base + 3 * 86_400 + 125 * 60 + 7;
        let clock = parse_huc3_origin(&GAMBATTE_HUC3_SIDECAR, false, at).expect("a clock");
        assert_eq!((clock.days, clock.minutes, clock.seconds, clock.taken_at), (3, 125, 7, at));
        assert_eq!(clock.memory, None, "nothing of the controller is kept");
        // Written back, the origin comes out as it went in, in either core's shape.
        assert_eq!(build_huc3_origin(&clock, 8, false), GAMBATTE_HUC3_SIDECAR);
        assert_eq!(build_huc3_origin(&clock, 4, true), 1_793_777_493u32.to_be_bytes());

        // The minute is the minute of the day and the days twelve bits, as both cores count them.
        let late =
            parse_huc3_origin(&GAMBATTE_HUC3_SIDECAR, false, base + 4097 * 86_400 + 1439 * 60).unwrap();
        assert_eq!((late.days, late.minutes), (1, 1439));
        // And an origin after the moment it is read at is no clock.
        assert_eq!(parse_huc3_origin(&GAMBATTE_HUC3_SIDECAR, false, base - 1), None);
    }

    #[test]
    fn a_huc3_sidecar_is_read_as_its_producer_keeps_one() {
        let at = 1_793_777_493 + 60;
        let origin = Some(Huc3Layout::Origin { timestamp_width: 8, big_endian: false });
        assert_eq!(parse_huc3_sidecar(&GAMBATTE_HUC3_SIDECAR, origin, at).map(|c| c.minutes), Some(1));
        // A producer keeping none, or keeping it on the end of the save, keeps nothing here.
        for layout in [Huc3Layout::NotKept, Huc3Layout::Memory, Huc3Layout::Packed { reserved: 512 }] {
            assert_eq!(parse_huc3_sidecar(&GAMBATTE_HUC3_SIDECAR, Some(layout), at), None, "{layout:?}");
        }
        // Nobody named, the length says which origin it is.
        assert!(parse_huc3_sidecar(&GAMBATTE_HUC3_SIDECAR, None, at).is_some());
        assert!(parse_huc3_sidecar(&1_793_777_493u32.to_be_bytes(), None, at).is_some());
        assert_eq!(parse_huc3_sidecar(&[0; 48], None, at), None);
    }

    /// The record the mGBA app appends for a HuC-3, laid out as `GBMBCHuC3Write` lays it: every
    /// nybble its address's own value, so a nybble put in the wrong half shows, then the latch time.
    fn mgba_huc3_record() -> Vec<u8> {
        let memory: [u8; 256] = std::array::from_fn(|at| u8::try_from(at % 16).unwrap());
        let mut record: Vec<u8> = (0..0x80).map(|n| memory[2 * n] | memory[2 * n + 1] << 4).collect();
        record.extend_from_slice(&1_793_779_606i64.to_le_bytes());
        record
    }

    #[test]
    fn the_mgba_apps_huc3_record_is_read_whole_and_written_back_whole() {
        let record = mgba_huc3_record();
        assert_eq!(record.len(), HUC3_MEMORY_LEN);
        let clock = parse_huc3_memory(&record).expect("a clock");
        // The counters are the nybbles at 0x10 to 0x12 and 0x13 to 0x16, least significant first.
        assert_eq!((clock.minutes, clock.days), (0x210, 0x6543));
        assert_eq!((clock.seconds, clock.taken_at), (0, 1_793_779_606));
        let memory = clock.memory.expect("the whole window");
        assert_eq!((memory[0x10], memory[0x11], memory[0xFF]), (0, 1, 15));
        assert_eq!(build_huc3_memory(&memory, clock.taken_at), record);

        // Every address goes into the key, so the game's own nybbles come back from it.
        let key = RtcHuc3::from(clock);
        assert!(key.memory.iter().all(Option::is_some));
        assert_eq!(build_huc3_memory(&key.filled(), clock.taken_at), record);

        // It is a footer only on a cartridge the ROM says has a HuC-3.
        let mut save = vec![0x33u8; 32_768];
        save.extend_from_slice(&record);
        assert_eq!(split(&save, Some("gbc")), None);
        let split = split_for(&save, Some("gbc"), Some("x.1sav.rtc.huc3")).expect("a clock");
        assert_eq!((split.sram.len(), split.clock), (32_768, Clock::Huc3(clock)));
        // A latch before 1970 is no time mGBA took.
        let mut negative = record;
        negative[HUC3_MEMORY_LEN - 1] = 0x80;
        assert_eq!(parse_huc3_memory(&negative), None);
    }

    #[test]
    fn a_huc3_goes_where_each_reader_keeps_one() {
        use one_saves_registry::{apps, cores};
        let clock = Huc3Clock {
            seconds: 7,
            minutes: 125,
            days: 3,
            taken_at: 1_793_777_493,
            memory: None,
            alarm: None,
        };
        let bundle = huc3_bundle(&clock, None, None);
        let origin = ClockForm::Sidecar(SidecarRecord::Origin { timestamp_width: 8, big_endian: false });
        for (core, app, want) in [
            (&cores::GAMEBOY_MISTER, None, ClockForm::Huc3Packed(PACKED_LEN_MISTER)),
            (&cores::BUDUDE2_GBC, None, ClockForm::Huc3Packed(PACKED_LEN_POCKET)),
            (&cores::GAMBATTE, Some(&apps::RETROARCH), origin),
            (&cores::VBA_M, Some(&apps::RETROARCH), origin),
            (&cores::MGBA, Some(&apps::MGBA), ClockForm::Huc3Memory),
            (&cores::MGBA, Some(&apps::RETROARCH), ClockForm::None),
            (&cores::SAMEBOY, Some(&apps::RETROARCH), ClockForm::None),
            // Nobody has seen OpenEmu's Gambatte with one, so nothing is guessed for it.
            (&cores::GAMBATTE, Some(&apps::OPENEMU), ClockForm::None),
        ] {
            assert_eq!(form_in(&bundle, core, app), want, "{} in {:?}", core.slug, app.map(|a| a.slug));
        }

        // Each written: the origin the clock counts from, beside the save; the app's window on the
        // end of it; or nothing at all.
        let beside = sidecar_of_form(&bundle, origin).expect("an origin");
        assert_eq!(parse_huc3_origin(&beside, false, clock.taken_at), Some(clock));
        assert_eq!(footer_of_form(&bundle, origin), None);
        let memory = footer_of_form(&bundle, ClockForm::Huc3Memory).expect("the app's record");
        let back = parse_huc3_memory(&memory).expect("reads back");
        assert_eq!((back.days, back.minutes, back.taken_at), (3, 125, clock.taken_at));
        assert_eq!(
            (footer_of_form(&bundle, ClockForm::None), sidecar_of_form(&bundle, ClockForm::None)),
            (None, None)
        );
        // A record that holds an MBC3 alone has no HuC-3 in it.
        assert_eq!(sidecar_of_form(&bundle, ClockForm::Sidecar(SidecarRecord::SameBoy)), None);
        // Asked for a sidecar with no reader in mind, Gambatte's.
        assert_eq!(sidecar_of(&bundle), Some(beside));
    }

    #[test]
    fn a_huc3_read_from_the_mgba_app_goes_back_to_it() {
        // `--from mgba` names no app. The libretro core keeps no HuC-3, so the clock came from the
        // app's footer, and that is the file it gets back, the game's nybbles and all.
        let clock = parse_huc3_memory(&mgba_huc3_record()).expect("a clock");
        let bundle = huc3_bundle(&clock, None, Some("mgba"));
        assert_eq!(form_of(&bundle), ClockForm::Huc3Memory);
        assert_eq!(footer_of(&bundle), Some(mgba_huc3_record()));
        // Named as RetroArch's, it is that and nothing else.
        assert_eq!(form_of(&huc3_bundle(&clock, Some("retroarch"), Some("mgba"))), ClockForm::None);
    }

    /// A bundle carrying a HuC-3, from the producer `app` and `core` name.
    fn huc3_bundle(clock: &Huc3Clock, app: Option<&str>, core: Option<&str>) -> one_saves::Bundle {
        use one_saves::{Header, Name, Part, Source};
        let mut extensions = Extensions::new();
        extensions.insert(RtcHuc3::from(*clock));
        let name = |text: Option<&str>| text.map(|text| Name::parse(text).unwrap());
        let source = Source::default().app(name(app)).core(name(core));
        one_saves::Bundle::new(
            Header::default().source(source).extensions(extensions),
            vec![Part::new(0, *b"SAVE")],
        )
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

    /// A bundle carrying `clock`, from the producer `app` and `core` name.
    fn bundle_with(clock: Mbc3Clock, app: Option<&str>, core: Option<&str>) -> one_saves::Bundle {
        use one_saves::{Header, Name, Part, Source};
        let split = Split {
            sram: Vec::new(),
            footer: Vec::new(),
            reading: Some(clock.reading()),
            clock: Clock::Mbc3(clock),
        };
        let name = |text: Option<&str>| text.map(|text| Name::parse(text).unwrap());
        let source = Source::default().app(name(app)).core(name(core));
        one_saves::Bundle::new(
            Header::default().source(source).extensions(split.extensions()),
            vec![Part::new(0, *b"SAVE")],
        )
    }

    #[test]
    fn the_producer_decides_which_shape_is_written_back() {
        // Producers that read one clock four ways. The bundle records the clock; `source` is what
        // says which shape to put it back in, so a round trip returns the file the emulator would
        // have written rather than some other emulator's idea of it. The clock is one read from a
        // file of its own, with no footer width, as every libretro core's is.
        let clock = Mbc3Clock { timestamp_width: None, ..parse_packed(&POCKET_FIRST).expect("a clock") };
        let origin = SidecarRecord::Origin { timestamp_width: 8, big_endian: false };
        let registers = SidecarRecord::Registers { timestamp_width: 8 };
        for (core, want) in [
            ("budude2-gbc", ClockForm::Packed(PACKED_LEN_POCKET)),
            ("gambatte", ClockForm::Sidecar(origin)),
            ("sameboy", ClockForm::Sidecar(SidecarRecord::SameBoy)),
            ("mgba", ClockForm::Sidecar(registers)),
            ("vba-m", ClockForm::Sidecar(registers)),
        ] {
            let bundle = bundle_with(clock, None, Some(core));
            assert_eq!(form_of(&bundle), want, "{core}");

            match want {
                // A sidecar is not appended, so there is nothing for the save to carry.
                ClockForm::Sidecar(_) => {
                    assert_eq!(footer_of(&bundle), None, "{core}");
                    assert_eq!(sidecar_of(&bundle), sidecar_of_form(&bundle, want), "{core}");
                }
                _ => assert_eq!(footer_of(&bundle).as_deref(), Some(&POCKET_FIRST[..]), "{core}"),
            }
        }
        let lengths = |core| sidecar_of(&bundle_with(clock, None, Some(core))).map(|file| file.len());
        assert_eq!(
            ["gambatte", "sameboy", "mgba"].map(lengths),
            [Some(GAMBATTE_LIBRETRO_RTC_LEN), Some(SAMEBOY_RTC_LEN), Some(MBC3_LEN_64)]
        );
    }

    #[test]
    fn a_clock_read_off_a_footer_goes_back_on_the_end() {
        // `--from mgba` names the core and not the app, which is how the mGBA app's `.sav` has
        // been converted all along. Its clock was split off the end of the file and says so with
        // key 3, so that is where it goes back, whatever the core does under libretro.
        let footer = parse_mbc3(&MGBA_LIBRETRO_SIDECAR).expect("a clock");
        assert_eq!(footer.timestamp_width, Some(8));
        for core in ["mgba", "sameboy", "vba-m"] {
            assert_eq!(form_of(&bundle_with(footer, None, Some(core))), ClockForm::Mbc3Appended(8), "{core}");
        }
        // The same clock read from RetroArch's `.rtc` carries no width, and goes back beside the
        // save, byte for byte.
        let beside = parse_registers_sidecar(&MGBA_LIBRETRO_SIDECAR).expect("a clock");
        let bundle = bundle_with(beside, Some("retroarch"), Some("mgba"));
        assert_eq!(form_of(&bundle), ClockForm::Sidecar(SidecarRecord::Registers { timestamp_width: 8 }));
        assert_eq!((footer_of(&bundle), sidecar_of(&bundle)), (None, Some(MGBA_LIBRETRO_SIDECAR.to_vec())));
        // And for the mGBA app, the same forty-eight bytes on the end of the save.
        let app = bundle_with(beside, Some("mgba"), Some("mgba"));
        assert_eq!(form_of(&app), ClockForm::Mbc3Appended(8));
        assert_eq!(footer_of(&app).as_deref(), Some(&MGBA_LIBRETRO_SIDECAR[..]));
    }

    #[test]
    fn a_clock_written_for_another_core_takes_that_cores_shape() {
        // The Pocket's timestamp is four bytes in a layout of its own. Restored for the mGBA app,
        // which appends, the footer is the one mGBA writes rather than one sized by the Pocket's;
        // restored for RetroArch's mGBA, it is the same bytes in a file of their own.
        use one_saves_registry::{apps, cores};
        let clock = parse_packed(&POCKET_FIRST).expect("a clock");
        let bundle = bundle_with(clock, None, Some("budude2-gbc"));
        assert_eq!(footer_of(&bundle).as_deref(), Some(&POCKET_FIRST[..]));

        let form = form_for(&bundle, cores::MGBA.clock_in(Some(&apps::MGBA)));
        assert_eq!(form, ClockForm::Mbc3Appended(8));
        let appended = footer_of_form(&bundle, form).expect("a footer");
        assert_eq!(appended.len(), MBC3_LEN_64);
        assert_eq!(sidecar_of_form(&bundle, form), None);

        let form = form_for(&bundle, cores::MGBA.clock_in(Some(&apps::RETROARCH)));
        assert_eq!(form, ClockForm::Sidecar(SidecarRecord::Registers { timestamp_width: 8 }));
        assert_eq!(footer_of_form(&bundle, form), None);
        assert_eq!(sidecar_of_form(&bundle, form), Some(appended.clone()));

        // A core nobody has recorded gets the width the clock's own footer had.
        assert_eq!(form_for(&bundle, None), ClockForm::Mbc3Appended(4));
        assert_eq!(form_for(&bundle, cores::GAMEBOY_MISTER.clock), ClockForm::Packed(PACKED_LEN_MISTER));
        assert_eq!(
            parse_mbc3(&appended).map(|c| (c.live(), c.taken_at)),
            Some((clock.live(), clock.taken_at))
        );
    }

    #[test]
    fn the_core_and_the_app_around_it_pick_the_layout_together() {
        // RetroArch writes what each core exposes, so the same app lays a clock down two ways; and
        // one engine run by two apps lays it down two ways as well. An `app` of `gambatte` is a 0.3
        // spelling that now names an app nobody listed: read as written, it says nothing about a
        // core, so it gets the common form, at the width key 3 says the clock's own footer had.
        let clock = Mbc3Clock { timestamp_width: None, ..parse_packed(&POCKET_FIRST).expect("a clock") };
        let footer = parse_packed(&POCKET_FIRST).expect("a clock");
        let origin = |width, big_endian| {
            ClockForm::Sidecar(SidecarRecord::Origin { timestamp_width: width, big_endian })
        };
        for (clock, app, core, want) in [
            (clock, Some("retroarch"), Some("gambatte"), origin(8, false)),
            (clock, Some("openemu"), Some("gambatte"), origin(4, true)),
            (clock, Some("retroarch"), Some("sameboy"), ClockForm::Sidecar(SidecarRecord::SameBoy)),
            (clock, Some("mgba"), Some("mgba"), ClockForm::Mbc3Appended(8)),
            // OpenEmu's own mGBA nobody has sampled, so it gets the common form.
            (clock, Some("openemu"), Some("mgba"), ClockForm::Mbc3Appended(8)),
            (footer, Some("gambatte"), None, ClockForm::Mbc3Appended(4)),
        ] {
            assert_eq!(form_of(&bundle_with(clock, app, core)), want, "{app:?} {core:?}");
        }

        // OpenEmu's Gambatte writes the origin as the file under data/saves/openemu/gambatte has it.
        let read = parse_gambatte_sidecar(&GAMBATTE_SIDECAR, GAMBATTE_WRITTEN_AT).expect("a clock");
        let openemu = bundle_with(read, Some("openemu"), Some("gambatte"));
        assert_eq!(sidecar_of(&openemu).as_deref(), Some(&GAMBATTE_SIDECAR[..]));
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
            ("budude2-gbc", ClockForm::Packed(PACKED_LEN_POCKET)),
            // One whose slug does, and whose layout nobody has read: no source is published, so
            // the registry records nothing and the common form is the guess.
            ("spiritualized-supergb", ClockForm::Mbc3Appended(4)),
            // MiSTer cores for systems with no MBC3 anywhere near them. The suffix rule gave each
            // of these the Game Boy core's 512-byte block.
            ("psx-mister", ClockForm::Mbc3Appended(4)),
            ("n64-mister", ClockForm::Mbc3Appended(4)),
            ("saturn-mister", ClockForm::Mbc3Appended(4)),
            // A producer the registry records no clock for still gets the common form.
            ("pcsx2", ClockForm::Mbc3Appended(4)),
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
