//! Writing a pak image out of a set of notes.

use crate::{
    CAPACITY, CHAIN_END, CHAIN_FREE, Error, FIRST_DATA_PAGE, INDEX_BACKUP_PAGE, INDEX_PAGE, NOTE_ENTRY,
    NOTE_TABLE_PAGE, NOTES, Note, PAGE, PAGES, Result, font,
};

/// Builds a pak image from notes.
///
/// The index table, its backup and the note table are all regenerated. A note's entry comes back
/// verbatim with only its start page rewritten: page placement is the allocator's business and
/// everything else in the entry is the game's.
///
/// What comes out is a pak a console accepts. Before it reads a note, a game checks the ID block
/// in page 0 (four copies, each carrying a checksum and its complement) and the checksum in byte 1
/// of the index table, and a pak failing either is reported as damaged and offered for repair,
/// which formats it. So the builder writes both: the ID area of the pak it was given, or a freshly
/// formatted one.
#[derive(Debug, Clone, Default)]
pub struct PakBuilder {
    notes: Vec<Note>,
    label: Option<Vec<u8>>,
    id_area: Option<Vec<u8>>,
}

/// Page 0 of a freshly formatted pak: the label area, then the ID block four times over.
///
/// Every byte of it is libdragon's `format_mempak`, which took it from the first page every pak
/// dump in circulation shared. A fixed serial rather than a random one, so that building the same
/// notes twice gives the same bytes; the checksums are recomputed by [`seal_id_block`] all the same,
/// so this is the layout and not something trusted to be right.
#[allow(clippy::cast_possible_truncation, reason = "the label area's bytes count 0 to 31")]
const FORMATTED_ID_AREA: [u8; PAGE] = {
    let mut page = [0u8; PAGE];
    let mut i = 0;
    while i < NOTE_ENTRY {
        page[i] = i as u8;
        i += 1;
    }
    page[0] = 0x81;
    let block: [u8; 32] = [
        0xff, 0xff, 0xff, 0xff, 0x05, 0x1a, 0x5f, 0x13, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff,
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01, 0xff, 0x66, 0x25, 0x99, 0xcd,
    ];
    let mut copy = 0;
    while copy < ID_BLOCKS.len() {
        let mut i = 0;
        while i < block.len() {
            page[ID_BLOCKS[copy] + i] = block[i];
            i += 1;
        }
        copy += 1;
    }
    page
};

/// Where page 0 keeps its copies of the ID block.
pub(crate) const ID_BLOCKS: [usize; 4] = [0x20, 0x60, 0x80, 0xC0];

/// The ID block's checksum: the sum of its first fourteen big-endian words.
pub(crate) fn id_checksum(block: &[u8]) -> u16 {
    let (words, _) = block[..0x1C].as_chunks::<2>();
    words.iter().fold(0u16, |sum, &word| sum.wrapping_add(u16::from_be_bytes(word)))
}

/// Writes every copy's checksum and its complement, which a console compares against `0xFFF2`.
fn seal_id_block(page: &mut [u8]) {
    for at in ID_BLOCKS {
        let block = &mut page[at..at + 0x20];
        let sum = id_checksum(block);
        block[0x1C..0x1E].copy_from_slice(&sum.to_be_bytes());
        block[0x1E..0x20].copy_from_slice(&0xFFF2u16.wrapping_sub(sum).to_be_bytes());
    }
}

/// The index table's checksum: the low byte of every data page's entry, summed.
pub(crate) fn index_checksum(table: &[u8]) -> u8 {
    (FIRST_DATA_PAGE..PAGES).fold(0u8, |sum, page| sum.wrapping_add(table[page * 2 + 1]))
}

impl PakBuilder {
    /// A builder for an empty pak.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Keeps the 32-byte label read off another pak, which belongs to no note.
    ///
    /// More than 32 bytes is truncated; fewer is written as far as it goes.
    pub fn label(&mut self, bytes: impl Into<Vec<u8>>) -> &mut Self {
        self.label = Some(bytes.into());
        self
    }

    /// Keeps the whole of page 0 read off another pak: its label and the ID block that is the pak's
    /// identity, serial and all.
    ///
    /// For rebuilding a pak in place, where the pak is the same pak afterwards. A label given as
    /// well is written over the first 32 bytes. Fewer than 256 bytes is written as far as it goes,
    /// over a formatted page.
    pub fn id_area(&mut self, bytes: impl Into<Vec<u8>>) -> &mut Self {
        self.id_area = Some(bytes.into());
        self
    }

    /// Adds a note. Its `slot` is ignored: the note table index is the order notes are added in.
    pub fn add(&mut self, note: Note) -> &mut Self {
        self.notes.push(note);
        self
    }

