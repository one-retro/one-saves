//! Reading and writing Nintendo 64 Controller Pak images.
//!
//! A pak is 32 KiB of 256-byte pages. Page 0 is the ID area, pages 1 and 2 are the index table and
//! its backup, pages 3 and 4 are the note table, and pages 5 to 127 hold the notes' data. A note's
//! pages are chained through the index table rather than laid out contiguously.
//!
//! ```no_run
//! use n64_cpak::ControllerPak;
//!
//! let pak = ControllerPak::parse(&std::fs::read("pak.mpk")?)?;
//! for note in pak.notes() {
//!     println!("{} ({}) {} bytes", note.path(), note.game_code, note.data.len());
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Note names are not ASCII
//!
//! A note's name is written in the N64 font's own code table, in which the digits start at 16 and
//! the letters at 26. Reading one as bytes yields nonsense; [`font`] is the table.
//!
//! # Two notes can be the same note
//!
//! A pak permits two notes with the same game code and name, so the note table index is the only
//! thing that tells them apart. That is why [`Note`] carries a `slot`.

#![forbid(unsafe_code)]

mod build;
mod error;

pub use build::PakBuilder;
pub use error::{Error, Result};

/// One page: the unit the index table counts.
pub const PAGE: usize = 256;

/// How many pages a pak has.
pub const PAGES: usize = 128;

/// The whole pak, which is also its data capacity.
pub const CAPACITY: usize = PAGE * PAGES;

/// Where the index table lives.
pub const INDEX_PAGE: usize = 1;

/// Where the index table's backup copy lives.
pub const INDEX_BACKUP_PAGE: usize = 2;

/// Where the note table starts. It runs two pages, holding sixteen 32-byte entries.
pub const NOTE_TABLE_PAGE: usize = 3;

/// How many notes a pak holds.
pub const NOTES: usize = 16;

/// A note table entry's length.
pub const NOTE_ENTRY: usize = 32;

/// The first page a note's data can occupy.
pub const FIRST_DATA_PAGE: usize = 5;

/// An index entry meaning "this is the last page of its chain".
pub const CHAIN_END: u16 = 1;

/// An index entry meaning "this page is free".
pub const CHAIN_FREE: u16 = 3;

/// The N64 font's code table, which is what a note name is written in.
pub mod font {
    /// The table itself: the character each code stands for, by index.
    ///
    /// It is not ASCII and not a subset of it: the digits start at 16 and the letters at 26, so
    /// reading a note name as bytes yields nonsense.
    pub const CHARSET: &str =
        "\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ!\"#`*+,-./:=?@";

    /// The character a code stands for, or NUL for one the table does not list.
    #[must_use]
    pub fn decode_char(code: u8) -> char {
        CHARSET.chars().nth(code as usize).unwrap_or('\0')
    }

    /// The code for a character, or the code for a space when the table does not carry it.
    #[must_use]
    pub fn encode_char(character: char) -> u8 {
        let upper = character.to_ascii_uppercase();
        u8::try_from(CHARSET.chars().position(|c| c == upper).unwrap_or(15)).unwrap_or(15)
    }

    /// Decodes a run of codes, trimming the trailing spaces a fixed field pads with.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> String {
        bytes.iter().map(|&b| decode_char(b)).collect::<String>().trim_end().replace('\0', "")
    }

    /// Encodes text into a field of `len` codes, truncating what does not fit.
    #[must_use]
    pub fn encode(text: &str, len: usize) -> Vec<u8> {
        let mut field = vec![0u8; len];
        for (index, character) in text.chars().take(len).enumerate() {
            field[index] = encode_char(character);
        }
        field
    }
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([bytes[offset], bytes[offset + 1]])
}

/// Whether an index table entry could be part of a chain.
fn plausible(entry: u16) -> bool {
    entry == CHAIN_FREE || entry == CHAIN_END || (FIRST_DATA_PAGE..PAGES).contains(&(entry as usize))
}

/// Whether these bytes look like a Controller Pak.
///
/// There is no magic to check, so this leans on the index table: a formatted pak marks every data
/// page free or chained, and the three reserved entries before the first data page are never a
/// plausible chain.
#[must_use]
pub fn detect(bytes: &[u8]) -> bool {
    if bytes.len() != CAPACITY {
        return false;
    }
    let index = &bytes[INDEX_PAGE * PAGE..(INDEX_PAGE + 1) * PAGE];
    (FIRST_DATA_PAGE..PAGES).all(|page| plausible(u16_at(index, page * 2)))
}

