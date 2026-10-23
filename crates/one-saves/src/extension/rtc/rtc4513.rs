//! [`x.1sav.rtc.rtc4513`](https://docs.1retro.com/specifications/extensions/x.1sav.rtc.rtc4513/).

use std::fmt;

use dcbor::{CBOR, Map};

use super::{
    Rtc, array, digits, field, hour_from_12, optional_epoch, utc, want_array, want_nybble, write_taken,
};
use crate::extension::{Extension, epoch, only_keys, want_map};
use crate::name::ReverseDnsName;

/// The registers of the Epson RTC-4513 beside Tengai Makyou Zero's SPC7110, and the moment a
/// producer took them.
///
/// Sixteen registers of four bits, each as a read of it returns it. The digits are BCD and the
/// flags sit in the bits the chip gives them, so nothing here is a field of its own: LOST beside
/// the tens of seconds, PM beside the tens of hours, the control bits in D, E and F. bsnes, Snes9x
/// and Mesen2 each write the chip to a file, in three different layouts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtcRtc4513 {
    /// Registers 0 to F, in index order.
    pub registers: [u8; 16],
    /// The producer's clock when it took the registers, in epoch seconds: the moment it advances
    /// the chip from when the save is next loaded. On the producer's own scale.
    pub taken_at: Option<i64>,
}

impl RtcRtc4513 {
    /// Whether the chip lost its time, as on a battery failure: bit 3 of register 1.
    #[must_use]
    pub fn lost(&self) -> bool {
        self.registers[1] & 0x08 != 0
    }

    /// Whether the day, month and year registers are a date, which CAL/HW, bit 1 of register D,
    /// says. Clear, they are user RAM.
    #[must_use]
    pub fn calendar(&self) -> bool {
        self.registers[0xd] & 0x02 != 0
    }

    /// Whether the chip is in 24-hour mode, which is 24/12, bit 2 of register F.
    #[must_use]
    pub fn hour24(&self) -> bool {
        self.registers[0xf] & 0x04 != 0
    }

    /// Whether the clock is stopped, which is STOP, bit 1 of register F.
    #[must_use]
    pub fn stopped(&self) -> bool {
        self.registers[0xf] & 0x02 != 0
    }

    /// The year's two digits, the month and the day, as the registers spell them.
    fn date(&self) -> Option<(u8, u8, u8)> {
        let r = &self.registers;
        Some((digits(r[0xb], r[0xa])?, digits(r[9] & 0x01, r[8])?, digits(r[7] & 0x03, r[6])?))
    }

    /// The hour as the registers spell it, in whichever mode the chip is in, then the minute and
    /// the second.
    fn time(&self) -> Option<(u8, u8, u8)> {
        let r = &self.registers;
        Some((digits(r[5] & 0x03, r[4])?, digits(r[3] & 0x07, r[2])?, digits(r[1] & 0x07, r[0])?))
    }

    /// Whether the hour is after noon, which in 12-hour mode is PM, bit 2 of register 5.
    fn pm(&self) -> bool {
        self.registers[5] & 0x04 != 0
    }

    /// The chip's clock on the Unix scale: its components read as though they were UTC.
    ///
    /// The year is two digits with no century, so the caller supplies one: `century` is the year
    /// the digits count from, 1900 reading 95 as 1995.
    ///
    /// `None` when the components are not a date, and when the calendar is off and the date
    /// registers are user RAM. A reading that cannot be computed is omitted.
    #[must_use]
    pub fn reading(&self, century: i64) -> Option<Rtc> {
        if !self.calendar() {
            return None;
        }
        let (year, month, day) = self.date()?;
        let (hour, minute, second) = self.time()?;
        let hour = if self.hour24() { hour } else { hour_from_12(hour, self.pm())? };
        Some(Rtc::new(utc(century + i64::from(year), month, day, hour, minute, second)?))
    }
}

impl Extension for RtcRtc4513 {
    const KEY: ReverseDnsName = ReverseDnsName::from_static("x.1sav.rtc.rtc4513");
}

impl From<RtcRtc4513> for CBOR {
    fn from(clock: RtcRtc4513) -> Self {
        let mut map = Map::new();
        map.insert(0u64, array(&clock.registers, CBOR::from));
        if let Some(taken_at) = clock.taken_at {
            map.insert(1u64, epoch(taken_at));
        }
        map.into()
    }
}

impl TryFrom<CBOR> for RtcRtc4513 {
    type Error = dcbor::Error;

    fn try_from(value: CBOR) -> dcbor::Result<Self> {
        let map = want_map(&value)?;
        only_keys(map, &[0, 1])?;
        Ok(RtcRtc4513 {
            registers: want_array(&field(map, 0)?, want_nybble)?,
            taken_at: optional_epoch(map, 1)?,
        })
    }
}

impl fmt::Display for RtcRtc4513 {
    /// The registers as the digits they spell, with the year's two digits and no century, since
    /// the chip keeps none.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some((hour, minute, second)) = self.time() {
            if let (true, Some((year, month, day))) = (self.calendar(), self.date()) {
                write!(f, "{year:02}-{month:02}-{day:02} ")?;
            }
            write!(f, "{hour:02}:{minute:02}:{second:02}")?;
            if !self.hour24() {
                f.write_str(if self.pm() { " PM" } else { " AM" })?;
            }
        } else {
            f.write_str("not BCD: ")?;
            self.registers.iter().try_for_each(|nybble| write!(f, "{nybble:x}"))?;
        }
        f.write_str(if self.hour24() { ", 24-hour" } else { ", 12-hour" })?;
        if !self.calendar() {
            f.write_str(", calendar off")?;
        }
        if self.lost() {
            f.write_str(", lost")?;
        }
        if self.stopped() {
            f.write_str(", stopped")?;
        }
        write_taken(f, self.taken_at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1995-12-29 23:59:50 with the calendar on, interrupts masked and 24-hour mode, which is where
    /// MiSTer powers the chip up.
    const CLOCK: RtcRtc4513 = RtcRtc4513 {
        registers: [0, 5, 9, 5, 3, 2, 9, 2, 2, 1, 5, 9, 5, 2, 15, 4],
        taken_at: Some(1_044_057_600),
    };

    #[test]
    fn an_epson_clock_round_trips_and_shows_its_digits() {
        assert_eq!(RtcRtc4513::try_from(CBOR::from(CLOCK)).unwrap(), CLOCK);
        assert_eq!(CLOCK.to_string(), "95-12-29 23:59:50, 24-hour, taken 2003-02-01 00:00:00");
    }

    #[test]
    fn the_reading_takes_a_century_and_the_meridian() {
        assert_eq!(CLOCK.reading(1900), Some(Rtc::new(820_281_590)));
        // 11:59:50 PM in 12-hour mode after a battery failure: PM is bit 2 of the hours tens and
        // LOST bit 3 of the seconds tens.
        let twelve =
            RtcRtc4513 { registers: [0, 13, 9, 5, 1, 5, 9, 2, 2, 1, 5, 9, 5, 2, 15, 0], taken_at: None };
        assert_eq!(twelve.reading(1900), CLOCK.reading(1900));
        assert_eq!(twelve.to_string(), "95-12-29 11:59:50 PM, 12-hour, lost");
        // With the calendar off the date registers are RAM, and there is no date to read.
        let ram =
            RtcRtc4513 { registers: [0, 5, 9, 5, 3, 2, 15, 7, 15, 7, 15, 15, 5, 0, 15, 4], taken_at: None };
        assert_eq!(ram.reading(1900), None);
        assert_eq!(ram.to_string(), "23:59:50, 24-hour, calendar off");
    }
}
