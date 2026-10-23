//! [`x.1sav.rtc.huc3`](https://docs.1retro.com/specifications/extensions/x.1sav.rtc.huc3/).

use std::fmt;

use dcbor::{CBOR, Map};

use super::{
    Rtc, array, elapsed, field, nullable, nybble_or_null, optional_epoch, or_null, want_array, want_nybble,
    write_taken,
};
use crate::extension::{Extension, epoch, only_keys, want_map, want_uint};
use crate::name::ReverseDnsName;

/// Where the minute of the day starts: three nybbles, least significant first.
const MINUTES: usize = 0x10;
/// Where the day counter starts: four nybbles, least significant first.
const DAYS: usize = 0x13;

/// The memory of the clock controller in a Hudson HuC-3 Game Boy cartridge, the seconds it has
/// counted into its minute, and the moment a producer took them.
///
/// A game reads and writes the controller a nybble at a time, through a window of 256 addresses.
/// Games keep data of their own beside the clock's, so every address is carried, and one is
/// `None` where the producer's source does not keep it: mGBA keeps all 256, SameBoy the two
/// counters and the event time, MiSTer the two counters alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcHuc3 {
    /// The 256 nybbles of the window, element _n_ at address _n_.
    pub memory: [Option<u8>; 256],
    /// The producer's clock when it took the memory, in epoch seconds: the moment it advances the
    /// counters from when the save is next loaded. On the producer's own scale.
    pub taken_at: Option<i64>,
    /// How many seconds the clock had counted into the current minute, 0 to 59, or `None` where
    /// the producer's source does not keep them.
    ///
    /// No address holds them and a game cannot read them, but they decide when the minute counter
    /// next moves. MiSTer keeps them; mGBA's are 0, since it moves its anchor a minute at a time.
    /// A rebuild takes 0 for `None`, so the minute begins at the anchor.
    pub seconds: Option<u8>,
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

    /// The day counter, at addresses 0x13 to 0x16.
    #[must_use]
    pub fn days(&self) -> Option<u16> {
        self.value(DAYS, 4)
    }

    /// The clock on the Unix scale: the day counter's days and the minute counter's minutes after
    /// 1970-01-01 UTC. The seconds are left out, since a game cannot read them.
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
        // The clock always counts seconds, so a producer that did not get them says so with null
        // rather than by leaving the key out.
        map.insert(2u64, or_null(clock.seconds, CBOR::from));
        map.into()
    }
}

impl TryFrom<CBOR> for RtcHuc3 {
    type Error = dcbor::Error;

    fn try_from(value: CBOR) -> dcbor::Result<Self> {
        let map = want_map(&value)?;
        only_keys(map, &[0, 1, 2])?;
        Ok(RtcHuc3 {
            memory: want_array(&field(map, 0)?, |item| nullable(item, want_nybble))?,
            taken_at: optional_epoch(map, 1)?,
            seconds: nullable(&field(map, 2)?, want_second)?,
        })
    }
}

/// Seconds into a minute: at 60 the minute counter has already moved on.
fn want_second(value: &CBOR) -> dcbor::Result<u8> {
    let second: u8 = want_uint(value)?;
    if second > 59 {
        return Err(dcbor::Error::OutOfRange);
    }
    Ok(second)
}

impl fmt::Display for RtcHuc3 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.days(), self.minutes()) {
            (Some(days), Some(minutes)) => write!(f, "day {days} {:02}:{:02}", minutes / 60, minutes % 60)?,
            _ => f.write_str("counters not kept")?,
        }
        if let Some(seconds) = self.seconds {
            write!(f, ", {seconds}s into the minute")?;
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
        }
        for index in 0..4 {
            memory[DAYS + index] = Some(u8::try_from(days >> (4 * index) & 0xf).unwrap());
        }
        for slot in &mut memory[0x58..0x5f] {
            *slot = Some(0);
        }
        RtcHuc3 { memory, taken_at: Some(1_700_000_000), seconds: Some(20) }
    }

    #[test]
    fn a_huc3_clock_round_trips_with_the_addresses_it_lacks() {
        let clock = counters(13 * 60 + 45, 300);
        assert_eq!(RtcHuc3::try_from(CBOR::from(clock.clone())).unwrap(), clock);
        assert_eq!(
            clock.to_string(),
            "day 300 13:45, 20s into the minute, 14 of 256 addresses kept, taken 2023-11-14 22:13:20"
        );
        assert_eq!(clock.filled()[MINUTES..MINUTES + 3], [9, 3, 3]);
        assert_eq!(clock.filled()[0], 0);
    }

    #[test]
    fn the_reading_is_the_counters_in_minutes() {
        let clock = counters(13 * 60 + 45, 300);
        assert_eq!(clock.reading(), Some(Rtc::new(300 * 86_400 + (13 * 60 + 45) * 60)), "no seconds");
        // The fourth nybble of days is the reading's too: MiSTer and SameBoy count sixteen bits.
        let long = counters(0, 0xA000);
        assert_eq!(long.days(), Some(0xA000));
        assert_eq!(long.reading(), Some(Rtc::new(0xA000 * 86_400)));
        let mut lost = clock;
        lost.memory[DAYS + 3] = None;
        lost.seconds = None;
        assert_eq!(lost.reading(), None, "a counter the source did not keep reads as nothing");
        assert_eq!(
            lost.to_string(),
            "counters not kept, 13 of 256 addresses kept, taken 2023-11-14 22:13:20"
        );
    }

    #[test]
    fn the_seconds_are_written_even_when_unknown() {
        let unknown = RtcHuc3 { seconds: None, ..counters(0, 0) };
        let map = CBOR::from(unknown.clone());
        assert!(want_map(&map).unwrap().get::<u64, CBOR>(2).is_some_and(|value| value.is_null()));
        assert_eq!(RtcHuc3::try_from(map).unwrap(), unknown);
    }
}
