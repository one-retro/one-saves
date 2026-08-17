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
#[derive(Debug, Clone, Default)]
pub struct PakBuilder {
    notes: Vec<Note>,
    label: Option<Vec<u8>>,
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
        if let Some(label) = &self.label {
            let len = label.len().min(NOTE_ENTRY);
            pak[..len].copy_from_slice(&label[..len]);
        }

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

        for (page, entry) in index.iter().enumerate() {
            let bytes = entry.to_be_bytes();
            pak[INDEX_PAGE * PAGE + page * 2..][..2].copy_from_slice(&bytes);
            pak[INDEX_BACKUP_PAGE * PAGE + page * 2..][..2].copy_from_slice(&bytes);
        }
        Ok(pak)
    }
}

/// The note table entry a note gets: its own, or one carrying just the name and extension.
fn note_entry(note: &Note) -> Vec<u8> {
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
        builder.label(parsed.label().to_vec());
        for note in parsed.notes() {
            builder.add(note.clone());
        }
        builder.build().expect("writes")
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
        assert_eq!(parsed.notes()[0].path(), "GHOST.DAT");
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
