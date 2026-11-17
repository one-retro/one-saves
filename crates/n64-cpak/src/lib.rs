//! Reading and writing Nintendo 64 Controller Pak images.
//!
//! A pak is 32 KiB of 256-byte pages. Page 0 is the ID area, pages 1 and 2 are the index table and
//! its backup, pages 3 and 4 are the note table, and pages 5 to 127 hold the notes' data. A note's
//! pages are chained through the index table rather than laid out contiguously.
//!
//! # A pak of several banks
//!
//! A large pak is banks of 32 KiB that libultra switches between, up to 62 of them, and it is one
//! filesystem whose notes run on from one bank into the next. Its ID block records how many
//! banks, and bank 0 holds all the structure: an index page for every bank, then a backup of each,
//! then the note table. [`Layout`] says where each sits. An index entry names the bank in its high
//! byte and the page in its low one, which on a pak of one bank is the page alone. ares and
//! N64FlashcartMenu keep such a pak as one file, bank 0 first.
//!
//! A pak that switches with a button rather than through libultra shows the console one bank at a
//! time, and each position records one bank: each is a pak of its own, read as one.
//!
//! ```no_run
//! use n64_cpak::ControllerPak;
//!
//! let pak = ControllerPak::parse(&std::fs::read("pak.mpk")?)?;
//! for note in pak.notes() {
//!     println!("{} ({}) {} bytes", note.name, note.game_code, note.data.len());
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Note names are not ASCII
//!
//! A note's name is written in the N64 font's own code table, in which the digits start at 16 and
//! the letters at 26, and katakana follows the punctuation. Reading one as bytes yields nonsense;
//! [`font`] is the table. [`Note::name`] is the name to show, and [`Note::path`] the exact
//! spelling, which two readers of one note agree on.
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

/// The most banks a pak can have, which is libultra's `PFS_MAX_BANKS`.
pub const MAX_BANKS: usize = 62;

/// Where a pak of so many banks keeps its structure, all of it in bank 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    banks: usize,
}

impl Layout {
    /// The layout of a pak of `banks` banks, 1 to [`MAX_BANKS`].
    #[must_use]
    pub fn new(banks: usize) -> Option<Self> {
        (1..=MAX_BANKS).contains(&banks).then_some(Layout { banks })
    }

    /// The layout of a pak image `len` bytes long: a whole number of banks.
    #[must_use]
    pub fn of_len(len: usize) -> Option<Self> {
        if len.is_multiple_of(CAPACITY) { Layout::new(len / CAPACITY) } else { None }
    }

    /// How many banks.
    #[must_use]
    pub fn banks(self) -> usize {
        self.banks
    }

    /// How long the image is.
    #[must_use]
    pub fn image_len(self) -> usize {
        self.banks * CAPACITY
    }

    /// The page in bank 0 holding a bank's index table.
    #[must_use]
    pub fn index_page(self, bank: usize) -> usize {
        INDEX_PAGE + bank
    }

    /// The page in bank 0 holding the backup of a bank's index table.
    #[must_use]
    pub fn index_backup_page(self, bank: usize) -> usize {
        INDEX_PAGE + self.banks + bank
    }

    /// The first of the two pages in bank 0 that hold the note table.
    #[must_use]
    pub fn note_table_page(self) -> usize {
        INDEX_PAGE + 2 * self.banks
    }

    /// The first page of a bank that holds data. In bank 0 that is the page after the note table;
    /// in every other, page 1, since entry 0 of its index table is the table's checksum.
    #[must_use]
    pub fn first_data_page(self, bank: usize) -> usize {
        if bank == 0 { self.note_table_page() + 2 } else { 1 }
    }

    /// How many pages hold data, over every bank.
    #[must_use]
    pub fn data_pages(self) -> usize {
        (0..self.banks).map(|bank| PAGES - self.first_data_page(bank)).sum()
    }

    /// Whether a bank and page name a page that holds data.
    fn is_data_page(self, bank: usize, page: usize) -> bool {
        bank < self.banks && (self.first_data_page(bank)..PAGES).contains(&page)
    }

    /// Whether an index table entry could be part of a chain.
    fn plausible(self, entry: u16) -> bool {
        let [bank, page] = entry.to_be_bytes();
        entry == CHAIN_FREE || entry == CHAIN_END || self.is_data_page(bank.into(), page.into())
    }
}

