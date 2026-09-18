//! Cards a PlayStation BIOS wrote, as against ones this crate built itself.
//!
//! A round trip through [`CardBuilder`] cannot catch a field this crate writes and reads back the
//! same wrong way. These are out of DuckStation, and the second of them is what found exactly
//! that: the directory's size field belongs to a save's *first* entry and to no other, and writing
//! the length on every entry of the chain produced a card this crate was perfectly happy with and
//! a BIOS stopped listing saves out of.

use std::path::PathBuf;

use ps1_memcard::{CardBuilder, MemoryCard};

/// The directory block: the header frame, fifteen entries, the broken-sector list and its spares.
///
/// Frame 63 is left out on purpose. It is the frame a console scribbles on to test that the card
/// is writable, so it says nothing about what is stored and does not survive being read back.
const DIRECTORY: std::ops::Range<usize> = 0..(63 * 128);

fn card(rest: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/saves/PS1").join(rest);
    std::fs::read(path).expect("a vendored card")
}

/// Rebuilding a card a console wrote reproduces its directory byte for byte.
///
/// This is the check that a round trip through this crate cannot make. Reading a card and writing
/// it back has to be a byte copy of everything that matters, and "what matters" has to be measured
/// against a card something else produced.
#[test]
fn a_rebuilt_card_matches_the_one_a_console_wrote() {
    for name in [
        "Gran Turismo/DuckStation/shared_card_1.mcd",
        "Castlevania Symphony of the Night/DuckStation/shared_card_2.mcd",
    ] {
        let original = card(name);
        let parsed = MemoryCard::parse(&original).expect("parses");

        let mut builder = CardBuilder::new();
        for save in parsed.saves() {
            builder.add(save.clone());
        }
        let rebuilt = builder.build().expect("builds");

        assert_eq!(rebuilt.len(), original.len(), "{name}: a card keeps its size");
        assert_eq!(
            &rebuilt[DIRECTORY], &original[DIRECTORY],
            "{name}: the directory a console wrote is what this writes back"
        );

        // And every block the directory says is in use comes back unchanged. Blocks it says are
        // free are not compared: a console leaves whatever was there, and this zeroes them.
        for (index, save) in parsed.saves().iter().enumerate() {
            let again = MemoryCard::parse(&rebuilt).expect("reads what it wrote");
            assert_eq!(again.saves()[index].name, save.name, "{name}");
            assert_eq!(again.saves()[index].data, save.data, "{name}: {} data", save.name);
        }
    }
}

/// The size field is the first entry's alone.
#[test]
fn a_continuation_entry_carries_no_size() {
    // Gran Turismo spans five blocks, so its chain has one first entry and four continuations.
    let original = card("Gran Turismo/DuckStation/shared_card_1.mcd");
    let parsed = MemoryCard::parse(&original).expect("parses");
    let mut builder = CardBuilder::new();
    for save in parsed.saves() {
        builder.add(save.clone());
    }
    let rebuilt = builder.build().expect("builds");

    let size_of = |card: &[u8], frame: usize| {
        u32::from_le_bytes(card[frame * 128 + 4..frame * 128 + 8].try_into().expect("four bytes"))
    };
    assert_eq!(size_of(&rebuilt, 1), 5 * 8192, "the first entry carries the length");
    for frame in 2..=5 {
        assert_eq!(size_of(&rebuilt, frame), 0, "continuation entry {frame} carries none");
        assert_eq!(size_of(&rebuilt, frame), size_of(&original, frame), "as the console wrote it");
    }
}
