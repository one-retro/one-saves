//! [`x.1sav.rtc`](https://docs.1retro.com/specifications/extensions/x.1sav.rtc/) and the chip
//! states under it.
//!
//! The reading is what any consumer wants, and a chip key is what a producer rebuilding its file
//! needs. Every chip key's type here says how its registers go on the Unix scale, through a
//! `reading` method, so that a producer and a consumer computing [`Rtc`] from one get the same
//! answer.

use std::fmt;

use dcbor::{CBOR, Map};

use super::time::Civil;
use super::{Extension, epoch, only_keys, want_epoch, want_map, want_uint};
use crate::name::ReverseDnsName;

mod huc3;
mod joybus;
mod mbc3;
mod rtc4513;
mod s3511a;
mod srtc;
mod tc8521;
mod tpp1;

pub use huc3::RtcHuc3;
pub use joybus::RtcJoybus;
pub use mbc3::RtcMbc3;
pub use rtc4513::RtcRtc4513;
pub use s3511a::{RtcS3511a, S3511aAlarm};
pub use srtc::RtcSrtc;
pub use tc8521::RtcTc8521;
pub use tpp1::RtcTpp1;

/// A game's clock reading, on the Unix scale.
///
/// **Never the anchor a chip key carries**, which is the host clock a producer advances the chip
/// from. How a chip's registers go on the scale depends on the kind of clock. A counting clock,
/// such as an MBC3, reads as its elapsed time after 1970-01-01 UTC. A dating clock, such as the
/// GBA's Seiko chip, reads as its components taken as though they were UTC. Each chip key's type
/// has a `reading` method that does this. A producer that cannot compute the reading omits the
/// key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rtc {
    /// The game's clock, in whole seconds on the Unix scale.
    pub reading: i64,
    /// The uncertainty of the reading, in milliseconds, where a producer measured it.
    pub accuracy_ms: Option<u64>,
}

impl Rtc {
    /// A reading with no stated accuracy, which is what a producer that cannot measure one writes.
    #[must_use]
    pub fn new(reading: i64) -> Self {
        Rtc { reading, accuracy_ms: None }
    }

    /// States how far the reading may be off.
    #[must_use]
    pub fn accuracy_ms(mut self, accuracy_ms: u64) -> Self {
        self.accuracy_ms = Some(accuracy_ms);
        self
    }
}

impl Extension for Rtc {
    const KEY: ReverseDnsName = ReverseDnsName::from_static("x.1sav.rtc");
}

impl From<Rtc> for CBOR {
    fn from(rtc: Rtc) -> Self {
        let mut map = Map::new();
        map.insert(0u64, epoch(rtc.reading));
        if let Some(accuracy) = rtc.accuracy_ms {
            map.insert(1u64, accuracy);
        }
        map.into()
    }
}

impl TryFrom<CBOR> for Rtc {
    type Error = dcbor::Error;

    fn try_from(value: CBOR) -> dcbor::Result<Self> {
        let map = want_map(&value)?;
        only_keys(map, &[0, 1])?;
        Ok(Rtc {
            reading: want_epoch(&field(map, 0)?)?,
            accuracy_ms: map.get::<u64, CBOR>(1).map(|value| want_uint(&value)).transpose()?,
        })
    }
}

impl fmt::Display for Rtc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} UTC", Civil(self.reading))?;
        if let Some(accuracy) = self.accuracy_ms {
            write!(f, ", ±{accuracy} ms")?;
        }
        Ok(())
    }
}

/// A key the schema requires.
fn field(map: &Map, key: u64) -> dcbor::Result<CBOR> {
    map.get::<u64, CBOR>(key).ok_or(dcbor::Error::MissingMapKey)
}

/// The anchor a chip key may carry: whole epoch seconds under tag 1, when the key is there.
fn optional_epoch(map: &Map, key: u64) -> dcbor::Result<Option<i64>> {
    map.get::<u64, CBOR>(key).map(|value| want_epoch(&value)).transpose()
}

/// A byte string of exactly `N` bytes.
fn want_bytes<const N: usize>(value: &CBOR) -> dcbor::Result<[u8; N]> {
    let bytes = value.as_byte_string().ok_or(dcbor::Error::WrongType)?;
    bytes.try_into().map_err(|_| dcbor::Error::OutOfRange)
}

/// An array of exactly `N` items, each read by `read`.
fn want_array<T: Copy + Default, const N: usize>(
    value: &CBOR,
    read: impl Fn(&CBOR) -> dcbor::Result<T>,
) -> dcbor::Result<[T; N]> {
    let items = value.as_array().ok_or(dcbor::Error::WrongType)?;
    if items.len() != N {
        return Err(dcbor::Error::OutOfRange);
    }
    let mut out = [T::default(); N];
    for (slot, item) in out.iter_mut().zip(items) {
        *slot = read(item)?;
    }
    Ok(out)
}

