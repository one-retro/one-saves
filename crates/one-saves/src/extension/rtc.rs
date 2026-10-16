//! [`x.1sav.rtc`](https://docs.1retro.com/specifications/extensions/x.1sav.rtc/) and the chip
//! states under it.

use std::fmt;

use dcbor::{CBOR, Map};

use super::time::Civil;
use super::{Extension, epoch, only_keys, want_epoch, want_map, want_uint};
use crate::name::ReverseDnsName;

/// What a cartridge's clock showed, as a Unix instant.
///
/// **Never the point a counting clock advances from.** An MBC3 keeps elapsed time beside a host
/// timestamp, and that timestamp is the anchor rather than the reading: the reading is the anchor
/// plus the elapsed time. A producer that cannot compute the reading omits the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rtc {
    /// What the clock showed, in whole epoch seconds.
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
            reading: want_epoch(&map.get::<u64, CBOR>(0).ok_or(dcbor::Error::MissingMapKey)?)?,
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

/// The clock an MBC3 Game Boy cartridge keeps, register by register.
///
/// Every byte of the footer an emulator appends is accounted for here, so the footer is rebuilt
/// from this rather than stored: forty bytes of registers, then the timestamp at its stated width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtcMbc3 {
    /// The live registers: seconds, minutes, hours, day counter low, day counter high.
    pub live: [u32; 5],
    /// The latched copy the game reads, in the same order.
    pub latched: [u32; 5],
    /// The host clock when the emulator wrote the save, in epoch seconds. The anchor, not the
    /// reading.
    pub written_at: i64,
    /// How many bytes that timestamp occupied: 4 or 8, depending on the emulator.
    pub timestamp_width: u8,
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
}

impl Extension for RtcMbc3 {
    const KEY: ReverseDnsName = ReverseDnsName::from_static("x.1sav.rtc.mbc3");
}

fn registers(values: [u32; 5]) -> CBOR {
    values.iter().map(|&value| CBOR::from(value)).collect::<Vec<_>>().into()
}

fn read_registers(value: &CBOR) -> dcbor::Result<[u32; 5]> {
    let items = value.as_array().ok_or(dcbor::Error::WrongType)?;
    if items.len() != 5 {
        return Err(dcbor::Error::OutOfRange);
    }
    let mut out = [0u32; 5];
    for (slot, item) in out.iter_mut().zip(items) {
        *slot = want_uint(item)?;
    }
    Ok(out)
}

impl From<RtcMbc3> for CBOR {
    fn from(clock: RtcMbc3) -> Self {
        let mut map = Map::new();
        map.insert(0u64, registers(clock.live));
        map.insert(1u64, registers(clock.latched));
        map.insert(2u64, epoch(clock.written_at));
        map.insert(3u64, u64::from(clock.timestamp_width));
        map.into()
    }
}

impl TryFrom<CBOR> for RtcMbc3 {
    type Error = dcbor::Error;

    fn try_from(value: CBOR) -> dcbor::Result<Self> {
        let map = want_map(&value)?;
        only_keys(map, &[0, 1, 2, 3])?;
        let field = |key: u64| map.get::<u64, CBOR>(key).ok_or(dcbor::Error::MissingMapKey);
        let timestamp_width = want_uint(&field(3)?)?;
        if !matches!(timestamp_width, 4 | 8) {
            return Err(dcbor::Error::OutOfRange);
        }
        Ok(RtcMbc3 {
            live: read_registers(&field(0)?)?,
            latched: read_registers(&field(1)?)?,
            written_at: want_epoch(&field(2)?)?,
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
        write!(f, ", written {} UTC", Civil(self.written_at))
    }
}

/// The latched state of a GBA cartridge's Seiko S-3511A.
///
/// The instant the chip was read is not here: it is a Unix instant, so it is exactly what
/// [`Rtc`] carries beside this, and holding it twice would let the two drift apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtcS3511a {
    /// The seven latched BCD bytes: year since 2000, month, day, weekday, hour, minute, second.
    pub components: [u8; 7],
    /// The control register as read. Bit 6 is 24-hour mode.
    pub control: u8,
}

impl Extension for RtcS3511a {
    const KEY: ReverseDnsName = ReverseDnsName::from_static("x.1sav.rtc.s3511a");
}

impl From<RtcS3511a> for CBOR {
    fn from(clock: RtcS3511a) -> Self {
        let mut map = Map::new();
        map.insert(0u64, CBOR::to_byte_string(clock.components));
        map.insert(1u64, u64::from(clock.control));
        map.into()
    }
}

impl TryFrom<CBOR> for RtcS3511a {
    type Error = dcbor::Error;

