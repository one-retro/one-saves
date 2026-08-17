//! Writing a card image out of a set of saves.

use crate::{
    BLOCK, CHECKSUM_OFFSET, Container, ENTRY_LEN, Error, FREE_ENTRY, Geometry, MISTER_BACKUP_RAM,
    MISTER_CARD_REGION, MemoryCard, Result, Save, fat, fat_checksum,
};

/// Builds a card image from saves.
///
/// The directory entries and both allocation tables are regenerated, along with the two checksums
/// the header carries for them. Everything else in the reserved region — the signature, the
/// username, the region byte, the bytes free directory slots happen to hold — is passed through
/// from whatever [`system_area`](Self::system_area) was given, so a card round-trips rather than
/// being reformatted.
#[derive(Debug, Clone)]
pub struct CardBuilder {
    geometry: Geometry,
    saves: Vec<Save>,
    system_area: Option<Vec<u8>>,
    container: Container,
    backup_ram: Option<Vec<u8>>,
}

impl CardBuilder {
    /// A builder for an empty card of this size in bytes.
    ///
    /// The size must be one cards come in; see [`SIZES`](crate::SIZES).
    pub fn new(size: usize) -> Result<Self> {
        Ok(CardBuilder {
            geometry: Geometry::of(size).ok_or(Error::UnknownSize(size))?,
            saves: Vec::new(),
            system_area: None,
            container: Container::Bare,
            backup_ram: None,
        })
    }

    /// A builder set up to rebuild the card this was read from, container and all.
    #[must_use]
    pub fn from_card(card: &MemoryCard) -> Self {
        CardBuilder {
            geometry: card.geometry(),
            saves: card.saves().to_vec(),
            system_area: Some(card.system_area().to_vec()),
            container: card.container(),
            backup_ram: card.backup_ram().map(<[u8]>::to_vec),
        }
    }

    /// Keeps the reserved region read off another card: header, directory and both tables.
    ///
    /// A region that is not the right length for this card's geometry is ignored, since it is not
    /// this card's reserved region.
    pub fn system_area(&mut self, bytes: impl Into<Vec<u8>>) -> &mut Self {
        self.system_area = Some(bytes.into());
        self
    }

    /// Writes the card out in this container rather than bare.
    pub fn container(&mut self, container: Container) -> &mut Self {
        self.container = container;
        self
    }

    /// Keeps the cabinet backup RAM that rides in front of the card in a MiSTer save.
    pub fn backup_ram(&mut self, bytes: impl Into<Vec<u8>>) -> &mut Self {
        self.backup_ram = Some(bytes.into());
        self
    }

    /// Adds a save. Its `slot` is ignored: the directory is filled in the order saves are added.
    pub fn add(&mut self, save: Save) -> &mut Self {
        self.saves.push(save);
        self
    }

    /// Writes the card out, in whatever container was asked for.
    pub fn build(&self) -> Result<Vec<u8>> {
        let card = self.build_bare()?;
        match self.container {
            Container::Bare => Ok(card),
            // The core writes the cabinet's backup RAM first, then the card with the two bytes of
            // every 16-bit word swapped, in a region twice the card's length.
            Container::MiSter => {
                let mut out = match &self.backup_ram {
                    Some(ram) if ram.len() == MISTER_BACKUP_RAM => ram.clone(),
                    _ => vec![0xFF; MISTER_BACKUP_RAM],
                };
                // The buffer is 8 KiB whatever the card is: the core sizes it for CD mode and a
                // cart's 2 KiB card simply leaves the rest of it alone.
                let region_len = MISTER_CARD_REGION;
                let mut region = vec![0xFF; region_len];
                region[..card.len()].copy_from_slice(&card);
                out.extend((0..region_len).map(|i| region[i ^ 1]));
                Ok(out)
            }
        }
    }

