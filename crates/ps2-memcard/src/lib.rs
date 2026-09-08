//! Reading and writing PlayStation 2 memory card images.
//!
//! A PS2 card is not a DOS FAT volume, though it rhymes with one. It has Sony's own superblock,
//! a cluster-allocation table reached through **two** levels of indirection, 512-byte directory
//! entries, and an error-correcting code in a spare area attached to every page. No FAT library
//! reads it.
//!
//! ```no_run
//! use ps2_memcard::MemoryCard;
//!
//! let card = MemoryCard::parse(&std::fs::read("card.ps2")?)?;
//! for save in card.saves()? {
//!     println!("{} ({} files)", save.name, save.files.len());
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # The shape of a card
//!
//! | Unit | Size | Notes |
//! | ---- | ---- | ----- |
//! | Page | 512 bytes | 528 in a hardware dump, the extra 16 being [`ecc`] spare area |
//! | Cluster | 2 pages | what the allocation table counts |
//! | Block | 16 pages | the erase unit; the last two are reserved for backups |
//!
//! A save is a **directory**, not a file: `BASLUS-20312/` holding `icon.sys`, an icon and the
//! game's own data. That is why [`Save`] has files rather than bytes.
//!
//! # What round-trips, and what does not
//!
//! [`MemoryCard::parse`] keeps every directory entry's 512 bytes verbatim, so a save's mode bits
//! and timestamps survive being read out and written back. [`CardBuilder`] regenerates everything
//! structural — the allocation table, the directory tree, the superblock and the ECC — because
//! that is what a writer has to do when saves land at different clusters on the destination card.
//!
//! So a card built from saves read out of another card holds the same saves, and is **not** a
//! byte-for-byte copy of the original. Keep the original image if you need that.

#![forbid(unsafe_code)]

pub mod ecc;

mod build;
mod error;
mod superblock;

pub use build::{Capacity, CardBuilder};
pub use error::{Error, Result};
pub use superblock::SuperBlock;

/// A page of data, without its spare area.
pub const PAGE: usize = 512;

/// The spare area attached to each page in a hardware dump.
pub const SPARE: usize = 16;

/// A page as a hardware dump stores it: data then spare.
pub const RAW_PAGE: usize = PAGE + SPARE;

/// A directory entry, which is also the unit a save's `dirent` is kept in.
pub const ENTRY: usize = 512;

/// The magic every card's superblock opens with.
pub const MAGIC: &[u8] = b"Sony PS2 Memory Card Format ";

/// A FAT entry's high bit, set when the cluster it names is allocated.
const ALLOCATED: u32 = 0x8000_0000;

/// The value in a FAT entry's low bits meaning "no next cluster".
const CHAIN_END: u32 = 0x7FFF_FFFF;

/// Directory entry mode bits.
pub mod mode {
    /// The entry names a file.
    pub const FILE: u16 = 0x0010;
    /// The entry names a directory.
    pub const DIRECTORY: u16 = 0x0020;
    /// The entry is in use. An entry without this bit is a free slot.
    pub const EXISTS: u16 = 0x8000;

    /// What this crate writes for a directory: exists, readable, writable, executable.
    pub const NEW_DIRECTORY: u16 = 0x8427;
    /// What this crate writes for a file.
    pub const NEW_FILE: u16 = 0x8497;
}

/// One file inside a save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    /// The file's name, as the directory holds it.
    pub name: String,
    /// The directory entry's 512 bytes, verbatim and opaque.
    pub dirent: Vec<u8>,
    /// The file's contents.
    pub data: Vec<u8>,
}

/// One save: a directory, and the files in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Save {
    /// The directory's name, which is usually the game's product code.
    pub name: String,
    /// The directory's own entry, 512 bytes verbatim.
    ///
    /// This is the one carrying the save's mode bits and timestamps, and it belongs to no file.
    pub dirent: Vec<u8>,
    /// The files in the directory, in the order the directory lists them, excluding `.` and `..`.
    pub files: Vec<File>,
}

impl Save {
    /// How many bytes the save's files come to.
    #[must_use]
    pub fn len(&self) -> usize {
        self.files.iter().map(|file| file.data.len()).sum()
    }

    /// Whether the save holds no files at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// A parsed memory card image.
pub struct MemoryCard {
    /// The card with any spare areas stripped, so offsets here are pure data.
    data: Vec<u8>,
    /// Whether the image it came from carried spare areas.
    had_spare: bool,
    superblock: SuperBlock,
}

impl MemoryCard {
    /// Reads a card image, with or without spare areas.
    ///
    /// Which it is comes from the length: an image whose page count divides by 528 rather than
    /// 512 is a hardware dump, and its spare areas are stripped here so that everything below
    /// addresses data only.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let (data, had_spare) = strip_spare(bytes)?;
        let superblock = SuperBlock::parse(&data)?;
        Ok(MemoryCard { data, had_spare, superblock })
    }

