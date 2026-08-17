//! Reading and writing Dreamcast Visual Memory Unit images.
//!
//! A VMU is 128 KiB of 512-byte blocks. Block 255 is the root block, block 254 the allocation
//! table, blocks 253 down to 241 the directory, and blocks 0 to 199 the saves. The directory grows
//! **downward** from 253, which is the one thing about this layout that catches people out.
//!
//! ```no_run
//! use dreamcast_vmu::Vmu;
//!
//! let vmu = Vmu::parse(&std::fs::read("vmu.bin")?)?;
//! for save in vmu.saves() {
//!     println!("{} ({:?}) {} bytes", save.name, save.kind, save.data.len());
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Mini-games are not ordinary saves
//!
//! A mini-game executes in place from flash, so it must start at block 0 and stay contiguous. The
//! directory entry's type byte says which a save is, so a writer can honour that without
//! understanding the payload: [`VmuBuilder`] places every [`FileKind::Game`] before anything else.
//!
//! # What round-trips
//!
//! [`Vmu::parse`] keeps each save's 32-byte directory entry verbatim, so its timestamp and header
//! offset survive being read out and written back. [`VmuBuilder`] regenerates the allocation table
//! and the directory, and rewrites only the entry's first block, block count and type byte.

#![forbid(unsafe_code)]

mod build;
mod error;

pub use build::VmuBuilder;
pub use error::{Error, Result};

/// One block: the unit the allocation table counts and a save occupies.
pub const BLOCK: usize = 512;

/// How many blocks an image holds.
pub const BLOCKS: usize = 256;

/// The whole image.
pub const IMAGE_LEN: usize = BLOCK * BLOCKS;

/// The root block, holding the VMU's own colour and icon.
pub const ROOT_BLOCK: usize = 255;

/// The allocation table: 256 entries of two bytes.
pub const FAT_BLOCK: usize = 254;

/// The directory's first block. It runs downward from here.
pub const DIR_FIRST_BLOCK: usize = 253;

/// How many blocks the directory runs to.
pub const DIR_BLOCKS: usize = 13;

/// How many saves a VMU can hold.
pub const MAX_ENTRIES: usize = 200;

/// How many blocks saves can occupy.
pub const USER_BLOCKS: usize = 200;

/// The VMU's data capacity: what saves can occupy, not the length of an image.
pub const CAPACITY: usize = BLOCK * USER_BLOCKS;

/// A directory entry's length.
pub const ENTRY_LEN: usize = 32;

/// How many directory entries fit in one block.
pub const ENTRIES_PER_BLOCK: usize = BLOCK / ENTRY_LEN;

/// An allocation table entry meaning "this is the last block of its file".
pub const CHAIN_END: u16 = 0xFFFC;

/// An allocation table entry meaning "this block is free".
pub const FREE: u16 = 0xFFFA;

/// What a directory entry's type byte says a save is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FileKind {
    /// An ordinary save.
    #[default]
    Data,
    /// A mini-game, which executes in place and so must start at block 0 and stay contiguous.
    Game,
}

impl FileKind {
    /// The type byte a kind is written as.
    #[must_use]
    pub const fn as_byte(self) -> u8 {
        match self {
            FileKind::Data => 0x33,
            FileKind::Game => 0xCC,
        }
    }

    /// The kind a type byte names, or `None` for a byte that is neither and so not a save at all.
    #[must_use]
    pub const fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0x33 => Some(FileKind::Data),
            0xCC => Some(FileKind::Game),
            _ => None,
        }
    }
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn block(bytes: &[u8], index: usize) -> &[u8] {
    &bytes[index * BLOCK..(index + 1) * BLOCK]
}

/// Whether these bytes look like a VMU image.
///
/// A formatted VMU opens its root block with sixteen bytes of `0x55`, which is the format check
/// the console itself makes.
#[must_use]
pub fn detect(bytes: &[u8]) -> bool {
    bytes.len() == IMAGE_LEN && block(bytes, ROOT_BLOCK)[..16].iter().all(|&b| b == 0x55)
}

/// Where one directory entry sits.
///
/// The directory runs downward from block 253, so entry 16 is the first entry of block 252 and not
/// the seventeenth byte-run of a contiguous region.
#[must_use]
pub fn entry_offset(index: usize) -> usize {
    let block_index = DIR_FIRST_BLOCK - index / ENTRIES_PER_BLOCK;
    block_index * BLOCK + (index % ENTRIES_PER_BLOCK) * ENTRY_LEN
}