    /// Writes the bare card, whatever container the result is going into.
    fn build_bare(&self) -> Result<Vec<u8>> {
        let g = self.geometry;
        if self.saves.len() > g.entries {
            return Err(Error::TooManySaves { given: self.saves.len(), available: g.entries });
        }

        let mut card = erased(g.size);
        if let Some(area) = &self.system_area
            && area.len() == g.first_data_block * BLOCK
        {
            card[..area.len()].copy_from_slice(area);
        } else {
            format_header(&mut card, g);
        }

        // Every entry starts free; the saves below claim the ones they need. Only the first byte
        // says whether a slot is used, so the rest of a free entry is left as it was found.
        for index in 0..g.entries {
            card[g.directory_block * BLOCK + index * ENTRY_LEN] = FREE_ENTRY;
        }

        let mut fat = vec![fat::FREE; g.fat_len()];
        for entry in fat.iter_mut().take(g.first_data_block) {
            *entry = fat::RESERVED;
        }

        let mut next_block = g.first_data_block;
        for (slot, save) in self.saves.iter().enumerate() {
            let blocks = save.data.len().div_ceil(BLOCK);
            if blocks == 0 {
                return Err(Error::EmptySave(save.ngh));
            }
            if next_block + blocks > g.blocks {
                return Err(Error::Full {
                    needed: next_block - g.first_data_block + blocks,
                    available: g.blocks - g.first_data_block,
                });
            }

            let first = next_block;
            let start = first * BLOCK;
            card[start..start + save.data.len()].copy_from_slice(&save.data);
            // The table is a chain: each block points at the next and the last says so. Blocks are
            // handed out in a run here, but nothing about the format requires that — a reader
            // follows the links rather than assuming they are adjacent.
            for step in 0..blocks {
                let block = first + step;
                fat[block] = if step + 1 == blocks {
                    fat::CHAIN_END
                } else {
                    u8::try_from(block + 1).expect("a block index fits")
                };
            }

            // The entry's first three bytes are the game's; only where the save landed is the
            // allocator's, which is why the first block is rewritten and nothing else is.
            let mut entry = if save.dirent.len() == ENTRY_LEN {
                save.dirent.clone()
            } else {
                let [hi, lo] = save.ngh.to_be_bytes();
                vec![save.sub, hi, lo, 0]
            };
            entry[3] = u8::try_from(first).expect("a block index fits");
            let at = g.directory_block * BLOCK + slot * ENTRY_LEN;
            card[at..at + ENTRY_LEN].copy_from_slice(&entry);

            next_block += blocks;
        }

        card[g.fat1_block * BLOCK..][..g.fat_len()].copy_from_slice(&fat);
        card[g.fat2_block * BLOCK..][..g.fat_len()].copy_from_slice(&fat);
        let sum = fat_checksum(&fat);
        card[CHECKSUM_OFFSET] = sum;
        card[CHECKSUM_OFFSET + 1] = sum;
        Ok(card)
    }
}

/// A card with nothing written to it.
///
/// Not zeros: a block no save occupies reads back **twice its own offset**, big-endian, so the run
/// at `$180` is `03 00 03 04 03 08`. That is the address the byte answers to showing through, which
/// is the same thing the signature's filler bytes are. It holds to the last byte of the card on
/// every dump here, at both card sizes.
///
/// Reproducing it is what lets a card a console wrote come back byte for byte, since the blocks a
/// save does not occupy are full of it.
fn erased(size: usize) -> Vec<u8> {
    let mut card = vec![0u8; size];
    for offset in (0..size).step_by(2) {
        let echo = u16::try_from((offset * 2) & 0xFFFF).expect("masked to a word").to_be_bytes();
        card[offset..offset + 2].copy_from_slice(&echo);
    }
    card
}