/// How many banks a pak's ID block records, out of the first of its four copies whose checksums
/// hold, where that is a count a pak can have.
#[must_use]
pub fn recorded_banks(bytes: &[u8]) -> Option<usize> {
    build::ID_BLOCKS.iter().find_map(|&at| {
        let block = bytes.get(at..at + 0x20)?;
        let sum = build::id_checksum(block);
        let sealed = u16_at(block, 0x1C) == sum && u16_at(block, 0x1E) == 0xFFF2u16.wrapping_sub(sum);
        sealed.then(|| usize::from(block[0x1A])).filter(|banks| (1..=MAX_BANKS).contains(banks))
    })
}

/// An index entry meaning "this is the last page of its chain".
pub const CHAIN_END: u16 = 1;

/// An index entry meaning "this page is free".
pub const CHAIN_FREE: u16 = 3;

/// The N64 font's code table, which is what a note name is written in.
///
/// It is the table [N64brew](https://n64brew.dev/wiki/Controller_Pak/Filesystem) documents: a
/// space at `0x0F`, the digits from `0x10`, the letters from `0x1A`, fourteen marks of
/// punctuation from `0x34`, and katakana with its marks from `0x42` to `0x94`. `0x00` is the NUL
/// that pads a field, and no other code has a character.
pub mod font {
    /// The table itself: the character each code stands for, by index, NUL where there is none.
    ///
    /// It is not ASCII and not a subset of it: the digits start at 16 and the letters at 26, so
    /// reading a note name as bytes yields nonsense.
    pub const CHARSET: &str = concat!(
        "\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
        " 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ!\"#'*+,-./:=?@",
        "。゛゜ァィゥェォッャュョヲン",
        "アイウエオカキクケコサシスセソタチツテトナニヌネノハヒフヘホマミムメモヤユヨラリルレロワ",
        "ガギグゲゴザジズゼゾダヂヅデドバビブベボパピプペポ",
    );

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

    /// A field as text to show: up to its first NUL, any code the table leaves empty left out,
    /// and trailing spaces trimmed.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> String {
        until_nul(bytes)
            .iter()
            .map(|&b| decode_char(b))
            .filter(|&c| c != '\0')
            .collect::<String>()
            .trim_end()
            .to_owned()
    }

    /// A field spelled exactly: up to its first NUL, every code through the table, spaces and
    /// all. `None` when a code before the NUL has no character, since nothing then spells it.
    ///
    /// Where [`decode`] tidies a name up to show, this is the spelling two readers of one field
    /// agree on.
    #[must_use]
    pub fn spell(bytes: &[u8]) -> Option<String> {
        until_nul(bytes).iter().map(|&b| Some(decode_char(b)).filter(|&c| c != '\0')).collect()
    }

    /// A field up to its first NUL, which pads it.
    fn until_nul(bytes: &[u8]) -> &[u8] {
        &bytes[..bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len())]
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

/// Whether these bytes look like a Controller Pak.
///
/// There is no magic to check, so this leans on the index table: a formatted pak marks every data
/// page free or chained, and the reserved entries before the first data page are never a
/// plausible chain. A pak of several banks has to say so in its ID block as well, since four paks
/// kept in one file, as mupen64plus keeps them, are as long as one pak of four banks.
#[must_use]
pub fn detect(bytes: &[u8]) -> bool {
    let Some(layout) = Layout::of_len(bytes.len()) else {
        return false;
    };
    if layout.banks > 1 && recorded_banks(bytes) != Some(layout.banks) {
        return false;
    }
    let index = &bytes[layout.index_page(0) * PAGE..][..PAGE];
    (layout.first_data_page(0)..PAGES).all(|page| layout.plausible(u16_at(index, page * 2)))
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
    /// The note's name, decoded from the font table to show, as [`font::decode`] does.
    pub name: String,
    /// The note's extension, decoded the same way. Usually empty.
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
    /// The note's name and extension as one path: the name [spelled](font::spell), then a dot
    /// and the spelled extension where the extension's first code is not NUL. `GAME` with the
    /// extension `A` is `GAME.A`.
    ///
    /// Read off the note's entry, so it is what the pak holds rather than the name tidied up to
    /// show. `None` when either field holds a code with no character, or when the entry is not
    /// an entry's length. A note built by hand, with no entry, is spelled as the entry a
    /// [`PakBuilder`] would write for it.
    #[must_use]
    pub fn path(&self) -> Option<String> {
        let entry = match self.dirent.len() {
            NOTE_ENTRY => self.dirent.clone(),
            0 => build::note_entry(self),
            _ => return None,
        };
        let name = font::spell(&entry[0x10..0x20])?;
        if entry[0x0C] == 0 {
            return Some(name);
        }
        Some(format!("{name}.{}", font::spell(&entry[0x0C..0x10])?))
    }
}