    /// The card's superblock.
    #[must_use]
    pub fn superblock(&self) -> &SuperBlock {
        &self.superblock
    }

    /// Whether the image this came from carried spare areas.
    #[must_use]
    pub fn had_spare(&self) -> bool {
        self.had_spare
    }

    /// The card's **data** capacity in bytes: what saves can occupy.
    ///
    /// Never the length of a dump of the card. An 8 MB card holds 8388608 bytes of data and dumps
    /// to 8650752, and the two are supposed to disagree.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.superblock.clusters_per_card * self.superblock.cluster_size()
    }

    /// Every save on the card, in the order the root directory lists them.
    pub fn saves(&self) -> Result<Vec<Save>> {
        let mut saves = Vec::new();
        for entry in self.read_directory(self.superblock.rootdir_cluster)? {
            // Only directories are saves; a bare file in the root is not one, and `.` and `..`
            // are the directory naming itself and its parent.
            if entry.mode & mode::DIRECTORY == 0 || entry.name == "." || entry.name == ".." {
                continue;
            }
            let mut files = Vec::new();
            for file in self.read_directory(entry.cluster as usize)? {
                if file.mode & mode::FILE == 0 || file.name == "." || file.name == ".." {
                    continue;
                }
                files.push(File {
                    name: file.name.clone(),
                    dirent: file.raw.clone(),
                    data: self.read_chain_bytes(file.cluster as usize, file.length)?,
                });
            }
            saves.push(Save { name: entry.name.clone(), dirent: entry.raw.clone(), files });
        }
        Ok(saves)
    }

    /// Reads the entries of the directory starting at `cluster`.
    fn read_directory(&self, cluster: usize) -> Result<Vec<Entry>> {
        // The first entry of a directory is `.`, whose length field counts every entry in it.
        let head = self.read_chain_bytes(cluster, ENTRY)?;
        let count = Entry::parse(&head)?.length;
        if count > self.superblock.alloc_end {
            return Err(Error::Corrupt(format!("a directory claims {count} entries")));
        }

        let bytes = self.read_chain_bytes(cluster, count * ENTRY)?;
        bytes
            .as_chunks::<ENTRY>()
            .0
            .iter()
            .map(|entry| Entry::parse(entry.as_slice()))
            .filter(|entry| entry.as_ref().is_ok_and(|entry| entry.mode & mode::EXISTS != 0))
            .collect()
    }

    /// Reads `length` bytes along the cluster chain starting at `start`.
    fn read_chain_bytes(&self, start: usize, length: usize) -> Result<Vec<u8>> {
        let cluster_size = self.superblock.cluster_size();
        let mut out = Vec::with_capacity(length);
        let mut cluster = start;
        let mut seen = std::collections::HashSet::new();

        while out.len() < length {
            if !seen.insert(cluster) {
                return Err(Error::Corrupt(format!("the chain from cluster {start} loops")));
            }
            if cluster >= self.superblock.alloc_end {
                return Err(Error::Corrupt(format!("cluster {cluster} is past the allocated area")));
            }
            let offset = (self.superblock.alloc_offset + cluster) * cluster_size;
            let end = (offset + cluster_size).min(self.data.len());
            if offset >= self.data.len() {
                return Err(Error::Corrupt(format!("cluster {cluster} is past the image")));
            }
            let take = (length - out.len()).min(end - offset);
            out.extend_from_slice(&self.data[offset..offset + take]);

            match self.fat_entry(cluster)? {
                None => break,
                Some(next) => cluster = next,
            }
        }
        if out.len() < length {
            return Err(Error::Corrupt(format!(
                "the chain from cluster {start} ran out after {} of {length} bytes",
                out.len()
            )));
        }
        Ok(out)
    }

    /// The next cluster in `cluster`'s chain, or `None` at the end of it.
    ///
    /// The allocation table is reached through two levels of indirection: the superblock holds a
    /// list of *indirect* FAT clusters, each of which holds the numbers of the FAT clusters, each
    /// of which holds the entries themselves.
    fn fat_entry(&self, cluster: usize) -> Result<Option<usize>> {
        let per_cluster = self.superblock.entries_per_cluster();
        let indirect_index = cluster / per_cluster;
        let indirect_slot = indirect_index / per_cluster;

        let ifc = *self
            .superblock
            .ifc_list
            .get(indirect_slot)
            .ok_or_else(|| Error::Corrupt(format!("cluster {cluster} is past the indirect FAT")))?;
        if ifc == 0 {
            return Err(Error::Corrupt(format!("cluster {cluster} has no indirect FAT entry")));
        }

        let fat_cluster = self.raw_cluster_u32(ifc as usize, indirect_index % per_cluster)?;
        let entry = self.raw_cluster_u32(fat_cluster as usize, cluster % per_cluster)?;

        if entry & ALLOCATED == 0 {
            return Err(Error::Corrupt(format!("cluster {cluster} is in a chain but not allocated")));
        }
        let next = entry & !ALLOCATED;
        Ok((next != CHAIN_END).then_some(next as usize))
    }

    /// One 32-bit entry out of a cluster addressed from the start of the card.
    ///
    /// Unlike a chain read, this is not relative to `alloc_offset`: the FAT lives in front of the
    /// allocated area, not inside it.
    fn raw_cluster_u32(&self, cluster: usize, index: usize) -> Result<u32> {
        let offset = cluster * self.superblock.cluster_size() + index * 4;
        let bytes = self
            .data
            .get(offset..offset + 4)
            .ok_or_else(|| Error::Corrupt(format!("cluster {cluster} is past the image")))?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }
}