/// One save on a VMU.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Save {
    /// The directory index it occupied.
    ///
    /// Read only: a [`VmuBuilder`] fills the directory in the order saves are added.
    pub slot: usize,
    /// Whether this is an ordinary save or a mini-game, which constrains where it can go.
    pub kind: FileKind,
    /// The save's name, out of the entry's twelve-byte name field.
    pub name: String,
    /// The directory entry's 32 bytes, verbatim and opaque.
    ///
    /// Empty when a save was built by hand rather than read off a VMU.
    pub dirent: Vec<u8>,
    /// The save's bytes: every block it occupies, in chain order.
    pub data: Vec<u8>,
}

/// A parsed VMU image.
#[derive(Debug, Clone)]
pub struct Vmu {
    image: Vec<u8>,
    saves: Vec<Save>,
}

impl Vmu {
    /// Reads a VMU image.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != IMAGE_LEN {
            return Err(Error::WrongLength(bytes.len()));
        }
        if !block(bytes, ROOT_BLOCK)[..16].iter().all(|&b| b == 0x55) {
            return Err(Error::NotFormatted);
        }

        let fat = block(bytes, FAT_BLOCK);
        let mut saves = Vec::new();

        for slot in 0..MAX_ENTRIES {
            let offset = entry_offset(slot);
            let entry = &bytes[offset..offset + ENTRY_LEN];
            let Some(kind) = FileKind::from_byte(entry[0]) else {
                continue;
            };

            let first = u16_at(entry, 2) as usize;
            let blocks = u16_at(entry, 0x18) as usize;
            if first >= USER_BLOCKS || blocks == 0 {
                return Err(Error::Corrupt(format!(
                    "entry {slot} starts at block {first} for {blocks} blocks"
                )));
            }

            let chain = follow_chain(fat, first, blocks)?;
            let data = chain.iter().flat_map(|&b| block(bytes, b)).copied().collect();

            let name = {
                let field = &entry[4..16];
                let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
                String::from_utf8_lossy(&field[..end]).trim_end().to_owned()
            };

            saves.push(Save { slot, kind, name, dirent: entry.to_vec(), data });
        }
        Ok(Vmu { image: bytes.to_vec(), saves })
    }

    /// Every save on the VMU, in directory order.
    #[must_use]
    pub fn saves(&self) -> &[Save] {
        &self.saves
    }

    /// Takes the saves, for a caller that is going to rebuild rather than read on.
    #[must_use]
    pub fn into_saves(self) -> Vec<Save> {
        self.saves
    }

    /// The whole image.
    #[must_use]
    pub fn image(&self) -> &[u8] {
        &self.image
    }

    /// The VMU's data capacity: the 200 blocks saves can occupy, not the image's length.
    #[must_use]
    pub fn capacity(&self) -> usize {
        CAPACITY
    }

    /// The root block, which carries the VMU's custom colour and icon shape.
    ///
    /// Those belong to no save, so a caller that splits a VMU into its saves and keeps nothing
    /// else drops them.
    #[must_use]
    pub fn root_block(&self) -> &[u8] {
        block(&self.image, ROOT_BLOCK)
    }
}

