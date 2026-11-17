//! Clocks a console keeps rather than a cartridge, in the files emulators keep them in.
//!
//! A cartridge's clock rides on the cartridge's save, and [`rtc`](crate::rtc) reads those. A
//! console's own clock rides on a device's header instead, since every game on the console reads
//! it: a Saturn's SMPC, which [`card`](crate::card) reads with the backup RAM beside it, and the
//! two that ares keeps in a file of their own, the 64DD drive's and an MVS board's.
//!
//! # ares's `time.rtc`
//!
//! Sixteen bytes, the same for both chips: the chip's six bytes of time, two bytes ares leaves
//! alone, then the moment it saved them as a little-endian count of Unix seconds. ares writes the
//! file whenever it unloads the system, so the time and the anchor were taken together, and on the
//! next load it runs the clock on by however long the host was off. A file whose time or anchor
//! is all `0xFF`, or whose anchor is zero, is one ares has never saved, and it sets such a clock
//! from the host's: there is no reading in it.
//!
//! Where it is: `Systems/Nintendo 64DD.sys/time.rtc` and `Systems/Neo Geo MVS.sys/time.rtc` under
//! ares's folder, or with a saves folder set, `Nintendo 64DD/Nintendo 64DD.rtc` and
//! `Neo Geo MVS/Neo Geo MVS.rtc` in it.

use one_saves::extension::{Rtc, RtcN64dd, RtcUpd4990a};

/// How long ares's `time.rtc` is.
pub const ARES_RTC_LEN: usize = 16;

/// The name ares gives the 64DD drive's clock in a saves folder, beside the disks' saves.
pub const ARES_N64DD_RTC: &str = "Nintendo 64DD.rtc";

/// The six bytes of time and the anchor out of ares's file, or `None` for one that holds no clock
/// ares saved.
fn ares_parts(bytes: &[u8]) -> Option<([u8; 6], i64)> {
    let bytes: &[u8; ARES_RTC_LEN] = bytes.try_into().ok()?;
    let time: [u8; 6] = bytes[..6].try_into().expect("six bytes");
    let anchor = u64::from_le_bytes(bytes[8..].try_into().expect("eight bytes"));
    if bytes[..8].iter().all(|&byte| byte == 0xFF) || anchor == 0 || anchor == u64::MAX {
        return None;
    }
    Some((time, i64::try_from(anchor).ok()?))
}

/// ares's file for a chip's six bytes of time, taken at `taken_at`.
fn ares_file(time: [u8; 6], taken_at: i64) -> [u8; ARES_RTC_LEN] {
    let mut file = [0xFF; ARES_RTC_LEN];
    file[..6].copy_from_slice(&time);
    // A moment before 1970 is none ares could have saved at; zero is what it reads as never.
    file[8..].copy_from_slice(&u64::try_from(taken_at).unwrap_or(0).to_le_bytes());
    file
}

/// The 64DD drive's clock out of ares's `time.rtc`.
///
/// A clock ares has never saved holds none. One whose bytes are no time ares would keep, a minute
/// with its stopped bit set among them, ares throws away on its next load; it is kept here as read,
/// and the [`reading`](RtcN64dd::reading) it gives is none.
#[must_use]
pub fn parse_ares_n64dd(bytes: &[u8]) -> Option<RtcN64dd> {
    let (time, taken_at) = ares_parts(bytes)?;
    Some(RtcN64dd { time, taken_at: Some(taken_at) })
}

/// ares's `time.rtc` for the 64DD drive's clock, anchored where the clock was taken, or at
/// `now` when nothing recorded when.
#[must_use]
pub fn build_ares_n64dd(clock: &RtcN64dd, now: i64) -> [u8; ARES_RTC_LEN] {
    ares_file(clock.time, clock.taken_at.unwrap_or(now))
}

