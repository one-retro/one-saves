//! Reading and writing PlayStation memory card images.
//!
//! A card is 16 blocks of 8 KiB. Block 0 is the header block: 64 frames of 128 bytes, of which
//! frame 0 is the magic, frames 1 to 15 are the directory — one entry per data block — and the
//! rest is the broken-sector list and a write-test copy of frame 0. Blocks 1 to 15 hold the
//! saves, chained through a link field so one save can span several.
//!
//! ```no_run
//! use ps1_memcard::MemoryCard;
//!
//! let card = MemoryCard::parse(&std::fs::read("card.mcr")?)?;
//! for save in card.saves() {
//!     println!("{} in slot {} ({} bytes)", save.name, save.slot, save.data.len());
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # One card, five extensions
//!
//! The same 131072 bytes ship as `.mcr`, `.mcd`, `.bin` and `.srm` bare, and behind a header as
//! `.gme` (DexDrive), `.vgs` (VGS/Connectix) and `.vmp` (PSP). [`strip_container`] takes the
//! wrapper off, which is why this is one reader rather than five.
//!
//! # What round-trips, and what does not
//!
//! [`MemoryCard::parse`] keeps each save's 128-byte directory entry verbatim, so whatever the
//! entry says beyond the fields here survives being read out and written back. [`CardBuilder`]
//! regenerates everything structural — the block chains, the directory, every frame checksum —
//! because that is what a writer has to do when saves land at different blocks on the destination.
//!
//! What is deliberately **not** recorded is where a save's blocks sat. No console addresses a save
//! by block and every tool that moves saves between cards throws that field away, so a builder
//! allocates fresh blocks and packs from the front.

#![forbid(unsafe_code)]

mod build;
mod error;

pub use build::CardBuilder;
pub use error::{Error, Result};

/// One frame: the unit the header block is divided into, and a directory entry's length.
pub const FRAME: usize = 128;

/// One block: the unit a save occupies.
pub const BLOCK: usize = 8192;

/// One header block plus fifteen data blocks.
pub const BLOCKS: usize = 16;

/// The whole card, which for this format is also its data capacity.
pub const CAPACITY: usize = BLOCK * BLOCKS;

/// How many directory entries there are, one per data block.
pub const SLOTS: usize = 15;

/// The magic the header block opens with.
pub const MAGIC: &[u8] = b"MC";

/// A directory entry's state, in the first four bytes of the entry.
pub mod state {
    /// In use, and the first block of its save.
    pub const FIRST: u32 = 0x51;
    /// In use, and a middle block of its save.
    pub const MIDDLE: u32 = 0x52;
    /// In use, and the last block of its save.
    pub const LAST: u32 = 0x53;
    /// Free, on a formatted card.
    pub const FREE: u32 = 0xA0;
}

/// The link field's value on the last block of a chain.
pub const NO_NEXT: u16 = 0xFFFF;

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]])
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

/// The XOR checksum a frame carries in its last byte.
#[must_use]
pub fn frame_checksum(frame: &[u8]) -> u8 {
    frame[..FRAME - 1].iter().fold(0u8, |sum, byte| sum ^ byte)
}

/// Whether these bytes look like a card, in any of the containers one ships in.
#[must_use]
pub fn detect(bytes: &[u8]) -> bool {
    matches!(strip_container(bytes), Some(card) if card.starts_with(MAGIC))
}

/// Finds the raw 128 KiB card inside whatever container it arrived in.
///
/// The same card ships under several extensions. `.mcr`, `.mcd`, `.bin` and `.srm` are the card
/// and nothing else; `.gme` (DexDrive) and `.vgs` (VGS/Connectix) put a header in front of it;
/// `.vmp` (PSP) puts a 128-byte header in front. All of them are the same 131072 bytes once the
/// wrapper is off.
#[must_use]
pub fn strip_container(bytes: &[u8]) -> Option<&[u8]> {
    // A bare card, the common case.
    if bytes.len() == CAPACITY {
        return Some(bytes);
    }
    // DexDrive: 3904-byte header beginning "123-456-STD".
    if bytes.len() == CAPACITY + 3904 && bytes.starts_with(b"123-456-STD") {
        return Some(&bytes[3904..]);
    }
    // VGS: 64-byte header beginning "VgsM".
    if bytes.len() == CAPACITY + 64 && bytes.starts_with(b"VgsM") {
        return Some(&bytes[64..]);
    }
    // PSP virtual card: 128-byte header, magic 0x00 "PMV".
    if bytes.len() == CAPACITY + 128 && bytes[1..4] == *b"PMV" {
        return Some(&bytes[128..]);
    }
    None
}

