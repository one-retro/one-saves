//! What an MVS BIOS keeps in a cabinet's backup RAM about the games in its slots.
//!
//! The RAM is 64 KiB at `$D00000`, read in 16-bit words. The first time the BIOS boots without
//! finding its signature it sets all of it up, and from then on it keeps a table of which game is
//! in each slot: the game's NGH number, and which of eight name blocks holds its title. Everything
//! here takes the RAM in the byte order the 68000 sees, which is what
//! [`MemoryCard::backup_ram`](crate::MemoryCard::backup_ram) and
//! [`split_mister`](crate::split_mister) hand out.
//!
//! The offsets are NeoGeoDev's [map of the backup RAM](https://wiki.neogeodev.org/index.php?title=Backup_RAM),
//! and MiSTer saves of three games in MVS mode bear them out: each has the signature, and a slot
//! table naming the game in name block 0, which holds its name — NGH `0x0250` and `METAL SLUG X`,
//! `0x0008` and `JOCKEY GRANDPRIX`, `0x03E7` and ` DYNA VLINER`, padding and all.
//!
//! ```
//! use neogeo_memcard::cabinet;
//!
//! // RAM nothing has set up names no game.
//! assert!(cabinet::games(&vec![0xFF; 0x10000]).is_empty());
//! ```

use crate::legible;

/// What the BIOS writes at [`SIGNATURE_OFFSET`] once it has set the RAM up.
///
/// With a space before the `!`, which a real cabinet's RAM shows and a rendering of it is apt to
/// drop. A byte `0x80` follows, which is not checked.
pub const SIGNATURE: &[u8; 15] = b"BACKUP RAM OK !";

/// Where the signature sits.
pub const SIGNATURE_OFFSET: usize = 0x10;

/// How many slots the table has room for, which is the most any MVS board takes.
pub const SLOTS: usize = 8;

/// The slot table: four bytes a slot, the NGH number and then the name block, both big-endian.
const SLOT_TABLE: usize = 0x124;

/// The names: sixteen bytes a block, one block for each slot's game.
const NAME_TABLE: usize = 0x2A0;

/// How long a name is.
const NAME_LEN: usize = 16;

/// A slot's name block when the slot is empty.
const NO_BLOCK: u16 = 0xFFFF;

/// A game the BIOS has seen in one of the cabinet's slots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Game {
    /// Which slot it was in, from 0.
    pub slot: usize,
    /// Its NGH number, binary-coded decimal as the card stores it: Metal Slug X's is `0x0250`.
    pub ngh: u16,
    /// The name the BIOS keeps for it, without the spaces padding it, or empty when nothing legible
    /// is there.
    pub name: String,
}

/// Whether the BIOS has set this RAM up, which is what its signature says.
#[must_use]
pub fn is_set_up(ram: &[u8]) -> bool {
    ram.get(SIGNATURE_OFFSET..SIGNATURE_OFFSET + SIGNATURE.len()) == Some(SIGNATURE.as_slice())
}

/// The games the BIOS has recorded, in slot order. Empty when it has not set the RAM up.
///
/// A slot the BIOS has never seen a game in holds NGH `0` and name block `0xFFFF`, and is left out.
#[must_use]
pub fn games(ram: &[u8]) -> Vec<Game> {
    if !is_set_up(ram) {
        return Vec::new();
    }
    (0..SLOTS)
        .filter_map(|slot| {
            let entry = ram.get(SLOT_TABLE + slot * 4..SLOT_TABLE + slot * 4 + 4)?;
            let ngh = u16::from_be_bytes([entry[0], entry[1]]);
            let block = u16::from_be_bytes([entry[2], entry[3]]);
            if ngh == 0 || block == NO_BLOCK {
                return None;
            }
            // A block past the table would be some other record's bytes, not a name.
            let block = usize::from(block);
            let name = (block < SLOTS)
                .then(|| ram.get(NAME_TABLE + block * NAME_LEN..NAME_TABLE + (block + 1) * NAME_LEN))
                .flatten()
                .map(legible)
                .unwrap_or_default();
            Some(Game { slot, ngh, name })
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// RAM a BIOS set up with one game in one slot, as the MiSTer core's one-slot board leaves it.
    pub(crate) fn ram_with(ngh: u16, name: &str) -> Vec<u8> {
        let mut ram = vec![0u8; 0x10000];
        ram[SIGNATURE_OFFSET..SIGNATURE_OFFSET + SIGNATURE.len()].copy_from_slice(SIGNATURE);
        for slot in 0..SLOTS {
            let at = SLOT_TABLE + slot * 4;
            ram[at..at + 4].copy_from_slice(&[0, 0, 0xFF, 0xFF]);
        }
        let [high, low] = ngh.to_be_bytes();
        ram[SLOT_TABLE..SLOT_TABLE + 4].copy_from_slice(&[high, low, 0, 0]);
        let mut field = [b' '; NAME_LEN];
        field[..name.len()].copy_from_slice(name.as_bytes());
        ram[NAME_TABLE..NAME_TABLE + NAME_LEN].copy_from_slice(&field);
        ram
    }

    #[test]
    fn the_bios_names_the_game_in_its_slot() {
        let games = games(&ram_with(0x0250, "METAL SLUG X"));
        assert_eq!(games, [Game { slot: 0, ngh: 0x0250, name: "METAL SLUG X".into() }]);
    }

    #[test]
    fn ram_the_bios_has_not_set_up_names_nothing() {
        let mut ram = ram_with(0x0250, "METAL SLUG X");
        ram[SIGNATURE_OFFSET] = b'b';
        assert!(!is_set_up(&ram));
        assert_eq!(games(&ram), []);
        assert_eq!(games(&vec![0xFF; 0x10000]), [], "as MiSTer starts a file");
        assert_eq!(games(&[]), []);
    }

    #[test]
    fn a_name_comes_without_its_padding() {
        // V-Liner's, as the BIOS copied it out of the cartridge's header.
        let games = games(&ram_with(0x03E7, " DYNA VLINER"));
        assert_eq!(games, [Game { slot: 0, ngh: 0x03E7, name: "DYNA VLINER".into() }]);
    }

    #[test]
    fn a_name_block_past_the_table_names_no_name() {
        let mut ram = ram_with(0x0047, "FATAL FURY 2");
        ram[SLOT_TABLE + 3] = 9;
        assert_eq!(games(&ram), [Game { slot: 0, ngh: 0x0047, name: String::new() }]);
    }
}
