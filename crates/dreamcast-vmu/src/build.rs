//! Writing a VMU image out of a set of saves.

use crate::{
    BLOCK, BLOCKS, CHAIN_END, DIR_BLOCKS, DIR_FIRST_BLOCK, ENTRY_LEN, Error, FAT_BLOCK, FREE, FileKind,
    IMAGE_LEN, MAX_ENTRIES, ROOT_BLOCK, Result, Save, USER_BLOCKS, entry_offset,
};

/// Builds a VMU image from saves.
///
/// The allocation table and the directory are regenerated. Only an entry's first block, block
/// count and type byte are rewritten; its name and timestamp are the game's and come through
/// unchanged.
#[derive(Debug, Clone, Default)]
pub struct VmuBuilder {
    saves: Vec<Save>,
    root: Option<Vec<u8>>,
}

impl VmuBuilder {
    /// A builder for an empty VMU.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Keeps a root block read off another VMU, so its colour and icon survive.
    ///
    /// A block that is not [`BLOCK`] bytes is ignored, since it is not a root block.
    pub fn root_block(&mut self, bytes: impl Into<Vec<u8>>) -> &mut Self {
        self.root = Some(bytes.into());
        self
    }

    /// Adds a save. Its `slot` is ignored: the directory is filled in the order saves are added.
    pub fn add(&mut self, save: Save) -> &mut Self {
        self.saves.push(save);
        self
    }

    /// Writes the VMU out, all 128 KiB of it.
    pub fn build(&self) -> Result<Vec<u8>> {
        if self.saves.len() > MAX_ENTRIES {
            return Err(Error::TooManySaves { given: self.saves.len(), available: MAX_ENTRIES });
        }

        let mut image = vec![0u8; IMAGE_LEN];

        // Everything starts free, and the directory starts empty.
        let mut fat = [FREE; BLOCKS];
        for entry in &mut fat[USER_BLOCKS..] {
            *entry = CHAIN_END;
        }

        // A mini-game has to start at block 0 and stay contiguous, so it is placed before anything
        // else. Ordinary saves take whatever is left. The directory index stays the order saves
        // were added in; only where they land is reordered.
        let mut order: Vec<usize> = (0..self.saves.len()).collect();
        order.sort_by_key(|&index| self.saves[index].kind != FileKind::Game);

        let mut next_block = 0usize;
        for &index in &order {
            let save = &self.saves[index];
            let blocks = save.data.len().div_ceil(BLOCK);
            if blocks == 0 {
                return Err(Error::EmptySave(save.name.clone()));
            }
            if next_block + blocks > USER_BLOCKS {
                return Err(Error::Full { needed: next_block + blocks, available: USER_BLOCKS });
            }

            let first = next_block;
            for step in 0..blocks {
                let current = first + step;
                let from = step * BLOCK;
                let to = ((step + 1) * BLOCK).min(save.data.len());
                image[current * BLOCK..current * BLOCK + (to - from)].copy_from_slice(&save.data[from..to]);
                fat[current] = if step + 1 == blocks {
                    CHAIN_END
                } else {
                    u16::try_from(current + 1).expect("block index fits")
                };
            }

            let mut entry = directory_entry(save);
            entry.resize(ENTRY_LEN, 0);
            entry[0] = save.kind.as_byte();
            entry[2..4].copy_from_slice(&u16::try_from(first).expect("block fits").to_le_bytes());
            entry[0x18..0x1A].copy_from_slice(&u16::try_from(blocks).expect("count fits").to_le_bytes());

            let at = entry_offset(index);
            image[at..at + ENTRY_LEN].copy_from_slice(&entry);
            next_block += blocks;
        }

        // The root block: kept if one was given, otherwise formatted from nothing.
        let root = match &self.root {
            Some(area) if area.len() == BLOCK => area.clone(),
            _ => default_root_block(),
        };
        image[ROOT_BLOCK * BLOCK..].copy_from_slice(&root);

        for (index, entry) in fat.iter().enumerate() {
            let at = FAT_BLOCK * BLOCK + index * 2;
            image[at..at + 2].copy_from_slice(&entry.to_le_bytes());
        }
        Ok(image)
    }
}

