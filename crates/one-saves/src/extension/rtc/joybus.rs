//! [`x.1sav.rtc.joybus`](https://docs.1retro.com/specifications/extensions/x.1sav.rtc.joybus/).

use std::fmt;

use dcbor::{CBOR, Map};

use super::{Rtc, field, from_bcd, optional_epoch, utc, want_bytes, write_taken};
use crate::extension::{Extension, epoch, only_keys, want_map};
use crate::name::ReverseDnsName;

/// The clock an N64 cartridge answers on the Joybus, as Doubutsu no Mori's does, and the moment a
/// producer took it.
///
/// The console reads and writes it in blocks of eight bytes. Block 3 always reads zero and is not
/// carried. ares is the one emulator that writes this clock to a file, and the three blocks and
/// the anchor are everything that file holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtcJoybus {
    /// Block 0. Bit 0 of byte 0 write-protects block 1 and bit 1 block 2; bits 1 and 2 of byte 1
    /// stop the clock. Bytes 4 and 5 hold values with no known effect, kept as read.
    pub control: [u8; 8],
    /// Block 1: battery-backed RAM, free for the game. Doubutsu no Mori does not use it.
    pub ram: [u8; 8],
    /// Block 2: the time, one packed BCD byte each. Seconds, minutes, hours with bit 7 set, day of
    /// the month, weekday, month, year, and the century counted from 1900.
    pub time: [u8; 8],
    /// The producer's clock when it took the blocks, in epoch seconds: the moment it advances the
    /// clock from when the save is next loaded. On the producer's own scale.
    pub taken_at: Option<i64>,
}

impl RtcJoybus {
    /// Whether the clock is stopped, which bits 1 and 2 of control byte 1 say.
    #[must_use]
    pub fn stopped(&self) -> bool {
        self.control[1] & 0x06 != 0
    }

    /// The clock on the Unix scale: block 2 read as though it were UTC, with the whole year 1900,
    /// plus 100 times the century, plus the two year digits.
    ///
    /// `None` when the time is not BCD or not a date. The clock has no 12-hour mode.
    #[must_use]
    pub fn reading(&self) -> Option<Rtc> {
        let [second, minute, hour, day, _weekday, month, year, century] = self.time;
        let year = 1900 + 100 * i64::from(from_bcd(century)?) + i64::from(from_bcd(year)?);
        let hour = from_bcd(hour & 0x7f)?;
        let seconds =
            utc(year, from_bcd(month)?, from_bcd(day)?, hour, from_bcd(minute)?, from_bcd(second)?)?;
        Some(Rtc::new(seconds))
    }
}

impl Extension for RtcJoybus {
    const KEY: ReverseDnsName = ReverseDnsName::from_static("x.1sav.rtc.joybus");
}

impl From<RtcJoybus> for CBOR {
    fn from(clock: RtcJoybus) -> Self {
        let mut map = Map::new();
        map.insert(0u64, CBOR::to_byte_string(clock.control));
        map.insert(1u64, CBOR::to_byte_string(clock.ram));
        map.insert(2u64, CBOR::to_byte_string(clock.time));
        if let Some(taken_at) = clock.taken_at {
            map.insert(3u64, epoch(taken_at));
        }
        map.into()
    }
}

impl TryFrom<CBOR> for RtcJoybus {
    type Error = dcbor::Error;

    fn try_from(value: CBOR) -> dcbor::Result<Self> {
        let map = want_map(&value)?;
        only_keys(map, &[0, 1, 2, 3])?;
        Ok(RtcJoybus {
            control: want_bytes(&field(map, 0)?)?,
            ram: want_bytes(&field(map, 1)?)?,
            time: want_bytes(&field(map, 2)?)?,
            taken_at: optional_epoch(map, 3)?,
        })
    }
}

impl fmt::Display for RtcJoybus {
    /// Block 2 as the digits it spells, or as its bytes when it spells none.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [second, minute, hour, day, _weekday, month, year, century] = self.time;
        let hour = hour & 0x7f;
        let bcd = |byte: u8| byte >> 4 <= 9 && byte & 0x0f <= 9;
        if [second, minute, hour, day, month, year, century].into_iter().all(bcd) {
            let century = 19 + u16::from(century >> 4) * 10 + u16::from(century & 0x0f);
            write!(f, "{century}{year:02x}-{month:02x}-{day:02x} {hour:02x}:{minute:02x}:{second:02x}")?;
        } else {
            f.write_str("not BCD: h'")?;
            self.time.iter().try_for_each(|byte| write!(f, "{byte:02x}"))?;
            f.write_str("'")?;
        }
        if self.stopped() {
            f.write_str(" (stopped)")?;
        }
        write_taken(f, self.taken_at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The specification's example: 2001-04-14 12:34:56, a Saturday, both blocks write-protected.
    const CLOCK: RtcJoybus = RtcJoybus {
        control: [0x03, 0, 0, 0, 0, 0, 0, 0],
        ram: [0; 8],
        time: [0x56, 0x34, 0x92, 0x14, 0x06, 0x04, 0x01, 0x01],
        taken_at: Some(1_044_057_600),
    };

    #[test]
    fn a_joybus_clock_round_trips_and_shows_its_date() {
        assert_eq!(RtcJoybus::try_from(CBOR::from(CLOCK)).unwrap(), CLOCK);
        assert_eq!(CLOCK.to_string(), "2001-04-14 12:34:56, taken 2003-02-01 00:00:00");
        let stopped = RtcJoybus { control: [0x03, 0x04, 0, 0, 0, 0, 0, 0], taken_at: None, ..CLOCK };
        assert_eq!(stopped.to_string(), "2001-04-14 12:34:56 (stopped)");
    }

    #[test]
    fn the_reading_is_block_2_as_utc() {
        assert_eq!(CLOCK.reading(), Some(Rtc::new(987_251_696)));
        let twentieth = RtcJoybus { time: [0x56, 0x34, 0x92, 0x14, 0x06, 0x04, 0x99, 0x00], ..CLOCK };
        assert_eq!(twentieth.reading(), Some(Rtc::new(924_093_296)), "century 0 is 19xx");
        let garbled = RtcJoybus { time: [0x5a, 0x34, 0x92, 0x14, 0x06, 0x04, 0x01, 0x01], ..CLOCK };
        assert_eq!(garbled.reading(), None);
    }
}
