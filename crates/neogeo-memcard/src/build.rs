//! Writing a card image out of a set of saves.

use crate::{
    BLOCK, CHECKSUM_OFFSET, Container, ENTRY_LEN, Error, FREE_ENTRY, Geometry, HEADER_LEN, MAGIC,
    MAGIC_OFFSET, MISTER_BACKUP_RAM, MISTER_CARD_REGION, MISTER_SAVE, MemoryCard, Result, SIZE_OFFSET, Save,
    USERNAME_OFFSET, fat, fat_checksum, swap_pairs,
};

/// Builds a card image from saves.
///
/// The directory entries and both allocation tables are regenerated, and so is every field of the
/// header that follows from the card being built: its size, the two checksums and the signature.
/// The rest of the header — the username, the region byte, the bytes nothing documents — is passed
/// through from whatever [`system_area`](Self::system_area) was given, so a card round-trips
/// rather than being reformatted. A size copied off a larger card would promise room this one does
/// not have, which is why that one is never passed through.
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
    ///
    /// The card is rebuilt the size of the medium it was read off, which is what its header says
    /// unless it was read with [`MemoryCard::parse_within`].
    #[must_use]
    pub fn from_card(card: &MemoryCard) -> Self {
        CardBuilder {
            geometry: Geometry::of(card.capacity()).expect("a card is read only off a medium of a real size"),
            saves: card.saves().to_vec(),
            system_area: Some(card.system_area().to_vec()),
            container: card.container(),
            backup_ram: card.backup_ram().map(<[u8]>::to_vec),
        }
    }

    /// Keeps the header read off another card, which is [`HEADER_LEN`] bytes.
    ///
    /// Anything longer gives its first block, which takes a card's whole reserved region as it
    /// was kept before. Anything shorter is not a header and is ignored, leaving the card freshly
    /// formatted.
    pub fn system_area(&mut self, bytes: impl Into<Vec<u8>>) -> &mut Self {
        self.system_area = Some(bytes.into());
        self
    }

    /// Writes the card out in this container rather than bare.
    pub fn container(&mut self, container: Container) -> &mut Self {
        self.container = container;
        self
    }

    /// Keeps the cabinet backup RAM that rides in front of the card in a MiSTer save, in the byte
    /// order the 68000 sees.
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
        if self.container == Container::MiSter && self.geometry.size > MISTER_CARD_REGION {
            return Err(Error::TooLargeForContainer { size: self.geometry.size, room: MISTER_CARD_REGION });
        }
        let card = self.build_bare()?;
        match self.container {
            Container::Bare => Ok(card),
            // The core writes the cabinet's backup RAM first, then the card, in a buffer sized for
            // CD mode's 8 KiB card that a cart's 2 KiB one leaves the rest of alone.
            Container::MiSter => join_mister(self.backup_ram.as_deref().unwrap_or_default(), &card),
        }
    }

    /// Writes the bare card, whatever container the result is going into.
    fn build_bare(&self) -> Result<Vec<u8>> {
        let g = self.geometry;
        if self.saves.len() > g.entries {
            return Err(Error::TooManySaves { given: self.saves.len(), available: g.entries });
        }

        let mut card = erased(g.size);
        match &self.system_area {
            Some(area) if area.len() >= HEADER_LEN => card[..HEADER_LEN].copy_from_slice(&area[..HEADER_LEN]),
            _ => card[USERNAME_OFFSET..USERNAME_OFFSET + 16].fill(b' '),
        }
        stamp_header(&mut card, g);

        // Every entry starts free; the saves below claim the ones they need. Only the first byte
        // says whether a slot is used, so the rest of a free entry is left as it was found.
        for index in 0..g.entries {
            card[g.directory_block * BLOCK + index * ENTRY_LEN] = FREE_ENTRY;
        }

        // A console writes each table's blocks whole, the bytes past the last block's entry free.
        let mut fat = vec![fat::FREE; (g.fat2_block - g.fat1_block) * BLOCK];
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

        card[g.fat1_block * BLOCK..][..fat.len()].copy_from_slice(&fat);
        card[g.fat2_block * BLOCK..][..fat.len()].copy_from_slice(&fat);
        let sum = fat_checksum(&fat[..g.fat_len()]);
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
        card[offset..offset + 2].copy_from_slice(&echo(offset));
    }
    card
}