/// A parsed Controller Pak image.
#[derive(Debug, Clone)]
pub struct ControllerPak {
    image: Vec<u8>,
    layout: Layout,
    notes: Vec<Note>,
}

impl ControllerPak {
    /// Reads a pak image: 32 KiB a bank, as many banks as its ID block records.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let layout = Layout::of_len(bytes.len()).ok_or(Error::WrongLength(bytes.len()))?;
        if let Some(recorded) = recorded_banks(bytes)
            && recorded != layout.banks
        {
            return Err(Error::Corrupt(format!(
                "its ID block records {recorded} bank(s), and the image holds {}",
                layout.banks
            )));
        }
        let index = index_tables(bytes, layout)?;
        let mut notes = Vec::new();

        for slot in 0..NOTES {
            let offset = layout.note_table_page() * PAGE + slot * NOTE_ENTRY;
            let entry = &bytes[offset..offset + NOTE_ENTRY];

            let start = (usize::from(entry[6]), usize::from(entry[7]));
            // A note with no plausible start page is an empty slot, not a broken pak.
            if !layout.is_data_page(start.0, start.1) {
                continue;
            }

            let chain = follow_chain(&index, layout, start)?;
            let data = chain
                .iter()
                .flat_map(|&(bank, page)| &bytes[bank * CAPACITY + page * PAGE..][..PAGE])
                .copied()
                .collect();

            notes.push(Note {
                slot,
                game_code: decode_ascii(&entry[0..4]),
                name: font::decode(&entry[0x10..0x20]),
                extension: font::decode(&entry[0x0C..0x10]),
                dirent: entry.to_vec(),
                data,
            });
        }
        Ok(ControllerPak { image: bytes.to_vec(), layout, notes })
    }

    /// Where the pak keeps its structure, which follows from how many banks it has.
    #[must_use]
    pub fn layout(&self) -> Layout {
        self.layout
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

    /// The pak's data capacity, which for this format is the whole pak: 32 KiB a bank.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.layout.image_len()
    }

    /// The pak's own 32-byte label, out of the ID area.
    ///
    /// It belongs to no note, so a caller that splits a pak into its notes and keeps nothing else
    /// drops it.
    #[must_use]
    pub fn label(&self) -> &[u8] {
        &self.image[..NOTE_ENTRY]
    }

    /// The whole of page 0: the label, and the ID block a console identifies the pak by.
    ///
    /// What [`PakBuilder::id_area`] takes to rebuild this pak as itself rather than as a freshly
    /// formatted one.
    #[must_use]
    pub fn id_area(&self) -> &[u8] {
        &self.image[..PAGE]
    }
}

/// Reads every bank's index table, each falling back to its backup when the primary is unusable.
fn index_tables(bytes: &[u8], layout: Layout) -> Result<Vec<Vec<u16>>> {
    (0..layout.banks)
        .map(|bank| {
            [layout.index_page(bank), layout.index_backup_page(bank)]
                .into_iter()
                .map(|page| {
                    let table = &bytes[page * PAGE..][..PAGE];
                    (0..PAGES).map(|i| u16_at(table, i * 2)).collect::<Vec<u16>>()
                })
                .find(|entries| {
                    entries[layout.first_data_page(bank)..].iter().all(|&entry| layout.plausible(entry))
                })
                .ok_or(Error::NotAControllerPak)
        })
        .collect()
}

