//! [`x.1sav.rtc.upd4990a`](https://docs.1retro.com/specifications/extensions/x.1sav.rtc.upd4990a/).

use std::fmt;

use dcbor::{CBOR, Map};

use super::{Rtc, field, from_bcd, optional_epoch, utc, want_bytes, write_taken};
use crate::extension::{Extension, epoch, only_keys, want_map};
use crate::name::ReverseDnsName;

/// The NEC uPD4990A clock on a Neo Geo MVS board, and the moment a producer took it.
///
/// It goes on the header of the MVS's device, beside its backup RAM and its card, or on a state's,
/// never on a save: the clock is the board's. An AES has none. ares keeps it in the MVS's
/// `time.rtc`, and the six bytes and the anchor are everything that file holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtcUpd4990a {
    /// The time in the order the chip shifts it out: second, minute, hour and day, one BCD byte
    /// each; then the month, 1 to 12 in binary, in the high nibble and the weekday, 0 to 6, in the
    /// low one; then the year, two BCD digits. The hours run 0 to 23.
    pub time: [u8; 6],
    /// The producer's clock when it took the time, in epoch seconds. On the producer's own scale.
    pub taken_at: Option<i64>,
}

impl RtcUpd4990a {
    /// The weekday the chip keeps, 0 to 6.
    #[must_use]
    pub fn weekday(&self) -> u8 {
        self.time[4] & 0x0f
    }

    /// The clock on the Unix scale: the time read as though it were UTC, its two year digits
    /// counted from `century`, such as 2000.
    ///
    /// The chip keeps no century, so the caller supplies one. `None` when the time is not BCD or
    /// not a date.
    #[must_use]
    pub fn reading(&self, century: i64) -> Option<Rtc> {
        let [second, minute, hour, day, month_weekday, year] = self.time;
        let year = century + i64::from(from_bcd(year)?);
        let seconds = utc(
            year,
            month_weekday >> 4,
            from_bcd(day)?,
            from_bcd(hour)?,
            from_bcd(minute)?,
            from_bcd(second)?,
        )?;
        Some(Rtc::new(seconds))
    }
}

impl Extension for RtcUpd4990a {
    const KEY: ReverseDnsName = ReverseDnsName::from_static("x.1sav.rtc.upd4990a");
}

impl From<RtcUpd4990a> for CBOR {
    fn from(clock: RtcUpd4990a) -> Self {
        let mut map = Map::new();
        map.insert(0u64, CBOR::to_byte_string(clock.time));
        if let Some(taken_at) = clock.taken_at {
            map.insert(1u64, epoch(taken_at));
        }
        map.into()
    }
}

impl TryFrom<CBOR> for RtcUpd4990a {
    type Error = dcbor::Error;

    fn try_from(value: CBOR) -> dcbor::Result<Self> {
        let map = want_map(&value)?;
        only_keys(map, &[0, 1])?;
        Ok(RtcUpd4990a { time: want_bytes(&field(map, 0)?)?, taken_at: optional_epoch(map, 1)? })
    }
}

impl fmt::Display for RtcUpd4990a {
    /// The time as the digits it spells, with no century since the chip keeps none, or as its
    /// bytes when it spells none.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [second, minute, hour, day, month_weekday, year] = self.time;
        let month = month_weekday >> 4;
        let bcd = |byte: u8| byte >> 4 <= 9 && byte & 0x0f <= 9;
        if [second, minute, hour, day, year].into_iter().all(bcd) {
            write!(f, "'{year:02x}-{month:02}-{day:02x} {hour:02x}:{minute:02x}:{second:02x}")?;
        } else {
            f.write_str("not BCD: h'")?;
            self.time.iter().try_for_each(|byte| write!(f, "{byte:02x}"))?;
            f.write_str("'")?;
        }
        write_taken(f, self.taken_at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The specification's example, as ares keeps it: 2001-04-14 12:34:56, a Saturday.
    const CLOCK: RtcUpd4990a = RtcUpd4990a { time: [0x56, 0x34, 0x12, 0x14, 0x46, 0x01], taken_at: None };

    #[test]
    fn an_mvs_clock_round_trips_and_shows_its_date() {
        assert_eq!(RtcUpd4990a::try_from(CBOR::from(CLOCK)).unwrap(), CLOCK);
        let taken = RtcUpd4990a { taken_at: Some(1_044_057_600), ..CLOCK };
        assert_eq!(RtcUpd4990a::try_from(CBOR::from(taken)).unwrap(), taken);
        assert_eq!(CLOCK.to_string(), "'01-04-14 12:34:56");
        assert_eq!(CLOCK.weekday(), 6);
    }

    #[test]
    fn the_reading_takes_the_century_it_is_given() {
        assert_eq!(CLOCK.reading(2000), Some(Rtc::new(987_251_696)));
        assert_eq!(CLOCK.reading(1900), Some(Rtc::new(-2_168_508_304)));
        let thirteenth_month = RtcUpd4990a { time: [0x56, 0x34, 0x12, 0x14, 0xd6, 0x01], ..CLOCK };
        assert_eq!(thirteenth_month.reading(2000), None);
    }
}