/// The product code out of a save's filename, when the filename carries one.
///
/// A save is named region code, then the disc's product code, then whatever the game adds:
/// `BASCUS-94163FF7-S01`. The middle ten characters are the serial.
///
/// Note this is the *save's* product code and not necessarily the disc that wrote it. All three
/// US discs of Final Fantasy VII carry the disc-1 code in the save name so every disc reads the
/// same saves.
#[must_use]
pub fn serial_from_filename(name: &str) -> Option<&str> {
    if name.len() < 12 || !name.is_char_boundary(2) || !name.is_char_boundary(12) {
        return None;
    }
    let serial = &name[2..12];
    let shape = serial.as_bytes();
    let looks_right = shape[..4].iter().all(u8::is_ascii_uppercase)
        && shape[4] == b'-'
        && shape[5..].iter().all(u8::is_ascii_digit);
    looks_right.then_some(serial)
}

/// One save on a card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Save {
    /// The directory index it occupied, 1 to 15.
    ///
    /// Read only: a [`CardBuilder`] allocates its own, because block placement is an allocator's
    /// business and no console addresses a save by it.
    pub slot: usize,
    /// The filename out of the directory entry, which is what tells two saves of one game apart.
    pub name: String,
    /// The directory entry's 128 bytes, verbatim and opaque.
    ///
    /// Empty when a save was built by hand rather than read off a card; a builder then writes an
    /// entry carrying the name and nothing else.
    pub dirent: Vec<u8>,
    /// The save's bytes: every block it occupies, in chain order.
    pub data: Vec<u8>,
}

/// A parsed memory card image.
#[derive(Debug, Clone)]
pub struct MemoryCard {
    /// The bare card, with any container header stripped.
    image: Vec<u8>,
    saves: Vec<Save>,
}

impl MemoryCard {
    /// Reads a card image, in any of the containers one ships in.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let card = strip_container(bytes).ok_or(Error::WrongLength(bytes.len()))?;
        if !card.starts_with(MAGIC) {
            return Err(Error::NotAMemoryCard);
        }
        let saves = read_directory(card)?;
        Ok(MemoryCard { image: card.to_vec(), saves })
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

    /// The bare card, with any container header stripped.
    #[must_use]
    pub fn image(&self) -> &[u8] {
        &self.image
    }

    /// The card's data capacity, which for this format is the whole card.
    #[must_use]
    pub fn capacity(&self) -> usize {
        CAPACITY
    }

    /// The header block, all 8 KiB of it.
    #[must_use]
    pub fn header_block(&self) -> &[u8] {
        &self.image[..BLOCK]
    }

    /// The header block, but only when it holds something a builder would not regenerate.
    ///
    /// A builder rebuilds the magic frame, the free directory entries, the broken-sector list and
    /// the write-test frame from nothing, so a card whose header block holds only those needs
    /// nothing kept. A card that differs anywhere else — including one whose saves span several
    /// blocks, since the continuation entries are not regenerable from the directory alone —
    /// gives back its whole header block, the bytes that differ not being ones this crate claims
    /// to understand.
    #[must_use]
    pub fn system_area(&self) -> Option<&[u8]> {
        let regenerated = build::header_block(|slot| {
            self.saves.iter().find(|save| save.slot == slot).map(|save| save.dirent.clone())
        });
        (self.header_block() != regenerated.as_slice()).then(|| self.header_block())
    }
}

