//! GameCube memory cards.
//!
//! The filesystem itself lives in [`gc_memcard`], which is a standalone crate because a GameCube
//! card is a genuinely reusable format and nothing about reading one needs this container. What is
//! here is the adapter: the mapping between a card's saves and a bundle's nested parts.
//!
//! Slot A is [`memcard-1`](one_saves_registry::role) and Slot B is `memcard-2`. The registry keeps
//! one numbering convention rather than mirroring each console's silkscreen.

use gc_memcard::{CardBuilder, MemoryCard, Save};
use one_saves::{Bundle, Game};

use crate::CardOptions;
use crate::card::{card_header, card_image_part, image_only, nested_saves, save_part};
use crate::detect::Format;
use crate::error::{Error, Result};

/// What the format is called, for error messages.
const FORMAT: &str = Format::GcCard.label();

/// The card format slug a bundle carries.
pub const CARD_FORMAT: &str = match Format::GcCard.card_format() {
    Some(slug) => slug,
    None => panic!("a GameCube card is a card format"),
};

/// The system slug these saves are for.
pub const SYSTEM: &str = match Format::GcCard.system() {
    Some(slug) => slug,
    None => panic!("a GameCube card holds saves for a system"),
};

/// Whether these bytes look like a GameCube card.
#[must_use]
pub fn detect(bytes: &[u8]) -> bool {
    gc_memcard::detect(bytes)
}

/// Reads a card into a bundle: one nested bundle per save.
pub fn read(bytes: &[u8], options: &CardOptions) -> Result<Bundle> {
    let card = MemoryCard::parse(bytes).map_err(into_error)?;

    let mut parts = Vec::new();
    for save in card.saves() {
        // A GameCube save is opened by game code plus maker code plus filename, so the serial is
        // the pair rather than either alone.
        let game = Some(Game {
            serial: (!save.game_code.is_empty()).then(|| format!("{}{}", save.game_code, save.maker_code)),
            ..Game::default()
        });

        let mut part = save_part(Format::GcCard, parts.len(), save.data.clone(), game, options)?;
        part.path = Some(save.filename.clone());
        part.slot = Some(u64::try_from(save.slot).expect("slot fits"));
        part.dirent = Some(save.dirent.clone());
        parts.push(part);
    }

    if parts.is_empty() {
        parts.push(card_image_part(0, card.image(), &options.role_for(Format::GcCard)));
    }

    Ok(Bundle {
        header: card_header(
            Format::GcCard,
            card.capacity(),
            // The header block carries the card's flash serial, which is what a Phantasy Star
            // Online save is signed against, so it is worth keeping across a split.
            Some(card.header_block().to_vec()),
            options,
        ),
        parts,
    })
}

/// Writes a bundle back out as a raw card image.
///
/// The directory, the allocation table, both their mirrors and every checksum are regenerated. The
/// header block is kept from `system_area` when the bundle carries one, so the card's flash serial
/// survives.
pub fn write(bundle: &Bundle) -> Result<Vec<u8>> {
    // A card with nothing on it carries a whole-card image rather than nested saves, and writing
    // it back is a byte copy.
    if let Some(image) = image_only(bundle)? {
        return Ok(image);
    }

    let mut builder = CardBuilder::new();
    if let Some(card) = bundle.header.card.as_ref() {
        if let Some(area) = card.system_area.as_ref() {
            builder.header_block(area.clone());
        }
        if let Ok(capacity) = usize::try_from(card.capacity) {
            builder.capacity(capacity);
        }
    }
    for save in nested_saves(bundle)? {
        // Only reached for a save that arrived without an entry; one that has an entry carries its
        // codes inside it, verbatim.
        let serial = save.inner.header.game.as_ref().and_then(|game| game.serial.clone()).unwrap_or_default();
        let (game_code, maker_code) = serial.split_at(serial.len().min(4));
        builder.add(Save {
            slot: 0,
            game_code: game_code.to_owned(),
            maker_code: maker_code.to_owned(),
            filename: save.path.unwrap_or_default(),
            dirent: save.dirent.unwrap_or_default(),
            data: save.bytes,
        });
    }
    builder.build().map_err(into_error)
}