    fn try_from(value: CBOR) -> dcbor::Result<Self> {
        let map = want_map(&value)?;
        only_keys(map, &[0, 1])?;
        let components = map.get::<u64, CBOR>(0).ok_or(dcbor::Error::MissingMapKey)?;
        let components = components.as_byte_string().ok_or(dcbor::Error::WrongType)?;
        Ok(RtcS3511a {
            components: components.try_into().map_err(|_| dcbor::Error::OutOfRange)?,
            control: want_uint(&map.get::<u64, CBOR>(1).ok_or(dcbor::Error::MissingMapKey)?)?,
        })
    }
}

impl fmt::Display for RtcS3511a {
    /// The components as the chip holds them, BCD read as the digits it spells. They are a wall
    /// clock with no zone, so nothing here turns them into an instant.
    ///
    /// Components that are not BCD spell no digits, so they are shown as the bytes they are rather
    /// than as a date nobody wrote.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [year, month, day, weekday, hour, minute, second] = self.components;
        // Bit 7 of the hour byte is the chip's AM/PM flag, which is not a digit.
        let hour = hour & 0x3f;
        let mode = if self.control & 0x40 != 0 { "24-hour" } else { "12-hour" };
        let bcd = |byte: u8| byte >> 4 <= 9 && byte & 0x0f <= 9;
        if [year, month, day, weekday, hour, minute, second].into_iter().all(bcd) {
            write!(f, "20{year:02x}-{month:02x}-{day:02x} {hour:02x}:{minute:02x}:{second:02x}, {mode}")
        } else {
            f.write_str("not BCD: h'")?;
            self.components.iter().try_for_each(|byte| write!(f, "{byte:02x}"))?;
            write!(f, "', {mode}")
        }
    }
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
    fn an_mbc3_clock_round_trips() {
        let clock = RtcMbc3 {
            live: [30, 45, 13, 44, 1],
            latched: [30, 45, 13, 44, 1],
            written_at: 1_700_000_000,
            timestamp_width: 8,
        };
        assert_eq!(RtcMbc3::try_from(CBOR::from(clock)).unwrap(), clock);
        assert_eq!(clock.days(), 300);
        assert_eq!(clock.to_string(), "day 300 13:45:30, written 2023-11-14 22:13:20 UTC");
        // The width is 4 or 8 and nothing else, since the footer is rebuilt from it.
        assert!(RtcMbc3::try_from(CBOR::from(RtcMbc3 { timestamp_width: 6, ..clock })).is_err());
    }

    #[test]
    fn a_seiko_clock_shows_its_digits() {
        let clock = RtcS3511a { components: [0x26, 0x08, 0x12, 0x03, 0x10, 0x39, 0x43], control: 0x40 };
        assert_eq!(RtcS3511a::try_from(CBOR::from(clock)).unwrap(), clock);
        assert_eq!(clock.to_string(), "2026-08-12 10:39:43, 24-hour");
        let garbled = RtcS3511a { components: [0x01, 0x20, 0x3f, 0x00, 0x3d, 0x9c, 0xbb], control: 0 };
        assert_eq!(garbled.to_string(), "not BCD: h'01203f003d9cbb', 12-hour");
    }
}
