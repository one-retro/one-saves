//! Dreamcast Visual Memory Units.
//!
//! A VMU is 128 KiB of 512-byte blocks. Block 255 is the root block, block 254 the allocation
//! table, blocks 253 down to 241 the directory, and blocks 0 to 199 the saves. The directory
//! grows *downward* from 253, which is the one thing about this layout that catches people out.
//!
//! A mini-game is a save that executes in place from flash, so it must start at block 0 and stay
//! contiguous. The directory entry's type byte says which a save is — `0xCC` for a mini-game and
//! `0x33` for data — so a writer can honour that without understanding the payload.

use one_saves::{Bundle, Card, Game, Header, Part, PartKind, Slug};

use crate::CardOptions;
use crate::error::{Error, Result, corrupt};

const FORMAT: &str = "Dreamcast VMU";

/// The card format slug a bundle carries.
pub const CARD_FORMAT: &str = "vmu";

/// The system slug these saves are for.
pub const SYSTEM: &str = "dreamcast";

const BLOCK: usize = 512;
const BLOCKS: usize = 256;
/// The whole VMU image.
pub const IMAGE_LEN: usize = BLOCK * BLOCKS;

/// The root block, holding the VMU's own colour and icon.
const ROOT_BLOCK: usize = 255;
/// The allocation table: 256 entries of two bytes.
const FAT_BLOCK: usize = 254;
/// The directory's first block. It runs downward from here.
const DIR_FIRST_BLOCK: usize = 253;
const DIR_BLOCKS: usize = 13;
/// How many saves a VMU can hold.
const MAX_ENTRIES: usize = 200;
/// How many blocks saves can occupy, which is also the card's data capacity in blocks.
const USER_BLOCKS: usize = 200;
/// The card's data capacity: what saves can occupy, not the length of an image.
pub const CAPACITY: usize = BLOCK * USER_BLOCKS;

const ENTRY_LEN: usize = 32;
const ENTRIES_PER_BLOCK: usize = BLOCK / ENTRY_LEN;

/// An allocation table entry meaning "this is the last block of its file".
const CHAIN_END: u16 = 0xFFFC;
/// An allocation table entry meaning "this block is free".
const FREE: u16 = 0xFFFA;

