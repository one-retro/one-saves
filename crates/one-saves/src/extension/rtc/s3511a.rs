//! [`x.1sav.rtc.s3511a`](https://docs.1retro.com/specifications/extensions/x.1sav.rtc.s3511a/).

use std::fmt;

use dcbor::{CBOR, Map};

use super::{Rtc, field, from_bcd, optional_epoch, utc, want_bytes, write_taken};
use crate::extension::{Extension, epoch, only_keys, want_map, want_uint};
use crate::name::ReverseDnsName;

/// The alarm register the chip holds at power-on, which a rebuild uses where the key has none.
const POWER_ON_ALARM: [u8; 2] = [0x00, 0x80];

/// The latched state of a Seiko S-3511A, and the moment a producer took it.
///
/// The chip is in GBA cartridges and, behind the Bandai 2003 mapper, in WonderSwan ones.
///
/// **The moment is not the reading.** The chip runs from whatever the game last set it to, so
/// what it showed is the components, which [`reading`](Self::reading) puts on the Unix scale.
/// `taken_at` is the host clock the producer advances the chip from, on the producer's own scale,
/// which is what rebuilding its file needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtcS3511a {
    /// The seven latched BCD bytes: year since 2000, month, day, weekday, hour, minute, second.
    pub components: [u8; 7],
    /// The control register as read. Bit 6 is 24-hour mode.
    pub control: u8,
    /// The producer's clock when it took the components, in epoch seconds. The anchor, not the
    /// reading, and not necessarily UTC: MiSTer gives its cores local standard time counted as
    /// though it were.
    pub taken_at: Option<i64>,
    /// The alarm register, which a game writes and the chip never reads back.
    pub alarm: S3511aAlarm,
}

/// What a producer knows of the S-3511A's alarm register.
///
/// The chip reads the register back as all ones, so only a producer that saw the game write it
/// holds it. Not knowing is not the same as there being nothing to know, which is why there are
/// three cases and not two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum S3511aAlarm {
    /// The game never sets the alarm, as Pokémon Ruby, Sapphire and Emerald never do. The key is
    /// absent.
    NeverSet,
    /// The game may have set it, and the producer's source does not keep it. The key is null.
    ///
    /// What a producer that does not have the alarm writes, unless it knows the game never sets
    /// one.
    Unknown,
    /// The two bytes the game last wrote: an alarm hour and minute, or an interrupt frequency.
    Written([u8; 2]),
}

impl RtcS3511a {
    /// Whether the chip is in 24-hour mode, which is bit 6 of the control register.
    #[must_use]
    pub fn hour24(&self) -> bool {
        self.control & 0x40 != 0
    }

    /// The chip's clock on the Unix scale: its components, read as though they were UTC.
    ///
    /// The chip keeps no zone, so its wall clock goes on the scale the way `x.1sav.dirent` puts a
    /// console's there. In 12-hour mode, bit 7 of the hour byte is what says it is after noon.
    ///
    /// `taken_at` is not the answer, since it is when the components were taken rather than what
    /// they say, and the two can be years apart: GBA_MiSTer starts the chip at
    /// 2009-12-31 23:59:45, and a game that never sets it leaves it there.
    ///
    /// `None` when the components are not a date, since a reading that cannot be computed is
    /// omitted rather than approximated.
    #[must_use]
    pub fn reading(&self) -> Option<Rtc> {
        let [year, month, day, _weekday, hour, minute, second] = self.components;
        let digits = from_bcd(hour & 0x3f)?;
        let hour = if !self.hour24() && hour & 0x80 != 0 { digits % 12 + 12 } else { digits };
        let year = 2000 + i64::from(from_bcd(year)?);
        let seconds =
            utc(year, from_bcd(month)?, from_bcd(day)?, hour, from_bcd(minute)?, from_bcd(second)?)?;
        Some(Rtc::new(seconds))
    }

    /// The alarm register a rebuild puts back: the one the game wrote, or the chip's power-on
    /// value where the key has none.
    ///
    /// A game cannot read the alarm back, so it sees the difference only in when the chip
    /// interrupts it.
    #[must_use]
    pub fn alarm_or_power_on(&self) -> [u8; 2] {
        match self.alarm {
            S3511aAlarm::Written(alarm) => alarm,
            S3511aAlarm::NeverSet | S3511aAlarm::Unknown => POWER_ON_ALARM,
        }
    }
}

impl Extension for RtcS3511a {
    const KEY: ReverseDnsName = ReverseDnsName::from_static("x.1sav.rtc.s3511a");
}

impl From<RtcS3511a> for CBOR {
    fn from(clock: RtcS3511a) -> Self {
        let mut map = Map::new();
        map.insert(0u64, CBOR::to_byte_string(clock.components));
        map.insert(1u64, u64::from(clock.control));
        if let Some(taken_at) = clock.taken_at {
            map.insert(2u64, epoch(taken_at));
        }
        match clock.alarm {
            S3511aAlarm::NeverSet => {}
            S3511aAlarm::Unknown => map.insert(3u64, CBOR::null()),
            S3511aAlarm::Written(alarm) => map.insert(3u64, CBOR::to_byte_string(alarm)),
        }
        map.into()
    }
}

