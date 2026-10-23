//! [`x.1sav.rtc.srtc`](https://docs.1retro.com/specifications/extensions/x.1sav.rtc.srtc/).

use std::fmt;

use dcbor::{CBOR, Map};

use super::{Rtc, array, digits, field, optional_epoch, utc, want_array, want_nybble, write_taken};
use crate::extension::{Extension, epoch, only_keys, want_map};
use crate::name::ReverseDnsName;

/// The registers of the Sharp S-RTC in Daikaijuu Monogatari II, and the moment a producer took
/// them.
///
/// Thirteen registers of four bits, in the order a read returns them: seconds, minutes, hours and
/// day, each ones then tens; the month in binary; the year, ones then tens; the century in binary,
/// 9 for 19xx and 10 for 20xx; the weekday. bsnes and Snes9x each write the chip to a file, and
/// the registers and the anchor are everything either holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtcSrtc {
    /// Registers 0 to 12, in index order.
    pub registers: [u8; 13],
    /// The producer's clock when it took the registers, in epoch seconds: the moment it advances
    /// the chip from when the save is next loaded. On the producer's own scale.
    pub taken_at: Option<i64>,
}

impl RtcSrtc {
    /// The whole year, the month, the day, the hour, the minute and the second, as the registers
    /// spell them. The year is 1000, plus 100 times the century, plus the two digits.
    fn components(&self) -> Option<(i64, u8, u8, u8, u8, u8)> {
        let r = &self.registers;
        let year = 1000 + 100 * i64::from(r[11]) + i64::from(digits(r[10], r[9])?);
        Some((year, r[8], digits(r[7], r[6])?, digits(r[5], r[4])?, digits(r[3], r[2])?, digits(r[1], r[0])?))
    }

    /// The chip's clock on the Unix scale: its registers read as though they were UTC.
    ///
    /// The chip spells its century and has no 12-hour mode, so nothing has to be supplied. `None`
    /// when the registers are not a date.
    #[must_use]
    pub fn reading(&self) -> Option<Rtc> {
        let (year, month, day, hour, minute, second) = self.components()?;
        Some(Rtc::new(utc(year, month, day, hour, minute, second)?))
    }
}

impl Extension for RtcSrtc {
    const KEY: ReverseDnsName = ReverseDnsName::from_static("x.1sav.rtc.srtc");
}

impl From<RtcSrtc> for CBOR {
    fn from(clock: RtcSrtc) -> Self {
        let mut map = Map::new();
        map.insert(0u64, array(&clock.registers, CBOR::from));
        if let Some(taken_at) = clock.taken_at {
            map.insert(1u64, epoch(taken_at));
        }
        map.into()
    }
}

impl TryFrom<CBOR> for RtcSrtc {
    type Error = dcbor::Error;

    fn try_from(value: CBOR) -> dcbor::Result<Self> {
        let map = want_map(&value)?;
        only_keys(map, &[0, 1])?;
        Ok(RtcSrtc {
            registers: want_array(&field(map, 0)?, want_nybble)?,
            taken_at: optional_epoch(map, 1)?,
        })
    }
}

impl fmt::Display for RtcSrtc {
    /// The registers as the date and time they spell, or as the nybbles they are when they spell
    /// none.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some((year, month, day, hour, minute, second)) = self.components() {
            write!(f, "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")?;
        } else {
            f.write_str("not BCD: ")?;
            self.registers.iter().try_for_each(|nybble| write!(f, "{nybble:x}"))?;
        }
        write_taken(f, self.taken_at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1996-12-20 18:30:05, a Friday.
    const CLOCK: RtcSrtc = RtcSrtc { registers: [5, 0, 0, 3, 8, 1, 0, 2, 12, 6, 9, 9, 5], taken_at: None };

    #[test]
    fn a_sharp_clock_round_trips_and_shows_its_date() {
        assert_eq!(RtcSrtc::try_from(CBOR::from(CLOCK)).unwrap(), CLOCK);
        assert_eq!(CLOCK.to_string(), "1996-12-20 18:30:05");
        let anchored = RtcSrtc { taken_at: Some(1_044_057_600), ..CLOCK };
        assert_eq!(anchored.to_string(), "1996-12-20 18:30:05, taken 2003-02-01 00:00:00");
    }

    #[test]
    fn the_reading_spells_its_own_century() {
        assert_eq!(CLOCK.reading(), Some(Rtc::new(851_106_605)));
        let later = RtcSrtc { registers: [5, 0, 0, 3, 8, 1, 0, 2, 12, 6, 2, 10, 4], taken_at: None };
        assert_eq!(later.reading(), Some(Rtc::new(1_797_791_405)));
        let undated = RtcSrtc { registers: [5, 0, 0, 3, 8, 1, 0, 2, 13, 6, 9, 9, 5], taken_at: None };
        assert_eq!(undated.reading(), None, "there is no thirteenth month");
    }
}
