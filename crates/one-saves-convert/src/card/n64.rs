//! Nintendo 64 Controller Paks.
//!
//! The filesystem itself lives in [`n64_cpak`], which is a standalone crate because a Controller
//! Pak is a genuinely reusable format and nothing about reading one needs this container. What is
//! here is the adapter: the mapping between a pak's notes and a bundle's nested parts.
//!
//! A pak permits two notes with the same game code and note name, which is why a producer must
//! carry [`slot`](one_saves::Part::slot): without it the two would share an address and nothing
//! could tell them apart.

use n64_cpak::{ControllerPak, Note, PakBuilder};
use one_saves::{Bundle, Game};

use crate::CardOptions;
use crate::card::{card_header, card_image_part, image_only, nested_saves, save_part};
use crate::detect::Format;
use crate::error::{Error, Result};

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
        // The game code and note name together are how a pak names a note, and neither is unique.
        let game = (!note.game_code.is_empty()).then(|| Game {
            serial: Some(note.game_code.clone()),
            title: (!note.name.is_empty()).then(|| note.name.clone()),
            ..Game::default()
        });

        let mut part = save_part(Format::N64Pak, parts.len(), note.data.clone(), game, options)?;
        part.path = Some(note.path());
        // Two notes may agree on game code and name, so the table index is what keeps them apart.
        part.slot = Some(u64::try_from(note.slot).expect("slot fits"));
        part.dirent = Some(note.dirent.clone());
        parts.push(part);
    }

    if parts.is_empty() {
        parts.push(card_image_part(0, pak.image(), &options.role_for(Format::N64Pak)));
    }

    Ok(Bundle {
        header: card_header(
            Format::N64Pak,
            pak.capacity(),
            // The pak's own 32-byte label belongs to no note, so splitting would drop it.
            Some(pak.label().to_vec()),
            options,
        ),
        parts,
    })
}

/// Writes a bundle back out as a raw 32 KiB pak.
///
/// The index table, its backup and the note table are all regenerated; the pak's label is kept
/// from `system_area` when the bundle carries one.
pub fn write(bundle: &Bundle) -> Result<Vec<u8>> {
    // A card with nothing on it carries a whole-card image rather than nested saves, and writing
    // it back is a byte copy.
    if let Some(image) = image_only(bundle)? {
        return Ok(image);
    }

    let mut builder = PakBuilder::new();
    if let Some(area) = bundle.header.card.as_ref().and_then(|card| card.system_area.as_ref()) {
        builder.label(area.clone());
    }
    for save in nested_saves(bundle)? {
        let path = save.path.unwrap_or_default();
        let (name, extension) = path.split_once('.').unwrap_or((path.as_str(), ""));
        // Only reached for a save that arrived without an entry; one that has an entry carries its
        // game code inside it, verbatim.
        let game_code =
            save.inner.header.game.as_ref().and_then(|game| game.serial.clone()).unwrap_or_default();
        builder.add(Note {
            slot: 0,
            game_code,
            name: name.to_owned(),
            extension: extension.to_owned(),
            dirent: save.dirent.unwrap_or_default(),
            data: save.bytes,
        });
    }
    builder.build().map_err(into_error)
}

fn into_error(source: n64_cpak::Error) -> Error {
    match source {
        n64_cpak::Error::WrongLength(found) => {
            Error::WrongLength { format: FORMAT, expected: "32768 bytes", found }
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
    fn a_pak_with_nothing_on_it_becomes_a_card_image() {
        let pak = pak_with(&[]);
        let bundle = read(&pak, &CardOptions::default()).expect("an empty pak is still a pak");
        assert_eq!(bundle.parts.len(), 1);
        assert_eq!(bundle.parts[0].kind, one_saves::PartKind::CardImage);
        assert_eq!(write(&bundle).expect("writes"), pak);
    }

    #[test]
    fn something_that_is_not_a_pak_is_refused_in_this_format_s_words() {
        assert!(matches!(read(&[1, 2, 3], &CardOptions::default()), Err(Error::WrongLength { .. })));
    }
}
