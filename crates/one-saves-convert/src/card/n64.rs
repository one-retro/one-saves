//! Nintendo 64 Controller Paks.
//!
//! The filesystem itself lives in [`n64_cpak`], which is a standalone crate because a Controller
//! Pak is a genuinely reusable format and nothing about reading one needs this container. What is
//! here is the adapter: the mapping between a pak's notes and a bundle's nested parts.
//!
//! A pak permits two notes with the same game code and note name, which is why a producer must
//! carry [`slot`](one_saves::Part::slot): without it the two would share an address and nothing
//! could tell them apart.
//!
//! A note's name is bytes in the pak's own character table, so its [`path`](one_saves::Part::path)
//! is the spelling the specification gives, [`Note::path`]: the name up to its first NUL, then a
//! dot and the extension where there is one. A note that table cannot spell, or whose spelling is
//! no path — empty, say — goes without one, and its entry and `slot` still say which note it is.
//!
//! A note names its game by the four-character game code the cartridge's header carries, and its
//! [`serial`](one_saves::Game::serial) is that code as a product code: `NUS-` and the code, so
//! Mario Kart 64's `NKTE` is `NUS-NKTE`, the serial a cartridge save of the game names too.

use n64_cpak::{ControllerPak, Layout, NOTE_ENTRY, Note, PakBuilder};
use one_saves::extension::DirentN64Cpak;
use one_saves::{Bundle, Game};

use crate::CardOptions;
use crate::card::{card_header, card_image_part, image_only, nested_saves, path_of_name, save_part_with};
use crate::detect::Format;
use crate::error::{Error, Result};
use crate::label::value as label;
use crate::n64::{game_code_of, serial_of};

/// What the format is called, for error messages.
const FORMAT: &str = Format::N64Pak.label();

/// The card format slug a bundle carries.
pub const CARD_FORMAT: &str = match Format::N64Pak.card_format() {
    Some(slug) => slug,
    None => panic!("a Controller Pak is a card format"),
};

/// The system slug these saves are for.
pub const SYSTEM: &str = match Format::N64Pak.system() {
    Some(slug) => slug,
    None => panic!("a Controller Pak holds saves for a system"),
};

/// Whether these bytes look like a Controller Pak.
#[must_use]
pub fn detect(bytes: &[u8]) -> bool {
    n64_cpak::detect(bytes)
}

/// Reads a pak into a bundle: one nested bundle per note.
pub fn read(bytes: &[u8], options: &CardOptions) -> Result<Bundle> {
    let pak = ControllerPak::parse(bytes).map_err(into_error)?;

    let mut parts = Vec::new();
    for note in pak.notes() {
        // The game code is how a pak names the release; the note name is what the console lists
        // the note under, which is a label rather than a title and belongs under its own key.
        let game = (!note.game_code.is_empty()).then(|| Game::default().serial(serial_of(&note.game_code)));

        // A Controller Pak is the one format that keeps its text in the note table rather than in
        // the note, so this is read off the entry and not out of the payload.
        let mut inner = one_saves::Extensions::new();
        if let Some(label) = label(Some(note.name.clone()).filter(|n| !n.is_empty()), None) {
            inner.insert(label);
        }

        let mut part = save_part_with(Format::N64Pak, parts.len(), note.data.clone(), game, options, inner)?
            // Two notes may agree on game code and name, so the table index is what keeps them
            // apart.
            .slot(u64::try_from(note.slot).expect("slot fits"))
            .dirent(note.dirent.clone());
        // The entry again, field by field, for a writer that has to build one and does not know
        // where in the 32 bytes each field sits.
        if let Ok(entry) = <&[u8; NOTE_ENTRY]>::try_from(note.dirent.as_slice()) {
            part.extensions.insert(DirentN64Cpak::from_entry(entry));
        }
        part.path = note.path().as_deref().and_then(path_of_name);
        parts.push(part);
    }

    if parts.is_empty() {
        parts.push(card_image_part(0, pak.image(), &options.role_for(Format::N64Pak)));
    }

    Ok(Bundle::new(
        card_header(
            Format::N64Pak,
            pak.capacity(),
            // The pak's own 32-byte label belongs to no note, so splitting would drop it.
            Some(pak.label().to_vec()),
            options,
        ),
        parts,
    ))
}

