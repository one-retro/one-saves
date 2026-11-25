//! Save files that keep several games' saves, each under the name its game is known by.
//!
//! Most save files are one game's. A few are not. The P8 core for the MiSTer keeps a block per
//! PICO-8 cartdata id in a cart's `.sav`, and a cart can open several ids, each a save of the game
//! that id names. Each block is also the same save another runtime keeps as a file of its own,
//! `<id>.p8d.txt` ([`crate::pico8`]).
//!
//! So this reads such a file as its entries, and hands each over as the file it would be on its
//! own: what handles saves then names, identifies, wraps and lays out an entry exactly as it would
//! `<name>.<extension>`, and two runtimes that keep one save in different files arrive at the same
//! bundle. [`write()`] puts an entry back, leaving the rest of the file as it was.
//!
//! Memory cards are not this. A card's saves carry the card's own record of them and go back onto a
//! card through [`crate::card`]; an entry here carries nothing but its name.

use crate::error::{Error, Result};
use crate::pico8;

/// One game's save, out of a file that keeps several.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The name the save is kept under, which is how its game is known: a PICO-8 cartdata id.
    pub name: String,
    /// The save as a file of its own would hold it, `<name>.<extension>` ([`extension`]).
    pub file: Vec<u8>,
}

/// The extension a `system`'s entry has as a file of its own: `p8d.txt` for PICO-8. `None` for a
/// system that keeps no file of entries.
#[must_use]
pub fn extension(system: &str) -> Option<&'static str> {
    (system == "pico-8").then_some(pico8::TEXT_EXTENSION)
}

/// The entries in `file`, a save of `system`, in the order the file keeps them. `None` when it is
/// not a file of entries, which is what nearly every save file is.
#[must_use]
pub fn read(file: &[u8], system: &str) -> Option<Vec<Entry>> {
    if system != "pico-8" || !pico8::p8::is_save(file) {
        return None;
    }
    Some(
        pico8::p8::blocks(file)
            .into_iter()
            .map(|block| Entry {
                name: String::from_utf8_lossy(&block.id).into_owned(),
                file: pico8::text_of_memory(&block.memory),
            })
            .collect(),
    )
}

/// `file`, a save of `system`, with the entry `name` holding `entry`, given as the file of its own
/// that [`read`] hands over. The entry is rewritten where it is, or added after the others; an
/// empty `file` becomes one holding the entry alone.
pub fn write(file: &[u8], system: &str, name: &str, entry: &[u8]) -> Result<Vec<u8>> {
    if system != "pico-8" {
        return Err(Error::NotConvertible(format!("a {system} save keeps no entries")));
    }
    if !file.is_empty() && !pico8::p8::is_save(file) {
        return Err(Error::NotConvertible("not a file of PICO-8 cart data".into()));
    }
    let memory: [u8; pico8::MEMORY_LEN] = match entry.try_into() {
        Ok(memory) => memory,
        Err(_) => pico8::memory_of_text(entry)
            .ok_or_else(|| Error::NotConvertible(format!("{name}'s save is not PICO-8 cart data")))?,
    };
    pico8::p8::with_block(file, name.as_bytes(), &memory).ok_or_else(|| {
        Error::NotConvertible(format!(
            "{name} is not an id a P8 file can hold, or the file has no room for it"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pico8::{MEMORY_LEN, memory_of_text, text_of_memory};

    fn memory_with(slot: usize, value: u32) -> [u8; MEMORY_LEN] {
        let mut m = [0u8; MEMORY_LEN];
        m[slot * 4..slot * 4 + 4].copy_from_slice(&value.to_le_bytes());
        m
    }

    #[test]
    fn a_p8_file_is_its_ids_as_the_files_other_runtimes_keep() {
        let one = text_of_memory(&memory_with(14, 3 << 16));
        let two = text_of_memory(&memory_with(30, 1 << 16));
        let file = write(&[], "pico-8", "mot_gpp", &one).unwrap();
        let file = write(&file, "pico-8", "mot_gpp_results", &two).unwrap();

        let entries = read(&file, "pico-8").unwrap();
        assert_eq!(
            entries,
            [
                Entry { name: "mot_gpp".into(), file: one.clone() },
                Entry { name: "mot_gpp_results".into(), file: two }
            ]
        );
        assert_eq!(extension("pico-8"), Some("p8d.txt"));

        // Rewritten where it is, the other left alone.
        let three = text_of_memory(&memory_with(0, 9));
        let again = write(&file, "pico-8", "mot_gpp_results", &three).unwrap();
        assert_eq!(again.len(), file.len());
        assert_eq!(read(&again, "pico-8").unwrap()[0].file, one);
        assert_eq!(memory_of_text(&read(&again, "pico-8").unwrap()[1].file), Some(memory_with(0, 9)));
    }

    #[test]
    fn the_memory_itself_goes_in_as_well_as_the_text() {
        let file = write(&[], "pico-8", "porklike", &memory_with(1, 2)).unwrap();
        assert_eq!(read(&file, "pico-8").unwrap()[0].file, text_of_memory(&memory_with(1, 2)));
    }

    #[test]
    fn other_files_and_systems_keep_no_entries() {
        let text = text_of_memory(&[0; MEMORY_LEN]);
        assert_eq!(read(&text, "pico-8"), None);
        assert_eq!(read(&[0; 512], "pico-8"), None);
        let file = write(&[], "pico-8", "porklike", &text).unwrap();
        assert_eq!(read(&file, "gba"), None);
        assert_eq!(extension("gba"), None);
        assert!(write(&[], "gba", "porklike", &text).is_err());
        assert!(write(b"a cart's text", "pico-8", "porklike", &text).is_err());
        assert!(write(&[], "pico-8", "porklike", b"not cart data").is_err());
        assert!(write(&[], "pico-8", &"a".repeat(65), &text).is_err());
    }
}
