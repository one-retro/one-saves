//! [`x.1sav.rtc.tc8521`](https://docs.1retro.com/specifications/extensions/x.1sav.rtc.tc8521/).

use std::fmt;

use dcbor::{CBOR, Map};

use super::{
    Rtc, array, digits, field, hour_from_12, nullable, nybble_or_null, optional_epoch, utc, want_array,
    want_nybble, write_taken,
};
use crate::extension::{Extension, epoch, only_keys, want_map};
use crate::name::ReverseDnsName;

/// The mode a rebuild uses where the key has none: the clock running, block 0 selected, and the
/// alarm off.
const DEFAULT_MODE: u8 = 8;

/// Register A of block 1, which is 1 in 24-hour mode and 0 in 12-hour mode.
const HOUR24: usize = 0xa;
/// Register B of block 1, which counts years since a leap year.
const LEAP_YEAR: usize = 0xb;

/// The registers of the Toshiba TC8521 in the TAMA5 Tamagotchi cartridge, and the moment a
/// producer took them.
///
/// The chip has the registers of the Ricoh RP5C01: four blocks of thirteen, four bits each, and a
/// mode register common to them all. Emulators keep anything from the time alone to every block,
/// so a register is `None` where the producer's source does not keep it. mGBA keeps every block,
/// while ares keeps block 0 without the weekday, the two settings, and nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtcTc8521 {
    /// The four blocks. Block 0 is the time in BCD: the seconds, minutes and hours, each ones then
    /// tens, the weekday, then the day, month and year, each ones then tens. Block 1 is the alarm
    /// and the settings, and blocks 2 and 3 are RAM.
    pub blocks: [[Option<u8>; 13]; 4],
    /// The mode register, D: the block a game sees in bits 0 and 1, the alarm enable in bit 2, and
    /// bit 3 set while the clock runs. `None` where the source does not keep it.
    pub mode: Option<u8>,
    /// The producer's clock when it took the registers, in epoch seconds: the moment it advances
    /// the clock from when the save is next loaded. On the producer's own scale.
    pub taken_at: Option<i64>,
}

impl RtcTc8521 {
    /// Whether the chip is in 24-hour mode, which register A of block 1 says. Where the source did
    /// not keep it, the chip is read the way a rebuild starts it: in 24-hour mode.
    #[must_use]
    pub fn hour24(&self) -> bool {
        self.blocks[1][HOUR24].unwrap_or(1) & 1 != 0
    }

    /// The chip's clock on the Unix scale: block 0 read as though it were UTC.
    ///
    /// The year is two digits with no century, so the caller supplies one: `century` is the year
    /// the digits count from, 2000 reading 26 as 2026. In 12-hour mode, bit 1 of the hours tens
    /// is what says it is after noon.
    ///
    /// `None` when a register the date or the time needs was not kept, or when block 0 is not a
    /// date. A reading that cannot be computed is omitted.
    #[must_use]
    pub fn reading(&self, century: i64) -> Option<Rtc> {
        let time = &self.blocks[0];
        let register = |index: usize| time[index];
        let hour_tens = register(5)?;
        let hour = if self.hour24() {
            digits(hour_tens, register(4)?)?
        } else {
            hour_from_12(digits(hour_tens & 0x01, register(4)?)?, hour_tens & 0x02 != 0)?
        };
        let seconds = utc(
            century + i64::from(digits(register(0xc)?, register(0xb)?)?),
            digits(register(0xa)?, register(9)?)?,
            digits(register(8)?, register(7)?)?,
            hour,
            digits(register(3)?, register(2)?)?,
            digits(register(1)?, register(0)?)?,
        )?;
        Some(Rtc::new(seconds))
    }

    /// The blocks and the mode a rebuild puts back, with a value for every register the source
    /// did not keep.
    ///
    /// The mode runs the clock on block 0 with the alarm off. In block 1, register A is 24-hour
    /// mode and register B is block 0's year modulo 4, as MAME starts the chip, or 0 where the
    /// year was not kept either. Every other register is 0, which is where MAME starts the alarm
    /// and the RAM, and that includes the weekday: the chip counts it from whatever the game set,
    /// so the date does not give it.
    #[must_use]
    pub fn filled(&self) -> ([[u8; 13]; 4], u8) {
        let mut blocks = self.blocks.map(|block| block.map(|register| register.unwrap_or(0)));
        let time = &self.blocks[0];
        if self.blocks[1][HOUR24].is_none() {
            blocks[1][HOUR24] = 1;
        }
        if self.blocks[1][LEAP_YEAR].is_none() {
            let year = time[0xc].zip(time[0xb]).map_or(0, |(tens, ones)| tens * 10 + ones);
            blocks[1][LEAP_YEAR] = year % 4;
        }
        (blocks, self.mode.unwrap_or(DEFAULT_MODE))
    }
}

impl Extension for RtcTc8521 {
    const KEY: ReverseDnsName = ReverseDnsName::from_static("x.1sav.rtc.tc8521");
}