/// Walks the directory, following each used save's block chain.
fn read_directory(card: &[u8]) -> Result<Vec<Save>> {
    let mut saves = Vec::new();
    for slot in 1..=SLOTS {
        let dirent = &card[slot * FRAME..(slot + 1) * FRAME];
        // Only the first block of a save starts an entry; middle and last blocks are reached by
        // following the chain, and a deleted save's blocks belong to no save at all.
        if u32_at(dirent, 0) != state::FIRST {
            continue;
        }
        let chain = follow_chain(card, slot)?;
        let data =
            chain.iter().flat_map(|&block| &card[block * BLOCK..(block + 1) * BLOCK]).copied().collect();
        saves.push(Save { slot, name: filename(dirent), dirent: dirent.to_vec(), data });
    }
    Ok(saves)
}

/// Follows a save's block chain from its first block.
fn follow_chain(card: &[u8], first: usize) -> Result<Vec<usize>> {
    let mut chain = vec![first];
    let mut current = first;
    loop {
        let dirent = &card[current * FRAME..(current + 1) * FRAME];
        let next = u16_at(dirent, 8);
        if next == NO_NEXT {
            return Ok(chain);
        }
        // The link is a zero-based index over the data blocks, so block 1 is written as 0.
        let block = usize::from(next) + 1;
        if block > SLOTS {
            return Err(Error::Corrupt(format!("slot {current} links to block {block}, past the card")));
        }
        // A cycle would otherwise be an unbounded read, and a card with one is broken whatever it
        // was meant to say.
        if chain.contains(&block) {
            return Err(Error::Corrupt(format!("the chain from slot {first} loops at block {block}")));
        }
        let state = u32_at(&card[block * FRAME..(block + 1) * FRAME], 0);
        if state != state::MIDDLE && state != state::LAST {
            return Err(Error::Corrupt(format!(
                "slot {current} links to block {block}, which is not a continuation (state {state:#04x})"
            )));
        }
        chain.push(block);
        current = block;
    }
}

