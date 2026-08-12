//! Nintendo 64 Controller Paks.
//!
//! A pak is 32 KiB of 256-byte pages. Page 0 is the ID area, pages 1 and 2 are the index table
//! and its backup, pages 3 and 4 are the note table, and pages 5 to 127 hold the notes' data.
//! A note's pages are chained through the index table rather than laid out contiguously.
//!
//! A pak permits two notes with the same game code and note name, which is why a producer must
//! carry [`slot`](one_saves::Part::slot): without it the two would share an address and nothing
//! could tell them apart.

use one_saves::{Bundle, Card, Game, Header, Part, PartKind, Slug};

use crate::CardOptions;
use crate::error::{Error, Result, corrupt};

const FORMAT: &str = "N64 Controller Pak";

/// The card format slug a bundle carries.
pub const CARD_FORMAT: &str = "n64-cpak";

/// The system slug these saves are for.
pub const SYSTEM: &str = "n64";

const PAGE: usize = 256;
const PAGES: usize = 128;
/// The whole pak, which is also its data capacity.
pub const CAPACITY: usize = PAGE * PAGES;

/// Where the index table lives, and its backup.
const INDEX_PAGE: usize = 1;
const INDEX_BACKUP_PAGE: usize = 2;
/// Where the note table starts; it runs two pages, holding sixteen 32-byte entries.
const NOTE_TABLE_PAGE: usize = 3;
const NOTES: usize = 16;
const NOTE_ENTRY: usize = 32;
/// The first page a note's data can occupy.
const FIRST_DATA_PAGE: usize = 5;

/// An index entry meaning "this is the last page of its chain".
const CHAIN_END: u16 = 1;
/// An index entry meaning "this page is free".
const CHAIN_FREE: u16 = 3;

/// The N64 font code table, which is what a note name is written in.
///
/// It is not ASCII and not a subset of it: the digits start at 16 and the letters at 26, so
/// reading a note name as bytes yields nonsense.
const CHARSET: &str = "\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ!\"#`*+,-./:=?@";

fn decode_char(code: u8) -> char {
    CHARSET.chars().nth(code as usize).unwrap_or('\0')
}

fn encode_char(character: char) -> u8 {
    let upper = character.to_ascii_uppercase();
    u8::try_from(CHARSET.chars().position(|c| c == upper).unwrap_or(15)).unwrap_or(15)
}

/// Decodes a run of N64 font codes, trimming the trailing spaces a fixed field pads with.
fn decode_name(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| decode_char(b)).collect::<String>().trim_end().replace('\0', "")
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([bytes[offset], bytes[offset + 1]])
}

/// Whether these bytes look like a Controller Pak.
///
/// There is no magic to check, so this leans on the index table: a formatted pak marks every
/// data page free or chained, and the three reserved entries before the first data page are
/// never a plausible chain.
#[must_use]
pub fn detect(bytes: &[u8]) -> bool {
    if bytes.len() != CAPACITY {
        return false;
    }
    let index = &bytes[INDEX_PAGE * PAGE..(INDEX_PAGE + 1) * PAGE];
    (FIRST_DATA_PAGE..PAGES).all(|page| {
        let entry = u16_at(index, page * 2);
        entry == CHAIN_FREE || entry == CHAIN_END || (FIRST_DATA_PAGE..PAGES).contains(&(entry as usize))
    })
}