impl From<RtcTc8521> for CBOR {
    fn from(clock: RtcTc8521) -> Self {
        let mut map = Map::new();
        map.insert(0u64, array(&clock.blocks, |block| array(&block, nybble_or_null)));
        map.insert(1u64, nybble_or_null(clock.mode));
        if let Some(taken_at) = clock.taken_at {
            map.insert(2u64, epoch(taken_at));
        }
        map.into()
    }
}

impl TryFrom<CBOR> for RtcTc8521 {
    type Error = dcbor::Error;

    fn try_from(value: CBOR) -> dcbor::Result<Self> {
        let map = want_map(&value)?;
        only_keys(map, &[0, 1, 2])?;
        // A block is never null as a whole: a source that keeps none of one says so register by
        // register.
        let block = |value: &CBOR| want_array(value, |register| nullable(register, want_nybble));
        Ok(RtcTc8521 {
            blocks: want_array(&field(map, 0)?, block)?,
            mode: nullable(&field(map, 1)?, want_nybble)?,
            taken_at: optional_epoch(map, 2)?,
        })
    }
}

impl fmt::Display for RtcTc8521 {
    /// Block 0's digits in the order a date is written, the year's two and no century, with a `?`
    /// for each register the source did not keep.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let time = &self.blocks[0];
        let hour24 = self.hour24();
        let digit = |index: usize| {
            // In 12-hour mode the hours tens carries PM beside its one digit.
            let mask = if index == 5 && !hour24 { 0x01 } else { 0x0f };
            time[index].map_or('?', |nybble| char::from_digit(u32::from(nybble & mask), 16).unwrap_or('?'))
        };
        // Tens then ones: the year, the month, the day, the hour, the minute, the second.
        let shown = [0xc, 0xb, 0xa, 9, 8, 7, 5, 4, 3, 2, 1, 0].map(digit);
        let [year, month, day, hour, minute, second] =
            [0, 2, 4, 6, 8, 10].map(|at| String::from_iter(&shown[at..at + 2]));
        write!(f, "{year}-{month}-{day} {hour}:{minute}:{second}")?;
        if !hour24 {
            match time[5] {
                Some(tens) => f.write_str(if tens & 0x02 != 0 { " PM" } else { " AM" })?,
                None => f.write_str(" ?M")?,
            }
        }
        f.write_str(if hour24 { ", 24-hour" } else { ", 12-hour" })?;
        if self.mode.is_some_and(|mode| mode & 0x08 == 0) {
            f.write_str(", stopped")?;
        }
        write_taken(f, self.taken_at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// As ares keeps it: 2026-10-22 14:37:05 without its weekday, the 24-hour select and the
    /// leap-year counter, and nothing of the alarm, the RAM or the mode.
    fn ares() -> RtcTc8521 {
        let mut blocks = [[None; 13]; 4];
        blocks[0] = [5, 0, 7, 3, 4, 1, 0, 2, 2, 0, 1, 6, 2].map(Some);
        blocks[0][6] = None;
        blocks[1][HOUR24] = Some(1);
        blocks[1][LEAP_YEAR] = Some(2);
        RtcTc8521 { blocks, mode: None, taken_at: Some(1_044_057_600) }
    }

    #[test]
    fn a_toshiba_clock_round_trips_with_the_registers_it_lacks() {
        let clock = ares();
        assert_eq!(RtcTc8521::try_from(CBOR::from(clock)).unwrap(), clock);
        assert_eq!(clock.to_string(), "26-10-22 14:37:05, 24-hour, taken 2003-02-01 00:00:00");
    }

    #[test]
    fn the_reading_takes_a_century_and_the_meridian() {
        let clock = ares();
        assert_eq!(clock.reading(2000), Some(Rtc::new(1_792_679_825)));
        // 2:37 PM in 12-hour mode: bit 1 of the hours tens, and a tens digit of 0.
        let mut twelve = clock;
        twelve.blocks[1][HOUR24] = Some(0);
        twelve.blocks[0][4] = Some(2);
        twelve.blocks[0][5] = Some(0b10);
        assert_eq!(twelve.reading(2000), clock.reading(2000));
        assert_eq!(twelve.to_string(), "26-10-22 02:37:05 PM, 12-hour, taken 2003-02-01 00:00:00");
        // A register the date needs that was not kept leaves nothing to read.
        let mut lost = clock;
        lost.blocks[0][0xb] = None;
        assert_eq!(lost.reading(2000), None);
        assert_eq!(lost.to_string(), "2?-10-22 14:37:05, 24-hour, taken 2003-02-01 00:00:00");
    }

    #[test]
    fn a_rebuild_fills_each_register_the_way_the_specification_says() {
        let mut clock = ares();
        clock.blocks[1] = [None; 13];
        let (blocks, mode) = clock.filled();
        assert_eq!(mode, 8, "running, block 0, alarm off");
        assert_eq!(blocks[1][HOUR24], 1, "24-hour mode");
        assert_eq!(blocks[1][LEAP_YEAR], 26 % 4, "the year since a leap year");
        assert_eq!(blocks[0][6], 0, "a weekday the date does not give");
        assert_eq!(blocks[2], [0; 13], "and the RAM");
        clock.blocks[0][0xc] = None;
        assert_eq!(clock.filled().0[1][LEAP_YEAR], 0, "with no year, no count since a leap one");
    }
}
