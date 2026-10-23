//! [`x.1sav.rtc.mbc3`](https://docs.1retro.com/specifications/extensions/x.1sav.rtc.mbc3/).

use std::fmt;

use dcbor::{CBOR, Map};

use super::{Rtc, array, elapsed, field, nullable, or_null, want_array, write_taken};
use crate::extension::{Extension, epoch, only_keys, want_epoch, want_map, want_uint};
use crate::name::ReverseDnsName;

/// The clock an MBC3 Game Boy cartridge keeps, register by register.
///
/// Every byte of the footer most emulators append is accounted for here, so the footer is rebuilt
/// from this rather than stored: forty bytes of registers, then the timestamp at its stated width.
/// Gambatte and ares keep the clock in a file of their own instead and keep less of it, so the
/// latched copy and the width are optional: `None` where the source does not keep a latched
/// copy, and where there is no footer to have a width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtcMbc3 {
    /// The live registers: seconds, minutes, hours, day counter low, day counter high.
    pub live: [u32; 5],
    /// The latched copy the game reads, in the same order, or `None` where the producer's source
    /// does not keep it. A rebuild uses the live registers in its place; see
    /// [`latched_or_live`](Self::latched_or_live).
    pub latched: Option<[u32; 5]>,
    /// The host clock the emulator advances the counters from, in epoch seconds. SameBoy takes it
    /// when it writes the save, and VBA-M and mGBA when the game last latched the clock.
    ///
    /// The anchor, not the reading, and on the producer's own scale: MiSTer gives its cores local
    /// standard time counted as though it were UTC.
    pub taken_at: i64,
    /// How many bytes that timestamp occupied in the footer: 4 or 8, depending on the emulator.
    /// `None` where the source keeps the clock outside a footer. A footer built without it gives
    /// the timestamp 8; see [`footer_width`](Self::footer_width).
    pub timestamp_width: Option<u8>,
}

impl RtcMbc3 {
    /// How many days the live clock has counted: the low register and bit 0 of the high one.
    #[must_use]
    pub fn days(&self) -> u32 {
        self.live[3] | ((self.live[4] & 1) << 8)
    }

    /// Whether the live clock is halted, which is bit 6 of the high day register.
    #[must_use]
    pub fn halted(&self) -> bool {
        self.live[4] & 0x40 != 0
    }

    /// Whether the day counter has run past 511 days, which is bit 7 of the high day register.
    #[must_use]
    pub fn overflowed(&self) -> bool {
        self.live[4] & 0x80 != 0
    }

    /// The clock on the Unix scale: the live counters, as that long after 1970-01-01 UTC.
    ///
    /// Never `taken_at`, and never `taken_at` plus the counters. The two move at the same rate,
    /// so their sum runs at twice real time, and the anchor is on the producer's scale. What a
    /// game displays is this plus an offset of its own, in save memory, which the reading leaves
    /// out.
    #[must_use]
    pub fn reading(&self) -> Rtc {
        let [seconds, minutes, hours, ..] = self.live;
        Rtc::new(elapsed(self.days().into(), hours.into(), minutes.into(), seconds.into()))
    }

    /// The latched copy, or the live registers where the source did not keep one.
    ///
    /// A game latches the clock before it reads it, and the latch copies the running registers
    /// over the latched ones, so the live registers are what the latched copy would have held.
    #[must_use]
    pub fn latched_or_live(&self) -> [u32; 5] {
        self.latched.unwrap_or(self.live)
    }

    /// The width a footer gives the timestamp when key 3 does not say, as BGB, mGBA, SameBoy and
    /// VBA-M write it.
    pub const DEFAULT_TIMESTAMP_WIDTH: u8 = 8;

    /// How many bytes a footer built from this gives the timestamp: the width it occupied, or
    /// [`DEFAULT_TIMESTAMP_WIDTH`](Self::DEFAULT_TIMESTAMP_WIDTH) where the source had no footer.
    #[must_use]
    pub fn footer_width(&self) -> u8 {
        self.timestamp_width.unwrap_or(Self::DEFAULT_TIMESTAMP_WIDTH)
    }
}

