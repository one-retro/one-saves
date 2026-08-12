//! GameCube memory cards.
//!
//! A card is 8 KiB blocks, big-endian throughout. Block 0 is the header, blocks 1 and 2 the
//! directory and its mirror, blocks 3 and 4 the block-allocation table and its mirror, and block
//! 5 onward the saves. Each mirror carries an update counter, and the console reads whichever of
//! the pair counted higher — which is what makes a half-finished write survivable.
//!
//! Slot A is [`memcard-1`](one_saves_registry::role) and Slot B is `memcard-2`. The registry
//! keeps one numbering convention rather than mirroring each console's silkscreen.

use one_saves::{Bundle, Card, Game, Header, Part, PartKind, Slug};

use crate::CardOptions;
use crate::error::{Error, Result, corrupt};

const FORMAT: &str = "GameCube memory card";

/// The card format slug a bundle carries.
pub const CARD_FORMAT: &str = "gc-mc";

/// The system slug these saves are for.
pub const SYSTEM: &str = "gc";

const BLOCK: usize = 8192;
/// The header, the directory and its mirror, the table and its mirror.
const RESERVED_BLOCKS: usize = 5;

const DIR_BLOCK: usize = 1;
const DIR_MIRROR_BLOCK: usize = 2;
const BAT_BLOCK: usize = 3;
const BAT_MIRROR_BLOCK: usize = 4;

const ENTRY_LEN: usize = 64;
/// How many saves a card can hold, whatever its size.
const MAX_ENTRIES: usize = 127;

/// A block-allocation table entry meaning "this is the last block of its file".
const CHAIN_END: u16 = 0xFFFF;

/// Where the allocation table's map starts, past its checksum pair, counter, free count and
/// last-allocated fields.
const MAP_OFFSET: usize = 0x0A;

