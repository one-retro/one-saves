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
//!
//! Two of those are sixteen bytes and mean entirely different things by them, so the system is
//! what tells them apart. Which shape a clock goes back into is decided by
//! [`form_of`] from the bundle's `source`, since the encoding belongs to the producer rather than
//! to the cartridge.
//!
//! The appended forms say nothing about where the save memory ends, so they are found by length.
//! The sidecar is a different problem: it stores the instant the clock counts *from*, which is
//! the other operand of a reading rather than a reading, and turning one into the other needs an
//! instant the file does not record. See [`parse_gambatte_sidecar`].
//!
//! Left inline, that footer is a problem rather than a curiosity: it carries a **live timestamp**,
//! so two dumps of the same save minutes apart hash differently, and a
//! [content hash](one_saves::Bundle::content_hash) that moves on its own is one a store cannot
//! deduplicate on. Splitting it out is what makes a save's bytes stable.
//!
//! The shape follows what [`x.1sav.rtc`] prescribes, which is to carry two keys rather than one:
//!
//! - the save part's payload is the bare save memory,
//! - the portable reading goes in [`x.1sav.rtc`], normalized to a Unix instant, so a consumer can
//!   read it without knowing which chip produced it,
//! - the chip's own state goes in [`RTC_NATIVE_KEY`], read out as fields rather than kept as a
//!   blob, because that is the state only something that understands the layout can rebuild.
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

use one_saves::dcbor::{CBOR, CBORCase, Map, Tag};
use one_saves::{Extensions, ReverseDnsName};
use one_saves_registry::ClockLayout;

/// The extension key the portable reading is written under.
pub const RTC_KEY: &str = "x.1sav.rtc";

/// The extension key the chip's own state is written under.
///
/// An MBC3 clock, read out rather than kept as a blob, so a consumer can act on it:
///
/// ```text
/// "x.1sav.rtc.mbc3": {
///   0: [30, 45, 13, 44, 1],   ; live registers: s, m, h, day counter low, day counter high
///   1: [30, 45, 13, 44, 1],   ; the latched copy the game reads
///   2: 1(1700000000),         ; the host clock when the emulator wrote the save
///   3: 8,                     ; how many bytes that timestamp occupied
/// }
/// ```
///
/// Every byte of the footer is accounted for by those fields, so it is **regenerated** rather
/// than stored: 40 bytes of registers plus a timestamp of the stated width. Key 3 is what keeps
/// that byte-exact, since emulators differ over whether the timestamp is 32 or 64 bits.
pub const RTC_NATIVE_KEY: &str = "x.1sav.rtc.mbc3";

/// The extension key a GBA cartridge's Seiko S-3511A state is written under.
///
/// ```text
/// "x.1sav.rtc.s3511a": {
///   0: h'26081203103943',   ; latched BCD: year, month, day, weekday, hour, minute, second
///   1: 64,                  ; the control register as read; bit 6 is 24-hour mode
/// }
/// ```
///
/// The instant the chip was last read is not repeated here: it is a Unix instant, so it is
/// exactly what [`x.1sav.rtc`] carries, and holding it twice would let the two drift apart.
/// Between the two keys every byte of the footer is accounted for.
///
/// [`x.1sav.rtc`]: https://docs.1retro.com/specifications/extensions/x.1sav.rtc/
pub const RTC_S3511A_KEY: &str = "x.1sav.rtc.s3511a";

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
    pub reading: Option<Reading>,
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
    /// When the chip was last read, in epoch seconds.
    pub latched_at: i64,
}

impl S3511aClock {
    /// Whether the chip is in 24-hour mode, which is bit 6 of the control register.
    #[must_use]
    pub fn hour24(self) -> bool {
        self.control & 0x40 != 0
    }

