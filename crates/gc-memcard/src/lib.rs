//! Reading and writing GameCube memory card images.
//!
//! A card is 8 KiB blocks, big-endian throughout. Block 0 is the header, blocks 1 and 2 the
//! directory and its mirror, blocks 3 and 4 the block-allocation table and its mirror, and block 5
//! onward the saves. Each mirror carries an update counter, and the console reads whichever of the
//! pair counted higher — which is what makes a half-finished write survivable.
//!
//! ```no_run
//! use gc_memcard::MemoryCard;
//!
//! let card = MemoryCard::parse(&std::fs::read("card.raw")?)?;
//! for save in card.saves() {
//!     println!("{} ({}{}) {} bytes", save.filename, save.game_code, save.maker_code, save.data.len());
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # The two checksums are the opposite way round
//!
//! The directory's checksum pair sits in its **last** four bytes and covers everything before them.
//! The table's sits in its **first** four and covers everything after. Conflating the two produces
//! a card that reads back fine in one implementation and nowhere else, which is why [`checksum`]
//! and its two stampers are separate rather than one call with an offset.
//!
//! # What round-trips
//!
//! [`MemoryCard::parse`] keeps each save's 64-byte directory entry verbatim. [`CardBuilder`]
//! regenerates the directory, the allocation table, both their mirrors and every checksum, and
//! rewrites only the entry's first block and block count.

#![forbid(unsafe_code)]

mod build;
mod error;

pub use build::CardBuilder;
pub use error::{Error, Result};

/// One block: the unit the allocation table counts and a save occupies.
pub const BLOCK: usize = 8192;

/// The header, the directory and its mirror, the table and its mirror.
pub const RESERVED_BLOCKS: usize = 5;

/// Where the directory lives.
pub const DIR_BLOCK: usize = 1;

/// Where the directory's mirror lives.
pub const DIR_MIRROR_BLOCK: usize = 2;

/// Where the block-allocation table lives.
pub const BAT_BLOCK: usize = 3;

/// Where the allocation table's mirror lives.
pub const BAT_MIRROR_BLOCK: usize = 4;

/// A directory entry's length.
pub const ENTRY_LEN: usize = 64;

/// How many saves a card holds, whatever its size.
pub const MAX_ENTRIES: usize = 127;

/// A block-allocation table entry meaning "this is the last block of its file".
pub const CHAIN_END: u16 = 0xFFFF;

/// Where the allocation table's map starts, past its checksum pair, counter, free count and
/// last-allocated fields.
pub const MAP_OFFSET: usize = 0x0A;

/// The card sizes the console formats, in blocks.
pub const KNOWN_SIZES: [usize; 6] = [64, 128, 256, 512, 1024, 2048];

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([bytes[offset], bytes[offset + 1]])
}

fn block(bytes: &[u8], index: usize) -> &[u8] {
    &bytes[index * BLOCK..(index + 1) * BLOCK]
}

/// The checksum pair the console puts on the directory and the table.
///
/// It is a 16-bit sum of the region's big-endian words, alongside a sum of their complements. The
/// two are stored together so a region of all-ones, which is what an erased block reads as, does
/// not produce a checksum that happens to match.
#[must_use]
pub fn checksum(bytes: &[u8]) -> (u16, u16) {
    let mut sum: u16 = 0;
    let mut inverse: u16 = 0;
    for word in bytes.chunks_exact(2) {
        let value = u16::from_be_bytes([word[0], word[1]]);
        sum = sum.wrapping_add(value);
        inverse = inverse.wrapping_add(!value);
    }
    // The console normalises the all-ones case to zero on both halves.
    if sum == 0xFFFF {
        sum = 0;
    }
    if inverse == 0xFFFF {
        inverse = 0;
    }
    (sum, inverse)
}

/// The directory's checksum pair sits in its **last** four bytes and covers everything before them.
#[must_use]
pub fn directory_is_valid(directory: &[u8]) -> bool {
    directory.len() >= BLOCK
        && checksum(&directory[..BLOCK - 4]) == (u16_at(directory, BLOCK - 4), u16_at(directory, BLOCK - 2))
}

