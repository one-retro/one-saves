//! The superblock: what the card says about its own geometry.

use crate::{Error, MAGIC, Result};

/// The card's geometry and the whereabouts of its allocation table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuperBlock {
    /// The version string the formatter wrote, such as `1.2.0.0`.
    pub version: String,
    /// Bytes per page, always 512 in practice.
    pub page_size: usize,
    /// Pages per cluster, always 2 in practice.
    pub pages_per_cluster: usize,
    /// Pages per erase block, always 16 in practice.
    pub pages_per_block: usize,
    /// How many clusters the whole card holds, including the ones the FAT itself occupies.
    pub clusters_per_card: usize,
    /// The first allocatable cluster, counted from the start of the card.
    ///
    /// Every cluster number in a directory entry or a FAT chain is relative to this, which is why
    /// reading one means adding it back.
    pub alloc_offset: usize,
    /// How many clusters are allocatable.
    pub alloc_end: usize,
    /// Where the root directory starts, relative to [`alloc_offset`](Self::alloc_offset).
    pub rootdir_cluster: usize,
    /// The clusters holding the indirect FAT, which in turn names the FAT clusters.
    pub ifc_list: Vec<u32>,
    /// The two erase blocks reserved at the end of the card for the write-backup mechanism.
    pub backup_block1: u32,
    /// The second reserved block.
    pub backup_block2: u32,
    /// 2 for a PS2 card.
    pub card_type: u8,
    /// Formatter flags, including whether the card carries ECC.
    pub card_flags: u8,
}

impl SuperBlock {
    /// Bytes per cluster.
    #[must_use]
    pub fn cluster_size(&self) -> usize {
        self.page_size * self.pages_per_cluster
    }

    /// How many 32-bit entries fit in one cluster, which is the FAT's branching factor.
    #[must_use]
    pub fn entries_per_cluster(&self) -> usize {
        self.cluster_size() / 4
    }

    /// Reads a superblock off the front of a card.
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < 0x154 {
            return Err(Error::TooShort(data.len()));
        }
        if !data.starts_with(MAGIC) {
            return Err(Error::NotAMemoryCard);
        }

        let u16_at = |offset: usize| u16::from_le_bytes([data[offset], data[offset + 1]]) as usize;
        let u32_at = |offset: usize| {
            u32::from_le_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]])
        };

        let version_field = &data[0x1C..0x28];
        let end = version_field.iter().position(|&b| b == 0).unwrap_or(version_field.len());

        let page_size = u16_at(0x28);
        let pages_per_cluster = u16_at(0x2A);
        if page_size == 0 || pages_per_cluster == 0 {
            return Err(Error::Corrupt("the superblock states a zero page or cluster size".into()));
        }

        let ifc_list = (0..32).map(|i| u32_at(0x50 + i * 4)).collect();

        let superblock = SuperBlock {
            version: String::from_utf8_lossy(&version_field[..end]).trim().to_owned(),
            page_size,
            pages_per_cluster,
            pages_per_block: u16_at(0x2C),
            clusters_per_card: u32_at(0x30) as usize,
            alloc_offset: u32_at(0x34) as usize,
            alloc_end: u32_at(0x38) as usize,
            rootdir_cluster: u32_at(0x3C) as usize,
            ifc_list,
            backup_block1: u32_at(0x40),
            backup_block2: u32_at(0x44),
            card_type: data[0x150],
            card_flags: data[0x151],
        };

        // A card whose allocated area runs off the end of itself would otherwise be caught one
        // confusing chain read later.
        if superblock.alloc_offset + superblock.alloc_end > superblock.clusters_per_card {
            return Err(Error::Corrupt(format!(
                "the allocated area runs to cluster {}, past the {} the card has",
                superblock.alloc_offset + superblock.alloc_end,
                superblock.clusters_per_card
            )));
        }
        Ok(superblock)
    }

    /// Writes the superblock into the front of a card image.
    pub(crate) fn write_into(&self, data: &mut [u8]) {
        data[..MAGIC.len()].copy_from_slice(MAGIC);
        let version = self.version.as_bytes();
        let len = version.len().min(12);
        data[0x1C..0x1C + len].copy_from_slice(&version[..len]);

        let put16 = |data: &mut [u8], offset: usize, value: usize| {
            let value = u16::try_from(value).expect("a geometry field fits in 16 bits");
            data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
        };
        let put32 = |data: &mut [u8], offset: usize, value: u32| {
            data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        };

        put16(data, 0x28, self.page_size);
        put16(data, 0x2A, self.pages_per_cluster);
        put16(data, 0x2C, self.pages_per_block);
        // The formatter leaves this pair set, and emulators check it on some builds.
        put16(data, 0x2E, 0xFF00);
        put32(data, 0x30, u32::try_from(self.clusters_per_card).expect("a geometry field fits in 32 bits"));
        put32(data, 0x34, u32::try_from(self.alloc_offset).expect("a geometry field fits in 32 bits"));
        put32(data, 0x38, u32::try_from(self.alloc_end).expect("a geometry field fits in 32 bits"));
        put32(data, 0x3C, u32::try_from(self.rootdir_cluster).expect("a geometry field fits in 32 bits"));
        put32(data, 0x40, self.backup_block1);
        put32(data, 0x44, self.backup_block2);
        for (index, &cluster) in self.ifc_list.iter().enumerate().take(32) {
            put32(data, 0x50 + index * 4, cluster);
        }
        // The bad-block list is all-clear on a card this crate wrote.
        for index in 0..32 {
            put32(data, 0xD0 + index * 4, 0xFFFF_FFFF);
        }
        data[0x150] = self.card_type;
        data[0x151] = self.card_flags;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Capacity, CardBuilder};

    #[test]
    fn refuses_something_that_is_not_a_card() {
        assert!(matches!(SuperBlock::parse(&[0u8; 1024]), Err(Error::NotAMemoryCard)));
    }

    #[test]
    fn a_written_superblock_reads_back_the_same() {
        let card = CardBuilder::new(Capacity::Mb8).build().unwrap();
        let superblock = SuperBlock::parse(&card).unwrap();

        // The geometry every retail 8 MB card is formatted to.
        assert_eq!(superblock.page_size, 512);
        assert_eq!(superblock.pages_per_cluster, 2);
        assert_eq!(superblock.pages_per_block, 16);
        assert_eq!(superblock.clusters_per_card, 8192);
        assert_eq!(superblock.card_type, 2);
        assert_eq!(superblock.cluster_size(), 1024);
        assert_eq!(superblock.entries_per_cluster(), 256);
        assert!(superblock.alloc_offset + superblock.alloc_end <= superblock.clusters_per_card);
    }
}