/// An MVS board's uPD4990A clock out of ares's `time.rtc`.
#[must_use]
pub fn parse_ares_upd4990a(bytes: &[u8]) -> Option<RtcUpd4990a> {
    let (time, taken_at) = ares_parts(bytes)?;
    Some(RtcUpd4990a { time, taken_at: Some(taken_at) })
}

/// ares's `time.rtc` for an MVS board's clock, anchored where the clock was taken, or at `now`
/// when nothing recorded when.
#[must_use]
pub fn build_ares_upd4990a(clock: &RtcUpd4990a, now: i64) -> [u8; ARES_RTC_LEN] {
    ares_file(clock.time, clock.taken_at.unwrap_or(now))
}

/// What an MVS board's clock reads, in the century that puts it nearest the moment it was taken,
/// or `now` when nothing recorded when.
///
/// The chip keeps two digits of the year and no century, so a producer supplies one, and a board
/// whose clock runs anywhere near true is in the century of the time it was read.
#[must_use]
pub fn upd4990a_reading(clock: &RtcUpd4990a, now: i64) -> Option<Rtc> {
    let near = clock.taken_at.unwrap_or(now);
    // The century of the moment, as the Gregorian calendar has it, give or take the year's end.
    let century = (1970 + near.div_euclid(31_556_952)).div_euclid(100) * 100;
    [century - 100, century, century + 100]
        .into_iter()
        .filter_map(|century| clock.reading(century))
        .min_by_key(|rtc| (rtc.reading - near).unsigned_abs())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What ares saved for a 64DD drive at 2026-11-17 10:00:00 UTC: 26-11-17 09:59:58 on the
    /// clock, then `FF FF`, then the anchor.
    fn drive() -> Vec<u8> {
        let mut file = vec![0x26, 0x11, 0x17, 0x09, 0x59, 0x58, 0xFF, 0xFF];
        file.extend_from_slice(&1_794_909_600u64.to_le_bytes());
        file
    }

    #[test]
    fn the_drive_s_clock_is_its_six_bytes_and_when_ares_saved_them() {
        let clock = parse_ares_n64dd(&drive()).expect("a clock ares saved");
        assert_eq!(clock.time, [0x26, 0x11, 0x17, 0x09, 0x59, 0x58]);
        assert_eq!(clock.taken_at, Some(1_794_909_600));
        assert_eq!(clock.reading().map(|rtc| rtc.reading), Some(1_794_909_598));
        assert_eq!(build_ares_n64dd(&clock, 0).to_vec(), drive());
    }

    #[test]
    fn a_file_ares_never_saved_holds_no_clock() {
        assert_eq!(parse_ares_n64dd(&[0xFF; ARES_RTC_LEN]), None);
        let mut unanchored = drive();
        unanchored[8..].fill(0);
        assert_eq!(parse_ares_n64dd(&unanchored), None);
        assert_eq!(parse_ares_n64dd(&drive()[..15]), None, "and one of another length is no file of ares's");
    }

    #[test]
    fn an_mvs_clock_reads_in_the_century_of_the_moment_it_was_taken() {
        // 26-11-17 09:59:58, a Tuesday: second, minute, hour, day, month and weekday, year.
        let mut file = vec![0x58, 0x59, 0x09, 0x17, 0xB2, 0x26, 0xFF, 0xFF];
        file.extend_from_slice(&1_794_909_600u64.to_le_bytes());
        let clock = parse_ares_upd4990a(&file).expect("a clock ares saved");
        assert_eq!(upd4990a_reading(&clock, 0).map(|rtc| rtc.reading), Some(1_794_909_598));
        assert_eq!(build_ares_upd4990a(&clock, 0).to_vec(), file);

        // A clock set to 1999 read in 2001 is 1999's, not 2099's.
        let nineties = RtcUpd4990a { time: [0, 0, 0, 0x31, 0xC5, 0x99], taken_at: Some(978_307_200) };
        assert_eq!(upd4990a_reading(&nineties, 0).map(|rtc| rtc.reading), Some(946_598_400));
    }
}
