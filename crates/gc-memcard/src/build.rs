//! Writing a card image out of a set of saves.

use crate::{
    BAT_BLOCK, BAT_MIRROR_BLOCK, BLOCK, CHAIN_END, DIR_BLOCK, DIR_MIRROR_BLOCK, ENTRY_LEN, Error,
    KNOWN_SIZES, MAP_OFFSET, MAX_ENTRIES, RESERVED_BLOCKS, Result, Save, checksum, stamp_directory_checksum,
    stamp_table_checksum,
};

/// Builds a card image from saves.
///
/// The directory, the allocation table, both their mirrors and every checksum are regenerated.
/// Only a save entry's first block and block count are rewritten; the rest of it is the game's and
/// comes through unchanged.
#[derive(Debug, Clone, Default)]
pub struct CardBuilder {
    saves: Vec<Save>,
    header: Option<Vec<u8>>,
    capacity: Option<usize>,
}

impl CardBuilder {
    /// A builder for an empty card, sized to whatever the saves need.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Keeps a header block read off another card, so the card's flash serial survives.
    ///
    /// A block that is not [`BLOCK`] bytes is ignored, since it is not a header block.
    pub fn header_block(&mut self, bytes: impl Into<Vec<u8>>) -> &mut Self {
        self.header = Some(bytes.into());
        self
    }

    /// Asks for a card of this **data** capacity in bytes, as [`MemoryCard::capacity`] reports it.
    ///
    /// A capacity no card comes in is ignored rather than honoured: a console would not accept
    /// one. Without this the smallest card the saves fit on is used.
    ///
    /// [`MemoryCard::capacity`]: crate::MemoryCard::capacity
    pub fn capacity(&mut self, bytes: usize) -> &mut Self {
        self.capacity = Some(bytes);
        self
    }

    /// Adds a save. Its `slot` is ignored: the directory is filled in the order saves are added.
    pub fn add(&mut self, save: Save) -> &mut Self {
        self.saves.push(save);
        self
    }

    /// Writes the card out.
    pub fn build(&self) -> Result<Vec<u8>> {
        if self.saves.len() > MAX_ENTRIES {
            return Err(Error::TooManySaves { given: self.saves.len(), available: MAX_ENTRIES });
        }

        // Take the stated size when it is one cards come in, otherwise the smallest that fits: a
        // capacity nothing makes is not a card a console would accept.
        let needed: usize = self.saves.iter().map(|save| save.data.len().div_ceil(BLOCK)).sum();
        let stated = self.capacity.map(|capacity| capacity / BLOCK + RESERVED_BLOCKS);
        let blocks = stated
            .filter(|size| KNOWN_SIZES.contains(size))
            .or_else(|| KNOWN_SIZES.into_iter().find(|size| size - RESERVED_BLOCKS >= needed))
            .ok_or(Error::Full { needed, available: KNOWN_SIZES[KNOWN_SIZES.len() - 1] - RESERVED_BLOCKS })?;

        if needed > blocks - RESERVED_BLOCKS {
            return Err(Error::Full { needed, available: blocks - RESERVED_BLOCKS });
        }

        let mut card = vec![0xFFu8; blocks * BLOCK];
        let mut directory = vec![0xFFu8; BLOCK];
        let mut table = vec![0u8; BLOCK];

        let mut next_block = RESERVED_BLOCKS;
        for (slot, save) in self.saves.iter().enumerate() {
            let count = save.data.len().div_ceil(BLOCK);
            let first = next_block;

            for step in 0..count {
                let current = first + step;
                let from = step * BLOCK;
                let to = ((step + 1) * BLOCK).min(save.data.len());
                card[current * BLOCK..current * BLOCK + (to - from)].copy_from_slice(&save.data[from..to]);

                let link =
                    if step + 1 == count { CHAIN_END } else { u16::try_from(current + 1).expect("fits") };
                let at = MAP_OFFSET + (current - RESERVED_BLOCKS) * 2;
                table[at..at + 2].copy_from_slice(&link.to_be_bytes());
            }

            let mut entry = directory_entry(save);
            entry.resize(ENTRY_LEN, 0);
            entry[0x36..0x38].copy_from_slice(&u16::try_from(first).expect("block fits").to_be_bytes());
            entry[0x38..0x3A].copy_from_slice(&u16::try_from(count).expect("count fits").to_be_bytes());
            directory[slot * ENTRY_LEN..(slot + 1) * ENTRY_LEN].copy_from_slice(&entry);

            next_block += count;
        }

        // The table's own head: an update counter, how many blocks are free, and the last one
        // handed out. Both copies of each region start at the same counter, since neither is stale.
        let free = u16::try_from(blocks - next_block).expect("free count fits");
        table[4..6].copy_from_slice(&0u16.to_be_bytes());
        table[6..8].copy_from_slice(&free.to_be_bytes());
        table[8..10].copy_from_slice(&u16::try_from(next_block - 1).expect("block fits").to_be_bytes());
        directory[BLOCK - 6..BLOCK - 4].copy_from_slice(&0u16.to_be_bytes());

        stamp_directory_checksum(&mut directory);
        stamp_table_checksum(&mut table);

        let header = match &self.header {
            Some(area) if area.len() == BLOCK => area.clone(),
            _ => default_header(blocks),
        };
        card[..BLOCK].copy_from_slice(&header);
        card[DIR_BLOCK * BLOCK..(DIR_BLOCK + 1) * BLOCK].copy_from_slice(&directory);
        card[DIR_MIRROR_BLOCK * BLOCK..(DIR_MIRROR_BLOCK + 1) * BLOCK].copy_from_slice(&directory);
        card[BAT_BLOCK * BLOCK..(BAT_BLOCK + 1) * BLOCK].copy_from_slice(&table);
        card[BAT_MIRROR_BLOCK * BLOCK..(BAT_MIRROR_BLOCK + 1) * BLOCK].copy_from_slice(&table);
        Ok(card)
    }
}

