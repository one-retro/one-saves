//! [`x.1sav.rtc.huc3`](https://docs.1retro.com/specifications/extensions/x.1sav.rtc.huc3/).

use std::fmt;

use dcbor::{CBOR, Map};

use super::{
    Rtc, array, elapsed, field, nullable, nybble_or_null, optional_epoch, want_array, want_nybble,
    write_taken,
};
use crate::extension::{Extension, epoch, only_keys, want_map};
use crate::name::ReverseDnsName;

/// Where the minute of the day starts: three nybbles, least significant first.
const MINUTES: usize = 0x10;
/// Where the day counter starts: three nybbles, least significant first.
const DAYS: usize = 0x13;

/// The memory of the clock controller in a Hudson HuC-3 Game Boy cartridge, and the moment a
/// producer took it.
///
/// A game reads and writes the controller a nybble at a time, through a window of 256 addresses.
/// Games keep data of their own beside the clock's, so every address is carried, and one is
/// `None` where the producer's source does not keep it: mGBA keeps all 256, SameBoy the two
/// counters and the event time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcHuc3 {
    /// The 256 nybbles of the window, element _n_ at address _n_.
    pub memory: [Option<u8>; 256],
    /// The producer's clock when it took the memory, in epoch seconds: the moment it advances the
    /// counters from when the save is next loaded. On the producer's own scale.
    pub taken_at: Option<i64>,
}

impl RtcHuc3 {
    /// A value spanning `len` addresses from `at`, least significant nybble first, when the
    /// source kept every one of them.
    fn value(&self, at: usize, len: usize) -> Option<u16> {
        self.memory[at..at + len]
            .iter()
            .rev()
            .try_fold(0, |value, &nybble| Some(value << 4 | u16::from(nybble?)))
    }

    /// The minute of the day, 0 to 1439, at addresses 0x10 to 0x12.
    #[must_use]
    pub fn minutes(&self) -> Option<u16> {
        self.value(MINUTES, 3)
    }

    /// The day counter, at addresses 0x13 to 0x15.
    #[must_use]
    pub fn days(&self) -> Option<u16> {
        self.value(DAYS, 3)
    }

    /// The clock on the Unix scale: the day counter's days and the minute counter's minutes after
    /// 1970-01-01 UTC. The HuC-3 counts no seconds.
    ///
    /// `None` when the source did not keep a counter, since a reading that cannot be computed is
    /// omitted.
    #[must_use]
    pub fn reading(&self) -> Option<Rtc> {
        Some(Rtc::new(elapsed(self.days()?.into(), 0, self.minutes()?.into(), 0)))
    }

    /// The memory a rebuild puts back: 0 at every address the source did not keep, as emulators
    /// start it. A game's own data at such an address stays lost, as it already was.
    #[must_use]
    pub fn filled(&self) -> [u8; 256] {
        self.memory.map(|nybble| nybble.unwrap_or(0))
    }
}

impl Extension for RtcHuc3 {
    const KEY: ReverseDnsName = ReverseDnsName::from_static("x.1sav.rtc.huc3");
}

impl From<RtcHuc3> for CBOR {
    fn from(clock: RtcHuc3) -> Self {
        let mut map = Map::new();
        map.insert(0u64, array(&clock.memory, nybble_or_null));
        if let Some(taken_at) = clock.taken_at {
            map.insert(1u64, epoch(taken_at));
        }
        map.into()
    }
}

impl TryFrom<CBOR> for RtcHuc3 {
    type Error = dcbor::Error;

    fn try_from(value: CBOR) -> dcbor::Result<Self> {
        let map = want_map(&value)?;
        only_keys(map, &[0, 1])?;
        Ok(RtcHuc3 {
            memory: want_array(&field(map, 0)?, |item| nullable(item, want_nybble))?,
            taken_at: optional_epoch(map, 1)?,
        })
    }
}

impl fmt::Display for RtcHuc3 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.days(), self.minutes()) {
            (Some(days), Some(minutes)) => write!(f, "day {days} {:02}:{:02}", minutes / 60, minutes % 60)?,
            _ => f.write_str("counters not kept")?,
        }
        let kept = self.memory.iter().flatten().count();
        if kept < self.memory.len() {
            write!(f, ", {kept} of 256 addresses kept")?;
        }
        write_taken(f, self.taken_at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// As SameBoy keeps it: the counters and the event time, and nothing else.
    fn counters(minutes: u16, days: u16) -> RtcHuc3 {
        let mut memory = [None; 256];
        for index in 0..3 {
            memory[MINUTES + index] = Some(u8::try_from(minutes >> (4 * index) & 0xf).unwrap());
            memory[DAYS + index] = Some(u8::try_from(days >> (4 * index) & 0xf).unwrap());
        }
        for slot in &mut memory[0x58..0x5e] {
            *slot = Some(0);
        }
        RtcHuc3 { memory, taken_at: Some(1_700_000_000) }
    }

    #[test]
    fn a_huc3_clock_round_trips_with_the_addresses_it_lacks() {
        let clock = counters(13 * 60 + 45, 300);
        assert_eq!(RtcHuc3::try_from(CBOR::from(clock.clone())).unwrap(), clock);
        assert_eq!(clock.to_string(), "day 300 13:45, 12 of 256 addresses kept, taken 2023-11-14 22:13:20");
        assert_eq!(clock.filled()[MINUTES..MINUTES + 3], [9, 3, 3]);
        assert_eq!(clock.filled()[0], 0);
    }

    #[test]
    fn the_reading_is_the_counters_in_minutes() {
        let clock = counters(13 * 60 + 45, 300);
        assert_eq!(clock.reading(), Some(Rtc::new(300 * 86_400 + (13 * 60 + 45) * 60)));
        let mut lost = clock;
        lost.memory[DAYS + 2] = None;
        assert_eq!(lost.reading(), None, "a counter the source did not keep reads as nothing");
        assert_eq!(
            lost.to_string(),
            "counters not kept, 11 of 256 addresses kept, taken 2023-11-14 22:13:20"
        );
    }
}