/// The table's checksum pair sits in its **first** four bytes and covers everything after them.
#[must_use]
pub fn table_is_valid(table: &[u8]) -> bool {
    table.len() >= BLOCK && checksum(&table[4..]) == (u16_at(table, 0), u16_at(table, 2))
}

/// Puts the directory's checksum pair in its last four bytes.
pub fn stamp_directory_checksum(region: &mut [u8]) {
    let (sum, inverse) = checksum(&region[..BLOCK - 4]);
    region[BLOCK - 4..BLOCK - 2].copy_from_slice(&sum.to_be_bytes());
    region[BLOCK - 2..].copy_from_slice(&inverse.to_be_bytes());
}

/// Puts the table's checksum pair in its first four bytes.
pub fn stamp_table_checksum(region: &mut [u8]) {
    let (sum, inverse) = checksum(&region[4..]);
    region[0..2].copy_from_slice(&sum.to_be_bytes());
    region[2..4].copy_from_slice(&inverse.to_be_bytes());
}

/// Whether these bytes look like a GameCube card.
///
/// There is no magic, so this checks the shape and then the directory's checksum, which is what
/// actually distinguishes a formatted card from a file that happens to be the right length.
#[must_use]
pub fn detect(bytes: &[u8]) -> bool {
    if !bytes.len().is_multiple_of(BLOCK) || !KNOWN_SIZES.contains(&(bytes.len() / BLOCK)) {
        return false;
    }
    [DIR_BLOCK, DIR_MIRROR_BLOCK].iter().any(|&index| directory_is_valid(block(bytes, index)))
}

/// Picks whichever of a mirrored pair the console would trust.
///
/// Both carry an update counter, and the higher one is the more recently written. A pair where
/// only one checksums is decided by that instead.
fn pick_mirror<'a>(
    primary: &'a [u8],
    mirror: &'a [u8],
    valid: impl Fn(&[u8]) -> bool,
    counter_offset: usize,
) -> Option<&'a [u8]> {
    match (valid(primary), valid(mirror)) {
        (true, true) => {
            let a = u16_at(primary, counter_offset);
            let b = u16_at(mirror, counter_offset);
            // Counters wrap, so "higher" is the one the other is not immediately behind.
            Some(if b.wrapping_sub(a) < 0x8000 && b != a { mirror } else { primary })
        }
        (true, false) => Some(primary),
        (false, true) => Some(mirror),
        (false, false) => None,
    }
}

/// One save on a card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Save {
    /// The directory index it occupied.
    ///
    /// Read only: a [`CardBuilder`] fills the directory in the order saves are added.
    pub slot: usize,
    /// The four-character game code.
    pub game_code: String,
    /// The two-character maker code.
    ///
    /// A save is opened by game code plus maker code plus filename, so neither code identifies a
    /// game on its own.
    pub maker_code: String,
    /// The save's filename.
    pub filename: String,
    /// The directory entry's 64 bytes, verbatim and opaque.
    ///
    /// Empty when a save was built by hand rather than read off a card.
    pub dirent: Vec<u8>,
    /// The save's bytes: every block it occupies, in chain order.
    pub data: Vec<u8>,
}

/// A parsed memory card image.
#[derive(Debug, Clone)]
pub struct MemoryCard {
    image: Vec<u8>,
    saves: Vec<Save>,
    blocks: usize,
}

impl MemoryCard {
    /// Reads a card image.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let blocks = bytes.len() / BLOCK;
        if !bytes.len().is_multiple_of(BLOCK) || !KNOWN_SIZES.contains(&blocks) {
            return Err(Error::WrongLength(bytes.len()));
        }

        // The directory's counter sits four bytes before its checksum; the table's is near its
        // front.
        let directory = pick_mirror(
            block(bytes, DIR_BLOCK),
            block(bytes, DIR_MIRROR_BLOCK),
            directory_is_valid,
            BLOCK - 6,
        )
        .ok_or(Error::NotAMemoryCard)?;
        let table = pick_mirror(block(bytes, BAT_BLOCK), block(bytes, BAT_MIRROR_BLOCK), table_is_valid, 4)
            .ok_or_else(|| {
            Error::Corrupt("neither the allocation table nor its mirror checksums".into())
        })?;