impl Extension for RtcMbc3 {
    const KEY: ReverseDnsName = ReverseDnsName::from_static("x.1sav.rtc.mbc3");
}

fn registers(values: [u32; 5]) -> CBOR {
    array(&values, CBOR::from)
}

fn read_registers(value: &CBOR) -> dcbor::Result<[u32; 5]> {
    want_array(value, want_uint)
}

impl From<RtcMbc3> for CBOR {
    fn from(clock: RtcMbc3) -> Self {
        let mut map = Map::new();
        map.insert(0u64, registers(clock.live));
        map.insert(1u64, or_null(clock.latched, registers));
        map.insert(2u64, epoch(clock.taken_at));
        if let Some(width) = clock.timestamp_width {
            map.insert(3u64, u64::from(width));
        }
        map.into()
    }
}

impl TryFrom<CBOR> for RtcMbc3 {
    type Error = dcbor::Error;

    fn try_from(value: CBOR) -> dcbor::Result<Self> {
        let map = want_map(&value)?;
        only_keys(map, &[0, 1, 2, 3])?;
        // Absent when there was no footer, and never null: null would claim a footer whose width
        // nobody kept.
        let timestamp_width = map.get::<u64, CBOR>(3).map(|value| want_uint(&value)).transpose()?;
        if timestamp_width.is_some_and(|width| !matches!(width, 4 | 8)) {
            return Err(dcbor::Error::OutOfRange);
        }
        Ok(RtcMbc3 {
            live: read_registers(&field(map, 0)?)?,
            latched: nullable(&field(map, 1)?, read_registers)?,
            taken_at: want_epoch(&field(map, 2)?)?,
            timestamp_width,
        })
    }
}

impl fmt::Display for RtcMbc3 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [seconds, minutes, hours, ..] = self.live;
        write!(f, "day {} {hours:02}:{minutes:02}:{seconds:02}", self.days())?;
        if self.halted() {
            f.write_str(" (halted)")?;
        }
        write_taken(f, Some(self.taken_at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLOCK: RtcMbc3 = RtcMbc3 {
        live: [30, 45, 13, 44, 1],
        latched: Some([30, 45, 13, 44, 1]),
        taken_at: 1_700_000_000,
        timestamp_width: Some(8),
    };

    #[test]
    fn an_mbc3_clock_round_trips() {
        assert_eq!(RtcMbc3::try_from(CBOR::from(CLOCK)).unwrap(), CLOCK);
        assert_eq!(CLOCK.days(), 300);
        assert_eq!(CLOCK.to_string(), "day 300 13:45:30, taken 2023-11-14 22:13:20");
        // The width is 4 or 8 and nothing else, since the footer is rebuilt from it.
        assert!(RtcMbc3::try_from(CBOR::from(RtcMbc3 { timestamp_width: Some(6), ..CLOCK })).is_err());
    }

    #[test]
    fn the_reading_is_the_counters_alone() {
        assert_eq!(CLOCK.reading(), Rtc::new(300 * 86_400 + 13 * 3_600 + 45 * 60 + 30));
    }

    #[test]
    fn a_clock_kept_outside_a_footer_says_so() {
        // As Gambatte keeps it: no latched copy, and no footer to have a width.
        let clock = RtcMbc3 { latched: None, timestamp_width: None, ..CLOCK };
        let encoded = CBOR::from(clock);
        let map = encoded.as_map().unwrap();
        assert!(map.get::<u64, CBOR>(1).unwrap().is_null(), "the latched copy is null");
        assert!(map.get::<u64, CBOR>(3).is_none(), "and the width is absent");
        assert_eq!(RtcMbc3::try_from(encoded).unwrap(), clock);
        // What a rebuild fills them with.
        assert_eq!(clock.latched_or_live(), clock.live);
        assert_eq!(clock.footer_width(), 8);
    }
}
