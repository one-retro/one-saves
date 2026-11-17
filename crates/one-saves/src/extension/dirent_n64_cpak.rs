//! [`x.1sav.dirent.n64-cpak`](https://docs.1retro.com/specifications/extensions/x.1sav.dirent.n64-cpak/).

use std::fmt;

use dcbor::{CBOR, Map};

use super::{Extension, only_keys, want_map, want_uint};
use crate::name::ReverseDnsName;

/// An N64 Controller Pak note's 32-byte entry, field by field, for a writer that has to build one.
///
/// The write half of [`Dirent`](super::Dirent) for the `n64-cpak` format, beside
/// [`DirentGcMc`](super::DirentGcMc): a Controller Pak dates nothing, so `Dirent` has nothing to
/// say about a note. It goes on the `bundle` part of a pak's card that holds the note's save,
/// whose [`dirent`](crate::Part::dirent) is the original 32 bytes. The one field not here is the
/// start page at 0x06, which is the allocator's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirentN64Cpak {
    /// The game code at 0x00, as a cartridge's header holds it: `NKTE` for Mario Kart 64.
    pub game_code: [u8; 4],
    /// The publisher's code at 0x04.
    pub company_code: [u8; 2],
    /// The status byte at 0x08, whose bit 1 is set on every note a game has written.
    pub status: u8,
    /// The byte at 0x09, as the entry holds it. libultra never reads it.
    pub reserved: u8,
    /// The 16-bit field at 0x0a, as the entry holds it. libultra never reads it either, and some
    /// tools keep their own data there.
    pub data_sum: u16,
    /// The extension at 0x0c, in the pak's character set, padding included.
    pub extension: [u8; 4],
    /// The name at 0x10, in the pak's character set, padding included.
    pub name: [u8; 16],
}

impl DirentN64Cpak {
    /// The fields of a note's 32-byte entry, as a pak's note table holds it.
    #[must_use]
    pub fn from_entry(entry: &[u8; 32]) -> Self {
        let take = |from: usize| -> [u8; 4] { entry[from..from + 4].try_into().expect("four bytes") };
        DirentN64Cpak {
            game_code: take(0x00),
            company_code: [entry[0x04], entry[0x05]],
            status: entry[0x08],
            reserved: entry[0x09],
            data_sum: u16::from_be_bytes([entry[0x0a], entry[0x0b]]),
            extension: take(0x0c),
            name: entry[0x10..0x20].try_into().expect("sixteen bytes"),
        }
    }

    /// The entry these fields make, with the note's chain starting at `start_page`: the bank in
    /// the high byte and the page in the low, as the pak's index table numbers them.
    #[must_use]
    pub fn to_entry(&self, start_page: u16) -> [u8; 32] {
        let mut entry = [0u8; 32];
        entry[0x00..0x04].copy_from_slice(&self.game_code);
        entry[0x04..0x06].copy_from_slice(&self.company_code);
        entry[0x06..0x08].copy_from_slice(&start_page.to_be_bytes());
        entry[0x08] = self.status;
        entry[0x09] = self.reserved;
        entry[0x0a..0x0c].copy_from_slice(&self.data_sum.to_be_bytes());
        entry[0x0c..0x10].copy_from_slice(&self.extension);
        entry[0x10..0x20].copy_from_slice(&self.name);
        entry
    }
}

impl Extension for DirentN64Cpak {
    const KEY: ReverseDnsName = ReverseDnsName::from_static("x.1sav.dirent.n64-cpak");
}

/// A fixed-width field of the entry, which a value of any other length is not.
fn fixed<const N: usize>(value: &CBOR) -> dcbor::Result<[u8; N]> {
    let bytes = value.as_byte_string().ok_or(dcbor::Error::WrongType)?;
    bytes.try_into().map_err(|_| dcbor::Error::OutOfRange)
}

impl From<DirentN64Cpak> for CBOR {
    fn from(entry: DirentN64Cpak) -> Self {
        let mut map = Map::new();
        map.insert(0u64, CBOR::to_byte_string(entry.game_code));
        map.insert(1u64, CBOR::to_byte_string(entry.company_code));
        map.insert(2u64, u64::from(entry.status));
        map.insert(3u64, u64::from(entry.reserved));
        map.insert(4u64, u64::from(entry.data_sum));
        map.insert(5u64, CBOR::to_byte_string(entry.extension));
        map.insert(6u64, CBOR::to_byte_string(entry.name));
        map.into()
    }
}

impl TryFrom<CBOR> for DirentN64Cpak {
    type Error = dcbor::Error;

    fn try_from(value: CBOR) -> dcbor::Result<Self> {
        let map = want_map(&value)?;
        only_keys(map, &[0, 1, 2, 3, 4, 5, 6])?;
        let field = |key: u64| map.get::<u64, CBOR>(key).ok_or(dcbor::Error::MissingMapKey);
        Ok(DirentN64Cpak {
            game_code: fixed(&field(0)?)?,
            company_code: fixed(&field(1)?)?,
            status: want_uint(&field(2)?)?,
            reserved: want_uint(&field(3)?)?,
            data_sum: want_uint(&field(4)?)?,
            extension: fixed(&field(5)?)?,
            name: fixed(&field(6)?)?,
        })
    }
}

impl fmt::Display for DirentN64Cpak {
    /// The game and company codes, as text where they are plain ASCII and as bytes otherwise.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let codes = [&self.game_code[..], &self.company_code[..]].concat();
        if codes.iter().all(|byte| (0x20..0x7f).contains(byte)) {
            write!(f, "{}", String::from_utf8_lossy(&codes))
        } else {
            f.write_str("h'")?;
            codes.iter().try_for_each(|byte| write!(f, "{byte:02x}"))?;
            f.write_str("'")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mario Kart 64's note, its name spelled through the pak's table, its chain at page 5.
    fn mario_kart() -> [u8; 32] {
        let mut entry = [0u8; 32];
        entry[..6].copy_from_slice(b"NKTE01");
        entry[6..8].copy_from_slice(&[0x00, 0x05]);
        entry[8] = 0x02;
        entry[0x10..0x1b]
            .copy_from_slice(&[0x27, 0x1b, 0x2c, 0x23, 0x29, 0x25, 0x1b, 0x2c, 0x2e, 0x16, 0x14]);
        entry
    }

    #[test]
    fn an_entry_comes_back_but_for_where_its_chain_starts() {
        let fields = DirentN64Cpak::from_entry(&mario_kart());
        assert_eq!(fields.to_entry(5), mario_kart());
        assert_eq!(DirentN64Cpak::try_from(CBOR::from(fields)).unwrap(), fields);
        assert_eq!(fields.to_string(), "NKTE01");
    }

    #[test]
    fn refuses_a_field_wider_than_the_entry_holds() {
        let mut map = want_map(&CBOR::from(DirentN64Cpak::from_entry(&mario_kart()))).unwrap().clone();
        map.insert(6u64, CBOR::to_byte_string([0u8; 17]));
        assert!(DirentN64Cpak::try_from(CBOR::from(map.clone())).is_err(), "a name is 16 bytes");
        map.insert(6u64, CBOR::to_byte_string([0u8; 16]));
        map.insert(2u64, 256u64);
        assert!(DirentN64Cpak::try_from(CBOR::from(map)).is_err(), "status is one byte");
    }
}