/// One note on a pak.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    /// The note table index it occupied.
    ///
    /// Two notes may agree on game code and name, so this is the only thing that keeps them apart.
    /// A [`PakBuilder`] allocates its own, in the order notes are added.
    pub slot: usize,
    /// The four-character game code, in ASCII rather than the font table.
    pub game_code: String,
    /// The note's name, decoded from the font table.
    pub name: String,
    /// The note's extension, decoded from the font table. Usually empty.
    pub extension: String,
    /// The note table entry's 32 bytes, verbatim and opaque.
    ///
    /// Empty when a note was built by hand rather than read off a pak; a builder then writes an
    /// entry carrying the name and extension and nothing else.
    pub dirent: Vec<u8>,
    /// The note's bytes: every page it occupies, in chain order.
    pub data: Vec<u8>,
}

impl Note {
    /// The note's name and extension as one path, which is how a filesystem would spell it.
    #[must_use]
    pub fn path(&self) -> String {
        if self.extension.is_empty() {
            self.name.clone()
        } else {
            format!("{}.{}", self.name, self.extension)
        }
    }
}

/// A parsed Controller Pak image.
#[derive(Debug, Clone)]
pub struct ControllerPak {
    image: Vec<u8>,
    notes: Vec<Note>,
}

impl ControllerPak {
    /// Reads a pak image.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != CAPACITY {
            return Err(Error::WrongLength(bytes.len()));
        }
        let index = index_table(bytes)?;
        let mut notes = Vec::new();

        for slot in 0..NOTES {
            let offset = NOTE_TABLE_PAGE * PAGE + slot * NOTE_ENTRY;
            let entry = &bytes[offset..offset + NOTE_ENTRY];

            let start = u16_at(entry, 6) as usize;
            // A note with no plausible start page is an empty slot, not a broken pak.
            if !(FIRST_DATA_PAGE..PAGES).contains(&start) {
                continue;
            }

            let chain = follow_chain(&index, start)?;
            let data =
                chain.iter().flat_map(|&page| &bytes[page * PAGE..(page + 1) * PAGE]).copied().collect();

            notes.push(Note {
                slot,
                game_code: decode_ascii(&entry[0..4]),
                name: font::decode(&entry[0x10..0x20]),
                extension: font::decode(&entry[0x0C..0x10]),
                dirent: entry.to_vec(),
                data,
            });
        }
        Ok(ControllerPak { image: bytes.to_vec(), notes })
    }

    /// Every note on the pak, in note table order.
    #[must_use]
    pub fn notes(&self) -> &[Note] {
        &self.notes
    }

    /// Takes the notes, for a caller that is going to rebuild rather than read on.
    #[must_use]
    pub fn into_notes(self) -> Vec<Note> {
        self.notes
    }

    /// The whole image.
    #[must_use]
    pub fn image(&self) -> &[u8] {
        &self.image
    }

    /// The pak's data capacity, which for this format is the whole pak.
    #[must_use]
    pub fn capacity(&self) -> usize {
        CAPACITY
    }

    /// The pak's own 32-byte label, out of the ID area.
    ///
    /// It belongs to no note, so a caller that splits a pak into its notes and keeps nothing else
    /// drops it.
    #[must_use]
    pub fn label(&self) -> &[u8] {
        &self.image[..NOTE_ENTRY]
    }
}

/// Reads the index table, falling back to the backup copy when the primary is unusable.
fn index_table(bytes: &[u8]) -> Result<Vec<u16>> {
    for page in [INDEX_PAGE, INDEX_BACKUP_PAGE] {
        let table = &bytes[page * PAGE..(page + 1) * PAGE];
        let entries: Vec<u16> = (0..PAGES).map(|i| u16_at(table, i * 2)).collect();
        if entries[FIRST_DATA_PAGE..].iter().copied().all(plausible) {
            return Ok(entries);
        }
    }
    Err(Error::NotAControllerPak)
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
            return Err(Error::Corrupt(format!("page {current} links to {page}, which is not a data page")));
        }
        if chain.contains(&page) {
            return Err(Error::Corrupt(format!("the chain from page {start} loops at page {page}")));
        }
        chain.push(page);
        current = page;
    }
}