/// The filename out of a directory entry: 20 bytes at 0x0A, NUL-terminated.
fn filename(dirent: &[u8]) -> String {
    let field = &dirent[0x0A..0x0A + 20];
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a card with the given saves, as a console would have left it.
    ///
    /// Hand-rolled rather than built with [`CardBuilder`] on purpose: the point of the round-trip
    /// test is that what this crate writes matches what a console wrote, so the fixture has to
    /// come from somewhere other than the writer under test.
    pub(crate) fn card_with(saves: &[(&str, usize)]) -> Vec<u8> {
        let mut card = vec![0u8; CAPACITY];
        card[..2].copy_from_slice(MAGIC);
        card[FRAME - 1] = frame_checksum(&card[..FRAME]);

        for slot in 1..=SLOTS {
            let frame = &mut card[slot * FRAME..(slot + 1) * FRAME];
            frame[..4].copy_from_slice(&state::FREE.to_le_bytes());
            frame[8..10].copy_from_slice(&NO_NEXT.to_le_bytes());
            frame[FRAME - 1] = frame_checksum(frame);
        }
        for frame_index in 16..36 {
            let frame = &mut card[frame_index * FRAME..(frame_index + 1) * FRAME];
            frame[..4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
            frame[8..10].copy_from_slice(&NO_NEXT.to_le_bytes());
            frame[FRAME - 1] = frame_checksum(frame);
        }
        let (head, tail) = card.split_at_mut(63 * FRAME);
        tail[..FRAME].copy_from_slice(&head[..FRAME]);

        let mut block = 1usize;
        for (name, blocks) in saves {
            for index in 0..*blocks {
                let current = block + index;
                // Patterned payload, so an off-by-one that copied the wrong block shows up.
                for byte in 0..BLOCK {
                    card[current * BLOCK + byte] =
                        u8::try_from((byte + current * 7) & 0xff).expect("masked to a byte");
                }
                let frame = &mut card[current * FRAME..(current + 1) * FRAME];
                let state = match (index, blocks - 1) {
                    (0, _) => state::FIRST,
                    (i, last) if i == last => state::LAST,
                    _ => state::MIDDLE,
                };
                frame[..4].copy_from_slice(&state.to_le_bytes());
                frame[4..8].copy_from_slice(
                    &u32::try_from(blocks * BLOCK).expect("a card's worth fits").to_le_bytes(),
                );
                let link =
                    if index + 1 == *blocks { NO_NEXT } else { u16::try_from(current).expect("block fits") };
                frame[8..10].copy_from_slice(&link.to_le_bytes());
                if index == 0 {
                    frame[0x0A..0x0A + name.len()].copy_from_slice(name.as_bytes());
                }
                frame[FRAME - 1] = frame_checksum(frame);
            }
            block += blocks;
        }
        card
    }

    #[test]
    fn reads_a_card_of_three_saves_two_for_one_game() {
        let card = card_with(&[("BASCUS-94163FF7-S01", 1), ("BASCUS-94163FF7-S03", 1), ("BASLUS-00594", 1)]);
        let parsed = MemoryCard::parse(&card).expect("reads");

        assert_eq!(parsed.saves().len(), 3);
        assert_eq!(parsed.saves()[0].name, "BASCUS-94163FF7-S01");
        assert_eq!(parsed.saves()[0].slot, 1);
        assert_eq!(parsed.saves()[0].dirent.len(), FRAME);
        assert_eq!(parsed.saves()[0].data.len(), BLOCK);
        // A freshly formatted header block is entirely regenerable, so there is nothing to keep.
        assert_eq!(parsed.system_area(), None);
    }

    #[test]
    fn a_save_spanning_two_blocks_is_one_save_of_both() {
        let card = card_with(&[("BASLUS-00594", 2)]);
        let parsed = MemoryCard::parse(&card).expect("reads");
        assert_eq!(parsed.saves().len(), 1);
        assert_eq!(parsed.saves()[0].data.len(), 2 * BLOCK);
        // The continuation entry is not regenerable from the directory alone, so the header block
        // is kept rather than rebuilt.
        assert!(parsed.system_area().is_some());
    }

    #[test]
    fn strips_the_containers_the_same_card_ships_in() {
        let card = card_with(&[("BASLUS-00594", 1)]);

        let mut dexdrive = vec![0u8; 3904];
        dexdrive[..11].copy_from_slice(b"123-456-STD");
        dexdrive.extend_from_slice(&card);
        assert_eq!(strip_container(&dexdrive), Some(&card[..]));

        let mut vgs = vec![0u8; 64];
        vgs[..4].copy_from_slice(b"VgsM");
        vgs.extend_from_slice(&card);
        assert_eq!(strip_container(&vgs), Some(&card[..]));

        let mut psp = vec![0u8; 128];
        psp[1..4].copy_from_slice(b"PMV");
        psp.extend_from_slice(&card);
        assert_eq!(strip_container(&psp), Some(&card[..]));

        assert!(detect(&dexdrive) && detect(&vgs) && detect(&psp) && detect(&card));
        // Every container parses to the same card.
        assert_eq!(MemoryCard::parse(&dexdrive).unwrap().saves(), MemoryCard::parse(&card).unwrap().saves());
    }

    #[test]
    fn refuses_a_chain_that_loops() {
        let mut card = card_with(&[("BASLUS-00594", 2)]);
        // Point the second block back at the first, which no console would have written.
        card[2 * FRAME + 8..2 * FRAME + 10].copy_from_slice(&0u16.to_le_bytes());
        card[2 * FRAME + FRAME - 1] = frame_checksum(&card[2 * FRAME..3 * FRAME]);
        assert!(matches!(MemoryCard::parse(&card), Err(Error::Corrupt(_))));
    }

    #[test]
    fn refuses_something_that_is_not_a_card() {
        assert!(matches!(MemoryCard::parse(&[1, 2, 3]), Err(Error::WrongLength(3))));
        assert!(matches!(MemoryCard::parse(&vec![0u8; CAPACITY]), Err(Error::NotAMemoryCard)));
    }

    #[test]
    fn reads_a_serial_only_out_of_a_name_shaped_like_one() {
        assert_eq!(serial_from_filename("BASCUS-94163FF7-S01"), Some("SCUS-94163"));
        assert_eq!(serial_from_filename("BASLUS-00594"), Some("SLUS-00594"));
        assert_eq!(serial_from_filename("SHORT"), None);
        assert_eq!(serial_from_filename("not-a-serial-here"), None);
        // A multi-byte character across the slice boundary is not a panic.
        assert_eq!(serial_from_filename("é!SCUS-94163"), None);
    }
}