    /// Writes the pak out, all 32 KiB of it.
    pub fn build(&self) -> Result<Vec<u8>> {
        if self.notes.len() > NOTES {
            return Err(Error::TooManyNotes { given: self.notes.len(), available: NOTES });
        }

        let mut pak = vec![0u8; CAPACITY];
        pak[..PAGE].copy_from_slice(&FORMATTED_ID_AREA);
        if let Some(area) = &self.id_area {
            let len = area.len().min(PAGE);
            pak[..len].copy_from_slice(&area[..len]);
        }
        if let Some(label) = &self.label {
            let len = label.len().min(NOTE_ENTRY);
            pak[..len].copy_from_slice(&label[..len]);
        }
        seal_id_block(&mut pak[..PAGE]);

        let mut index = vec![CHAIN_FREE; PAGES];
        // The pages before the data area are structure rather than storage.
        for entry in index.iter_mut().take(FIRST_DATA_PAGE) {
            *entry = 0;
        }

        let mut next_page = FIRST_DATA_PAGE;
        for (slot, note) in self.notes.iter().enumerate() {
            let pages = note.data.len().div_ceil(PAGE);
            if next_page + pages > PAGES {
                return Err(Error::Full {
                    needed: next_page - FIRST_DATA_PAGE + pages,
                    available: PAGES - FIRST_DATA_PAGE,
                });
            }

            let first = next_page;
            for step in 0..pages {
                let page = first + step;
                let from = step * PAGE;
                let to = ((step + 1) * PAGE).min(note.data.len());
                pak[page * PAGE..page * PAGE + (to - from)].copy_from_slice(&note.data[from..to]);
                index[page] =
                    if step + 1 == pages { CHAIN_END } else { u16::try_from(page + 1).expect("page fits") };
            }

            let mut entry = note_entry(note);
            entry.resize(NOTE_ENTRY, 0);
            entry[6..8].copy_from_slice(&u16::try_from(first).expect("page fits").to_be_bytes());
            let offset = NOTE_TABLE_PAGE * PAGE + slot * NOTE_ENTRY;
            pak[offset..offset + NOTE_ENTRY].copy_from_slice(&entry);

            next_page += pages;
        }

        let mut table = vec![0u8; PAGE];
        for (page, entry) in index.iter().enumerate() {
            table[page * 2..page * 2 + 2].copy_from_slice(&entry.to_be_bytes());
        }
        // Entry 0 is never a page, so its low byte is where the table keeps its own checksum.
        table[1] = index_checksum(&table);
        pak[INDEX_PAGE * PAGE..(INDEX_PAGE + 1) * PAGE].copy_from_slice(&table);
        pak[INDEX_BACKUP_PAGE * PAGE..(INDEX_BACKUP_PAGE + 1) * PAGE].copy_from_slice(&table);
        Ok(pak)
    }
}