/// Reads a pak into a bundle: one nested bundle per note.
pub fn read(bytes: &[u8], options: &CardOptions) -> Result<Bundle> {
    if bytes.len() != CAPACITY {
        return Err(Error::WrongLength { format: FORMAT, expected: "32768 bytes", found: bytes.len() });
    }
    let index = index_table(bytes)?;
    let mut parts = Vec::new();

    for note in 0..NOTES {
        let offset = NOTE_TABLE_PAGE * PAGE + note * NOTE_ENTRY;
        let entry = &bytes[offset..offset + NOTE_ENTRY];

        let start = u16_at(entry, 6) as usize;
        // A note with no plausible start page is an empty slot, not a broken pak.
        if !(FIRST_DATA_PAGE..PAGES).contains(&start) {
            continue;
        }

        let chain = follow_chain(&index, start)?;
        let payload: Vec<u8> =
            chain.iter().flat_map(|&page| &bytes[page * PAGE..(page + 1) * PAGE]).copied().collect();

        // The game code and note name together are how a pak names a note, and neither is unique.
        let game_code = decode_ascii(&entry[0..4]);
        let name = decode_name(&entry[0x10..0x20]);
        let extension = decode_name(&entry[0x0C..0x10]);
        let path = if extension.is_empty() { name.clone() } else { format!("{name}.{extension}") };

        let game = (!game_code.is_empty()).then(|| Game {
            serial: Some(game_code),
            name: (!name.is_empty()).then(|| name.clone()),
            ..Game::default()
        });

        let inner = Bundle {
            header: Header {
                system: Some(Slug::parse(SYSTEM).expect("valid slug")),
                game: game.clone(),
                ..Header::default()
            },
            parts: vec![Part::new(0, payload)],
        };

        let mut part = Part::new(u64::try_from(parts.len()).expect("fits"), inner.to_vec()?);
        part.kind = PartKind::Bundle;
        part.role = Some(options.role.clone());
        part.path = Some(path);
        // Two notes may agree on game code and name, so the table index is what keeps them apart.
        part.slot = Some(u64::try_from(note).expect("fits"));
        part.dirent = Some(entry.to_vec());
        part.game = game;
        part.system = Some(Slug::parse(SYSTEM).expect("valid slug"));
        parts.push(part);
    }

    if parts.is_empty() {
        parts.push(crate::card::card_image_part(0, bytes, &options.role));
    }

    Ok(Bundle {
        header: Header {
            system: Some(Slug::parse(SYSTEM).expect("valid slug")),
            card: Some(Card {
                format: Slug::parse(CARD_FORMAT).expect("valid slug"),
                capacity: CAPACITY as u64,
                // The pak's own 32-byte label belongs to no note, so splitting would drop it.
                system_area: Some(bytes[..NOTE_ENTRY].to_vec()),
                unknown: one_saves::UnknownKeys::new(),
            }),
            source: options.source.clone(),
            ..Header::default()
        },
        parts,
    })
}

/// Reads the index table, falling back to the backup copy when the primary is unusable.
fn index_table(bytes: &[u8]) -> Result<Vec<u16>> {
    for page in [INDEX_PAGE, INDEX_BACKUP_PAGE] {
        let table = &bytes[page * PAGE..(page + 1) * PAGE];
        let entries: Vec<u16> = (0..PAGES).map(|i| u16_at(table, i * 2)).collect();
        let usable = entries[FIRST_DATA_PAGE..].iter().all(|&entry| {
            entry == CHAIN_FREE || entry == CHAIN_END || (FIRST_DATA_PAGE..PAGES).contains(&(entry as usize))
        });
        if usable {
            return Ok(entries);
        }
    }
    Err(corrupt!(FORMAT, "neither the index table nor its backup is readable"))
}

fn follow_chain(index: &[u16], start: usize) -> Result<Vec<usize>> {
    let mut chain = vec![start];
    let mut current = start;
    loop {
        let next = index[current];
        if next == CHAIN_END {
            return Ok(chain);
        }
        let page = next as usize;
        if !(FIRST_DATA_PAGE..PAGES).contains(&page) {
            return Err(corrupt!(FORMAT, "page {current} links to {page}, which is not a data page"));
        }
        if chain.contains(&page) {
            return Err(corrupt!(FORMAT, "the chain from page {start} loops at page {page}"));
        }
        chain.push(page);
        current = page;
    }
}

fn decode_ascii(bytes: &[u8]) -> String {
    bytes.iter().filter(|&&b| b.is_ascii_graphic()).map(|&b| b as char).collect()
}

