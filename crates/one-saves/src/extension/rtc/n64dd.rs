//! [`x.1sav.rtc.n64dd`](https://docs.1retro.com/specifications/extensions/x.1sav.rtc.n64dd/).

use std::fmt;

use dcbor::{CBOR, Map};

use super::{Rtc, field, from_bcd, optional_epoch, utc, want_bytes, write_taken};
use crate::extension::{Extension, epoch, only_keys, want_map};
use crate::name::ReverseDnsName;

/// The clock in a 64DD drive, which every disk in the drive reads, and the moment a producer took
/// it.
///
/// It goes on a device's header or a state's, never on a disk's save: the clock is the drive's.
/// ares keeps it in the 64DD's `time.rtc`, and the six bytes and the anchor are everything that
/// file holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtcN64dd {
    /// The clock as the drive returns it, one packed BCD byte each: year, month, day, hour, minute
    /// and second. The hours run 0 to 23. Bit 7 of the minute byte is set when the clock has
    /// stopped or its battery has run low, and is kept as read.
    pub time: [u8; 6],
    /// The producer's clock when it took the time, in epoch seconds. On the producer's own scale.
    pub taken_at: Option<i64>,
}

impl RtcN64dd {
    /// Whether the drive reports its clock as not set: bit 7 of the minute byte.
    #[must_use]
    pub fn not_set(&self) -> bool {
        self.time[4] & 0x80 != 0
    }

    /// The clock on the Unix scale: the time read as though it were UTC, with the century libleo
    /// supplies, so 96 to 99 are 1996 to 1999 and 00 to 95 are 2000 to 2095.
    ///
    /// `None` when the drive reports the clock as not set, which a game does not trust either, and
    /// when the time is not BCD or not a date.
    #[must_use]
    pub fn reading(&self) -> Option<Rtc> {
        if self.not_set() {
            return None;
        }
        let [year, month, day, hour, minute, second] = self.time;
        let year = i64::from(from_bcd(year)?);
        let year = if year >= 96 { 1900 + year } else { 2000 + year };
        let seconds = utc(
            year,
            from_bcd(month)?,
            from_bcd(day)?,
            from_bcd(hour)?,
            from_bcd(minute)?,
            from_bcd(second)?,
        )?;
        Some(Rtc::new(seconds))
    }
}

impl Extension for RtcN64dd {
    const KEY: ReverseDnsName = ReverseDnsName::from_static("x.1sav.rtc.n64dd");
}

impl From<RtcN64dd> for CBOR {
    fn from(clock: RtcN64dd) -> Self {
        let mut map = Map::new();
        map.insert(0u64, CBOR::to_byte_string(clock.time));
        if let Some(taken_at) = clock.taken_at {
            map.insert(1u64, epoch(taken_at));
        }
        map.into()
    }
}

impl TryFrom<CBOR> for RtcN64dd {
    type Error = dcbor::Error;

    fn try_from(value: CBOR) -> dcbor::Result<Self> {
        let map = want_map(&value)?;
        only_keys(map, &[0, 1])?;
        Ok(RtcN64dd { time: want_bytes(&field(map, 0)?)?, taken_at: optional_epoch(map, 1)? })
    }
}

impl fmt::Display for RtcN64dd {
    /// The time as the digits it spells, under the century libleo gives them, or as its bytes when
    /// it spells none.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [year, month, day, hour, minute, second] = self.time;
        let minute = minute & 0x7f;
        let bcd = |byte: u8| byte >> 4 <= 9 && byte & 0x0f <= 9;
        if [year, month, day, hour, minute, second].into_iter().all(bcd) {
            let century = if year >= 0x96 { 19 } else { 20 };
            write!(f, "{century}{year:02x}-{month:02x}-{day:02x} {hour:02x}:{minute:02x}:{second:02x}")?;
        } else {
            f.write_str("not BCD: h'")?;
            self.time.iter().try_for_each(|byte| write!(f, "{byte:02x}"))?;
            f.write_str("'")?;
        }
        if self.not_set() {
            f.write_str(" (not set)")?;
        }
        write_taken(f, self.taken_at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2001-04-14 12:34:56.
    const CLOCK: RtcN64dd =
        RtcN64dd { time: [0x01, 0x04, 0x14, 0x12, 0x34, 0x56], taken_at: Some(1_044_057_600) };

    #[test]
    fn a_drives_clock_round_trips_and_shows_its_date() {
        assert_eq!(RtcN64dd::try_from(CBOR::from(CLOCK)).unwrap(), CLOCK);
        assert_eq!(CLOCK.to_string(), "2001-04-14 12:34:56, taken 2003-02-01 00:00:00");
        let unset = RtcN64dd { time: [0x01, 0x04, 0x14, 0x12, 0xb4, 0x56], taken_at: None };
        assert_eq!(unset.to_string(), "2001-04-14 12:34:56 (not set)");
    }

    #[test]
    fn libleo_supplies_the_century() {
        assert_eq!(CLOCK.reading(), Some(Rtc::new(987_251_696)));
        let ninety_nine = RtcN64dd { time: [0x99, 0x04, 0x14, 0x12, 0x34, 0x56], ..CLOCK };
        assert_eq!(ninety_nine.reading(), Some(Rtc::new(924_093_296)), "96 to 99 are 19xx");
        let ninety_five = RtcN64dd { time: [0x95, 0x01, 0x01, 0x00, 0x00, 0x00], ..CLOCK };
        assert_eq!(ninety_five.reading(), Some(Rtc::new(3_944_678_400)), "00 to 95 are 20xx");
    }

    #[test]
    fn a_clock_the_drive_reports_as_not_set_has_no_reading() {
        let unset = RtcN64dd { time: [0x01, 0x04, 0x14, 0x12, 0xb4, 0x56], ..CLOCK };
        assert_eq!(unset.reading(), None);
    }
}