/// Writes a bundle back out as a raw 32 KiB pak.
///
/// The index table, its backup and the note table are all regenerated; the pak's label is kept
/// from `system_area` when the bundle carries one.
pub fn write(bundle: &Bundle) -> Result<Vec<u8>> {
    build(bundle, None)
}

/// Writes a bundle as a pak in place of the one already there, which stays the same pak.
///
/// A bundle carries a pak's label and its notes, but not the ID block a console knows the pak by,
/// so the one on the pak being written over is kept. What is there being no pak, the pak is
/// formatted afresh, as [`write`](fn@write) does.
pub fn write_onto(bundle: &Bundle, there: &[u8]) -> Result<Vec<u8>> {
    let id_area = ControllerPak::parse(there).ok().map(|pak| pak.id_area().to_vec());
    build(bundle, id_area)
}

fn build(bundle: &Bundle, id_area: Option<Vec<u8>>) -> Result<Vec<u8>> {
    // A card with nothing on it carries a whole-card image rather than nested saves, and writing
    // it back is a byte copy.
    if let Some(image) = image_only(bundle)? {
        return Ok(image);
    }

    let mut builder = PakBuilder::new();
    builder.banks(banks_of(bundle)?);
    if let Some(area) = id_area {
        builder.id_area(area);
    }
    if let Some(area) = bundle.header.card.as_ref().and_then(|card| card.system_area.as_ref()) {
        builder.label(area.clone());
    }
    for note in notes_of(bundle)? {
        builder.add(note);
    }
    builder.build().map_err(into_error)
}

/// How many banks a card's pak has: its capacity over 32 KiB, which is how a writer sets the count
/// the pak's ID block records. A card that states no capacity is a pak of one bank.
fn banks_of(bundle: &Bundle) -> Result<usize> {
    let capacity = bundle.header.card.as_ref().map_or(n64_cpak::CAPACITY as u64, |card| card.capacity);
    usize::try_from(capacity).ok().and_then(Layout::of_len).map(Layout::banks).ok_or_else(|| {
        Error::NotConvertible(format!(
            "a Controller Pak is 32768 bytes a bank, up to {} banks, and this card states {capacity}",
            n64_cpak::MAX_BANKS
        ))
    })
}

/// A pak card's saves as the notes a pak holds, in the card's order.
///
/// A save's entry is the one it carries, verbatim, or else the one its
/// [`x.1sav.dirent.n64-cpak`](DirentN64Cpak) fields make. One that arrived with neither takes
/// its game code from its serial and its name from its path, which is what a writer then puts in
/// the entry it builds.
pub(crate) fn notes_of(bundle: &Bundle) -> Result<Vec<Note>> {
    let mut notes = Vec::new();
    for save in nested_saves(bundle)? {
        let path = save.path.unwrap_or_default();
        let (name, extension) = name_and_extension(&path);
        // The start page is the allocator's, and the builder writes its own over this one.
        let entry = save
            .dirent
            .or_else(|| save.extensions.get::<DirentN64Cpak>().map(|fields| fields.to_entry(0).to_vec()));
        let game_code = match entry.as_deref().and_then(|entry| entry.get(..4)) {
            Some(code) => String::from_utf8_lossy(code).into_owned(),
            None => save
                .inner
                .header
                .game
                .as_ref()
                .and_then(|game| game.serial.as_deref())
                .map(|serial| game_code_of(serial).to_owned())
                .unwrap_or_default(),
        };
        notes.push(Note {
            slot: 0,
            game_code,
            name: name.to_owned(),
            extension: extension.to_owned(),
            dirent: entry.unwrap_or_default(),
            data: save.bytes,
        });
    }
    Ok(notes)
}

/// A path split back into the name and the extension a note would have been written with.
///
/// A dot is in the table, so a name can hold one, and a path does not say which dot, if any, comes
/// before an extension. The last one does when what follows it fits in an extension's four
/// characters and what precedes it in a name's sixteen; otherwise the path is all name.
pub(crate) fn name_and_extension(path: &str) -> (&str, &str) {
    match path.rsplit_once('.') {
        Some((name, extension))
            if name.chars().count() <= 16 && (1..=4).contains(&extension.chars().count()) =>
        {
            (name, extension)
        }
        _ => (path, ""),
    }
}

