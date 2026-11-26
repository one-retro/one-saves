//! Nintendo DS: the save memory each game card carries.
//!
//! A DS card keeps its saves in one serial chip, and nothing in the card's header says which: it
//! names the game in four characters at `0x0C`, `AMCE`, and stops there. Nothing a game asks the
//! chip tells its size either. So an emulator looks the game up, which is what [`cartridge`] does,
//! from the table melonDS keeps and SkyEmu carries under the MIT licence.
//!
//! The size is all a chip needs to be told by: an EEPROM, a FRAM and a flash chip of one size take
//! the same commands, and the address width follows from the size. The table gives the kind as
//! well, because it is what the card carries, but a save of one is a save of the other.
//!
//! Absence is two answers here as on the N64. A game the table holds with no save memory answers
//! `Some` with no [`save`](NdsCartridge::save); a game the table does not hold answers `None`,
//! and a caller that wants a chip has to find it elsewhere, never assume one.
//!
//! # NAND
//!
//! The table gives a few cards NAND of 8, 16 or 64 MiB: WarioWare D.I.Y. and Jam with the Band,
//! which save to it, and a hundred or so others melonDS also builds as NAND cards. Those answer
//! [`SaveKind::Nand`]. The Universal Saves Format does not yet say what a NAND card's save holds, so
//! a converter carries one as an `aux` part rather than as the card's save.

use crate::{Save, SaveKind};

mod games;

/// What a DS card carries for its saves, as far as its game code says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NdsCartridge {
    /// The save memory on the card, or `None` for a game known to have none.
    pub save: Option<Save>,
}

/// What a retail DS card carries, by its game code.
///
/// `game_code` is the header's four characters, `AMCE`, or a serial built from them, `NTR-AMCE`
/// or `NTR-AMCE-USA`, as a card's label and a catalog spell it (`TWL-` for a DSi-enhanced card).
///
/// ```
/// use rom_id::{SaveKind, nds};
///
/// let kart = nds::cartridge("NTR-AMCE-USA").expect("Mario Kart DS");
/// assert_eq!(kart.save.map(|save| (save.kind, save.size)), Some((SaveKind::Flash, Some(262_144))));
///
/// let sky = nds::cartridge("C2SE").expect("Explorers of Sky");
/// assert_eq!(sky.save.and_then(|save| save.size), Some(131_072));
/// ```
#[must_use]
pub fn cartridge(game_code: &str) -> Option<NdsCartridge> {
    let code = ["NTR-", "TWL-"].iter().find_map(|prefix| game_code.strip_prefix(prefix)).unwrap_or(game_code);
    let code = code.split('-').next().unwrap_or(code);
    let code: [u8; 4] = code.as_bytes().try_into().ok()?;
    lookup(code)
}

/// The table's answer for a header's code.
pub(crate) fn lookup(code: [u8; 4]) -> Option<NdsCartridge> {
    let at = games::GAMES.binary_search_by_key(&code, |&(game, _)| game).ok()?;
    Some(NdsCartridge { save: games::GAMES[at].1.save() })
}

/// The save memory a row names, as melonDS numbers it in its own table: 1 to 7 for EEPROM and
/// flash, 8 to 10 for NAND, 0 for none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Memory {
    /// No save memory at all.
    Nothing,
    /// 4 Kbit EEPROM, 512 bytes.
    Eeprom4k,
    /// 64 Kbit EEPROM, 8 KiB.
    Eeprom64k,
    /// 512 Kbit EEPROM, 64 KiB.
    Eeprom512k,
    /// 1 Mbit EEPROM, 128 KiB: Pokémon Mystery Dungeon: Explorers of Sky and a handful more.
    Eeprom1m,
    /// 2 Mbit flash, 256 KiB.
    Flash2m,
    /// 4 Mbit flash, 512 KiB.
    Flash4m,
    /// 8 Mbit flash, 1 MiB.
    Flash8m,
    /// NAND, 8 MiB.
    Nand8m,
    /// NAND, 16 MiB.
    Nand16m,
    /// NAND, 64 MiB.
    Nand64m,
}