/// Follows a save's block chain, checking it against the length the directory stated.
///
/// The directory's block count and the table's terminator are two statements about one save, and a
/// save whose two disagree is not one this crate can read: stopping at whichever came first would
/// hand back a payload that neither the directory nor the table describes.
fn follow_chain(fat: &[u8], first: usize, blocks: usize) -> Result<Vec<usize>> {
    let mut chain = Vec::with_capacity(blocks);
    let mut current = first;
    loop {
        if chain.contains(&current) {
            return Err(Error::Corrupt(format!("the chain from block {first} loops at block {current}")));
        }
        chain.push(current);
        let next = u16_at(fat, current * 2);

        if chain.len() == blocks {
            if next != CHAIN_END {
                return Err(Error::Corrupt(format!(
                    "the chain from block {first} carries on past the {blocks} blocks the directory claims"
                )));
            }
            return Ok(chain);
        }
        if next == CHAIN_END {
            return Err(Error::Corrupt(format!(
                "the chain from block {first} ended after {} blocks, not the {blocks} claimed",
                chain.len()
            )));
        }

        let next = next as usize;
        if next >= USER_BLOCKS {
            return Err(Error::Corrupt(format!(
                "block {current} links to {next}, which is not a save block"
            )));
        }
        current = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a VMU holding the given saves, as a console would have left it.
    ///
    /// Hand-rolled rather than built with [`VmuBuilder`] on purpose: the round-trip test is that
    /// what this crate writes matches what a console wrote, so the fixture cannot come from the
    /// writer under test.
    pub(crate) fn vmu_with(saves: &[(&str, usize, FileKind)]) -> Vec<u8> {
        let mut image = vec![0u8; IMAGE_LEN];
        image[ROOT_BLOCK * BLOCK..].copy_from_slice(&build::default_root_block());

        let mut fat = [FREE; BLOCKS];
        for entry in &mut fat[USER_BLOCKS..] {
            *entry = CHAIN_END;
        }

        let mut next = 0usize;
        for (index, (name, blocks, kind)) in saves.iter().enumerate() {
            let first = next;
            for step in 0..*blocks {
                let current = first + step;
                for byte in 0..BLOCK {
                    image[current * BLOCK + byte] =
                        u8::try_from((byte + current * 11) & 0xff).expect("a byte");
                }
                fat[current] =
                    if step + 1 == *blocks { CHAIN_END } else { u16::try_from(current + 1).unwrap() };
            }
            let at = entry_offset(index);
            image[at] = kind.as_byte();
            image[at + 2..at + 4].copy_from_slice(&u16::try_from(first).unwrap().to_le_bytes());
            image[at + 4..at + 4 + name.len()].copy_from_slice(name.as_bytes());
            image[at + 0x18..at + 0x1A].copy_from_slice(&u16::try_from(*blocks).unwrap().to_le_bytes());
            next += blocks;
        }
        for (index, entry) in fat.iter().enumerate() {
            let at = FAT_BLOCK * BLOCK + index * 2;
            image[at..at + 2].copy_from_slice(&entry.to_le_bytes());
        }
        image
    }

    #[test]
    fn the_directory_grows_downward_from_block_253() {
        // The layout detail this format is easiest to get wrong: entry 16 is the start of block
        // 252, not the seventeenth entry of a contiguous run upward.
        assert_eq!(entry_offset(0), DIR_FIRST_BLOCK * BLOCK);
        assert_eq!(entry_offset(15), DIR_FIRST_BLOCK * BLOCK + 15 * ENTRY_LEN);
        assert_eq!(entry_offset(16), (DIR_FIRST_BLOCK - 1) * BLOCK);
        assert_eq!(entry_offset(199), (DIR_FIRST_BLOCK - 12) * BLOCK + 7 * ENTRY_LEN);
    }

    #[test]
    fn reads_saves_and_keeps_the_colour_and_icon() {
        let image = vmu_with(&[("SONIC2___S01", 2, FileKind::Data), ("PSO_______", 1, FileKind::Data)]);
        let parsed = Vmu::parse(&image).expect("reads");

        assert_eq!(parsed.saves().len(), 2);
        assert_eq!(parsed.saves()[0].name, "SONIC2___S01");
        assert_eq!(parsed.saves()[0].kind, FileKind::Data);
        assert_eq!(parsed.saves()[0].data.len(), 2 * BLOCK);
        assert_eq!(parsed.saves()[0].dirent.len(), ENTRY_LEN);
        // The data capacity is 200 blocks, not the 131072 bytes the image runs to.
        assert_eq!(parsed.capacity(), 102_400);
        assert_eq!(parsed.root_block().len(), BLOCK);
    }

    #[test]
    fn refuses_a_chain_that_loops() {
        let mut image = vmu_with(&[("LOOP________", 2, FileKind::Data)]);
        // Point the second block back at the first.
        let at = FAT_BLOCK * BLOCK + 2;
        image[at..at + 2].copy_from_slice(&0u16.to_le_bytes());
        assert!(matches!(Vmu::parse(&image), Err(Error::Corrupt(_))));
    }

    #[test]
    fn refuses_an_image_that_is_not_formatted() {
        let image = vec![0u8; IMAGE_LEN];
        assert!(!detect(&image));
        assert!(matches!(Vmu::parse(&image), Err(Error::NotFormatted)));
        assert!(matches!(Vmu::parse(&[1, 2, 3]), Err(Error::WrongLength(3))));
    }

    #[test]
    fn a_type_byte_that_is_neither_is_not_a_save() {
        assert_eq!(FileKind::from_byte(0x33), Some(FileKind::Data));
        assert_eq!(FileKind::from_byte(0xCC), Some(FileKind::Game));
        assert_eq!(FileKind::from_byte(0), None);
    }
}