/// One directory entry, parsed as far as walking the tree needs.
struct Entry {
    mode: u16,
    length: usize,
    cluster: u32,
    name: String,
    /// The entry's 512 bytes, kept so a caller can round-trip what this struct does not model.
    raw: Vec<u8>,
}

impl Entry {
    fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < ENTRY {
            return Err(Error::Corrupt(format!("a directory entry is {ENTRY} bytes, found {}", bytes.len())));
        }
        let mode = u16::from_le_bytes([bytes[0], bytes[1]]);
        let length = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
        let cluster = u32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let name_field = &bytes[0x40..0x40 + 32];
        let end = name_field.iter().position(|&b| b == 0).unwrap_or(name_field.len());
        Ok(Entry {
            mode,
            length,
            cluster,
            name: String::from_utf8_lossy(&name_field[..end]).into_owned(),
            raw: bytes[..ENTRY].to_vec(),
        })
    }
}

/// Splits the spare areas off a hardware dump, or passes a plain image through.
fn strip_spare(bytes: &[u8]) -> Result<(Vec<u8>, bool)> {
    if bytes.len() < PAGE {
        return Err(Error::TooShort(bytes.len()));
    }
    if bytes.len().is_multiple_of(RAW_PAGE) && !bytes.len().is_multiple_of(PAGE) {
        let data = bytes.as_chunks::<RAW_PAGE>().0.iter().flat_map(|page| &page[..PAGE]).copied().collect();
        return Ok((data, true));
    }
    if bytes.len().is_multiple_of(PAGE) {
        // A length that divides by both is ambiguous, so the magic decides: a dump's second page
        // starts at 528 and a plain image's at 512.
        if bytes.len().is_multiple_of(RAW_PAGE)
            && !bytes[PAGE..].starts_with(MAGIC)
            && bytes.starts_with(MAGIC)
        {
            let looks_raw = bytes.len() > RAW_PAGE && bytes[RAW_PAGE..].len() >= PAGE;
            if looks_raw && bytes.len().is_multiple_of(RAW_PAGE) {
                let data =
                    bytes.as_chunks::<RAW_PAGE>().0.iter().flat_map(|page| &page[..PAGE]).copied().collect();
                return Ok((data, true));
            }
        }
        return Ok((bytes.to_vec(), false));
    }
    Err(Error::TooShort(bytes.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_image_keeps_its_length() {
        let image = vec![0u8; PAGE * 4];
        let (data, had_spare) = strip_spare(&image).unwrap();
        assert_eq!((data.len(), had_spare), (PAGE * 4, false));
    }

    #[test]
    fn a_hardware_dump_loses_its_spare_areas() {
        // Four pages of a dump are 2112 bytes and carry 2048 bytes of data.
        let dump = vec![0u8; RAW_PAGE * 4];
        let (data, had_spare) = strip_spare(&dump).unwrap();
        assert_eq!((data.len(), had_spare), (PAGE * 4, true));
    }

    #[test]
    fn capacity_is_data_and_not_the_length_of_a_dump() {
        // This is the distinction that bites on PS2: an 8 MB card dumps to 8650752 bytes.
        let card = CardBuilder::new(Capacity::Mb8).build().unwrap();
        let parsed = MemoryCard::parse(&card).unwrap();
        assert_eq!(parsed.capacity(), 8 * 1024 * 1024);
        assert_eq!(card.len(), 8 * 1024 * 1024);

        let dump = CardBuilder::new(Capacity::Mb8).build_with_spare().unwrap();
        assert_eq!(dump.len(), 8_650_752);
        assert_eq!(MemoryCard::parse(&dump).unwrap().capacity(), 8 * 1024 * 1024);
    }
}