impl TryFrom<CBOR> for RtcS3511a {
    type Error = dcbor::Error;

    fn try_from(value: CBOR) -> dcbor::Result<Self> {
        let map = want_map(&value)?;
        only_keys(map, &[0, 1, 2, 3])?;
        let alarm = match map.get::<u64, CBOR>(3) {
            None => S3511aAlarm::NeverSet,
            Some(value) if value.is_null() => S3511aAlarm::Unknown,
            Some(value) => S3511aAlarm::Written(want_bytes(&value)?),
        };
        Ok(RtcS3511a {
            components: want_bytes(&field(map, 0)?)?,
            control: want_uint(&field(map, 1)?)?,
            taken_at: optional_epoch(map, 2)?,
            alarm,
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
        let mode = if self.hour24() { "24-hour" } else { "12-hour" };
        let bcd = |byte: u8| byte >> 4 <= 9 && byte & 0x0f <= 9;
        if [year, month, day, weekday, hour, minute, second].into_iter().all(bcd) {
            write!(f, "20{year:02x}-{month:02x}-{day:02x} {hour:02x}:{minute:02x}:{second:02x}, {mode}")?;
        } else {
            f.write_str("not BCD: h'")?;
            self.components.iter().try_for_each(|byte| write!(f, "{byte:02x}"))?;
            write!(f, "', {mode}")?;
        }
        write_taken(f, self.taken_at)?;
        match self.alarm {
            S3511aAlarm::Written([high, low]) => write!(f, ", alarm h'{high:02x}{low:02x}'"),
            S3511aAlarm::NeverSet | S3511aAlarm::Unknown => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seiko_clock_shows_its_digits() {
        let clock = RtcS3511a {
            components: [0x26, 0x08, 0x12, 0x03, 0x10, 0x39, 0x43],
            control: 0x40,
            taken_at: None,
            alarm: S3511aAlarm::NeverSet,
        };
        assert_eq!(RtcS3511a::try_from(CBOR::from(clock)).unwrap(), clock);
        assert_eq!(clock.to_string(), "2026-08-12 10:39:43, 24-hour");
        let garbled =
            RtcS3511a { components: [0x01, 0x20, 0x3f, 0x00, 0x3d, 0x9c, 0xbb], control: 0, ..clock };
        assert_eq!(garbled.to_string(), "not BCD: h'01203f003d9cbb', 12-hour");
    }

    #[test]
    fn a_seiko_clock_carries_the_moment_it_was_taken() {
        // Key 2 is optional: a bundle written before it existed has only keys 0 and 1.
        let clock = RtcS3511a {
            components: [0x10, 0x01, 0x01, 0x00, 0x00, 0x08, 0x47],
            control: 0x40,
            taken_at: Some(1_792_576_838),
            alarm: S3511aAlarm::NeverSet,
        };
        let encoded = CBOR::from(clock);
        assert_eq!(encoded.as_map().unwrap().len(), 3);
        assert_eq!(RtcS3511a::try_from(encoded).unwrap(), clock);
        assert_eq!(clock.to_string(), "2010-01-01 00:08:47, 24-hour, taken 2026-10-21 10:00:38");
    }

    #[test]
    fn the_alarm_is_absent_null_or_what_the_game_wrote() {
        let clock = RtcS3511a {
            components: [0x26, 0x08, 0x12, 0x03, 0x10, 0x39, 0x43],
            control: 0x40,
            taken_at: None,
            alarm: S3511aAlarm::Unknown,
        };
        let encoded = CBOR::from(clock);
        assert!(encoded.as_map().unwrap().get::<u64, CBOR>(3).unwrap().is_null());
        assert_eq!(RtcS3511a::try_from(encoded).unwrap(), clock);
        assert_eq!(clock.alarm_or_power_on(), [0x00, 0x80]);

        let set = RtcS3511a { alarm: S3511aAlarm::Written([0x07, 0x30]), ..clock };
        assert_eq!(RtcS3511a::try_from(CBOR::from(set)).unwrap(), set);
        assert_eq!(set.alarm_or_power_on(), [0x07, 0x30]);
        assert_eq!(set.to_string(), "2026-08-12 10:39:43, 24-hour, alarm h'0730'");
    }

    #[test]
    fn the_reading_is_the_components_as_utc() {
        let clock = RtcS3511a {
            components: [0x26, 0x10, 0x21, 0x03, 0x13, 0x40, 0x16],
            control: 0x40,
            taken_at: Some(1),
            alarm: S3511aAlarm::Unknown,
        };
        assert_eq!(clock.reading(), Some(Rtc::new(1_792_590_016)));
        // 1:40 PM on the 12-hour clock is the same moment.
        let twelve =
            RtcS3511a { components: [0x26, 0x10, 0x21, 0x03, 0x81, 0x40, 0x16], control: 0, ..clock };
        assert_eq!(twelve.reading(), clock.reading());
        // And the 31st of September is no moment at all.
        let invalid = RtcS3511a { components: [0x26, 0x09, 0x31, 0x03, 0x13, 0x40, 0x16], ..clock };
        assert_eq!(invalid.reading(), None);
    }
}
