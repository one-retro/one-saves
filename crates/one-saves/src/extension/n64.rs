//! [`x.1sav.n64`](https://docs.1retro.com/specifications/extensions/x.1sav.n64/).

use std::fmt;

use dcbor::{CBOR, Map};

use super::{Extension, only_keys, want_map};
use crate::name::ReverseDnsName;

/// The save memory an N64 cartridge carries, on the part that holds it.
///
/// Its presence also says the payload is in **normal form**: the chip's bytes in the order the
/// console reads them, big-endian, at the chip's real size. That is what a flash cartridge and a
/// cartridge reader produce. Emulators do not all agree: mupen64plus and Project64 keep SRAM and
/// FlashRAM as host-order 32-bit words, every word's bytes reversed, and a libretro core pads
/// every chip into one file. A producer converts to normal form before it writes this key, and a
/// consumer converts back.
///
/// The type cannot be read off the payload. A 4 Kbit game's EEPROM and the first 512 bytes of a
/// 16 Kbit one look alike, and SRAM and FlashRAM share an address range, so an emulator decides
/// from a database and the decision is lost once the bytes are a file. This key keeps it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct N64 {
    /// Which chip the cartridge has.
    pub save: N64Save,
}

impl N64 {
    /// The key for a cartridge with this chip.
    #[must_use]
    pub fn new(save: N64Save) -> Self {
        N64 { save }
    }
}

/// A kind of N64 cartridge save memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum N64Save {
    /// 4 Kbit EEPROM, 512 bytes: Super Mario 64, Mario Kart 64.
    Eeprom4k,
    /// 16 Kbit EEPROM, 2 KiB: Yoshi's Story, Banjo-Tooie.
    Eeprom16k,
    /// 256 Kbit SRAM, 32 KiB: Ocarina of Time, F-Zero X.
    Sram256k,
    /// 768 Kbit SRAM, 96 KiB, three banks of 32 KiB: Dezaemon 3D alone.
    Sram768k,
    /// 1 Mbit FlashRAM, 128 KiB: Majora's Mask, Paper Mario.
    Flash1m,
}

impl N64Save {
    /// Every type, in the order the homebrew header numbers them.
    pub const ALL: [N64Save; 5] =
        [N64Save::Eeprom4k, N64Save::Eeprom16k, N64Save::Sram256k, N64Save::Sram768k, N64Save::Flash1m];

    /// How the key writes the type.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            N64Save::Eeprom4k => "eeprom-4k",
            N64Save::Eeprom16k => "eeprom-16k",
            N64Save::Sram256k => "sram-256k",
            N64Save::Sram768k => "sram-768k",
            N64Save::Flash1m => "flash-1m",
        }
    }

    /// The type a key's text names.
    #[must_use]
    pub fn from_name(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|save| save.as_str() == text)
    }

    /// The chip's size in bytes, which is the length of a payload in normal form.
    #[must_use]
    pub const fn size(self) -> usize {
        match self {
            N64Save::Eeprom4k => 512,
            N64Save::Eeprom16k => 2 * 1024,
            N64Save::Sram256k => 32 * 1024,
            N64Save::Sram768k => 96 * 1024,
            N64Save::Flash1m => 128 * 1024,
        }
    }

    /// Whether the chip is an EEPROM, which the console reaches over the Joybus in blocks of
    /// eight bytes rather than as words on the cartridge bus.
    #[must_use]
    pub const fn is_eeprom(self) -> bool {
        matches!(self, N64Save::Eeprom4k | N64Save::Eeprom16k)
    }
}

impl fmt::Display for N64Save {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Extension for N64 {
    const KEY: ReverseDnsName = ReverseDnsName::from_static("x.1sav.n64");
}

impl From<N64> for CBOR {
    fn from(n64: N64) -> Self {
        let mut map = Map::new();
        map.insert(0u64, n64.save.as_str());
        map.into()
    }
}

impl TryFrom<CBOR> for N64 {
    type Error = dcbor::Error;

    fn try_from(value: CBOR) -> dcbor::Result<Self> {
        let map = want_map(&value)?;
        only_keys(map, &[0])?;
        let text = String::try_from(map.get::<u64, CBOR>(0).ok_or(dcbor::Error::MissingMapKey)?)?;
        // A type this build does not know is a value it cannot read, not one to guess at: the
        // size it implies is what a consumer would write the save back out at.
        let save = N64Save::from_name(&text).ok_or(dcbor::Error::OutOfRange)?;
        Ok(N64 { save })
    }
}

impl fmt::Display for N64 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.save.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_the_schema() {
        for save in N64Save::ALL {
            let n64 = N64::new(save);
            assert_eq!(N64::try_from(CBOR::from(n64)).unwrap(), n64);
        }
        assert_eq!(N64::new(N64Save::Flash1m).to_string(), "flash-1m");
    }

    #[test]
    fn refuses_a_type_it_does_not_know() {
        let mut map = Map::new();
        map.insert(0u64, "sram-1m");
        assert!(N64::try_from(CBOR::from(map)).is_err());
    }

    #[test]
    fn refuses_keys_the_schema_does_not_define() {
        let mut map = Map::new();
        map.insert(0u64, "eeprom-4k");
        map.insert(1u64, "big-endian");
        assert!(N64::try_from(CBOR::from(map)).is_err());
    }
}