/// What an erased card holds at an even offset: twice the offset, big-endian.
fn echo(offset: usize) -> [u8; 2] {
    u16::try_from((offset * 2) & 0xFFFF).expect("masked to a word").to_be_bytes()
}

/// Writes the header fields that follow from the card being built: its size and its signature.
///
/// The checksums follow from it too, and are written once the tables are.
fn stamp_header(card: &mut [u8], g: Geometry) {
    card[SIZE_OFFSET..SIZE_OFFSET + 2].copy_from_slice(&g.size_word().to_be_bytes());
    // The signature is its characters every other byte; what sits between them is the erased
    // pattern showing through, which is why it reads as a run stepping by four.
    for (i, &c) in MAGIC.iter().enumerate() {
        let at = MAGIC_OFFSET + i * 2;
        card[at..at + 2].copy_from_slice(&echo(at));
        card[at] = c;
    }
}

/// A MiSTer save out of its two halves, each in the byte order the 68000 sees: the inverse of
/// [`split_mister`](crate::split_mister).
///
/// A half shorter than its room is filled out with `0xFF`, which is what the core reads where a
/// file holds nothing. So the region can take a 2 KiB card, an 8 KiB one or a cart's own RAM, and
/// a card with no cabinet RAM beside it gets the RAM a fresh file starts with.
pub fn join_mister(ram: &[u8], region: &[u8]) -> Result<Vec<u8>> {
    for (half, room) in [(ram, MISTER_BACKUP_RAM), (region, MISTER_CARD_REGION)] {
        if half.len() > room {
            return Err(Error::TooLargeForContainer { size: half.len(), room });
        }
    }
    let mut file = vec![0xFF; MISTER_SAVE];
    file[..ram.len()].copy_from_slice(ram);
    file[MISTER_BACKUP_RAM..MISTER_BACKUP_RAM + region.len()].copy_from_slice(region);
    Ok(swap_pairs(&file))
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

    /// A MiSTer save keeps 8 KiB for its card. Up to that, a card goes in and reads back out; past
    /// it, the card is refused rather than copied into a region too short for it.
    #[test]
    fn a_card_goes_in_a_mister_save_only_where_it_fits() {
        for size in [2048, 8192] {
            let mut builder = CardBuilder::new(size).expect("a real size");
            let image = builder.container(Container::MiSter).build().expect("fits");
            assert_eq!(image.len(), crate::MISTER_BACKUP_RAM + MISTER_CARD_REGION);
            let card = MemoryCard::parse(&image).expect("reads what it wrote");
            assert_eq!((card.container(), card.capacity()), (Container::MiSter, size));
        }

        let mut builder = CardBuilder::new(10_240).expect("a real size");
        assert_eq!(
            builder.container(Container::MiSter).build(),
            Err(Error::TooLargeForContainer { size: 10_240, room: MISTER_CARD_REGION })
        );
    }

    /// The core keeps its words low byte first, in the RAM as on the card, and everything here is
    /// in the 68000's order, high byte first.
    #[test]
    fn a_mister_save_s_ram_is_swapped_back_like_its_card() {
        let ram = crate::cabinet::tests::ram_with(0x0250, "METAL SLUG X");
        let mut builder = CardBuilder::new(2048).expect("a real size");
        let file = builder.container(Container::MiSter).backup_ram(ram.clone()).build().expect("writes");
        assert_eq!(&file[0x10..0x14], b"ABKC", "the signature, a byte pair at a time");

        let card = MemoryCard::parse(&file).expect("reads");
        assert_eq!(card.backup_ram(), Some(ram.as_slice()));
        let (split_ram, region) = crate::split_mister(&file).expect("a MiSTer save");
        assert_eq!(split_ram, ram);
        assert_eq!(&region[..2048], card.image());
        assert_eq!(join_mister(&split_ram, &region).expect("fits"), file);
        assert_eq!(join_mister(&[], &[]).expect("fits"), vec![0xFF; crate::MISTER_SAVE], "a fresh file");
    }

    /// A cart's own RAM where the card would be is no card, but the file is still one to read.
    #[test]
    fn a_mister_save_holding_a_cart_s_ram_is_told_by_its_cabinet_ram() {
        let ram = crate::cabinet::tests::ram_with(0x0008, "JOCKEY GRANDPRIX");
        let file = join_mister(&ram, &[0x5A; MISTER_CARD_REGION]).expect("fits");
        assert!(crate::detect(&file), "the BIOS set the RAM up");
        assert!(matches!(MemoryCard::parse(&file), Err(Error::WrongLength(_))), "but it holds no card");
        assert_eq!(crate::split_mister(&file).expect("a MiSTer save").1, vec![0x5A; MISTER_CARD_REGION]);
        assert!(!crate::detect(&join_mister(&[], &[0x5A; MISTER_CARD_REGION]).expect("fits")), "nor that");
    }

    /// The core addresses 2 KiB of card in cart mode, so a card whose header claims 8 KiB is read
    /// as the 2 KiB there are, and comes back out claiming 2 KiB.
    #[test]
    fn a_card_is_the_size_of_the_medium_it_is_read_off() {
        let save = |data: Vec<u8>| Save { slot: 0, sub: 0, ngh: 0x0250, dirent: Vec::new(), data };
        let mut eight = CardBuilder::new(8192).expect("a real size");
        let file = eight.container(Container::MiSter).add(save(vec![7; BLOCK])).build().expect("writes");

        let card = MemoryCard::parse_within(&file, crate::MISTER_CART_CARD).expect("reads");
        assert_eq!((card.capacity(), card.image().len()), (2048, 2048));
        assert_eq!(card.geometry().size, 8192, "what the header claims");
        assert_eq!(card.saves()[0].data, vec![7; BLOCK]);

        let rebuilt =
            MemoryCard::parse(&CardBuilder::from_card(&card).build().expect("writes")).expect("reads");
        assert_eq!(rebuilt.geometry(), Geometry::of(2048).expect("a real size"), "the header now says 2 KiB");
        assert_eq!(rebuilt.saves()[0].data, vec![7; BLOCK]);

        // A save the header placed past the 2 KiB is one the console would have written over the
        // start of the card.
        let mut builder = CardBuilder::new(8192).expect("a real size");
        let file = builder.add(save(vec![1; 40 * BLOCK])).build().expect("writes");
        assert!(matches!(MemoryCard::parse_within(&file, 2048), Err(Error::Corrupt(_))));
        assert_eq!(MemoryCard::parse_within(&file, 3000).err(), Some(Error::UnknownSize(3000)));
    }

    /// The header comes across, and the fields that follow from the card being built are written
    /// for that card, whatever the header they came in says.
    #[test]
    fn a_kept_header_takes_the_size_and_signature_of_the_card_it_lands_on() {
        let mut header =
            MemoryCard::parse(&CardBuilder::new(8192).expect("a real size").build().expect("writes"))
                .expect("reads")
                .system_area()
                .to_vec();
        assert_eq!(header.len(), HEADER_LEN);
        header[USERNAME_OFFSET..USERNAME_OFFSET + 4].copy_from_slice(b"HANS");
        header[crate::REGION_OFFSET] = 2;
        header[MAGIC_OFFSET + 1] = 0x99;

        let mut builder = CardBuilder::new(2048).expect("a real size");
        let built = builder.system_area(header.clone()).build().expect("writes");
        let card = MemoryCard::parse(&built).expect("reads");
        assert_eq!(card.capacity(), 2048, "not the 8 KiB the header came off");
        assert_eq!(&card.username()[..4], b"HANS");
        assert_eq!(card.region(), Region::Europe);
        assert_eq!(built[MAGIC_OFFSET + 1], 0x40, "the signature as a console writes it");

        // A whole reserved region, as a card used to be kept, gives its header.
        let mut legacy = header.clone();
        legacy.extend(vec![0x33; 12 * BLOCK]);
        let mut builder = CardBuilder::new(2048).expect("a real size");
        assert_eq!(builder.system_area(legacy).build().expect("writes"), built);
    }
}