    /// What this chip showed, as a Unix instant.
    ///
    /// A dating chip needs no anchor: it holds the instant already, and `latched_at` is it — the
    /// moment the producer last read the chip, which is the moment those components describe.
    ///
    /// Deriving the instant from the components instead is the obvious move and the wrong one.
    /// The chip holds wall-clock time with no zone, so re-deriving means inventing one: on the
    /// Ruby sample it lands 7 hours before the truth, which is exactly the offset of the zone
    /// mGBA was running in. `latched_at` is the same moment recorded by something that knew the
    /// zone, and it is the only copy — the chip's own key holds components and control, nothing
    /// else — so reading it back is also what lets a footer be rebuilt byte for byte.
    #[must_use]
    pub fn reading(self) -> Reading {
        Reading { instant: self.latched_at }
    }
}

/// A clock reading: what the game's clock showed when the bundle was written.
///
/// **Never the point a counting clock advances from.** An MBC3 keeps elapsed time beside a host
/// timestamp, and that timestamp is the anchor rather than the reading — writing it here would be
/// wrong by however long the cartridge has been running, which on a save a few days old is days.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reading {
    /// What the clock showed, in whole epoch seconds.
    pub instant: i64,
}

impl Reading {
    /// The CBOR value this reading is carried as.
    ///
    /// ```text
    /// { 0: 1(1700000000) }
    /// ```
    ///
    /// `accuracy_ms` is omitted throughout: it states the uncertainty of the reading, and nothing
    /// here can measure that. The key says to leave it out rather than guess.
    #[must_use]
    pub fn to_cbor(self) -> CBOR {
        let mut map = Map::new();
        // Tag 1 over whole seconds. The key admits a float and this format does not, for the
        // reason `created_at` gives: one instant would otherwise have two encodings.
        map.insert(0u64, CBOR::from(CBORCase::Tagged(Tag::new(1u64, "epoch"), self.instant.into())));
        map.into()
    }
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
    pub fn reading(self) -> Reading {
        Reading { instant: self.written_at + self.elapsed_seconds() }
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

        // A footer of exactly the MBC3 length that also *parses* is one. Requiring both is what
        // stops a 32816-byte file whose tail is save data from being mistaken for a clock.
        // Two producers write sixteen bytes and mean different things by them, so the system
        // decides which to try: a Seiko S-3511A is a GBA chip and an MBC3 is a Game Boy one.
        let sixteen = if system == Some("gba") {
            parse_s3511a(footer).map(Clock::S3511a)
        } else {
            parse_packed(footer).map(Clock::Mbc3)
        };
        let parsed = match footer.len() {
            MBC3_LEN_32 | MBC3_LEN_64 => parse_mbc3(footer).map(Clock::Mbc3),
            PACKED_LEN_POCKET => sixteen,
            PACKED_LEN_MISTER => parse_packed(footer).map(Clock::Mbc3),
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
        let reading = match clock {
            Clock::Mbc3(c) => Some(c.reading()),
            Clock::S3511a(c) => Some(c.reading()),
        };
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
            extensions.insert(rtc_key(), reading.to_cbor());
        }

        let mut native = Map::new();
        match self.clock {
            Clock::Mbc3(clock) => {
                native.insert(0u64, registers(&clock.live()));
                native.insert(1u64, registers(&clock.latched));
                native.insert(
                    2u64,
                    CBOR::from(CBORCase::Tagged(Tag::new(1u64, "epoch"), clock.written_at.into())),
                );
                native.insert(3u64, u64::try_from(clock.timestamp_width).expect("4 or 8"));
                extensions.insert(mbc3_key(), native.into());
            }
            Clock::S3511a(clock) => {
                native.insert(0u64, CBOR::to_byte_string(clock.components));
                native.insert(1u64, u64::from(clock.control));
                extensions.insert(s3511a_key(), native.into());
            }
        }
        extensions
    }
}

fn registers(values: &[u32; 5]) -> CBOR {
    values.iter().map(|&v| CBOR::from(v)).collect::<Vec<_>>().into()
}

fn read_registers(value: &CBOR) -> Option<[u32; 5]> {
    let items = value.as_array()?;
    let mut out = [0u32; 5];
    for (slot, item) in out.iter_mut().zip(items) {
        *slot = match item.as_case() {
            CBORCase::Unsigned(n) => u32::try_from(*n).ok()?,
            _ => return None,
        };
    }
    (items.len() == 5).then_some(out)
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
}