fn into_error(source: gc_memcard::Error) -> Error {
    match source {
        gc_memcard::Error::WrongLength(found) => Error::WrongLength {
            format: FORMAT,
            expected: "a whole number of 8 KiB blocks, in a size the console formats",
            found,
        },
        gc_memcard::Error::NotAMemoryCard => Error::NotThisFormat {
            format: FORMAT,
            why: "neither the directory nor its mirror checksums".into(),
        },
        gc_memcard::Error::Corrupt(why) => Error::Corrupt { format: FORMAT, why },
        other => Error::NotConvertible(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a card holding the given saves, each of the given block count.
    fn card_with(blocks: usize, saves: &[(&str, &str, usize)]) -> Vec<u8> {
        let mut builder = CardBuilder::new();
        builder.capacity((blocks - gc_memcard::RESERVED_BLOCKS) * gc_memcard::BLOCK);
        for (code, name, count) in saves {
            let (game_code, maker_code) = code.split_at(4);
            builder.add(Save {
                slot: 0,
                game_code: game_code.to_owned(),
                maker_code: maker_code.to_owned(),
                filename: (*name).to_owned(),
                dirent: Vec::new(),
                data: (0..count * gc_memcard::BLOCK)
                    .map(|byte| u8::try_from(byte & 0xff).expect("masked to a byte"))
                    .collect(),
            });
        }
        builder.build().expect("builds")
    }

    #[test]
    fn reads_saves_and_keeps_the_header_block() {
        let card = card_with(64, &[("GAFE01", "super_mario_sunshine", 2), ("GM4E01", "PSO_SYSTEM", 1)]);
        let bundle = read(&card, &CardOptions::default()).expect("reads");

        assert_eq!(bundle.parts.len(), 2);
        let card_map = bundle.header.card.as_ref().unwrap();
        assert_eq!(card_map.format.as_str(), "gc-mc");
        // The data capacity is the card minus the five blocks structure takes.
        assert_eq!(card_map.capacity, ((64 - 5) * gc_memcard::BLOCK) as u64);
        // The header block carries the flash serial a PSO save is signed against.
        assert_eq!(card_map.system_area.as_ref().map(Vec::len), Some(gc_memcard::BLOCK));

        assert_eq!(bundle.parts[0].path.as_deref(), Some("super_mario_sunshine"));
        assert_eq!(bundle.parts[0].dirent.as_ref().map(Vec::len), Some(64));
        // A save is opened by game code plus maker code, so the serial is the pair.
        assert_eq!(bundle.parts[0].game.as_ref().unwrap().serial.as_deref(), Some("GAFE01"));

        let inner = Bundle::from_slice(&bundle.parts[0].bytes().unwrap()).unwrap();
        assert_eq!(inner.parts[0].payload.len(), 2 * gc_memcard::BLOCK as u64);
        bundle.validate().expect("valid bundle");
    }

    #[test]
    fn a_card_round_trips_through_a_bundle() {
        let card = card_with(64, &[("GAFE01", "super_mario_sunshine", 2), ("GM4E01", "PSO_SYSTEM", 1)]);
        let bundle = read(&card, &CardOptions::default()).expect("reads");
        assert_eq!(write(&bundle).expect("writes"), card);
    }

    #[test]
    fn the_card_keeps_the_size_it_came_in_rather_than_shrinking_to_fit() {
        // A 128-block card holding one save must not be rebuilt as the 64-block card the save
        // would fit on: the capacity is the card's, not the saves'.
        let card = card_with(128, &[("GAFE01", "one", 1)]);
        let bundle = read(&card, &CardOptions::default()).expect("reads");
        assert_eq!(write(&bundle).expect("writes").len(), 128 * gc_memcard::BLOCK);
    }

    #[test]
    fn a_card_with_nothing_on_it_becomes_a_card_image() {
        let card = card_with(64, &[]);
        let bundle = read(&card, &CardOptions::default()).expect("an empty card is still a card");
        assert_eq!(bundle.parts.len(), 1);
        assert_eq!(bundle.parts[0].kind, one_saves::PartKind::CardImage);
        assert_eq!(write(&bundle).expect("writes"), card);
    }

    #[test]
    fn something_that_is_not_a_card_is_refused_in_this_format_s_words() {
        assert!(matches!(read(&[1, 2, 3], &CardOptions::default()), Err(Error::WrongLength { .. })));
        assert!(matches!(
            read(&vec![0u8; 64 * gc_memcard::BLOCK], &CardOptions::default()),
            Err(Error::NotThisFormat { .. })
        ));
    }
}