/// The directory entry a save gets: its own, or one carrying its codes and filename.
fn directory_entry(save: &Save) -> Vec<u8> {
    if !save.dirent.is_empty() {
        return save.dirent.clone();
    }
    let mut entry = vec![0u8; ENTRY_LEN];
    // A code field a save did not state is written as `-`, which is what the console shows for a
    // save whose game it cannot name.
    entry[..6].fill(b'-');
    let code = save.game_code.as_bytes();
    entry[..code.len().min(4)].copy_from_slice(&code[..code.len().min(4)]);
    let maker = save.maker_code.as_bytes();
    entry[4..4 + maker.len().min(2)].copy_from_slice(&maker[..maker.len().min(2)]);
    entry[6] = 0xFF;
    let name = save.filename.as_bytes();
    let len = name.len().min(32);
    entry[8..8 + len].copy_from_slice(&name[..len]);
    entry[0x3A..0x3C].copy_from_slice(&0xFFFFu16.to_be_bytes());
    entry
}

/// A freshly formatted header block.
pub(crate) fn default_header(blocks: usize) -> Vec<u8> {
    let mut header = vec![0xFFu8; BLOCK];
    // The size the console reports, in megabits.
    let megabits = u16::try_from(blocks * BLOCK / (1024 * 1024 / 8)).unwrap_or(4);
    header[0x20..0x22].copy_from_slice(&megabits.to_be_bytes());
    // Encoding 0 is ASCII; 1 would be Shift-JIS.
    header[0x22..0x24].copy_from_slice(&0u16.to_be_bytes());
    let (sum, inverse) = checksum(&header[..0x1FC]);
    header[0x1FC..0x1FE].copy_from_slice(&sum.to_be_bytes());
    header[0x1FE..0x200].copy_from_slice(&inverse.to_be_bytes());
    header
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MemoryCard, tests::card_with};

    fn round_trip(card: &[u8]) -> Vec<u8> {
        let parsed = MemoryCard::parse(card).expect("reads");
        let mut builder = CardBuilder::new();
        builder.header_block(parsed.header_block().to_vec()).capacity(parsed.capacity());
        for save in parsed.saves() {
            builder.add(save.clone());
        }
        builder.build().expect("writes")
    }

    #[test]
    fn a_card_a_console_wrote_is_rebuilt_byte_for_byte() {
        let card = card_with(64, &[("GAFE01", "super_mario_sunshine", 2), ("GM4E01", "PSO_SYSTEM", 1)]);
        assert_eq!(round_trip(&card), card);
    }

    #[test]
    fn a_save_built_by_hand_gets_an_entry_carrying_its_codes() {
        let mut builder = CardBuilder::new();
        builder.add(Save {
            slot: 0,
            game_code: "GAFE".into(),
            maker_code: "01".into(),
            filename: "super_mario_sunshine".into(),
            dirent: Vec::new(),
            data: vec![5u8; BLOCK],
        });
        let card = builder.build().expect("writes");

        let parsed = MemoryCard::parse(&card).expect("reads what it wrote");
        assert_eq!(parsed.saves().len(), 1);
        assert_eq!(parsed.saves()[0].game_code, "GAFE");
        assert_eq!(parsed.saves()[0].maker_code, "01");
        assert_eq!(parsed.saves()[0].filename, "super_mario_sunshine");
        assert_eq!(parsed.saves()[0].data, vec![5u8; BLOCK]);
    }

    #[test]
    fn a_capacity_no_card_comes_in_is_ignored_rather_than_honoured() {
        let mut builder = CardBuilder::new();
        builder.capacity(12_345).add(Save {
            slot: 0,
            game_code: "GAFE".into(),
            maker_code: "01".into(),
            filename: "x".into(),
            dirent: Vec::new(),
            data: vec![0u8; BLOCK],
        });
        let card = builder.build().expect("writes");
        // Falls back to the smallest card the save fits on rather than making up a size.
        assert_eq!(card.len(), 64 * BLOCK);
    }

    #[test]
    fn refuses_saves_that_do_not_fit_the_card_they_asked_for() {
        let mut builder = CardBuilder::new();
        builder.capacity((64 - RESERVED_BLOCKS) * BLOCK).add(Save {
            slot: 0,
            game_code: "GAFE".into(),
            maker_code: "01".into(),
            filename: "big".into(),
            dirent: Vec::new(),
            data: vec![0u8; 100 * BLOCK],
        });
        assert!(matches!(builder.build(), Err(Error::Full { needed: 100, available: 59 })));
    }
}
