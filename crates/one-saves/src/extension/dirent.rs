//! [`x.1sav.dirent`](https://docs.1retro.com/specifications/extensions/x.1sav.dirent/).

use std::fmt;

use dcbor::{CBOR, Map};

use super::time::Civil;
use super::{Extension, epoch, only_keys, want_epoch, want_map};
use crate::name::ReverseDnsName;

/// When a card's directory dates a save, and when the console last wrote it.
///
/// At least one of the two is present. A GameCube or Saturn entry dates only the last write, which
/// is why the schema admits `modified_at` alone rather than making `created_at` optional.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dirent {
    /// When the directory dates the entry.
    pub created_at: Option<EntryTime>,
    /// When the console last wrote it.
    pub modified_at: Option<EntryTime>,
}

/// One directory reading, and the zone it was taken in where that is known.
///
/// The seconds are the wall clock the console showed, put on the Unix scale by reading that wall
/// clock as though it were UTC. They are deliberately not an instant: a GameCube sets its clock
/// with no notion of a zone. The offset, in seconds east of UTC, is what would close the gap, and
/// it is written only where a producer actually knows the zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryTime {
    /// The wall clock, read as though it were UTC.
    pub seconds: i64,
    /// The zone that wall clock was in, in seconds east of UTC, from -43200 to 50400.
    pub utc_offset: Option<i32>,
}

impl EntryTime {
    /// A reading in no known zone.
    #[must_use]
    pub fn new(seconds: i64) -> Self {
        EntryTime { seconds, utc_offset: None }
    }

    /// States the zone the reading was taken in.
    #[must_use]
    pub fn utc_offset(mut self, utc_offset: i32) -> Self {
        self.utc_offset = Some(utc_offset);
        self
    }
}

impl Extension for Dirent {
    const KEY: ReverseDnsName = ReverseDnsName::from_static("x.1sav.dirent");
}

impl From<EntryTime> for CBOR {
    fn from(time: EntryTime) -> Self {
        match time.utc_offset {
            Some(offset) => vec![epoch(time.seconds), CBOR::from(offset)].into(),
            None => vec![epoch(time.seconds)].into(),
        }
    }
}

impl TryFrom<CBOR> for EntryTime {
    type Error = dcbor::Error;

    fn try_from(value: CBOR) -> dcbor::Result<Self> {
        let items = value.as_array().ok_or(dcbor::Error::WrongType)?;
        match items {
            [seconds] => Ok(EntryTime::new(want_epoch(seconds)?)),
            [seconds, offset] => {
                let offset = i32::try_from(offset.clone())?;
                if !(-43_200..=50_400).contains(&offset) {
                    return Err(dcbor::Error::OutOfRange);
                }
                Ok(EntryTime::new(want_epoch(seconds)?).utc_offset(offset))
            }
            _ => Err(dcbor::Error::OutOfRange),
        }
    }
}

impl From<Dirent> for CBOR {
    fn from(dirent: Dirent) -> Self {
        let mut map = Map::new();
        if let Some(created) = dirent.created_at {
            map.insert(0u64, created);
        }
        if let Some(modified) = dirent.modified_at {
            map.insert(1u64, modified);
        }
        map.into()
    }
}

impl TryFrom<CBOR> for Dirent {
    type Error = dcbor::Error;

    fn try_from(value: CBOR) -> dcbor::Result<Self> {
        let map = want_map(&value)?;
        only_keys(map, &[0, 1])?;
        let dirent = Dirent {
            created_at: map.get::<u64, CBOR>(0).map(EntryTime::try_from).transpose()?,
            modified_at: map.get::<u64, CBOR>(1).map(EntryTime::try_from).transpose()?,
        };
        if dirent.created_at.is_none() && dirent.modified_at.is_none() {
            return Err(dcbor::Error::MissingMapKey);
        }
        Ok(dirent)
    }
}

impl fmt::Display for EntryTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", Civil(self.seconds))?;
        if let Some(offset) = self.utc_offset {
            let sign = if offset < 0 { '-' } else { '+' };
            let minutes = offset.unsigned_abs() / 60;
            write!(f, " {sign}{:02}:{:02}", minutes / 60, minutes % 60)?;
        }
        Ok(())
    }
}

impl fmt::Display for Dirent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.created_at, &self.modified_at) {
            (Some(created), Some(modified)) => write!(f, "created {created}, modified {modified}"),
            (Some(created), None) => write!(f, "created {created}"),
            (None, Some(modified)) => write!(f, "modified {modified}"),
            (None, None) => f.write_str("undated"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_shows_the_wall_clock() {
        let dirent = Dirent {
            created_at: Some(EntryTime::new(1_700_000_000).utc_offset(32_400)),
            modified_at: Some(EntryTime::new(1_700_003_600).utc_offset(32_400)),
        };
        assert_eq!(Dirent::try_from(CBOR::from(dirent)).unwrap(), dirent);
        assert_eq!(
            dirent.to_string(),
            "created 2023-11-14 22:13:20 +09:00, modified 2023-11-14 23:13:20 +09:00"
        );
    }

    #[test]
    fn a_write_alone_is_enough_and_nothing_is_not() {
        let modified = Dirent { created_at: None, modified_at: Some(EntryTime::new(0)) };
        assert_eq!(Dirent::try_from(CBOR::from(modified)).unwrap(), modified);
        assert!(Dirent::try_from(CBOR::from(Map::new())).is_err());
    }

    #[test]
    fn refuses_an_offset_no_zone_has() {
        let far = Dirent { created_at: None, modified_at: Some(EntryTime::new(0).utc_offset(50_401)) };
        assert!(Dirent::try_from(CBOR::from(far)).is_err());
    }
}