fn decode_ascii(bytes: &[u8]) -> String {
    bytes.iter().filter(|&&b| b.is_ascii_graphic()).map(|&b| b as char).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a pak holding the given notes, as a console would have left it.
    ///
    /// Hand-rolled rather than built with [`PakBuilder`] on purpose: the round-trip test is that
    /// what this crate writes matches what a console wrote, so the fixture cannot come from the
    /// writer under test.
    pub(crate) fn pak_with(notes: &[(&str, &str, usize)]) -> Vec<u8> {
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
                    pak[page * PAGE + byte] = u8::try_from((byte + page * 13) & 0xff).expect("a byte");
                }
                index[page] =
                    if step + 1 == *pages { CHAIN_END } else { u16::try_from(page + 1).expect("page fits") };
            }
            let offset = NOTE_TABLE_PAGE * PAGE + slot * NOTE_ENTRY;
            pak[offset..offset + 4].copy_from_slice(code.as_bytes());
            pak[offset + 6..offset + 8]
                .copy_from_slice(&u16::try_from(first).expect("page fits").to_be_bytes());
            pak[offset + 0x10..offset + 0x20].copy_from_slice(&font::encode(name, 16));
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
        assert_eq!(font::decode_char(15), ' ');
        assert_eq!(font::decode_char(16), '0');
        assert_eq!(font::decode_char(26), 'A');
        assert_eq!(font::decode(&[26, 27, 28, 15, 15]), "ABC");
        assert_eq!(font::encode_char('A'), 26);
        assert_eq!(font::encode_char('0'), 16);
        assert_eq!(font::encode("AB", 4), vec![26, 27, 0, 0]);
    }

    #[test]
    fn reads_notes_and_keeps_the_pak_label() {
        let pak = pak_with(&[("NMKJ", "MARIOKART", 2), ("NSMJ", "SUPERMARIO", 1)]);
        let parsed = ControllerPak::parse(&pak).expect("reads");

        assert_eq!(parsed.notes().len(), 2);
        assert_eq!(parsed.notes()[0].name, "MARIOKART");
        assert_eq!(parsed.notes()[0].game_code, "NMKJ");
        assert_eq!(parsed.notes()[0].data.len(), 2 * PAGE);
        assert_eq!(parsed.notes()[0].dirent.len(), NOTE_ENTRY);
        assert_eq!(parsed.label(), [0xAAu8; NOTE_ENTRY]);
    }

    #[test]
    fn two_notes_of_one_game_stay_distinct_by_slot() {
        // A pak permits this, and the table index is the only thing that tells them apart.
        let pak = pak_with(&[("NMKJ", "GHOST", 1), ("NMKJ", "GHOST", 1)]);
        let parsed = ControllerPak::parse(&pak).expect("reads");
        assert_eq!(parsed.notes()[0].path(), parsed.notes()[1].path());
        assert_ne!(parsed.notes()[0].slot, parsed.notes()[1].slot);
    }

    #[test]
    fn a_name_and_extension_read_back_as_one_path() {
        let mut pak = pak_with(&[("NMKJ", "SAVE", 1)]);
        let offset = NOTE_TABLE_PAGE * PAGE + 0x0C;
        pak[offset..offset + 4].copy_from_slice(&font::encode("DAT", 4));
        let parsed = ControllerPak::parse(&pak).expect("reads");
        assert_eq!(parsed.notes()[0].path(), "SAVE.DAT");
    }

    #[test]
    fn refuses_a_chain_that_loops() {
        let mut pak = pak_with(&[("NMKJ", "LOOP", 2)]);
        let entry = u16::try_from(FIRST_DATA_PAGE).expect("page fits").to_be_bytes();
        pak[INDEX_PAGE * PAGE + (FIRST_DATA_PAGE + 1) * 2..][..2].copy_from_slice(&entry);
        pak[INDEX_BACKUP_PAGE * PAGE + (FIRST_DATA_PAGE + 1) * 2..][..2].copy_from_slice(&entry);
        assert!(matches!(ControllerPak::parse(&pak), Err(Error::Corrupt(_))));
    }

    #[test]
    fn falls_back_to_the_backup_index_table() {
        let mut pak = pak_with(&[("NMKJ", "MARIOKART", 2)]);
        // Scribble an implausible entry into the primary table only.
        pak[INDEX_PAGE * PAGE + FIRST_DATA_PAGE * 2..][..2].copy_from_slice(&0xBEEFu16.to_be_bytes());
        let parsed = ControllerPak::parse(&pak).expect("reads through the backup");
        assert_eq!(parsed.notes()[0].name, "MARIOKART");
    }

    #[test]
    fn refuses_something_that_is_not_a_pak() {
        assert!(matches!(ControllerPak::parse(&[1, 2, 3]), Err(Error::WrongLength(3))));
        assert!(!detect(&[1, 2, 3]));
    }
}