/// A register four bits wide.
fn want_nybble(value: &CBOR) -> dcbor::Result<u8> {
    let nybble: u8 = want_uint(value)?;
    if nybble > 0x0f {
        return Err(dcbor::Error::OutOfRange);
    }
    Ok(nybble)
}

/// A value read by `read`, or `None` for null: what the chip holds and the producer's source did
/// not keep.
fn nullable<T>(value: &CBOR, read: impl Fn(&CBOR) -> dcbor::Result<T>) -> dcbor::Result<Option<T>> {
    if value.is_null() { Ok(None) } else { read(value).map(Some) }
}

/// Writes each value with `write`, as an array.
fn array<T: Copy>(values: &[T], write: impl Fn(T) -> CBOR) -> CBOR {
    values.iter().map(|&value| write(value)).collect::<Vec<_>>().into()
}

/// Writes a value with `write`, or null for `None`.
fn or_null<T>(value: Option<T>, write: impl Fn(T) -> CBOR) -> CBOR {
    value.map_or_else(CBOR::null, write)
}

/// Writes a register four bits wide, or null where the source did not keep it.
fn nybble_or_null(value: Option<u8>) -> CBOR {
    or_null(value, CBOR::from)
}

/// The anchor a chip's line ends on, when it has one.
///
/// It is on the producer's own scale, which is why no zone is printed beside it.
fn write_taken(f: &mut fmt::Formatter<'_>, taken_at: Option<i64>) -> fmt::Result {
    match taken_at {
        Some(taken_at) => write!(f, ", taken {}", Civil(taken_at)),
        None => Ok(()),
    }
}

/// A BCD byte's value, when both its digits are digits.
fn from_bcd(byte: u8) -> Option<u8> {
    digits(byte >> 4, byte & 0x0f)
}

/// A tens digit and a ones digit, kept one to a register, as the value they spell.
fn digits(tens: u8, ones: u8) -> Option<u8> {
    (tens <= 9 && ones <= 9).then_some(tens * 10 + ones)
}

/// An hour on a 12-hour clock, put on the 24-hour one.
///
/// Hours 0 to 12 are taken, so a chip counting 1 to 12 and one counting 0 to 11 both read right:
/// twelve before noon is midnight.
fn hour_from_12(hour: u8, pm: bool) -> Option<u8> {
    (hour <= 12).then_some(hour % 12 + if pm { 12 } else { 0 })
}

/// A counting clock's registers as that long after 1970-01-01 UTC.
fn elapsed(days: i64, hours: i64, minutes: i64, seconds: i64) -> i64 {
    days * 86_400 + hours * 3_600 + minutes * 60 + seconds
}

/// A dating clock's components read as though they were UTC, when they are a date and a time of
/// day.
fn utc(year: i64, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> Option<i64> {
    let in_range = (1..=12).contains(&month)
        && (1..=days_in_month(year, month)).contains(&day)
        && hour <= 23
        && minute <= 59
        && second <= 59;
    in_range.then(|| elapsed(days_from_civil(year, month, day), hour.into(), minute.into(), second.into()))
}

/// How many days a month has, in the Gregorian calendar.
fn days_in_month(year: i64, month: u8) -> u8 {
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    match month {
        2 if leap => 29,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reading_round_trips_and_shows_as_a_date() {
        let rtc = Rtc::new(1_700_000_000);
        assert_eq!(Rtc::try_from(CBOR::from(rtc)).unwrap(), rtc);
        assert_eq!(rtc.to_string(), "2023-11-14 22:13:20 UTC");
        assert_eq!(rtc.accuracy_ms(1500).to_string(), "2023-11-14 22:13:20 UTC, ±1500 ms");
    }

    #[test]
    fn components_read_as_utc_only_when_they_are_a_date() {
        assert_eq!(utc(2023, 11, 14, 22, 13, 20), Some(1_700_000_000));
        assert_eq!(utc(2000, 2, 29, 0, 0, 0), Some(951_782_400));
        // 1900 was not a leap year, whatever a two-digit chip thinks of year 00.
        assert_eq!(utc(1900, 2, 29, 0, 0, 0), None);
        assert_eq!(utc(2026, 13, 1, 0, 0, 0), None);
        assert_eq!(utc(2026, 4, 31, 0, 0, 0), None);
        assert_eq!(utc(2026, 4, 30, 24, 0, 0), None);
    }

    #[test]
    fn a_twelve_hour_clock_reads_either_way_it_counts() {
        assert_eq!(hour_from_12(12, false), Some(0));
        assert_eq!(hour_from_12(0, false), Some(0));
        assert_eq!(hour_from_12(12, true), Some(12));
        assert_eq!(hour_from_12(0, true), Some(12));
        assert_eq!(hour_from_12(11, true), Some(23));
        assert_eq!(hour_from_12(13, false), None);
    }
}