impl Memory {
    fn save(self) -> Option<Save> {
        const KIB: u32 = 1024;
        let (kind, size) = match self {
            Memory::Nothing => return None,
            Memory::Eeprom4k => (SaveKind::Eeprom, 512),
            Memory::Eeprom64k => (SaveKind::Eeprom, 8 * KIB),
            Memory::Eeprom512k => (SaveKind::Eeprom, 64 * KIB),
            Memory::Eeprom1m => (SaveKind::Eeprom, 128 * KIB),
            Memory::Flash2m => (SaveKind::Flash, 256 * KIB),
            Memory::Flash4m => (SaveKind::Flash, 512 * KIB),
            Memory::Flash8m => (SaveKind::Flash, 1024 * KIB),
            Memory::Nand8m => (SaveKind::Nand, 8 * 1024 * KIB),
            Memory::Nand16m => (SaveKind::Nand, 16 * 1024 * KIB),
            Memory::Nand64m => (SaveKind::Nand, 64 * 1024 * KIB),
        };
        Some(Save { kind, size: Some(size), lane: None })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the table says of a game: `Err` where it does not hold the game, `Ok(None)` where it
    /// holds it with no save memory.
    fn chip(code: &str) -> Result<Option<(SaveKind, u32)>, ()> {
        let cart = cartridge(code).ok_or(())?;
        Ok(cart.save.map(|save| (save.kind, save.size.unwrap())))
    }

    #[test]
    fn the_table_is_sorted_and_holds_each_game_once() {
        assert!(games::GAMES.windows(2).all(|pair| pair[0].0 < pair[1].0));
    }

    #[test]
    fn every_chip_the_corpus_played_is_found_by_its_game() {
        // The ten games every libretro DS core was played on, and the sizes the cores that keep
        // the chip as it is wrote for them.
        assert_eq!(chip("ABME"), Ok(Some((SaveKind::Eeprom, 512))), "Bomberman");
        assert_eq!(chip("ATRE"), Ok(Some((SaveKind::Eeprom, 8192))), "Tetris DS");
        assert_eq!(chip("A8TE"), Ok(Some((SaveKind::Eeprom, 8192))), "Mario Party DS");
        assert_eq!(chip("ATDE"), Ok(Some((SaveKind::Eeprom, 8192))), "Clubhouse Games");
        assert_eq!(chip("AKYE"), Ok(Some((SaveKind::Eeprom, 65_536))), "Etrian Odyssey");
        assert_eq!(chip("C2SE"), Ok(Some((SaveKind::Eeprom, 131_072))), "Explorers of Sky");
        assert_eq!(chip("AMCE"), Ok(Some((SaveKind::Flash, 262_144))), "Mario Kart DS");
        assert_eq!(chip("APAE"), Ok(Some((SaveKind::Flash, 524_288))), "Pokémon Pearl");
        assert_eq!(chip("IPKE"), Ok(Some((SaveKind::Flash, 524_288))), "Pokémon HeartGold");
        assert_eq!(chip("C6PE"), Ok(Some((SaveKind::Flash, 1_048_576))), "Picross 3D");
        assert_eq!(chip("UXBP"), Ok(Some((SaveKind::Nand, 8 << 20))), "Jam with the Band");
        assert_eq!(chip("UORE"), Ok(Some((SaveKind::Nand, 16 << 20))), "WarioWare D.I.Y.");
    }

    #[test]
    fn a_serial_names_the_game_its_code_does() {
        assert_eq!(cartridge("NTR-AMCE-USA"), cartridge("AMCE"));
        assert_eq!(cartridge("NTR-AMCE"), cartridge("AMCE"));
        assert_eq!(cartridge("TWL-VAAV"), cartridge("VAAV"));
        assert_eq!(cartridge("AMC"), None, "three characters name no game");
        assert_eq!(cartridge("ZZZZ"), None, "no game has that code");
    }
}