/// The pages a note occupies, as bank and page, from the one its entry starts it at.
fn follow_chain(index: &[Vec<u16>], layout: Layout, start: (usize, usize)) -> Result<Vec<(usize, usize)>> {
    let mut seen = vec![false; layout.banks * PAGES];
    let mut chain = Vec::new();
    let mut current = start;
    loop {
        let (bank, page) = current;
        if std::mem::replace(&mut seen[bank * PAGES + page], true) {
            return Err(Error::Corrupt(format!(
                "the chain from bank {} page {} loops at bank {bank} page {page}",
                start.0, start.1
            )));
        }
        chain.push(current);
        let next = index[bank][page];
        if next == CHAIN_END {
            return Ok(chain);
        }
        let [next_bank, next_page] = next.to_be_bytes();
        let (next_bank, next_page) = (usize::from(next_bank), usize::from(next_page));
        if !layout.is_data_page(next_bank, next_page) {
            return Err(Error::Corrupt(format!(
                "bank {bank} page {page} links to bank {next_bank} page {next_page}, which holds no data"
            )));
        }
        current = (next_bank, next_page);
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
        // The ID block a console wrote when it formatted the pak: a serial of its own, the same in
        // all four copies, each sealed.
        for at in build::ID_BLOCKS {
            for (offset, byte) in pak[at..at + 0x18].iter_mut().enumerate() {
                *byte = u8::try_from(offset * 7 + 3).expect("a byte");
            }
            pak[at + 0x1A] = 0x01;
            let sum = build::id_checksum(&pak[at..at + 0x20]);
            pak[at + 0x1C..at + 0x1E].copy_from_slice(&sum.to_be_bytes());
            pak[at + 0x1E..at + 0x20].copy_from_slice(&0xFFF2u16.wrapping_sub(sum).to_be_bytes());
        }

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
        let mut table = vec![0u8; PAGE];
        for (page, entry) in index.iter().enumerate() {
            table[page * 2..page * 2 + 2].copy_from_slice(&entry.to_be_bytes());
        }
        table[1] = build::index_checksum(&table, FIRST_DATA_PAGE);
        pak[INDEX_PAGE * PAGE..(INDEX_PAGE + 1) * PAGE].copy_from_slice(&table);
        pak[INDEX_BACKUP_PAGE * PAGE..(INDEX_BACKUP_PAGE + 1) * PAGE].copy_from_slice(&table);
        pak
    }

    /// A pak of two banks as libultra lays one out, hand-rolled from that layout for the reason
    /// [`pak_with`] is: an index page for each bank and a backup of each in bank 0, the note table
    /// after them, and a note of 122 pages that fills bank 0's 121 data pages and runs on into
    /// bank 1, beside a note of two pages there.
    pub(crate) fn two_bank_pak() -> Vec<u8> {
        let mut pak = pak_with(&[]);
        pak.resize(2 * CAPACITY, 0);
        pak[INDEX_PAGE * PAGE..5 * PAGE].fill(0);
        for at in build::ID_BLOCKS {
            pak[at + 0x1A] = 2;
            let sum = build::id_checksum(&pak[at..at + 0x20]);
            pak[at + 0x1C..at + 0x1E].copy_from_slice(&sum.to_be_bytes());
            pak[at + 0x1E..at + 0x20].copy_from_slice(&0xFFF2u16.wrapping_sub(sum).to_be_bytes());
        }

        // Bank 0's pages 7 to 127 and bank 1's page 1, then bank 1's pages 2 and 3.
        let first: Vec<(usize, usize)> = (7..PAGES).map(|page| (0, page)).chain([(1, 1)]).collect();
        let second = [(1usize, 2usize), (1, 3)];
        let mut index = [vec![0u16; PAGES], vec![0u16; PAGES]];
        index[0][7..].fill(CHAIN_FREE);
        index[1][1..].fill(CHAIN_FREE);
        for chain in [&first[..], &second[..]] {
            for (step, &(bank, page)) in chain.iter().enumerate() {
                for byte in 0..PAGE {
                    pak[bank * CAPACITY + page * PAGE + byte] =
                        u8::try_from((byte + page * 13 + bank * 7) & 0xff).expect("a byte");
                }
                index[bank][page] = chain.get(step + 1).map_or(CHAIN_END, |&(bank, page)| {
                    u16::from_be_bytes([u8::try_from(bank).unwrap(), u8::try_from(page).unwrap()])
                });
            }
        }
        for (bank, (page, backup)) in [(1, 3), (2, 4)].into_iter().enumerate() {
            let mut table = vec![0u8; PAGE];
            for (entry, value) in index[bank].iter().enumerate() {
                table[entry * 2..entry * 2 + 2].copy_from_slice(&value.to_be_bytes());
            }
            let from = if bank == 0 { 7 } else { 1 };
            table[1] = table[from * 2..].iter().fold(0u8, |sum, &byte| sum.wrapping_add(byte));
            pak[page * PAGE..(page + 1) * PAGE].copy_from_slice(&table);
            pak[backup * PAGE..(backup + 1) * PAGE].copy_from_slice(&table);
        }
        for (slot, (code, name, start)) in
            [("NDMJ", "DATEL", [0u8, 7]), ("NMKJ", "GHOST", [1, 2])].iter().enumerate()
        {
            let offset = 5 * PAGE + slot * NOTE_ENTRY;
            pak[offset..offset + 4].copy_from_slice(code.as_bytes());
            pak[offset + 6..offset + 8].copy_from_slice(start);
            pak[offset + 0x10..offset + 0x20].copy_from_slice(&font::encode(name, 16));
        }
        pak
    }

    #[test]
    fn a_note_runs_on_from_one_bank_into_the_next() {
        let pak = two_bank_pak();
        assert!(detect(&pak));
        let parsed = ControllerPak::parse(&pak).expect("reads");
        assert_eq!((parsed.layout().banks(), parsed.capacity()), (2, 2 * CAPACITY));
        let notes = parsed.notes();
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].data.len(), 122 * PAGE, "all of bank 0's data pages and one of bank 1's");
        assert_eq!(
            notes[0].data[121 * PAGE..],
            pak[CAPACITY + PAGE..CAPACITY + 2 * PAGE],
            "bank 1 page 1 last"
        );
        assert_eq!(notes[1].data[..], pak[CAPACITY + 2 * PAGE..CAPACITY + 4 * PAGE]);
        assert_eq!(notes[1].path().as_deref(), Some("GHOST"));
    }

    #[test]
    fn four_paks_in_one_file_are_not_one_pak_of_four_banks() {
        // mupen64plus keeps a pak for each port in one file as long as a pak of four banks, and
        // each of the four records one bank.
        let one = pak_with(&[("NMKJ", "MARIOKART", 2)]);
        let four = one.repeat(4);
        assert!(!detect(&four));
        assert!(matches!(ControllerPak::parse(&four), Err(Error::Corrupt(_))));
        assert_eq!(recorded_banks(&four), Some(1));
        assert_eq!(recorded_banks(&two_bank_pak()), Some(2));
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

    /// The table as N64brew documents it: an apostrophe among the punctuation, and katakana with
    /// its marks after it, up to `0x94` and no further.
    #[test]
    fn the_font_table_runs_to_katakana() {
        assert_eq!(font::decode_char(0x37), '\'');
        assert_eq!(font::decode_char(0x41), '@');
        assert_eq!((font::decode_char(0x42), font::decode_char(0x50)), ('。', 'ア'));
        assert_eq!((font::decode_char(0x7B), font::decode_char(0x7C)), ('ワ', 'ガ'));
        assert_eq!((font::decode_char(0x94), font::decode_char(0x95)), ('ポ', '\0'));
        assert_eq!(font::decode_char(0x01), '\0');
        assert_eq!(font::CHARSET.chars().count(), 0x95);
        assert_eq!(font::spell(&font::encode("ゼルダノデンセツ", 16)).as_deref(), Some("ゼルダノデンセツ"));
    }

    /// A spelling stops at the first NUL and keeps spaces, and gives up on a code the table
    /// leaves empty; a name to show trims and skips instead.
    #[test]
    fn a_field_is_spelled_up_to_its_nul_and_shown_tidied() {
        assert_eq!(font::spell(&[26, 15, 0, 27]).as_deref(), Some("A "));
        assert_eq!(font::decode(&[26, 15, 0, 27]), "A");
        assert_eq!(font::spell(&[26, 0x05, 27]), None);
        assert_eq!(font::decode(&[26, 0x05, 27]), "AB");
        assert_eq!(font::spell(&[0, 26]).as_deref(), Some(""));
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
        assert_eq!(parsed.notes()[0].path().as_deref(), Some("SAVE.DAT"));
    }

    #[test]
    fn a_note_with_a_code_the_table_leaves_empty_has_no_path() {
        let mut pak = pak_with(&[("NMKJ", "SAVE", 1)]);
        pak[NOTE_TABLE_PAGE * PAGE + 0x11] = 0x05;
        let parsed = ControllerPak::parse(&pak).expect("still a note");
        assert_eq!(parsed.notes()[0].path(), None);
        assert_eq!(parsed.notes()[0].name, "SVE", "but there is still a name to show");
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