        let mut saves = Vec::new();
        for slot in 0..MAX_ENTRIES {
            let entry = &directory[slot * ENTRY_LEN..(slot + 1) * ENTRY_LEN];
            // A free entry is written as all-ones in the game code, which is what an erased entry
            // is.
            if entry[..4].iter().all(|&b| b == 0xFF) {
                continue;
            }

            let first = u16_at(entry, 0x36) as usize;
            let count = u16_at(entry, 0x38) as usize;
            if first < RESERVED_BLOCKS || first >= blocks || count == 0 {
                return Err(Error::Corrupt(format!(
                    "entry {slot} starts at block {first} for {count} blocks"
                )));
            }

            let chain = follow_chain(table, first, count, blocks)?;
            let data = chain.iter().flat_map(|&b| block(bytes, b)).copied().collect();

            let filename = {
                let field = &entry[8..8 + 32];
                let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
                String::from_utf8_lossy(&field[..end]).into_owned()
            };

            saves.push(Save {
                slot,
                game_code: ascii(&entry[0..4]),
                maker_code: ascii(&entry[4..6]),
                filename,
                dirent: entry.to_vec(),
                data,
            });
        }
        Ok(MemoryCard { image: bytes.to_vec(), saves, blocks })
    }

    /// Every save on the card, in directory order.
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

    /// How many blocks the card has in total, structure included.
    #[must_use]
    pub fn blocks(&self) -> usize {
        self.blocks
    }

    /// The card's **data** capacity in bytes: what saves can occupy.
    ///
    /// The card minus the five blocks the header, directory and table take, never the length of
    /// an image of it.
    #[must_use]
    pub fn capacity(&self) -> usize {
        (self.blocks - RESERVED_BLOCKS) * BLOCK
    }

    /// The header block, all 8 KiB of it.
    ///
    /// It carries the card's flash serial, which is what a Phantasy Star Online save is signed
    /// against, so it is worth keeping across a split.
    #[must_use]
    pub fn header_block(&self) -> &[u8] {
        &self.image[..BLOCK]
    }
}

fn ascii(bytes: &[u8]) -> String {
    bytes.iter().filter(|&&b| b.is_ascii_graphic()).map(|&b| b as char).collect()
}

/// The allocation table entry for a block.
///
/// The table describes blocks from [`RESERVED_BLOCKS`] onward, so block 5 is its first entry.
fn table_entry(table: &[u8], block_index: usize) -> u16 {
    u16_at(table, MAP_OFFSET + (block_index - RESERVED_BLOCKS) * 2)
}