/// Writes a bundle back out as a raw 32 KiB pak.
///
/// The index table, its backup and the note table are all regenerated; the pak's label is kept
/// from `system_area` when the bundle carries one.
pub fn write(bundle: &Bundle) -> Result<Vec<u8>> {
    // A card with nothing on it carries a whole-card image rather than nested saves, and writing
    // it back is a byte copy.
    if let Some(image) = crate::card::image_only(bundle)? {
        return Ok(image);
    }
    let saves = crate::card::nested_saves(bundle)?;
    if saves.len() > NOTES {
        return Err(Error::NotConvertible(format!(
            "a pak holds {NOTES} notes and this bundle has {}",
            saves.len()
        )));
    }

    let mut pak = vec![0u8; CAPACITY];
    if let Some(area) = bundle.header.card.as_ref().and_then(|c| c.system_area.as_ref()) {
        let len = area.len().min(NOTE_ENTRY);
        pak[..len].copy_from_slice(&area[..len]);
    }

    let mut index = vec![CHAIN_FREE; PAGES];
    // The three pages before the data area are structure rather than storage.
    for entry in index.iter_mut().take(FIRST_DATA_PAGE) {
        *entry = 0;
    }

    let mut next_page = FIRST_DATA_PAGE;
    for (note, save) in saves.iter().enumerate() {
        let pages = save.bytes.len().div_ceil(PAGE);
        if next_page + pages > PAGES {
            return Err(Error::NotConvertible(format!(
                "these notes need {} pages, and a pak has {}",
                next_page - FIRST_DATA_PAGE + pages,
                PAGES - FIRST_DATA_PAGE
            )));
        }

        let first = next_page;
        for step in 0..pages {
            let page = first + step;
            let from = step * PAGE;
            let to = ((step + 1) * PAGE).min(save.bytes.len());
            pak[page * PAGE..page * PAGE + (to - from)].copy_from_slice(&save.bytes[from..to]);
            index[page] =
                if step + 1 == pages { CHAIN_END } else { u16::try_from(page + 1).expect("page index fits") };
        }

        // The note entry comes back verbatim, with only its start page rewritten: block placement
        // is the allocator's business and everything else in the entry is the game's.
        let mut entry = save.dirent.clone().unwrap_or_else(|| default_entry(save));
        entry.resize(NOTE_ENTRY, 0);
        entry[6..8].copy_from_slice(&u16::try_from(first).expect("page fits").to_be_bytes());
        let offset = NOTE_TABLE_PAGE * PAGE + note * NOTE_ENTRY;
        pak[offset..offset + NOTE_ENTRY].copy_from_slice(&entry);

        next_page += pages;
    }

    for (page, entry) in index.iter().enumerate() {
        let bytes = entry.to_be_bytes();
        pak[INDEX_PAGE * PAGE + page * 2..INDEX_PAGE * PAGE + page * 2 + 2].copy_from_slice(&bytes);
        pak[INDEX_BACKUP_PAGE * PAGE + page * 2..INDEX_BACKUP_PAGE * PAGE + page * 2 + 2]
            .copy_from_slice(&bytes);
    }
    Ok(pak)
}