/// Writes a freshly formatted header over an erased card: signature, size and username.
fn format_header(card: &mut [u8], g: Geometry) {
    card[crate::SIZE_OFFSET..crate::SIZE_OFFSET + 2].copy_from_slice(&g.size_word().to_be_bytes());
    // The signature is its characters every other byte; what sits between them is the erased
    // pattern showing through, which is why it reads as a run stepping by four.
    for (i, &c) in crate::MAGIC.iter().enumerate() {
        card[crate::MAGIC_OFFSET + i * 2] = c;
    }
    card[crate::USERNAME_OFFSET..crate::USERNAME_OFFSET + 16].fill(b' ');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MemoryCard, Region};

    #[test]
    fn a_card_built_from_nothing_reads_back() {
        let mut builder = CardBuilder::new(4096).expect("a real size");
        builder.add(Save {
            slot: 0,
            sub: 0,
            ngh: 0x0250,
            dirent: Vec::new(),
            data: b"METAL SLUG X        ".iter().copied().chain(std::iter::repeat_n(0, 44)).collect(),
        });
        let image = builder.build().expect("writes");

        let card = MemoryCard::parse(&image).expect("reads what it wrote");
        assert_eq!(card.capacity(), 4096);
        let first_data = Geometry::of(4096).expect("a real size").first_data_block;
        assert_eq!(card.region(), Region::Japan, "a formatted card states region 0");
        assert_eq!(card.saves().len(), 1);
        assert_eq!(card.saves()[0].ngh, 0x0250);
        assert_eq!(card.saves()[0].title(), "METAL SLUG X");
        assert_eq!(usize::from(card.saves()[0].dirent[3]), first_data, "the first data block");
    }

    /// A save's blocks are wherever its chain leads, and nothing requires them to be adjacent.
    /// This builder hands out a run, so the layout is rewritten by hand into 13 → 20 → 14 to check
    /// that the reader follows the links rather than walking forward from the first block.
    #[test]
    fn a_chain_is_followed_rather_than_assumed_to_be_a_run() {
        let mut builder = CardBuilder::new(8192).expect("a real size");
        builder.add(Save {
            slot: 0,
            sub: 0,
            ngh: 0x0201,
            dirent: Vec::new(),
            data: (0..3 * BLOCK).map(|b| u8::try_from(b & 0xFF).expect("a byte")).collect(),
        });
        let mut card = builder.build().expect("writes");
        let g = Geometry::of(8192).expect("a real size");
        assert_eq!(g.first_data_block, 13, "the geometry two real CD cards confirm");

        // Move the middle block of the chain out to block 20 and relink around it.
        let (from, to) = (14 * BLOCK, 20 * BLOCK);
        let middle: Vec<u8> = card[from..from + BLOCK].to_vec();
        card[to..to + BLOCK].copy_from_slice(&middle);
        card[from..from + BLOCK].fill(0);
        for fat_block in [g.fat1_block, g.fat2_block] {
            let fat = fat_block * BLOCK;
            card[fat + 13] = 20; // first block now points past the gap
            card[fat + 20] = 14; // which points back to the third
            card[fat + 14] = fat::CHAIN_END;
        }

        let parsed = MemoryCard::parse(&card).expect("reads");
        assert_eq!(parsed.saves().len(), 1);
        let data = &parsed.saves()[0].data;
        assert_eq!(data.len(), 3 * BLOCK, "all three blocks, wherever they sit");
        // Block order follows the chain, so the payload is the same bytes it started as.
        assert_eq!(data[..BLOCK], (0..BLOCK).map(|b| u8::try_from(b).expect("a byte")).collect::<Vec<_>>());
        assert_eq!(data[BLOCK..2 * BLOCK], middle[..]);
    }

    /// A chain that runs into a block no save holds is broken, and saying so beats handing back a
    /// payload with somebody else's bytes in it.
    #[test]
    fn a_chain_into_a_free_block_is_refused() {
        let mut builder = CardBuilder::new(8192).expect("a real size");
        builder.add(Save { slot: 0, sub: 0, ngh: 1, dirent: Vec::new(), data: vec![7u8; 2 * BLOCK] });
        let mut card = builder.build().expect("writes");
        let g = Geometry::of(8192).expect("a real size");
        for fat_block in [g.fat1_block, g.fat2_block] {
            card[fat_block * BLOCK + 14] = fat::FREE;
        }
        assert!(matches!(MemoryCard::parse(&card), Err(Error::Corrupt(_))));
    }

    #[test]
    fn a_save_spanning_two_blocks_comes_back_whole() {
        let mut builder = CardBuilder::new(4096).expect("a real size");
        builder
            .add(Save { slot: 0, sub: 0, ngh: 0x0047, dirent: Vec::new(), data: vec![1u8; 2 * BLOCK] })
            .add(Save { slot: 0, sub: 1, ngh: 0x0250, dirent: Vec::new(), data: vec![2u8; BLOCK] });
        let card = MemoryCard::parse(&builder.build().expect("writes")).expect("reads");

        assert_eq!(card.saves().len(), 2);
        assert_eq!(card.saves()[0].data, vec![1u8; 2 * BLOCK]);
        assert_eq!(card.saves()[1].data, vec![2u8; BLOCK]);
        let first_data = Geometry::of(4096).expect("a real size").first_data_block;
        assert_eq!(
            usize::from(card.saves()[1].dirent[3]),
            first_data + 2,
            "the second save starts after the first's two blocks"
        );
    }

    #[test]
    fn refuses_what_will_not_fit() {
        let mut builder = CardBuilder::new(2048).expect("a real size");
        builder.add(Save { slot: 0, sub: 0, ngh: 1, dirent: Vec::new(), data: vec![0u8; 4096] });
        assert!(matches!(builder.build(), Err(Error::Full { .. })));

        let mut empty = CardBuilder::new(4096).expect("a real size");
        empty.add(Save { slot: 0, sub: 0, ngh: 9, dirent: Vec::new(), data: Vec::new() });
        assert!(matches!(empty.build(), Err(Error::EmptySave(9))));

        assert!(matches!(CardBuilder::new(3000), Err(Error::UnknownSize(3000))));
    }
}