/// Directory entry type bytes.
mod file_type {
    /// An ordinary save.
    pub const DATA: u8 = 0x33;
    /// A mini-game, which executes in place and so must start at block 0 and stay contiguous.
    pub const GAME: u8 = 0xCC;
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
/// The directory runs downward from block 253, so entry 16 is the first entry of block 252 and
/// not the seventeenth byte-run of a contiguous region.
fn entry_offset(index: usize) -> usize {
    let block_index = DIR_FIRST_BLOCK - index / ENTRIES_PER_BLOCK;
    block_index * BLOCK + (index % ENTRIES_PER_BLOCK) * ENTRY_LEN
}

/// Reads a VMU into a bundle: one nested bundle per save.
pub fn read(bytes: &[u8], options: &CardOptions) -> Result<Bundle> {
    if bytes.len() != IMAGE_LEN {
        return Err(Error::WrongLength { format: FORMAT, expected: "131072 bytes", found: bytes.len() });
    }
    if !block(bytes, ROOT_BLOCK)[..16].iter().all(|&b| b == 0x55) {
        return Err(Error::NotThisFormat {
            format: FORMAT,
            why: "the root block does not open with sixteen 0x55 bytes".into(),
        });
    }

    let fat = block(bytes, FAT_BLOCK);
    let mut parts = Vec::new();

    for index in 0..MAX_ENTRIES {
        let offset = entry_offset(index);
        let entry = &bytes[offset..offset + ENTRY_LEN];
        let kind = entry[0];
        if kind != file_type::DATA && kind != file_type::GAME {
            continue;
        }

        let first = u16_at(entry, 2) as usize;
        let blocks = u16_at(entry, 0x18) as usize;
        if first >= USER_BLOCKS || blocks == 0 {
            return Err(corrupt!(FORMAT, "entry {index} starts at block {first} for {blocks} blocks"));
        }

        let chain = follow_chain(fat, first, blocks)?;
        let payload: Vec<u8> = chain.iter().flat_map(|&b| block(bytes, b)).copied().collect();

        let name = {
            let field = &entry[4..16];
            let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
            String::from_utf8_lossy(&field[..end]).trim_end().to_owned()
        };

        let inner = Bundle {
            header: Header {
                system: Some(slug(SYSTEM)),
                game: Some(Game { name: Some(name.clone()), ..Game::default() }),
                ..Header::default()
            },
            parts: vec![Part::new(0, payload)],
        };

        let mut part = Part::new(u64::try_from(parts.len()).expect("fits"), inner.to_vec()?);
        part.kind = PartKind::Bundle;
        part.role = Some(options.role.clone());
        part.path = Some(name.clone());
        part.slot = Some(u64::try_from(index).expect("fits"));
        part.dirent = Some(entry.to_vec());
        part.game = Some(Game { name: Some(name), ..Game::default() });
        part.system = Some(slug(SYSTEM));
        parts.push(part);
    }

    if parts.is_empty() {
        parts.push(crate::card::card_image_part(0, bytes, &options.role));
    }

    Ok(Bundle {
        header: Header {
            system: Some(slug(SYSTEM)),
            card: Some(Card {
                format: slug(CARD_FORMAT),
                capacity: CAPACITY as u64,
                // The root block carries the VMU's custom colour and icon shape, which belong to
                // no save and would otherwise be dropped by splitting the card.
                system_area: Some(block(bytes, ROOT_BLOCK).to_vec()),
                unknown: one_saves::UnknownKeys::new(),
            }),
            source: options.source.clone(),
            ..Header::default()
        },
        parts,
    })
}

fn slug(text: &str) -> Slug {
    Slug::parse(text).expect("a spec slug is well-formed")
}

/// Follows a save's block chain, checking it against the length the directory stated.
///
/// The directory's block count and the table's terminator are two statements about one save, and
/// a save whose two disagree is not one this format can read: stopping at whichever came first
/// would hand back a payload that neither the directory nor the table describes.
fn follow_chain(fat: &[u8], first: usize, blocks: usize) -> Result<Vec<usize>> {
    let mut chain = Vec::with_capacity(blocks);
    let mut current = first;
    loop {
        if chain.contains(&current) {
            return Err(corrupt!(FORMAT, "the chain from block {first} loops at block {current}"));
        }
        chain.push(current);
        let next = u16_at(fat, current * 2);

        if chain.len() == blocks {
            if next != CHAIN_END {
                return Err(corrupt!(
                    FORMAT,
                    "the chain from block {first} carries on past the {blocks} blocks the directory claims"
                ));
            }
            return Ok(chain);
        }
        if next == CHAIN_END {
            return Err(corrupt!(
                FORMAT,
                "the chain from block {first} ended after {} blocks, not the {blocks} claimed",
                chain.len()
            ));
        }

        let next = next as usize;
        if next >= USER_BLOCKS {
            return Err(corrupt!(FORMAT, "block {current} links to {next}, which is not a save block"));
        }
        current = next;
    }
}

/// Writes a bundle back out as a raw 128 KiB VMU image.
///
/// The allocation table and the directory are regenerated; the root block is kept from
/// `system_area` when the bundle carries one, so the VMU's colour and icon survive.
pub fn write(bundle: &Bundle) -> Result<Vec<u8>> {
    // A card with nothing on it carries a whole-card image rather than nested saves, and writing
    // it back is a byte copy.
    if let Some(image) = crate::card::image_only(bundle)? {
        return Ok(image);
    }
    let saves = crate::card::nested_saves(bundle)?;
    if saves.len() > MAX_ENTRIES {
        return Err(Error::NotConvertible(format!(
            "a VMU holds {MAX_ENTRIES} saves and this bundle has {}",
            saves.len()
        )));
    }

    let mut image = vec![0u8; IMAGE_LEN];

    // Everything starts free, and the directory starts empty.
    let mut fat = [FREE; BLOCKS];
    for entry in &mut fat[USER_BLOCKS..] {
        *entry = CHAIN_END;
    }

    // A mini-game has to start at block 0 and stay contiguous, so it is placed before anything
    // else. Ordinary saves take whatever is left.
    let mut order: Vec<usize> = (0..saves.len()).collect();
    order.sort_by_key(|&index| !is_minigame(&saves[index]));

    let mut next_block = 0usize;
    for &index in &order {
        let save = &saves[index];
        let blocks = save.bytes.len().div_ceil(BLOCK);
        if blocks == 0 {
            return Err(Error::NotConvertible(format!("save {:?} is empty", save.path)));
        }
        if next_block + blocks > USER_BLOCKS {
            return Err(Error::NotConvertible(format!(
                "these saves need {} blocks and a VMU has {USER_BLOCKS}",
                next_block + blocks
            )));
        }

        let first = next_block;
        for step in 0..blocks {
            let current = first + step;
            let from = step * BLOCK;
            let to = ((step + 1) * BLOCK).min(save.bytes.len());
            image[current * BLOCK..current * BLOCK + (to - from)].copy_from_slice(&save.bytes[from..to]);
            fat[current] = if step + 1 == blocks {
                CHAIN_END
            } else {
                u16::try_from(current + 1).expect("block index fits")
            };
        }

        let mut entry = save.dirent.clone().unwrap_or_else(|| default_entry(save));
        entry.resize(ENTRY_LEN, 0);
        // Only where the save landed is the writer's; the type byte, name and timestamp are the
        // game's and come through unchanged.
        if entry[0] != file_type::DATA && entry[0] != file_type::GAME {
            entry[0] = file_type::DATA;
        }
        entry[2..4].copy_from_slice(&u16::try_from(first).expect("block fits").to_le_bytes());
        entry[0x18..0x1A].copy_from_slice(&u16::try_from(blocks).expect("block count fits").to_le_bytes());

        let at = entry_offset(index);
        image[at..at + ENTRY_LEN].copy_from_slice(&entry);
        next_block += blocks;
    }

    // The root block: kept if the bundle carries one, otherwise formatted from nothing.
    let root = match bundle.header.card.as_ref().and_then(|c| c.system_area.as_ref()) {
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

/// Whether a save is a mini-game, which constrains where it can go.
fn is_minigame(save: &crate::card::NestedSave) -> bool {
    save.dirent.as_ref().is_some_and(|dirent| dirent.first() == Some(&file_type::GAME))
}

fn default_entry(save: &crate::card::NestedSave) -> Vec<u8> {
    let mut entry = vec![0u8; ENTRY_LEN];
    entry[0] = file_type::DATA;
    if let Some(path) = &save.path {
        let name = path.as_bytes();
        let len = name.len().min(12);
        entry[4..4 + len].copy_from_slice(&name[..len]);
    }
    entry
}

/// A freshly formatted root block.
fn default_root_block() -> Vec<u8> {
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

    /// Builds a VMU holding the given saves, as a console would have left it.
    fn vmu_with(saves: &[(&str, usize, u8)]) -> Vec<u8> {
        let mut image = vec![0u8; IMAGE_LEN];
        image[ROOT_BLOCK * BLOCK..].copy_from_slice(&default_root_block());

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
                        u8::try_from((byte + current * 11) & 0xff).expect("masked to a byte");
                }
                fat[current] =
                    if step + 1 == *blocks { CHAIN_END } else { u16::try_from(current + 1).unwrap() };
            }
            let at = entry_offset(index);
            image[at] = *kind;
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
        let image = vmu_with(&[("SONIC2___S01", 2, file_type::DATA), ("PSO_______", 1, file_type::DATA)]);
        let bundle = read(&image, &CardOptions::default()).expect("reads");

        assert_eq!(bundle.parts.len(), 2);
        let card = bundle.header.card.as_ref().unwrap();
        assert_eq!(card.format.as_str(), "vmu");
        // The data capacity is 200 blocks, not the 131072 bytes the image runs to.
        assert_eq!(card.capacity, 102_400);
        assert_eq!(card.system_area.as_ref().map(Vec::len), Some(512));

        assert_eq!(bundle.parts[0].path.as_deref(), Some("SONIC2___S01"));
        assert_eq!(bundle.parts[0].dirent.as_ref().map(Vec::len), Some(32));

        let inner = Bundle::from_slice(&bundle.parts[0].bytes().unwrap()).unwrap();
        assert_eq!(inner.parts[0].payload.len(), 1024, "two blocks");
        bundle.validate().expect("valid bundle");
    }

    #[test]
    fn a_vmu_round_trips_through_a_bundle() {
        let image = vmu_with(&[("SONIC2___S01", 2, file_type::DATA), ("PSO_______", 1, file_type::DATA)]);
        let bundle = read(&image, &CardOptions::default()).expect("reads");
        assert_eq!(write(&bundle).expect("writes"), image);
    }

    #[test]
    fn a_minigame_is_placed_at_block_zero() {
        // A mini-game executes in place from flash, so it cannot be put anywhere else. Here the
        // data save is listed first, and the writer still has to put the game at block 0.
        let image = vmu_with(&[("DATA________", 1, file_type::DATA), ("MINIGAME____", 3, file_type::GAME)]);
        let bundle = read(&image, &CardOptions::default()).expect("reads");
        let rebuilt = write(&bundle).expect("writes");

        let reread = read(&rebuilt, &CardOptions::default()).expect("reads back");
        let game = reread
            .parts
            .iter()
            .find(|part| part.dirent.as_ref().unwrap()[0] == file_type::GAME)
            .expect("the mini-game survived");
        let first = u16_at(game.dirent.as_ref().unwrap(), 2);
        assert_eq!(first, 0, "a mini-game has to start at block 0");
    }

    #[test]
    fn refuses_a_chain_that_loops() {
        let mut image = vmu_with(&[("LOOP________", 2, file_type::DATA)]);
        // Point the second block back at the first.
        let at = FAT_BLOCK * BLOCK + 2;
        image[at..at + 2].copy_from_slice(&0u16.to_le_bytes());
        assert!(matches!(read(&image, &CardOptions::default()), Err(Error::Corrupt { .. })));
    }

    #[test]
    fn refuses_an_image_that_is_not_formatted() {
        let image = vec![0u8; IMAGE_LEN];
        assert!(!detect(&image));
        assert!(matches!(read(&image, &CardOptions::default()), Err(Error::NotThisFormat { .. })));
    }
}