/// A note entry for a save that arrived without one.
fn default_entry(save: &crate::card::NestedSave) -> Vec<u8> {
    let mut entry = vec![0u8; NOTE_ENTRY];
    if let Some(path) = &save.path {
        let (name, extension) = path.split_once('.').unwrap_or((path.as_str(), ""));
        for (index, character) in name.chars().take(16).enumerate() {
            entry[0x10 + index] = encode_char(character);
        }
        for (index, character) in extension.chars().take(4).enumerate() {
            entry[0x0C + index] = encode_char(character);
        }
    }
    entry
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a pak holding the given notes, each of the given page count.
    fn pak_with(notes: &[(&str, &str, usize)]) -> Vec<u8> {
        let mut pak = vec![0u8; CAPACITY];
        pak[..NOTE_ENTRY].copy_from_slice(&[0xAAu8; NOTE_ENTRY]);

        let mut index = vec![CHAIN_FREE; PAGES];
        for entry in index.iter_mut().take(FIRST_DATA_PAGE) {
            *entry = 0;
        }

        let mut next = FIRST_DATA_PAGE;
        for (slot, (code, name, pages)) in notes.iter().enumerate() {
            let first = next;
            for step in 0..*pages {
                let page = first + step;
                for byte in 0..PAGE {
                    pak[page * PAGE + byte] =
                        u8::try_from((byte + page * 13) & 0xff).expect("masked to a byte");
                }
                index[page] =
                    if step + 1 == *pages { CHAIN_END } else { u16::try_from(page + 1).expect("page fits") };
            }
            let offset = NOTE_TABLE_PAGE * PAGE + slot * NOTE_ENTRY;
            pak[offset..offset + 4].copy_from_slice(code.as_bytes());
            pak[offset + 6..offset + 8]
                .copy_from_slice(&u16::try_from(first).expect("page fits").to_be_bytes());
            for (i, c) in name.chars().take(16).enumerate() {
                pak[offset + 0x10 + i] = encode_char(c);
            }
            next += pages;
        }
        for (page, entry) in index.iter().enumerate() {
            let bytes = entry.to_be_bytes();
            pak[INDEX_PAGE * PAGE + page * 2..][..2].copy_from_slice(&bytes);
            pak[INDEX_BACKUP_PAGE * PAGE + page * 2..][..2].copy_from_slice(&bytes);
        }
        pak
    }

    #[test]
    fn the_font_table_is_not_ascii() {
        // Reading a note name as bytes yields nonsense, which is the whole reason for the table.
        assert_eq!(decode_char(15), ' ');
        assert_eq!(decode_char(16), '0');
        assert_eq!(decode_char(26), 'A');
        assert_eq!(decode_name(&[26, 27, 28, 15, 15]), "ABC");
        assert_eq!(encode_char('A'), 26);
        assert_eq!(encode_char('0'), 16);
    }

    #[test]
    fn reads_notes_and_keeps_the_pak_label() {
        let pak = pak_with(&[("NMKJ", "MARIOKART", 2), ("NSMJ", "SUPERMARIO", 1)]);
        let bundle = read(&pak, &CardOptions::default()).expect("reads");

        assert_eq!(bundle.parts.len(), 2);
        let card = bundle.header.card.as_ref().unwrap();
        assert_eq!(card.format.as_str(), "n64-cpak");
        assert_eq!(card.capacity, 32768);
        // The label belongs to no note, so splitting the pak would otherwise drop it.
        assert_eq!(card.system_area.as_ref().map(Vec::len), Some(32));

        assert_eq!(bundle.parts[0].path.as_deref(), Some("MARIOKART"));
        assert_eq!(bundle.parts[0].dirent.as_ref().map(Vec::len), Some(32));
        assert_eq!(bundle.parts[0].game.as_ref().unwrap().serial.as_deref(), Some("NMKJ"));

        let inner = Bundle::from_slice(&bundle.parts[0].bytes().unwrap()).unwrap();
        assert_eq!(inner.parts[0].payload.len(), 512, "two pages");
        bundle.validate().expect("valid bundle");
    }

    #[test]
    fn two_notes_of_one_game_stay_distinct_by_slot() {
        // A pak permits this, and slot is the only thing that tells them apart.
        let pak = pak_with(&[("NMKJ", "GHOST", 1), ("NMKJ", "GHOST", 1)]);
        let bundle = read(&pak, &CardOptions::default()).expect("reads");
        assert_eq!(bundle.parts[0].path, bundle.parts[1].path);
        assert_ne!(bundle.parts[0].slot, bundle.parts[1].slot);
        bundle.validate().expect("slot keeps the two addresses apart");
    }

    #[test]
    fn a_pak_round_trips_through_a_bundle() {
        let pak = pak_with(&[("NMKJ", "MARIOKART", 2), ("NSMJ", "SUPERMARIO", 1)]);
        let bundle = read(&pak, &CardOptions::default()).expect("reads");
        assert_eq!(write(&bundle).expect("writes"), pak);
    }

    #[test]
    fn refuses_a_chain_that_loops() {
        let mut pak = pak_with(&[("NMKJ", "LOOP", 2)]);
        // Point the second page back at the first.
        let entry = u16::try_from(FIRST_DATA_PAGE).expect("page fits").to_be_bytes();
        pak[INDEX_PAGE * PAGE + (FIRST_DATA_PAGE + 1) * 2..][..2].copy_from_slice(&entry);
        pak[INDEX_BACKUP_PAGE * PAGE + (FIRST_DATA_PAGE + 1) * 2..][..2].copy_from_slice(&entry);
        assert!(matches!(read(&pak, &CardOptions::default()), Err(Error::Corrupt { .. })));
    }
}