/// The card sizes the console formats, in blocks.
const KNOWN_SIZES: [usize; 6] = [64, 128, 256, 512, 1024, 2048];

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
fn checksum(bytes: &[u8]) -> (u16, u16) {
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

/// Whether these bytes look like a GameCube card.
///
/// There is no magic, so this checks the shape and then the directory's checksum, which is what
/// actually distinguishes a formatted card from a file that happens to be the right length.
#[must_use]
pub fn detect(bytes: &[u8]) -> bool {
    if !KNOWN_SIZES.contains(&(bytes.len() / BLOCK)) || !bytes.len().is_multiple_of(BLOCK) {
        return false;
    }
    [DIR_BLOCK, DIR_MIRROR_BLOCK].iter().any(|&index| directory_is_valid(block(bytes, index)))
}

/// The directory's checksum pair sits in its **last** four bytes and covers everything before them.
fn directory_is_valid(directory: &[u8]) -> bool {
    checksum(&directory[..BLOCK - 4]) == (u16_at(directory, BLOCK - 4), u16_at(directory, BLOCK - 2))
}

/// The table's checksum pair sits in its **first** four bytes and covers everything after them.
///
/// The two regions are checksummed the opposite way round, which is easy to miss and produces a
/// card that reads back fine in this implementation and nowhere else.
fn table_is_valid(table: &[u8]) -> bool {
    checksum(&table[4..]) == (u16_at(table, 0), u16_at(table, 2))
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

/// Reads a card into a bundle: one nested bundle per save.
pub fn read(bytes: &[u8], options: &CardOptions) -> Result<Bundle> {
    let blocks = bytes.len() / BLOCK;
    if !bytes.len().is_multiple_of(BLOCK) || !KNOWN_SIZES.contains(&blocks) {
        return Err(Error::WrongLength {
            format: FORMAT,
            expected: "a whole number of 8 KiB blocks, in a size the console formats",
            found: bytes.len(),
        });
    }

    // The directory's counter sits four bytes before its checksum; the table's is near its front.
    let directory =
        pick_mirror(block(bytes, DIR_BLOCK), block(bytes, DIR_MIRROR_BLOCK), directory_is_valid, BLOCK - 6)
            .ok_or_else(|| corrupt!(FORMAT, "neither the directory nor its mirror checksums"))?;
    let table = pick_mirror(block(bytes, BAT_BLOCK), block(bytes, BAT_MIRROR_BLOCK), table_is_valid, 4)
        .ok_or_else(|| corrupt!(FORMAT, "neither the allocation table nor its mirror checksums"))?;

    let mut parts = Vec::new();
    for index in 0..MAX_ENTRIES {
        let entry = &directory[index * ENTRY_LEN..(index + 1) * ENTRY_LEN];
        // A free entry is written as all-ones in the game code, which is what an erased entry is.
        if entry[..4].iter().all(|&b| b == 0xFF) {
            continue;
        }

        let first = u16_at(entry, 0x36) as usize;
        let count = u16_at(entry, 0x38) as usize;
        if first < RESERVED_BLOCKS || first >= blocks || count == 0 {
            return Err(corrupt!(FORMAT, "entry {index} starts at block {first} for {count} blocks"));
        }

        let chain = follow_chain(table, first, count, blocks)?;
        let payload: Vec<u8> = chain.iter().flat_map(|&b| block(bytes, b)).copied().collect();

        let game_code = ascii(&entry[0..4]);
        let maker_code = ascii(&entry[4..6]);
        let filename = {
            let field = &entry[8..8 + 32];
            let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
            String::from_utf8_lossy(&field[..end]).into_owned()
        };

        // A GameCube save is opened by game code plus maker code plus filename, so the serial is
        // the pair rather than either alone.
        let game = Some(Game {
            serial: (!game_code.is_empty()).then(|| format!("{game_code}{maker_code}")),
            ..Game::default()
        });

        let inner = Bundle {
            header: Header { system: Some(slug(SYSTEM)), game: game.clone(), ..Header::default() },
            parts: vec![Part::new(0, payload)],
        };

        let mut part = Part::new(u64::try_from(parts.len()).expect("fits"), inner.to_vec()?);
        part.kind = PartKind::Bundle;
        part.role = Some(options.role.clone());
        part.path = Some(filename);
        part.slot = Some(u64::try_from(index).expect("fits"));
        part.dirent = Some(entry.to_vec());
        part.game = game;
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
                // The data capacity: the blocks saves can occupy, which is the card minus the
                // five the header, directory and table take.
                capacity: ((blocks - RESERVED_BLOCKS) * BLOCK) as u64,
                // The header block carries the card's flash serial, which is what a Phantasy Star
                // Online save is signed against, so it is worth keeping across a split.
                system_area: Some(block(bytes, 0).to_vec()),
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
            return Err(corrupt!(FORMAT, "the chain from block {first} loops at block {current}"));
        }
        chain.push(current);
        let next = table_entry(table, current);

        if chain.len() == count {
            if next != CHAIN_END {
                return Err(corrupt!(
                    FORMAT,
                    "the chain from block {first} carries on past the {count} blocks the entry claims"
                ));
            }
            return Ok(chain);
        }
        if next == CHAIN_END {
            return Err(corrupt!(
                FORMAT,
                "the chain from block {first} ended after {} blocks, not the {count} claimed",
                chain.len()
            ));
        }
        let next = next as usize;
        if !(RESERVED_BLOCKS..blocks).contains(&next) {
            return Err(corrupt!(FORMAT, "block {current} links to {next}, which is not a save block"));
        }
        current = next;
    }
}

/// Writes a bundle back out as a raw card image.
///
/// The directory, the allocation table, both their mirrors and every checksum are regenerated.
/// The header block is kept from `system_area` when the bundle carries one, so the card's flash
/// serial survives.
pub fn write(bundle: &Bundle) -> Result<Vec<u8>> {
    // A card with nothing on it carries a whole-card image rather than nested saves, and writing
    // it back is a byte copy.
    if let Some(image) = crate::card::image_only(bundle)? {
        return Ok(image);
    }
    let saves = crate::card::nested_saves(bundle)?;
    if saves.len() > MAX_ENTRIES {
        return Err(Error::NotConvertible(format!(
            "a card holds {MAX_ENTRIES} saves and this bundle has {}",
            saves.len()
        )));
    }

    // Take the card's own size when it states one this console formats, otherwise the smallest
    // that fits: a capacity nothing makes is not a card a console would accept.
    let needed: usize = saves.iter().map(|save| save.bytes.len().div_ceil(BLOCK)).sum();
    let stated = bundle
        .header
        .card
        .as_ref()
        .and_then(|card| usize::try_from(card.capacity).ok())
        .map(|capacity| capacity / BLOCK + RESERVED_BLOCKS);
    let blocks = stated
        .filter(|size| KNOWN_SIZES.contains(size))
        .or_else(|| KNOWN_SIZES.into_iter().find(|size| size - RESERVED_BLOCKS >= needed))
        .ok_or_else(|| {
            Error::NotConvertible(format!("these saves need {needed} blocks, more than any card holds"))
        })?;

    if needed > blocks - RESERVED_BLOCKS {
        return Err(Error::NotConvertible(format!(
            "these saves need {needed} blocks and this card has {}",
            blocks - RESERVED_BLOCKS
        )));
    }

    let mut card = vec![0xFFu8; blocks * BLOCK];
    let mut directory = vec![0xFFu8; BLOCK];
    let mut table = vec![0u8; BLOCK];

    let mut next_block = RESERVED_BLOCKS;
    for (index, save) in saves.iter().enumerate() {
        let count = save.bytes.len().div_ceil(BLOCK);
        let first = next_block;

        for step in 0..count {
            let current = first + step;
            let from = step * BLOCK;
            let to = ((step + 1) * BLOCK).min(save.bytes.len());
            card[current * BLOCK..current * BLOCK + (to - from)].copy_from_slice(&save.bytes[from..to]);

            let link = if step + 1 == count {
                CHAIN_END
            } else {
                u16::try_from(current + 1).expect("block index fits")
            };
            let at = MAP_OFFSET + (current - RESERVED_BLOCKS) * 2;
            table[at..at + 2].copy_from_slice(&link.to_be_bytes());
        }

        let mut entry = save.dirent.clone().unwrap_or_else(|| default_entry(save));
        entry.resize(ENTRY_LEN, 0);
        entry[0x36..0x38].copy_from_slice(&u16::try_from(first).expect("block fits").to_be_bytes());
        entry[0x38..0x3A].copy_from_slice(&u16::try_from(count).expect("count fits").to_be_bytes());
        directory[index * ENTRY_LEN..(index + 1) * ENTRY_LEN].copy_from_slice(&entry);

        next_block += count;
    }

    // The table's own head: an update counter, how many blocks are free, and the last one handed
    // out. Both copies of each region start at the same counter, since neither is stale.
    let free = u16::try_from(blocks - next_block).expect("free count fits");
    table[4..6].copy_from_slice(&0u16.to_be_bytes());
    table[6..8].copy_from_slice(&free.to_be_bytes());
    table[8..10].copy_from_slice(&u16::try_from(next_block - 1).expect("block fits").to_be_bytes());
    directory[BLOCK - 6..BLOCK - 4].copy_from_slice(&0u16.to_be_bytes());

    stamp_directory_checksum(&mut directory);
    stamp_table_checksum(&mut table);

    let header = match bundle.header.card.as_ref().and_then(|c| c.system_area.as_ref()) {
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

/// Puts the directory's checksum pair in its last four bytes.
fn stamp_directory_checksum(region: &mut [u8]) {
    let (sum, inverse) = checksum(&region[..BLOCK - 4]);
    region[BLOCK - 4..BLOCK - 2].copy_from_slice(&sum.to_be_bytes());
    region[BLOCK - 2..].copy_from_slice(&inverse.to_be_bytes());
}

/// Puts the table's checksum pair in its first four bytes.
fn stamp_table_checksum(region: &mut [u8]) {
    let (sum, inverse) = checksum(&region[4..]);
    region[0..2].copy_from_slice(&sum.to_be_bytes());
    region[2..4].copy_from_slice(&inverse.to_be_bytes());
}

fn default_entry(save: &crate::card::NestedSave) -> Vec<u8> {
    let mut entry = vec![0u8; ENTRY_LEN];
    entry[..6].fill(b'-');
    entry[6] = 0xFF;
    if let Some(path) = &save.path {
        let name = path.as_bytes();
        let len = name.len().min(32);
        entry[8..8 + len].copy_from_slice(&name[..len]);
    }
    entry[0x3A..0x3C].copy_from_slice(&0xFFFFu16.to_be_bytes());
    entry
}

/// A freshly formatted header block.
fn default_header(blocks: usize) -> Vec<u8> {
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

    /// Builds a card holding the given saves, as a console would have left it.
    fn card_with(blocks: usize, saves: &[(&str, &str, usize)]) -> Vec<u8> {
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
                        u8::try_from((byte + current * 17) & 0xff).expect("masked to a byte");
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

        card[..BLOCK].copy_from_slice(&default_header(blocks));
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
        // The directory's pair is at the end and the table's at the front. Conflating them makes
        // a card that round-trips here and is rejected by every console and emulator.
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
        let bundle = read(&card, &CardOptions::default()).expect("reads");

        assert_eq!(bundle.parts.len(), 2);
        let card_map = bundle.header.card.as_ref().unwrap();
        assert_eq!(card_map.format.as_str(), "gc-mc");
        // The data capacity is the card minus the five blocks structure takes.
        assert_eq!(card_map.capacity, ((64 - 5) * BLOCK) as u64);
        // The header block carries the flash serial a PSO save is signed against.
        assert_eq!(card_map.system_area.as_ref().map(Vec::len), Some(BLOCK));

        assert_eq!(bundle.parts[0].path.as_deref(), Some("super_mario_sunshine"));
        assert_eq!(bundle.parts[0].dirent.as_ref().map(Vec::len), Some(64));
        // A save is opened by game code plus maker code, so the serial is the pair.
        assert_eq!(bundle.parts[0].game.as_ref().unwrap().serial.as_deref(), Some("GAFE01"));

        let inner = Bundle::from_slice(&bundle.parts[0].bytes().unwrap()).unwrap();
        assert_eq!(inner.parts[0].payload.len(), 2 * BLOCK as u64);
        bundle.validate().expect("valid bundle");
    }

    #[test]
    fn a_card_round_trips_through_a_bundle() {
        let card = card_with(64, &[("GAFE01", "super_mario_sunshine", 2), ("GM4E01", "PSO_SYSTEM", 1)]);
        let bundle = read(&card, &CardOptions::default()).expect("reads");
        assert_eq!(write(&bundle).expect("writes"), card);
    }

    #[test]
    fn a_stale_mirror_loses_to_the_one_counted_higher() {
        // Both halves checksum, so the update counter is what decides. A card caught mid-write
        // has exactly this shape, and reading the stale half would lose the newest save.
        let card = card_with(64, &[("GAFE01", "current", 1)]);
        let mut tampered = card.clone();

        let mut stale = block(&card, DIR_BLOCK).to_vec();
        stale[8..8 + 7].copy_from_slice(b"OLDNAME");
        stale[BLOCK - 6..BLOCK - 4].copy_from_slice(&0u16.to_be_bytes());
        stamp_directory_checksum(&mut stale);

        let mut fresh = block(&card, DIR_BLOCK).to_vec();
        fresh[BLOCK - 6..BLOCK - 4].copy_from_slice(&1u16.to_be_bytes());
        stamp_directory_checksum(&mut fresh);

        // Put the stale copy in the primary and the fresh one in the mirror.
        tampered[DIR_BLOCK * BLOCK..(DIR_BLOCK + 1) * BLOCK].copy_from_slice(&stale);
        tampered[DIR_MIRROR_BLOCK * BLOCK..(DIR_MIRROR_BLOCK + 1) * BLOCK].copy_from_slice(&fresh);

        let bundle = read(&tampered, &CardOptions::default()).expect("reads");
        assert_eq!(bundle.parts[0].path.as_deref(), Some("current"), "the higher counter wins");
    }

    #[test]
    fn a_corrupt_primary_falls_back_to_its_mirror() {
        let card = card_with(64, &[("GAFE01", "onlycopy", 1)]);
        let mut tampered = card.clone();
        // Scribble on the primary directory so only its mirror checksums.
        tampered[DIR_BLOCK * BLOCK + 100] ^= 0xFF;

        let bundle = read(&tampered, &CardOptions::default()).expect("reads through the mirror");
        assert_eq!(bundle.parts[0].path.as_deref(), Some("onlycopy"));
    }

    #[test]
    fn refuses_a_chain_that_loops() {
        let mut card = card_with(64, &[("GAFE01", "loop", 2)]);
        let mut table = block(&card, BAT_BLOCK).to_vec();
        // Point the second block back at the first.
        let at = MAP_OFFSET + (RESERVED_BLOCKS + 1 - RESERVED_BLOCKS) * 2;
        table[at..at + 2].copy_from_slice(&u16::try_from(RESERVED_BLOCKS).unwrap().to_be_bytes());
        stamp_table_checksum(&mut table);
        card[BAT_BLOCK * BLOCK..(BAT_BLOCK + 1) * BLOCK].copy_from_slice(&table);
        card[BAT_MIRROR_BLOCK * BLOCK..(BAT_MIRROR_BLOCK + 1) * BLOCK].copy_from_slice(&table);

        assert!(matches!(read(&card, &CardOptions::default()), Err(Error::Corrupt { .. })));
    }

    #[test]
    fn detects_a_formatted_card_and_not_a_file_of_the_right_length() {
        let card = card_with(64, &[("GAFE01", "x", 1)]);
        assert!(detect(&card));
        // The right length, but nothing that checksums.
        assert!(!detect(&vec![0u8; 64 * BLOCK]));
        assert!(!detect(&vec![0u8; 63 * BLOCK]));
    }
}
