//! [`x.1sav.dirent.gc-mc`](https://docs.1retro.com/specifications/extensions/x.1sav.dirent.gc-mc/).

use std::fmt;

use dcbor::{CBOR, Map};

use super::{Extension, only_keys, want_map, want_uint};
use crate::name::ReverseDnsName;

/// A GameCube memory card's directory entry, field by field, for a writer that has to build one.
///
/// The write half of [`Dirent`](super::Dirent), which normalizes the times every card records but
/// does not rebuild an entry. Four fields of the 64 bytes are not here: `modtime` is `Dirent`'s
/// `modified_at`, the first block and the block count are the allocator's, and the padding is
/// constant. The original 64 bytes are the part's [`dirent`](crate::Part::dirent).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirentGcMc {
    /// The game code at 0x00: four ASCII characters, such as `GALE`.
    pub game_code: String,
    /// The maker code at 0x04: two ASCII characters, such as `01`.
    pub maker_code: String,
    /// The name at 0x08 without its padding, 1 to 32 bytes.
    ///
    /// Bytes rather than text: a Japanese save names itself in Shift-JIS, and transcoding into
    /// UTF-8 and back out need not land on the same 32 bytes.
    pub filename: Vec<u8>,
    /// The banner flags at 0x07, packed as the hardware packs them.
    pub banner_flags: u8,
    /// The icon formats at 0x30, two bits per icon.
    pub icon_format: u16,
    /// The animation speeds at 0x32, two bits per frame.
    pub anim_speed: u16,
    /// The copy protection bits at 0x34.
    pub permissions: u8,
    /// The count a console decrements on each copy, at 0x35.
    pub copy_counter: u8,
    /// Where the picture starts in the payload, from 0x2c. Absent where the entry holds `0xffffffff`.
    pub image_offset: Option<u32>,
    /// Where the comments start in the payload, from 0x3c. Absent where the entry holds
    /// `0xffffffff`.
    pub comments_offset: Option<u32>,
}

impl Extension for DirentGcMc {
    const KEY: ReverseDnsName = ReverseDnsName::from_static("x.1sav.dirent.gc-mc");
}

impl From<DirentGcMc> for CBOR {
    fn from(entry: DirentGcMc) -> Self {
        let mut map = Map::new();
        map.insert(0u64, entry.game_code);
        map.insert(1u64, entry.maker_code);
        map.insert(2u64, CBOR::to_byte_string(entry.filename));
        map.insert(3u64, u64::from(entry.banner_flags));
        map.insert(4u64, u64::from(entry.icon_format));
        map.insert(5u64, u64::from(entry.anim_speed));
        map.insert(6u64, u64::from(entry.permissions));
        map.insert(7u64, u64::from(entry.copy_counter));
        if let Some(offset) = entry.image_offset {
            map.insert(8u64, u64::from(offset));
        }
        if let Some(offset) = entry.comments_offset {
            map.insert(9u64, u64::from(offset));
        }
        map.into()
    }
}

impl TryFrom<CBOR> for DirentGcMc {
    type Error = dcbor::Error;

    fn try_from(value: CBOR) -> dcbor::Result<Self> {
        let map = want_map(&value)?;
        only_keys(map, &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9])?;
        let field = |key: u64| map.get::<u64, CBOR>(key).ok_or(dcbor::Error::MissingMapKey);
        // The codes are fixed-width fields of the entry, so a value of any other length is not one.
        let code = |key: u64, len: usize| -> dcbor::Result<String> {
            let text = String::try_from(field(key)?)?;
            if text.len() == len { Ok(text) } else { Err(dcbor::Error::OutOfRange) }
        };
        let filename = field(2)?.as_byte_string().ok_or(dcbor::Error::WrongType)?.to_vec();
        if !(1..=32).contains(&filename.len()) {
            return Err(dcbor::Error::OutOfRange);
        }
        Ok(DirentGcMc {
            game_code: code(0, 4)?,
            maker_code: code(1, 2)?,
            filename,
            banner_flags: want_uint(&field(3)?)?,
            icon_format: want_uint(&field(4)?)?,
            anim_speed: want_uint(&field(5)?)?,
            permissions: want_uint(&field(6)?)?,
            copy_counter: want_uint(&field(7)?)?,
            image_offset: map.get::<u64, CBOR>(8).map(|value| want_uint(&value)).transpose()?,
            comments_offset: map.get::<u64, CBOR>(9).map(|value| want_uint(&value)).transpose()?,
        })
    }
}

impl fmt::Display for DirentGcMc {
    /// The codes and the name, which is what tells two saves apart. A name that is not plain ASCII
    /// is shown as its bytes, since this does not know which encoding the save chose.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{} ", self.game_code, self.maker_code)?;
        if self.filename.iter().all(|byte| (0x20..0x7f).contains(byte)) {
            write!(f, "{:?}", String::from_utf8_lossy(&self.filename))
        } else {
            f.write_str("h'")?;
            self.filename.iter().try_for_each(|byte| write!(f, "{byte:02x}"))?;
            f.write_str("'")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn melee() -> DirentGcMc {
        DirentGcMc {
            game_code: "GALE".into(),
            maker_code: "01".into(),
            filename: b"SuperSmashBros0110290334".to_vec(),
            banner_flags: 2,
            icon_format: 0x0002,
            anim_speed: 0x0003,
            permissions: 4,
            copy_counter: 0,
            image_offset: Some(0),
            comments_offset: None,
        }
    }

    #[test]
    fn round_trips_and_shows_the_name() {
        assert_eq!(DirentGcMc::try_from(CBOR::from(melee())).unwrap(), melee());
        assert_eq!(melee().to_string(), r#"GALE01 "SuperSmashBros0110290334""#);
        let japanese = DirentGcMc { filename: vec![0x83, 0x5a, 0x81, 0x5b, 0x83, 0x75], ..melee() };
        assert_eq!(japanese.to_string(), "GALE01 h'835a815b8375'");
    }

    #[test]
    fn refuses_a_field_wider_than_the_entry_holds() {
        assert!(DirentGcMc::try_from(CBOR::from(DirentGcMc { game_code: "GAL".into(), ..melee() })).is_err());
        assert!(
            DirentGcMc::try_from(CBOR::from(DirentGcMc { filename: vec![b'a'; 33], ..melee() })).is_err()
        );
        let mut map = want_map(&CBOR::from(melee())).unwrap().clone();
        map.insert(6u64, 256u64);
        assert!(DirentGcMc::try_from(CBOR::from(map)).is_err(), "permissions is one byte");
    }
}
