//! [`x.1sav.rtc.tpp1`](https://docs.1retro.com/specifications/extensions/x.1sav.rtc.tpp1/).

use std::fmt;

use dcbor::{CBOR, Map};

use super::{Rtc, elapsed, field, nullable, optional_epoch, or_null, want_bytes, write_taken};
use crate::extension::{Extension, epoch, only_keys, want_map, want_uint};
use crate::name::ReverseDnsName;

/// The clock bits of MR4: bit 2 set while the clock runs, bit 3 once the week counter has
/// overflowed. Bits 0 and 1 are the rumble speed, which is not the clock's.
const STATUS_BITS: u8 = 0x0c;

/// The clock of the TPP1 homebrew Game Boy mapper, and the moment a producer took it.
///
/// The mapper's specification defines the registers and no save format, so this is the
/// registers: RTCW, RTCDH, RTCM and RTCS, live and latched, and the clock's half of MR4.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtcTpp1 {
    /// The running registers: the week, 0 to 255; the weekday in bits 5 to 7 and the hour in bits
    /// 0 to 4; the minutes; the seconds.
    pub live: [u8; 4],
    /// The latched copy the game reads, in the same order, or `None` where the producer's source
    /// does not keep it, as SameBoy's save does not. See
    /// [`latched_or_live`](Self::latched_or_live).
    pub latched: Option<[u8; 4]>,
    /// MR4 with the rumble bits clear: 0, 4, 8 or 12.
    pub status: u8,
    /// The producer's clock when it took the registers, in epoch seconds: the moment it advances
    /// the clock from when the save is next loaded. On the producer's own scale.
    pub taken_at: Option<i64>,
}

impl RtcTpp1 {
    /// Whether the clock is running, which is bit 2 of MR4.
    #[must_use]
    pub fn running(&self) -> bool {
        self.status & 0x04 != 0
    }

    /// Whether the week counter has overflowed, which is bit 3 of MR4.
    #[must_use]
    pub fn overflowed(&self) -> bool {
        self.status & 0x08 != 0
    }

    /// The clock on the Unix scale: the elapsed time the running registers give, after
    /// 1970-01-01 UTC. Seven days for each week, then the weekday, the hours, the minutes and the
    /// seconds.
    #[must_use]
    pub fn reading(&self) -> Rtc {
        let [week, day_hour, minutes, seconds] = self.live;
        let days = i64::from(week) * 7 + i64::from(day_hour >> 5);
        Rtc::new(elapsed(days, (day_hour & 0x1f).into(), minutes.into(), seconds.into()))
    }

    /// The latched copy, or the live registers where the source did not keep one.
    ///
    /// A game latches the clock before it reads it, and the latch copies the running registers
    /// over the latched ones, so the live registers are what the latched copy would have held.
    #[must_use]
    pub fn latched_or_live(&self) -> [u8; 4] {
        self.latched.unwrap_or(self.live)
    }
}

impl Extension for RtcTpp1 {
    const KEY: ReverseDnsName = ReverseDnsName::from_static("x.1sav.rtc.tpp1");
}

impl From<RtcTpp1> for CBOR {
    fn from(clock: RtcTpp1) -> Self {
        let mut map = Map::new();
        map.insert(0u64, CBOR::to_byte_string(clock.live));
        map.insert(1u64, or_null(clock.latched, CBOR::to_byte_string));
        map.insert(2u64, u64::from(clock.status));
        if let Some(taken_at) = clock.taken_at {
            map.insert(3u64, epoch(taken_at));
        }
        map.into()
    }
}

impl TryFrom<CBOR> for RtcTpp1 {
    type Error = dcbor::Error;

    fn try_from(value: CBOR) -> dcbor::Result<Self> {
        let map = want_map(&value)?;
        only_keys(map, &[0, 1, 2, 3])?;
        let status: u8 = want_uint(&field(map, 2)?)?;
        // The rumble speed shares the register and is not clock state.
        if status & !STATUS_BITS != 0 {
            return Err(dcbor::Error::OutOfRange);
        }
        Ok(RtcTpp1 {
            live: want_bytes(&field(map, 0)?)?,
            latched: nullable(&field(map, 1)?, want_bytes)?,
            status,
            taken_at: optional_epoch(map, 3)?,
        })
    }
}

impl fmt::Display for RtcTpp1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [week, day_hour, minutes, seconds] = self.live;
        let (weekday, hour) = (day_hour >> 5, day_hour & 0x1f);
        write!(f, "week {week}, day {weekday} {hour:02}:{minutes:02}:{seconds:02}")?;
        if !self.running() {
            f.write_str(" (stopped)")?;
        }
        if self.overflowed() {
            f.write_str(", overflowed")?;
        }
        write_taken(f, self.taken_at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Week 2, Wednesday at 14:30:15, running: weekday 3 in bits 5 to 7 of RTCDH and hour 14 below.
    const CLOCK: RtcTpp1 = RtcTpp1 {
        live: [2, (3 << 5) | 0x0e, 30, 15],
        latched: Some([2, (3 << 5) | 0x0e, 30, 15]),
        status: 4,
        taken_at: Some(1_044_057_600),
    };

    #[test]
    fn a_tpp1_clock_round_trips_and_shows_its_counters() {
        assert_eq!(RtcTpp1::try_from(CBOR::from(CLOCK)).unwrap(), CLOCK);
        assert_eq!(CLOCK.to_string(), "week 2, day 3 14:30:15, taken 2003-02-01 00:00:00");
        let stopped = RtcTpp1 { live: [0; 4], latched: None, status: 8, taken_at: None };
        assert_eq!(RtcTpp1::try_from(CBOR::from(stopped)).unwrap(), stopped);
        assert_eq!(stopped.to_string(), "week 0, day 0 00:00:00 (stopped), overflowed");
        assert_eq!(stopped.latched_or_live(), [0; 4]);
    }

    #[test]
    fn the_reading_is_the_elapsed_time() {
        assert_eq!(CLOCK.reading(), Rtc::new((2 * 7 + 3) * 86_400 + 14 * 3_600 + 30 * 60 + 15));
    }

    #[test]
    fn the_rumble_bits_are_not_the_clocks() {
        assert!(RtcTpp1::try_from(CBOR::from(RtcTpp1 { status: 5, ..CLOCK })).is_err());
    }
}