/// The shape a bundle's producer writes, from what its `source` says.
///
/// `source.app` names the software that wrote the bytes, so it is what says which of the shapes
/// to expect. The layout each core writes is registry data — see [`ClockLayout`] — rather than
/// anything this module reads out of the slug: a core is not obliged to spell its own name in a
/// way that gives its clock away, and two that do spell it the same way need not agree.
///
/// A bundle naming nobody, or naming a core the registry records no clock for, gets the form most
/// producers use.
#[must_use]
pub fn form_of(bundle: &one_saves::Bundle) -> ClockForm {
    // Which chip wrote the state is the save's own business — a Seiko S-3511A is a Game Boy
    // Advance part and an MBC3 a Game Boy one — so the key settles it before the producer is
    // consulted at all. What the producer settles is the layout, which is the rest of this.
    if bundle.header.extensions.contains_key(&s3511a_key()) {
        return ClockForm::S3511a;
    }
    let app = bundle.header.source.as_ref().and_then(|source| source.app.as_ref());
    // `source.app` carries the slug for a listed core, which is what the registry is keyed by.
    let layout = app.map(one_saves::Name::as_str).and_then(one_saves_registry::core).and_then(|c| c.clock);
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
    if form == ClockForm::Sidecar {
        return None;
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
    if let Some(native) = bundle.header.extensions.get(&s3511a_key()) {
        let map = native.as_map()?;
        let components: [u8; 7] = map.get::<u64, CBOR>(0)?.as_byte_string()?.try_into().ok()?;
        let control = match map.get::<u64, CBOR>(1)?.as_case() {
            CBORCase::Unsigned(n) => u8::try_from(*n).ok()?,
            _ => return None,
        };
        // The instant lives in the portable key, since that is what it is.
        let reading = bundle.header.extensions.get(&rtc_key())?.as_map()?;
        let latched_at = match reading.get::<u64, CBOR>(0)?.as_case() {
            CBORCase::Tagged(_, inner) => match inner.as_case() {
                CBORCase::Unsigned(n) => i64::try_from(*n).ok()?,
                _ => return None,
            },
            _ => return None,
        };
        return Some(build_s3511a(&components, control, latched_at));
    }

    let native = bundle.header.extensions.get(&mbc3_key())?;
    let map = native.as_map()?;

    let live = read_registers(&map.get::<u64, CBOR>(0)?)?;
    let latched = read_registers(&map.get::<u64, CBOR>(1)?)?;
    let written_at = match map.get::<u64, CBOR>(2)?.as_case() {
        CBORCase::Tagged(_, inner) => match inner.as_case() {
            CBORCase::Unsigned(n) => i64::try_from(*n).ok()?,
            _ => return None,
        },
        _ => return None,
    };
    let width = match map.get::<u64, CBOR>(3)?.as_case() {
        CBORCase::Unsigned(n) => usize::try_from(*n).ok()?,
        _ => return None,
    };
    Some(build_mbc3(&live, &latched, written_at, width))
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
    let bcd = |byte: u8| {
        let (high, low) = (byte >> 4, byte & 0x0f);
        (high <= 9 && low <= 9).then_some(high * 10 + low)
    };

    let year = bcd(components[0])?;
    let month = bcd(components[1])?;
    let day = bcd(components[2])?;
    // The weekday is a plain value rather than BCD in practice, and the chip's own idea of which
    // day is which, so it is range-checked and not otherwise interpreted.
    let weekday = components[3];
    let hour = bcd(components[4] & 0x3f)?;
    let minute = bcd(components[5])?;
    let second = bcd(components[6])?;

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

/// Rebuilds the sidecar file a bundle's clock came from, when it carries an MBC3 clock.
#[must_use]
pub fn sidecar_of(bundle: &one_saves::Bundle) -> Option<Vec<u8>> {
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

/// The extension key `x.1sav.rtc` is written under.
#[must_use]
pub fn rtc_key() -> ReverseDnsName {
    ReverseDnsName::parse(RTC_KEY).expect("x.1sav.rtc is a well-formed name")
}

/// The extension key an MBC3 clock is written under.
#[must_use]
pub fn mbc3_key() -> ReverseDnsName {
    ReverseDnsName::parse(RTC_NATIVE_KEY).expect("x.1sav.rtc.mbc3 is a well-formed name")
}

/// The extension key a Seiko S-3511A's state is written under.
#[must_use]
pub fn s3511a_key() -> ReverseDnsName {
    ReverseDnsName::parse(RTC_S3511A_KEY).expect("x.1sav.rtc.s3511a is a well-formed name")
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(split.reading.unwrap().instant, 1_700_000_000 + elapsed);
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
        let encoded = Reading { instant: 1_700_000_000 }.to_cbor().to_cbor_data();
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

        assert!(extensions.contains_key(&rtc_key()), "the portable reading");
        let native = extensions.get(&mbc3_key()).expect("the chip state");
        let map = native.as_map().expect("a map");

        // Keys 0 to 3, exactly as the published schema numbers them. Read out as fields, not kept
        // as a blob, so a consumer can see what time it is.
        assert_eq!(read_registers(&map.get::<u64, CBOR>(0).unwrap()), Some([30, 45, 13, 44, 1]));
        assert_eq!(read_registers(&map.get::<u64, CBOR>(1).unwrap()), Some([30, 45, 13, 44, 1]));
        assert_eq!(map.get::<u64, CBOR>(3).unwrap().as_case(), &CBORCase::Unsigned(8));
        // No layout discriminator: the key name says which chip this is.
        assert!(map.get::<u64, CBOR>(4).is_none());
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
        // A dating chip needs no anchor: the latched components are the reading.
        assert_eq!(split.reading.unwrap().instant, 1_786_556_383);

        // Both keys go on, and the chip key carries only what the chip holds.
        let extensions = split.extensions();
        assert!(extensions.contains_key(&rtc_key()));
        let native = extensions.get(&s3511a_key()).expect("chip state").as_map().unwrap();
        assert_eq!(native.get::<u64, CBOR>(0).unwrap().as_byte_string().unwrap().len(), 7);
        assert_eq!(native.get::<u64, CBOR>(1).unwrap().as_case(), &CBORCase::Unsigned(0x40));
        // The instant is not repeated here; it is what the portable key is for.
        assert!(native.get::<u64, CBOR>(2).is_none());

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
        for (app, want) in [
            ("gameboy-mister", PACKED_LEN_MISTER),
            ("sgb-mister", PACKED_LEN_MISTER),
            ("budude2-gbc", PACKED_LEN_POCKET),
            ("spiritualized-gb", PACKED_LEN_POCKET),
        ] {
            let bundle = one_saves::Bundle {
                header: Header {
                    source: Some(Source { app: Name::parse(app).ok(), ..Source::default() }),
                    extensions: split.extensions(),
                    ..Header::default()
                },
                parts: vec![Part::new(0, *b"SAVE")],
            };
            assert_eq!(form_of(&bundle), ClockForm::Packed(want), "{app}");
            assert_eq!(footer_of(&bundle).map(|f| f.len()), Some(want), "{app}");
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
        // Four producers, one clock, four encodings. The bundle records the clock; `source.app`
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

        for (app, want) in [
            ("budude2-gbc", ClockForm::Packed(PACKED_LEN_POCKET)),
            ("gambatte", ClockForm::Sidecar),
            ("sameboy", ClockForm::Mbc3Appended),
            ("mgba", ClockForm::Mbc3Appended),
        ] {
            let bundle = one_saves::Bundle {
                header: Header {
                    source: Some(Source { app: Name::parse(app).ok(), ..Source::default() }),
                    extensions: split.extensions(),
                    ..Header::default()
                },
                parts: vec![Part::new(0, *b"SAVE")],
            };
            assert_eq!(form_of(&bundle), want, "{app}");

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

        for (app, want) in [
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
            let bundle = one_saves::Bundle {
                header: Header {
                    source: Some(Source { app: Name::parse(app).ok(), ..Source::default() }),
                    extensions: split.extensions(),
                    ..Header::default()
                },
                parts: vec![Part::new(0, *b"SAVE")],
            };
            assert_eq!(form_of(&bundle), want, "{app}");
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