/// The directory entry a save gets: its own, or one carrying just the name.
fn directory_entry(save: &Save) -> Vec<u8> {
    if !save.dirent.is_empty() {
        return save.dirent.clone();
    }
    let mut entry = vec![0u8; ENTRY_LEN];
    let name = save.name.as_bytes();
    let len = name.len().min(12);
    entry[4..4 + len].copy_from_slice(&name[..len]);
    entry
}

/// A freshly formatted root block.
pub(crate) fn default_root_block() -> Vec<u8> {
    let mut root = vec![0u8; BLOCK];
    root[..16].fill(0x55);
    // Where the table and the directory live, which the console reads rather than assumes.
    root[0x40..0x42].copy_from_slice(&u16::try_from(FAT_BLOCK).expect("fits").to_le_bytes());
    root[0x42..0x44].copy_from_slice(&1u16.to_le_bytes());
    root[0x44..0x46].copy_from_slice(&u16::try_from(DIR_FIRST_BLOCK).expect("fits").to_le_bytes());
    root[0x46..0x48].copy_from_slice(&u16::try_from(DIR_BLOCKS).expect("fits").to_le_bytes());
    root[0x4A..0x4C].copy_from_slice(&u16::try_from(USER_BLOCKS).expect("fits").to_le_bytes());
    root
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Vmu, tests::vmu_with};

    fn round_trip(image: &[u8]) -> Vec<u8> {
        let parsed = Vmu::parse(image).expect("reads");
        let mut builder = VmuBuilder::new();
        builder.root_block(parsed.root_block().to_vec());
        for save in parsed.saves() {
            builder.add(save.clone());
        }
        builder.build().expect("writes")
    }

    #[test]
    fn a_vmu_a_console_wrote_is_rebuilt_byte_for_byte() {
        let image = vmu_with(&[("SONIC2___S01", 2, FileKind::Data), ("PSO_______", 1, FileKind::Data)]);
        assert_eq!(round_trip(&image), image);
    }

    #[test]
    fn a_minigame_is_placed_at_block_zero() {
        // A mini-game executes in place from flash, so it cannot be put anywhere else. Here the
        // data save is listed first, and the writer still has to put the game at block 0.
        let image = vmu_with(&[("DATA________", 1, FileKind::Data), ("MINIGAME____", 3, FileKind::Game)]);
        let rebuilt = round_trip(&image);

        let reread = Vmu::parse(&rebuilt).expect("reads back");
        let game = reread.saves().iter().find(|save| save.kind == FileKind::Game).expect("survived");
        assert_eq!(u16::from_le_bytes([game.dirent[2], game.dirent[3]]), 0, "a mini-game starts at block 0");
        // The directory index is still the order the saves were added in.
        assert_eq!(game.slot, 1);
    }

    #[test]
    fn a_save_built_by_hand_gets_an_entry_carrying_its_name() {
        let mut builder = VmuBuilder::new();
        builder.add(Save {
            slot: 0,
            kind: FileKind::Data,
            name: "SONIC2___S01".into(),
            dirent: Vec::new(),
            data: vec![3u8; BLOCK],
        });
        let image = builder.build().expect("writes");

        let parsed = Vmu::parse(&image).expect("reads what it wrote");
        assert_eq!(parsed.saves().len(), 1);
        assert_eq!(parsed.saves()[0].name, "SONIC2___S01");
        assert_eq!(parsed.saves()[0].kind, FileKind::Data);
        assert_eq!(parsed.saves()[0].data, vec![3u8; BLOCK]);
    }

    #[test]
    fn refuses_saves_that_do_not_fit() {
        let mut builder = VmuBuilder::new();
        for index in 0..3 {
            builder.add(Save {
                slot: 0,
                kind: FileKind::Data,
                name: format!("BIG{index}"),
                dirent: Vec::new(),
                data: vec![0u8; 100 * BLOCK],
            });
        }
        assert!(matches!(builder.build(), Err(Error::Full { needed: 300, available: 200 })));
    }

    #[test]
    fn refuses_a_save_with_no_bytes() {
        let mut builder = VmuBuilder::new();
        builder.add(Save {
            slot: 0,
            kind: FileKind::Data,
            name: "EMPTY".into(),
            dirent: Vec::new(),
            data: Vec::new(),
        });
        assert!(matches!(builder.build(), Err(Error::EmptySave(_))));
    }
}