fn into_error(source: n64_cpak::Error) -> Error {
    match source {
        n64_cpak::Error::WrongLength(found) => {
            Error::WrongLength { format: FORMAT, expected: "32768 bytes a bank, up to 62 banks", found }
        }
        n64_cpak::Error::NotAControllerPak => Error::NotThisFormat {
            format: FORMAT,
            why: "neither the index table nor its backup is readable".into(),
        },
        n64_cpak::Error::Corrupt(why) => Error::Corrupt { format: FORMAT, why },
        other => Error::NotConvertible(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a pak holding the given notes, each of the given page count.
    fn pak_with(notes: &[(&str, &str, usize)]) -> Vec<u8> {
        let mut builder = PakBuilder::new();
        builder.label(vec![0xAAu8; n64_cpak::NOTE_ENTRY]);
        for (code, name, pages) in notes {
            builder.add(Note {
                slot: 0,
                game_code: (*code).to_owned(),
                name: (*name).to_owned(),
                extension: String::new(),
                dirent: Vec::new(),
                data: (0..pages * n64_cpak::PAGE)
                    .map(|byte| u8::try_from(byte & 0xff).expect("masked to a byte"))
                    .collect(),
            });
        }
        builder.build().expect("builds")
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
        assert_eq!(bundle.parts[0].game.as_ref().unwrap().serial.as_deref(), Some("NUS-NMKJ"));

        let inner = Bundle::from_slice(&bundle.parts[0].payload).unwrap();
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
    fn a_pak_with_nothing_on_it_becomes_a_card_image() {
        let pak = pak_with(&[]);
        let bundle = read(&pak, &CardOptions::default()).expect("an empty pak is still a pak");
        assert_eq!(bundle.parts.len(), 1);
        assert_eq!(bundle.parts[0].kind, one_saves::PartKind::CardImage);
        assert_eq!(write(&bundle).expect("writes"), pak);
    }

    /// A note's name is spelled through the pak's table, katakana included, and one the table
    /// cannot spell goes without a path rather than with a guess at one.
    #[test]
    fn a_note_s_path_is_its_name_spelled_through_the_pak_s_table() {
        let mut pak =
            pak_with(&[("NZLJ", "ゼルダノデンセツ", 1), ("NMKJ", "MARIOKART", 1), ("NSMJ", "MARIO", 1)]);
        let entry = |slot: usize| n64_cpak::NOTE_TABLE_PAGE * n64_cpak::PAGE + slot * n64_cpak::NOTE_ENTRY;
        // An extension on the second, and a code the table leaves empty in the third's name.
        pak[entry(1) + 0x0C] = n64_cpak::font::encode_char('A');
        pak[entry(2) + 0x12] = 0x05;

        let bundle = read(&pak, &CardOptions::default()).expect("reads");
        bundle.validate().expect("every path spelled the way the specification spells it");
        let paths: Vec<Option<&str>> = bundle.parts.iter().map(|part| part.path.as_deref()).collect();
        assert_eq!(paths, [Some("ゼルダノデンセツ"), Some("MARIOKART.A"), None]);
        assert_eq!(bundle.parts[2].slot, Some(2), "the slot still says which note it is");

        // And the pak comes back as it was, each note from its own entry.
        assert_eq!(write(&bundle).expect("writes"), pak);
    }

    /// A note that arrives without its entry is written from its path and its game's serial,
    /// whether that serial is a product code or the bare code a bundle from before 0.7 carried.
    #[test]
    fn a_note_without_its_entry_takes_its_game_code_from_the_serial() {
        let pak = pak_with(&[("NKTE", "MARIOKART64", 1)]);
        for serial in ["NUS-NKTE", "NKTE"] {
            let mut bundle = read(&pak, &CardOptions::default()).expect("reads");
            let mut save = Bundle::from_slice(&bundle.parts[0].payload).unwrap();
            save.header.game = Some(Game::default().serial(serial));
            bundle.parts[0] = one_saves::Part::new(0, save.to_vec().unwrap())
                .kind(one_saves::PartKind::Bundle)
                .path("MARIOKART64")
                .slot(0);
            let written = ControllerPak::parse(&write(&bundle).expect("writes")).expect("a pak");
            assert_eq!(written.notes()[0].game_code, "NKTE", "{serial}");
            assert_eq!(written.notes()[0].path().as_deref(), Some("MARIOKART64"));
        }
    }

    /// Every note carries its entry's fields beside the entry, and a note that arrives with the
    /// fields alone, as a writer that dropped the opaque bytes would leave it, is written back as
    /// it was.
    #[test]
    fn a_note_s_entry_rides_as_fields_and_is_rebuilt_from_them() {
        let mut pak = pak_with(&[("NKTE", "MARIOKART64", 1), ("NSME", "MARIO", 2)]);
        let entry = |slot: usize| n64_cpak::NOTE_TABLE_PAGE * n64_cpak::PAGE + slot * NOTE_ENTRY;
        // A publisher's code, a status and a sum, none of which a builder would make up.
        pak[entry(0) + 0x04..entry(0) + 0x06].copy_from_slice(b"01");
        pak[entry(0) + 0x08..entry(0) + 0x0c].copy_from_slice(&[0x02, 0x5a, 0x12, 0x34]);

        let mut bundle = read(&pak, &CardOptions::default()).expect("reads");
        let fields = *bundle.parts[0].extensions.get::<DirentN64Cpak>().expect("the fields ride");
        assert_eq!((fields.game_code, fields.company_code), (*b"NKTE", *b"01"));
        assert_eq!((fields.status, fields.reserved, fields.data_sum), (0x02, 0x5a, 0x1234));
        assert_eq!(&fields.to_entry(0)[..], {
            let mut original = bundle.parts[0].dirent.clone().unwrap();
            original[6..8].fill(0);
            original
        });

        for part in &mut bundle.parts {
            part.dirent = None;
        }
        bundle.validate().expect("the fields need no entry beside them");
        assert_eq!(write(&bundle).expect("writes"), pak);
    }

    /// A pak of several banks is one card, whose capacity says how many, and whose notes run on
    /// from bank to bank.
    #[test]
    fn a_pak_of_several_banks_is_one_card_as_large_as_its_banks() {
        let mut builder = PakBuilder::new();
        builder.banks(4).label(vec![0xAAu8; NOTE_ENTRY]);
        for (code, name, pages) in [("NDMJ", "LONGNOTE", 200), ("NMKJ", "GHOST", 3)] {
            builder.add(Note {
                slot: 0,
                game_code: code.to_owned(),
                name: name.to_owned(),
                extension: String::new(),
                dirent: Vec::new(),
                data: (0..pages * n64_cpak::PAGE)
                    .map(|byte| u8::try_from(byte % 251).expect("a byte"))
                    .collect(),
            });
        }
        let pak = builder.build().expect("builds");
        assert!(detect(&pak));

        let bundle = read(&pak, &CardOptions::default()).expect("reads");
        bundle.validate().expect("valid bundle");
        assert_eq!(bundle.header.card.as_ref().unwrap().capacity, 4 * 32_768);
        assert_eq!(bundle.parts.len(), 2);
        assert_eq!(write(&bundle).expect("writes"), pak);

        // A capacity that is no whole number of banks is no pak to write.
        let mut odd = bundle.clone();
        odd.header.card.as_mut().unwrap().capacity = 40_000;
        assert!(write(&odd).is_err());
    }

    #[test]
    fn a_path_splits_back_at_an_extension_that_fits() {
        assert_eq!(name_and_extension("MARIOKART64.A"), ("MARIOKART64", "A"));
        assert_eq!(name_and_extension("MR.DRILLER"), ("MR.DRILLER", ""), "no extension is seven long");
        assert_eq!(name_and_extension("GAME"), ("GAME", ""));
        // A note's name can spell what no path is, since `/` and `.` are in the table.
        for refused in ["", "/A", "A/../B", "A/", "A//B", ".", "A/./B"] {
            assert_eq!(path_of_name(refused), None, "{refused:?}");
        }
        for taken in [".A", "A/B", "..A"] {
            assert_eq!(path_of_name(taken).as_deref(), Some(taken));
        }
    }

    #[test]
    fn something_that_is_not_a_pak_is_refused_in_this_format_s_words() {
        assert!(matches!(read(&[1, 2, 3], &CardOptions::default()), Err(Error::WrongLength { .. })));
    }
}
