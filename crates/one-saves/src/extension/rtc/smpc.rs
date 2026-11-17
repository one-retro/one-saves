//! [`x.1sav.rtc.smpc`](https://docs.1retro.com/specifications/extensions/x.1sav.rtc.smpc/).

use std::fmt;

use dcbor::{CBOR, Map};

use super::{Rtc, field, from_bcd, nullable, optional_epoch, or_null, utc, want_bytes, write_taken};
use crate::extension::{Extension, epoch, only_keys, want_map};
use crate::name::ReverseDnsName;

/// The clock in a Sega Saturn's SMPC, the four bytes of memory beside it, and the moment a producer
/// took them.
///
/// It goes on a device's header or a state's, never on a save: the clock is the console's, even
/// where a tool keeps one per game. Mednafen keeps the SMPC in a 12-byte `.smpc` file: a byte
/// saying whether the clock was set, the seven bytes of the time, then the four of memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtcSmpc {
    /// The clock as the SMPC returns it: the year in four BCD digits over two bytes, high byte
    /// first; the weekday, 0 to 6, in the high nibble and the month, 1 to 12 in binary, in the low
    /// one; then the day, hour, minute and second, one BCD byte each. The hours run 0 to 23.
    pub time: [u8; 7],
    /// The four bytes of battery-backed memory where the BIOS keeps the console's settings, or
    /// `None` where the producer's source does not keep them.
    pub smem: Option<[u8; 4]>,
    /// The producer's clock when it took the time, in epoch seconds. On the producer's own scale.
    pub taken_at: Option<i64>,
}

impl RtcSmpc {
    /// The weekday the SMPC keeps, 0 to 6.
    #[must_use]
    pub fn weekday(&self) -> u8 {
        self.time[2] >> 4
    }

    /// The clock on the Unix scale: the time read as though it were UTC, with the whole year the
    /// SMPC keeps.
    ///
    /// `None` when the time is not BCD or not a date.
    #[must_use]
    pub fn reading(&self) -> Option<Rtc> {
        let [high, low, weekday_month, day, hour, minute, second] = self.time;
        let year = i64::from(from_bcd(high)?) * 100 + i64::from(from_bcd(low)?);
        let seconds = utc(
            year,
            weekday_month & 0x0f,
            from_bcd(day)?,
            from_bcd(hour)?,
            from_bcd(minute)?,
            from_bcd(second)?,
        )?;
        Some(Rtc::new(seconds))
    }
}

impl Extension for RtcSmpc {
    const KEY: ReverseDnsName = ReverseDnsName::from_static("x.1sav.rtc.smpc");
}

impl From<RtcSmpc> for CBOR {
    fn from(clock: RtcSmpc) -> Self {
        let mut map = Map::new();
        map.insert(0u64, CBOR::to_byte_string(clock.time));
        map.insert(1u64, or_null(clock.smem, CBOR::to_byte_string));
        if let Some(taken_at) = clock.taken_at {
            map.insert(2u64, epoch(taken_at));
        }
        map.into()
    }
}

impl TryFrom<CBOR> for RtcSmpc {
    type Error = dcbor::Error;

    fn try_from(value: CBOR) -> dcbor::Result<Self> {
        let map = want_map(&value)?;
        only_keys(map, &[0, 1, 2])?;
        Ok(RtcSmpc {
            time: want_bytes(&field(map, 0)?)?,
            smem: nullable(&field(map, 1)?, want_bytes)?,
            taken_at: optional_epoch(map, 2)?,
        })
    }
}

impl fmt::Display for RtcSmpc {
    /// The time as the digits it spells, or as its bytes when it spells none.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [high, low, weekday_month, day, hour, minute, second] = self.time;
        let month = weekday_month & 0x0f;
        let bcd = |byte: u8| byte >> 4 <= 9 && byte & 0x0f <= 9;
        if [high, low, day, hour, minute, second].into_iter().all(bcd) {
            write!(f, "{high:02x}{low:02x}-{month:02}-{day:02x} {hour:02x}:{minute:02x}:{second:02x}")?;
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

    /// 2001-04-14 12:34:56, a Saturday.
    const CLOCK: RtcSmpc = RtcSmpc {
        time: [0x20, 0x01, 0x64, 0x14, 0x12, 0x34, 0x56],
        smem: Some([0, 0, 0, 0]),
        taken_at: Some(1_044_057_600),
    };

    #[test]
    fn a_saturns_clock_round_trips_and_shows_its_date() {
        assert_eq!(RtcSmpc::try_from(CBOR::from(CLOCK)).unwrap(), CLOCK);
        let kept_elsewhere = RtcSmpc { smem: None, taken_at: None, ..CLOCK };
        assert_eq!(RtcSmpc::try_from(CBOR::from(kept_elsewhere)).unwrap(), kept_elsewhere);
        assert_eq!(CLOCK.to_string(), "2001-04-14 12:34:56, taken 2003-02-01 00:00:00");
        assert_eq!(CLOCK.weekday(), 6);
    }

    #[test]
    fn the_reading_is_the_whole_year_as_utc() {
        assert_eq!(CLOCK.reading(), Some(Rtc::new(987_251_696)));
        let nineteen = RtcSmpc { time: [0x19, 0x99, 0x44, 0x14, 0x12, 0x34, 0x56], ..CLOCK };
        assert_eq!(nineteen.reading(), Some(Rtc::new(924_093_296)));
        let thirteenth_month = RtcSmpc { time: [0x20, 0x01, 0x6d, 0x14, 0x12, 0x34, 0x56], ..CLOCK };
        assert_eq!(thirteenth_month.reading(), None);
    }
}
