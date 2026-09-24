//! The SMPC's non-volatile state: the console's clock and four bytes of BIOS settings.
//!
//! The System Manager & Peripheral Control chip keeps a battery-backed area the Saturn's clock
//! runs in, alongside four bytes the BIOS reads its settings out of. It is not backup RAM and
//! holds no saves — it is the console's own state — but it sits in the same place a dump of the
//! backup RAM does, and dropping it loses the clock.
//!
//! Mednafen and its libretro fork write it as a twelve-byte file beside the save, `.smpc`. The
//! layout is that emulator's rather than the console's, and it is taken from the source rather
//! than inferred: `SMPC_SaveNV` in `mednafen/ss/smpc.c` writes three fields and says so.
//!
//! | Byte | Field | Meaning |
//! | ---- | ----- | ------- |
//! | 0 | `Valid` | Zero means the BIOS resynthesises the clock from host time on the next read. |
//! | 1 | `year[0]` | BCD century. |
//! | 2 | `year[1]` | BCD year within it. |
//! | 3 | `wday_mon` | Weekday in the high nibble, 0 to 6 with 6 for Saturday; month 1 to 12 in the low. |
//! | 4 | `mday` | BCD. |
//! | 5 | `hour` | BCD. |
//! | 6 | `minute` | BCD. |
//! | 7 | `second` | BCD. |
//! | 8–11 | `SaveMem` | The BIOS's four bytes, read through `OREG` and written through `IREG`. |
//!
//! The low nibble of `SaveMem[3]` is the console's language — `SaveMem[3] = (SaveMem[3] & 0xF0) |
//! lang` — which is the same setting a game copies into the language byte of a save's entry.
//!
//! # Kept verbatim
//!
//! Both runs of bytes are held as they were read, and the fields below decode them on demand.
//! That is deliberate: the clock is BCD with a weekday the calendar could contradict, and a
//! console is free to hold a value no decoder would produce. Writing back what was read is worth
//! more than writing back what a decode thought it meant.

use crate::{Error, Result};

/// One reading of the SMPC's non-volatile area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Smpc {
    /// Whether the console considers its clock set.
    ///
    /// False means the BIOS will invent a time from the host clock the next time it looks, so
    /// there is no reading here to record.
    pub valid: bool,
    /// The clock's own seven bytes, verbatim.
    pub clock: [u8; 7],
    /// The BIOS's four bytes, verbatim. [`language`](Self::language) is the part with a meaning.
    pub save_mem: [u8; 4],
}

impl Smpc {
    /// How long the file is.
    pub const LEN: usize = 12;

    /// Reads the twelve bytes.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != Smpc::LEN {
            return Err(Error::NotSmpc(bytes.len()));
        }
        let mut clock = [0u8; 7];
        clock.copy_from_slice(&bytes[1..8]);
        let mut save_mem = [0u8; 4];
        save_mem.copy_from_slice(&bytes[8..12]);
        Ok(Smpc { valid: bytes[0] != 0, clock, save_mem })
    }

    /// Writes the twelve bytes back.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; Smpc::LEN] {
        let mut out = [0u8; Smpc::LEN];
        out[0] = u8::from(self.valid);
        out[1..8].copy_from_slice(&self.clock);
        out[8..12].copy_from_slice(&self.save_mem);
        out
    }

    /// The year the clock holds, as a whole number.
    #[must_use]
    pub fn year(&self) -> u16 {
        u16::from(bcd(self.clock[0])) * 100 + u16::from(bcd(self.clock[1]))
    }

    /// The weekday, 0 for Sunday through 6 for Saturday.
    ///
    /// The console's own, which a consumer should prefer over deriving one: it is what the BIOS
    /// displays, and a console whose clock was set by hand can hold a weekday the calendar
    /// disagrees with.
    #[must_use]
    pub fn weekday(&self) -> u8 {
        self.clock[2] >> 4
    }

    /// The month, 1 through 12.
    #[must_use]
    pub fn month(&self) -> u8 {
        self.clock[2] & 0x0F
    }

    /// The day of the month.
    #[must_use]
    pub fn day(&self) -> u8 {
        bcd(self.clock[3])
    }

    /// The hour, on a 24-hour clock.
    #[must_use]
    pub fn hour(&self) -> u8 {
        bcd(self.clock[4])
    }

    /// The minute.
    #[must_use]
    pub fn minute(&self) -> u8 {
        bcd(self.clock[5])
    }

    /// The second.
    #[must_use]
    pub fn second(&self) -> u8 {
        bcd(self.clock[6])
    }

    /// The console's language setting, out of the low nibble of `SaveMem[3]`.
    ///
    /// The same value a game copies into the language byte of a save's entry, so this is what a
    /// European release reads to decide which language to run in.
    ///
    /// | Value | Language |
    /// | ----- | -------- |
    /// | 0 | English |
    /// | 1 | German |
    /// | 2 | French |
    /// | 3 | Spanish |
    /// | 4 | Italian |
    /// | 5 | Japanese |
    ///
    /// Not the order the field is usually described in. English is zero, not Japanese, so a
    /// console reading zero is an English one rather than an unset one.
    #[must_use]
    pub fn language(&self) -> u8 {
        self.save_mem[3] & 0x0F
    }

    /// What the clock reads, in whole epoch seconds, or `None` when there is no reading.
    ///
    /// `None` for a clock the console does not consider set, and for one holding a date no
    /// calendar has — a field a console never wrote is usually zero, and month zero is the tell.
    ///
    /// Read as the console showed it: the Saturn keeps a wall clock with no notion of a zone, so
    /// this fixes a date and a time of day and says nothing about where on Earth that was.
    #[must_use]
    pub fn read_at(&self) -> Option<i64> {
        if !self.valid {
            return None;
        }
        let (month, day) = (self.month(), self.day());
        let (hour, minute, second) = (self.hour(), self.minute(), self.second());
        if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            return None;
        }
        if hour > 23 || minute > 59 || second > 59 {
            return None;
        }
        Some(
            days_from_civil(i64::from(self.year()), u32::from(month), u32::from(day)) * 86_400
                + i64::from(hour) * 3_600
                + i64::from(minute) * 60
                + i64::from(second),
        )
    }
}