fn follow_chain(table: &[u8], first: usize, count: usize, blocks: usize) -> Result<Vec<usize>> {
    let mut chain = Vec::with_capacity(count);
    let mut current = first;
    loop {
        if chain.contains(&current) {
            return Err(Error::Corrupt(format!("the chain from block {first} loops at block {current}")));
        }
        chain.push(current);
        let next = table_entry(table, current);

        if chain.len() == count {
            if next != CHAIN_END {
                return Err(Error::Corrupt(format!(
                    "the chain from block {first} carries on past the {count} blocks the entry claims"
                )));
            }
            return Ok(chain);
        }
        if next == CHAIN_END {
            return Err(Error::Corrupt(format!(
                "the chain from block {first} ended after {} blocks, not the {count} claimed",
                chain.len()
            )));
        }
        let next = next as usize;
        if !(RESERVED_BLOCKS..blocks).contains(&next) {
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

    /// Builds a card holding the given saves, as a console would have left it.
    ///
    /// Hand-rolled rather than built with [`CardBuilder`] on purpose: the round-trip test is that
    /// what this crate writes matches what a console wrote, so the fixture cannot come from the
    /// writer under test.
    pub(crate) fn card_with(blocks: usize, saves: &[(&str, &str, usize)]) -> Vec<u8> {
        let mut card = vec![0xFFu8; blocks * BLOCK];
        let mut directory = vec![0xFFu8; BLOCK];
        let mut table = vec![0u8; BLOCK];

        let mut next = RESERVED_BLOCKS;
        for (index, (code, name, count)) in saves.iter().enumerate() {
            let first = next;
            for step in 0..*count {
                let current = first + step;
                for byte in 0..BLOCK {
                    card[current * BLOCK + byte] =
                        u8::try_from((byte + current * 17) & 0xff).expect("a byte");
                }
                let link = if step + 1 == *count { CHAIN_END } else { u16::try_from(current + 1).unwrap() };
                let at = MAP_OFFSET + (current - RESERVED_BLOCKS) * 2;
                table[at..at + 2].copy_from_slice(&link.to_be_bytes());
            }
            let entry = &mut directory[index * ENTRY_LEN..(index + 1) * ENTRY_LEN];
            entry.fill(0);
            entry[..6].copy_from_slice(code.as_bytes());
            entry[6] = 0xFF;
            entry[8..8 + name.len()].copy_from_slice(name.as_bytes());
            entry[0x36..0x38].copy_from_slice(&u16::try_from(first).unwrap().to_be_bytes());
            entry[0x38..0x3A].copy_from_slice(&u16::try_from(*count).unwrap().to_be_bytes());
            next += count;
        }

        let free = u16::try_from(blocks - next).unwrap();
        table[4..6].copy_from_slice(&0u16.to_be_bytes());
        table[6..8].copy_from_slice(&free.to_be_bytes());
        table[8..10].copy_from_slice(&u16::try_from(next - 1).unwrap().to_be_bytes());
        directory[BLOCK - 6..BLOCK - 4].copy_from_slice(&0u16.to_be_bytes());
        stamp_directory_checksum(&mut directory);
        stamp_table_checksum(&mut table);

        card[..BLOCK].copy_from_slice(&build::default_header(blocks));
        card[DIR_BLOCK * BLOCK..(DIR_BLOCK + 1) * BLOCK].copy_from_slice(&directory);
        card[DIR_MIRROR_BLOCK * BLOCK..(DIR_MIRROR_BLOCK + 1) * BLOCK].copy_from_slice(&directory);
        card[BAT_BLOCK * BLOCK..(BAT_BLOCK + 1) * BLOCK].copy_from_slice(&table);
        card[BAT_MIRROR_BLOCK * BLOCK..(BAT_MIRROR_BLOCK + 1) * BLOCK].copy_from_slice(&table);
        card
    }

    #[test]
    fn a_checksum_landing_on_all_ones_is_folded_to_zero() {
        // 0xFFFF is the console's "no checksum here" sentinel, so a sum that lands on it is
        // written as zero instead. That is the whole of the normalisation: it is about the one
        // reserved value, not about erased regions in general.
        assert_eq!(checksum(&[0xFF, 0xFF]), (0, 0));
        // Two such words sum to 0xFFFE, which is an ordinary value and stays put.
        assert_eq!(checksum(&[0xFF, 0xFF, 0xFF, 0xFF]), (0xFFFE, 0));
    }

    #[test]
    fn the_two_regions_are_checksummed_the_opposite_way_round() {
        // The directory's pair is at the end and the table's at the front. Conflating them makes a
        // card that round-trips here and is rejected by every console and emulator.
        let mut directory = vec![0u8; BLOCK];
        directory[100] = 0x42;
        stamp_directory_checksum(&mut directory);
        assert!(directory_is_valid(&directory));

        let mut table = vec![0u8; BLOCK];
        table[100] = 0x42;
        stamp_table_checksum(&mut table);
        assert!(table_is_valid(&table));
        assert_ne!(&table[..4], &[0, 0, 0, 0], "the table's checksum is at its front");
    }

    #[test]
    fn reads_saves_and_keeps_the_header_block() {
        let card = card_with(64, &[("GAFE01", "super_mario_sunshine", 2), ("GM4E01", "PSO_SYSTEM", 1)]);
        let parsed = MemoryCard::parse(&card).expect("reads");

        assert_eq!(parsed.saves().len(), 2);
        assert_eq!(parsed.saves()[0].filename, "super_mario_sunshine");
        assert_eq!(parsed.saves()[0].game_code, "GAFE");
        assert_eq!(parsed.saves()[0].maker_code, "01");
        assert_eq!(parsed.saves()[0].data.len(), 2 * BLOCK);
        assert_eq!(parsed.saves()[0].dirent.len(), ENTRY_LEN);
        // The data capacity is the card minus the five blocks structure takes.
        assert_eq!(parsed.capacity(), (64 - 5) * BLOCK);
        assert_eq!(parsed.header_block().len(), BLOCK);
    }

    #[test]
    fn a_stale_mirror_loses_to_the_one_counted_higher() {
        // Both halves checksum, so the update counter is what decides. A card caught mid-write has
        // exactly this shape, and reading the stale half would lose the newest save.
        let card = card_with(64, &[("GAFE01", "current", 1)]);
        let mut tampered = card.clone();

        let mut stale = block(&card, DIR_BLOCK).to_vec();
        stale[8..8 + 7].copy_from_slice(b"OLDNAME");
        stale[BLOCK - 6..BLOCK - 4].copy_from_slice(&0u16.to_be_bytes());
        stamp_directory_checksum(&mut stale);

        let mut fresh = block(&card, DIR_BLOCK).to_vec();
        fresh[BLOCK - 6..BLOCK - 4].copy_from_slice(&1u16.to_be_bytes());
        stamp_directory_checksum(&mut fresh);

        tampered[DIR_BLOCK * BLOCK..(DIR_BLOCK + 1) * BLOCK].copy_from_slice(&stale);
        tampered[DIR_MIRROR_BLOCK * BLOCK..(DIR_MIRROR_BLOCK + 1) * BLOCK].copy_from_slice(&fresh);

        let parsed = MemoryCard::parse(&tampered).expect("reads");
        assert_eq!(parsed.saves()[0].filename, "current", "the higher counter wins");
    }

    #[test]
    fn a_corrupt_primary_falls_back_to_its_mirror() {
        let card = card_with(64, &[("GAFE01", "onlycopy", 1)]);
        let mut tampered = card.clone();
        // Scribble on the primary directory so only its mirror checksums.
        tampered[DIR_BLOCK * BLOCK + 100] ^= 0xFF;

        let parsed = MemoryCard::parse(&tampered).expect("reads through the mirror");
        assert_eq!(parsed.saves()[0].filename, "onlycopy");
    }

    #[test]
    fn refuses_a_chain_that_loops() {
        let mut card = card_with(64, &[("GAFE01", "loop", 2)]);
        let mut table = block(&card, BAT_BLOCK).to_vec();
        // Point the second block back at the first.
        let at = MAP_OFFSET + 2;
        table[at..at + 2].copy_from_slice(&u16::try_from(RESERVED_BLOCKS).unwrap().to_be_bytes());
        stamp_table_checksum(&mut table);
        card[BAT_BLOCK * BLOCK..(BAT_BLOCK + 1) * BLOCK].copy_from_slice(&table);
        card[BAT_MIRROR_BLOCK * BLOCK..(BAT_MIRROR_BLOCK + 1) * BLOCK].copy_from_slice(&table);

        assert!(matches!(MemoryCard::parse(&card), Err(Error::Corrupt(_))));
    }

    #[test]
    fn detects_a_formatted_card_and_not_a_file_of_the_right_length() {
        let card = card_with(64, &[("GAFE01", "x", 1)]);
        assert!(detect(&card));
        // The right length, but nothing that checksums.
        assert!(!detect(&vec![0u8; 64 * BLOCK]));
        assert!(!detect(&vec![0u8; 63 * BLOCK]));
        assert!(matches!(MemoryCard::parse(&vec![0u8; 64 * BLOCK]), Err(Error::NotAMemoryCard)));
        assert!(matches!(MemoryCard::parse(&vec![0u8; 63 * BLOCK]), Err(Error::WrongLength(_))));
    }
}