/// The note table entry a note gets: its own, or one carrying just the name and extension.
pub(crate) fn note_entry(note: &Note) -> Vec<u8> {
    if !note.dirent.is_empty() {
        return note.dirent.clone();
    }
    let mut entry = vec![0u8; NOTE_ENTRY];
    let code = note.game_code.as_bytes();
    let len = code.len().min(4);
    entry[..len].copy_from_slice(&code[..len]);
    entry[0x0C..0x10].copy_from_slice(&font::encode(&note.extension, 4));
    entry[0x10..0x20].copy_from_slice(&font::encode(&note.name, 16));
    entry
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ControllerPak, tests::pak_with};

    fn round_trip(pak: &[u8]) -> Vec<u8> {
        let parsed = ControllerPak::parse(pak).expect("reads");
        let mut builder = PakBuilder::new();
        builder.id_area(parsed.id_area().to_vec());
        for note in parsed.notes() {
            builder.add(note.clone());
        }
        builder.build().expect("writes")
    }

    /// What a console checks before it trusts a pak, as libdragon's `__get_valid_toc` does: every
    /// copy of the ID block agrees and is sealed, and the index table or its backup sums right.
    fn a_console_accepts(pak: &[u8]) -> bool {
        let page = &pak[..PAGE];
        let blocks_agree = page[0x20..0x30] == page[0x60..0x70]
            && page[0x80..0x90] == page[0xC0..0xD0]
            && page[0x20..0x30] == page[0x80..0x90];
        let sealed = ID_BLOCKS.iter().all(|&at| {
            let block = &page[at..at + 0x20];
            let sum = id_checksum(block);
            u16::from_be_bytes([block[0x1C], block[0x1D]]) == sum
                && u16::from_be_bytes([block[0x1E], block[0x1F]]) == 0xFFF2u16.wrapping_sub(sum)
        });
        let table_sums = [INDEX_PAGE, INDEX_BACKUP_PAGE].iter().any(|&index| {
            let table = &pak[index * PAGE..(index + 1) * PAGE];
            table[1] == index_checksum(table)
        });
        blocks_agree && sealed && table_sums
    }

    #[test]
    fn a_built_pak_passes_the_checks_a_console_makes() {
        // Zeroes in page 0 and in the table's checksum byte read as a damaged pak, and a game's
        // answer to a damaged pak is to offer to format it.
        assert!(a_console_accepts(&PakBuilder::new().build().unwrap()), "an empty pak");
        let parsed = ControllerPak::parse(&pak_with(&[("NMKJ", "MARIOKART", 2)])).unwrap();
        let mut builder = PakBuilder::new();
        builder.label(parsed.label().to_vec());
        builder.add(parsed.notes()[0].clone());
        assert!(a_console_accepts(&builder.build().unwrap()), "a pak with a note on it");
    }

    #[test]
    fn the_formatted_id_block_is_libdragon_s_to_the_byte() {
        // Its checksums are recomputed rather than copied, so they have to come out as the ones
        // every formatted pak in circulation carries.
        let pak = PakBuilder::new().build().unwrap();
        assert_eq!(&pak[0x3C..0x40], &[0x66, 0x25, 0x99, 0xcd]);
        assert_eq!(pak[0], 0x81);
        assert_eq!(&pak[1..4], &[1, 2, 3]);
    }

    #[test]
    fn a_pak_rebuilt_with_its_id_area_keeps_its_identity() {
        let mut builder = PakBuilder::new();
        let mut area = vec![0u8; PAGE];
        area[..NOTE_ENTRY].fill(0x42);
        // A serial of its own, which a fresh format would not have, in every copy.
        for at in ID_BLOCKS {
            area[at..at + 0x18].fill(0x5A);
        }
        builder.id_area(area.clone());
        let pak = builder.build().unwrap();

        let rebuilt = {
            let parsed = ControllerPak::parse(&pak).unwrap();
            let mut builder = PakBuilder::new();
            builder.id_area(parsed.id_area().to_vec());
            builder.build().unwrap()
        };
        assert_eq!(rebuilt, pak);
        assert_eq!(&rebuilt[0x20..0x38], &area[0x20..0x38]);
        assert!(a_console_accepts(&rebuilt), "resealed, whatever the area it was given held");
    }

    #[test]
    fn a_pak_a_console_wrote_is_rebuilt_byte_for_byte() {
        let pak = pak_with(&[("NMKJ", "MARIOKART", 2), ("NSMJ", "SUPERMARIO", 1)]);
        assert_eq!(round_trip(&pak), pak);
    }

    #[test]
    fn a_note_built_by_hand_gets_an_entry_carrying_its_name() {
        let mut builder = PakBuilder::new();
        builder.add(Note {
            slot: 0,
            game_code: "NMKJ".into(),
            name: "GHOST".into(),
            extension: "DAT".into(),
            dirent: Vec::new(),
            data: vec![9u8; PAGE],
        });
        let pak = builder.build().expect("writes");

        let parsed = ControllerPak::parse(&pak).expect("reads what it wrote");
        assert_eq!(parsed.notes().len(), 1);
        assert_eq!(parsed.notes()[0].game_code, "NMKJ");
        assert_eq!(parsed.notes()[0].path().as_deref(), Some("GHOST.DAT"));
        assert_eq!(parsed.notes()[0].data, vec![9u8; PAGE]);
    }

    #[test]
    fn refuses_more_notes_than_the_table_holds() {
        let mut builder = PakBuilder::new();
        for index in 0..=NOTES {
            builder.add(Note {
                slot: 0,
                game_code: "NMKJ".into(),
                name: format!("N{index}"),
                extension: String::new(),
                dirent: Vec::new(),
                data: vec![0u8; PAGE],
            });
        }
        assert!(matches!(builder.build(), Err(Error::TooManyNotes { given: 17, available: 16 })));
    }

    #[test]
    fn refuses_notes_that_do_not_fit() {
        let mut builder = PakBuilder::new();
        for index in 0..2 {
            builder.add(Note {
                slot: 0,
                game_code: "NMKJ".into(),
                name: format!("BIG{index}"),
                extension: String::new(),
                dirent: Vec::new(),
                data: vec![0u8; 100 * PAGE],
            });
        }
        assert!(matches!(builder.build(), Err(Error::Full { .. })));
    }
}