/// One packed BCD byte.
///
/// Lenient about a nibble past nine, which the console can hold and a decoder should not lose the
/// rest of the reading over. The bytes are kept verbatim regardless.
const fn bcd(byte: u8) -> u8 {
    (byte >> 4) * 10 + (byte & 0x0F)
}

/// Days from 1970-01-01 to a civil date, by Howard Hinnant's algorithm.
///
/// Spelled out rather than taken from a date crate: this is the only arithmetic of its kind here,
/// and a crate with no dependencies is not worth giving one for it.
const fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = year - if month <= 2 { 1 } else { 0 };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month as i64 + if month > 2 { -3 } else { 9 }) + 2) / 5 + day as i64 - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Panzer Dragoon Saga's, as Beetle Saturn wrote it.
    const PDS: [u8; 12] = [0x01, 0x20, 0x26, 0x29, 0x22, 0x17, 0x56, 0x00, 0, 0, 0, 0];

    #[test]
    fn a_real_file_decodes_to_the_instant_it_was_written() {
        let smpc = Smpc::parse(&PDS).expect("reads");
        assert!(smpc.valid);
        assert_eq!((smpc.year(), smpc.month(), smpc.day()), (2026, 9, 22));
        assert_eq!((smpc.hour(), smpc.minute(), smpc.second()), (17, 56, 0));
        assert_eq!(smpc.weekday(), 2, "Tuesday, and 2026-09-22 was one");
        assert_eq!(smpc.language(), 0);
        // 2026-09-22 17:56:00, read as the console showed it.
        assert_eq!(smpc.read_at(), Some(1_790_099_760));
    }

    #[test]
    fn the_bytes_come_back_as_they_went_in() {
        assert_eq!(Smpc::parse(&PDS).expect("reads").to_bytes(), PDS);
    }

    #[test]
    fn a_clock_the_console_does_not_trust_has_no_reading() {
        let mut bytes = PDS;
        bytes[0] = 0;
        let smpc = Smpc::parse(&bytes).expect("reads");
        assert!(!smpc.valid);
        assert_eq!(smpc.read_at(), None, "the BIOS will invent one from host time");
        // The fields are still there, and still come back byte for byte.
        assert_eq!(smpc.year(), 2026);
        assert_eq!(smpc.to_bytes(), bytes);
    }

    #[test]
    fn a_date_no_calendar_has_is_absence_rather_than_a_guess() {
        // What an area a console never wrote looks like: valid set, everything else zero.
        let zeroed = Smpc::parse(&[1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]).expect("reads");
        assert_eq!(zeroed.read_at(), None, "month zero is the tell");
        // And the factory default `SMPC_SetRTC` writes for a console with no host time.
        let factory = Smpc::parse(&[0, 0x19, 0x93, 0x5C, 0x31, 0x23, 0x59, 0x59, 0, 0, 0, 0]);
        let factory = factory.expect("reads");
        assert_eq!((factory.year(), factory.month(), factory.day()), (1993, 12, 31));
        assert_eq!(factory.weekday(), 5, "Friday, which 1993-12-31 was");
        assert_eq!(factory.read_at(), None, "and not a clock the console considers set");
    }

    #[test]
    fn the_language_is_the_low_nibble_and_nothing_else() {
        let mut bytes = PDS;
        bytes[11] = 0xA1;
        assert_eq!(Smpc::parse(&bytes).expect("reads").language(), 1);
        assert_eq!(Smpc::parse(&bytes).expect("reads").to_bytes()[11], 0xA1, "the rest is kept");
    }

    #[test]
    fn anything_but_twelve_bytes_is_refused() {
        assert!(matches!(Smpc::parse(&[0u8; 11]), Err(Error::NotSmpc(11))));
        assert!(matches!(Smpc::parse(&[0u8; 13]), Err(Error::NotSmpc(13))));
    }
}
