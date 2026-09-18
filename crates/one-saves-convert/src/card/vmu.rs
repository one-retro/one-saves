//! Dreamcast Visual Memory Units.
//!
//! The filesystem itself lives in [`dreamcast_vmu`], which is a standalone crate because a VMU is
//! a genuinely reusable format and nothing about reading one needs this container. What is here is
//! the adapter: the mapping between a VMU's saves and a bundle's nested parts.
//!
//! A mini-game is a save that executes in place from flash, so it must start at block 0 and stay
//! contiguous. Its directory entry says so, and that entry rides on the part as its `dirent`, so a
//! writer can honour the constraint without understanding the payload.

use dreamcast_vmu::{FileKind, Save, Vmu, VmuBuilder};
use one_saves::{Bundle, Game};

use crate::CardOptions;
use crate::card::{card_header, card_image_part, image_only, nested_saves, save_part};
use crate::detect::Format;
use crate::error::{Error, Result};

/// What the format is called, for error messages.
const FORMAT: &str = Format::Vmu.label();

/// The card format slug a bundle carries.
pub const CARD_FORMAT: &str = match Format::Vmu.card_format() {
    Some(slug) => slug,
    None => panic!("a VMU is a card format"),
};

/// The system slug these saves are for.
pub const SYSTEM: &str = match Format::Vmu.system() {
    Some(slug) => slug,
    None => panic!("a VMU holds saves for a system"),
};

/// Whether these bytes look like a VMU image.
#[must_use]
pub fn detect(bytes: &[u8]) -> bool {
    dreamcast_vmu::detect(bytes)
}

/// Reads a VMU into a bundle: one nested bundle per save.
pub fn read(bytes: &[u8], options: &CardOptions) -> Result<Bundle> {
    let vmu = Vmu::parse(bytes).map_err(into_error)?;

    let mut parts = Vec::new();
    for save in vmu.saves() {
        let game = Some(Game { title: Some(save.name.clone()), ..Game::default() });

        let mut part = save_part(Format::Vmu, parts.len(), save.data.clone(), game, options)?;
        part.path = Some(save.name.clone());
        part.slot = Some(u64::try_from(save.slot).expect("slot fits"));
        part.dirent = Some(save.dirent.clone());
        parts.push(part);
    }

    if parts.is_empty() {
        parts.push(card_image_part(0, vmu.image(), &options.role_for(Format::Vmu)));
    }

    Ok(Bundle {
        header: card_header(
            Format::Vmu,
            vmu.capacity(),
            // The root block carries the VMU's custom colour and icon shape, which belong to no
            // save and would otherwise be dropped by splitting the card.
            Some(vmu.root_block().to_vec()),
            options,
        ),
        parts,
    })
}

/// Writes a bundle back out as a raw 128 KiB VMU image.
///
/// The allocation table and the directory are regenerated; the root block is kept from
/// `system_area` when the bundle carries one, so the VMU's colour and icon survive.
pub fn write(bundle: &Bundle) -> Result<Vec<u8>> {
    // A card with nothing on it carries a whole-card image rather than nested saves, and writing
    // it back is a byte copy.
    if let Some(image) = image_only(bundle)? {
        return Ok(image);
    }

    let mut builder = VmuBuilder::new();
    if let Some(area) = bundle.header.card.as_ref().and_then(|card| card.system_area.as_ref()) {
        builder.root_block(area.clone());
    }
    for save in nested_saves(bundle)? {
        let dirent = save.dirent.unwrap_or_default();
        // Whether a save is a mini-game is in its entry's type byte, and a mini-game is the one
        // thing a writer cannot place freely. A save that arrived without an entry is data: it is
        // what the console writes for everything a game saves.
        let kind = dirent.first().copied().and_then(FileKind::from_byte).unwrap_or_default();
        builder.add(Save { slot: 0, kind, name: save.path.unwrap_or_default(), dirent, data: save.bytes });
    }
    builder.build().map_err(into_error)
}

fn into_error(source: dreamcast_vmu::Error) -> Error {
    match source {
        dreamcast_vmu::Error::WrongLength(found) => {
            Error::WrongLength { format: FORMAT, expected: "131072 bytes", found }
        }
        dreamcast_vmu::Error::NotFormatted => Error::NotThisFormat {
            format: FORMAT,
            why: "the root block does not open with sixteen 0x55 bytes".into(),
        },
        dreamcast_vmu::Error::Corrupt(why) => Error::Corrupt { format: FORMAT, why },
        other => Error::NotConvertible(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a VMU holding the given saves, each of the given block count.
    fn vmu_with(saves: &[(&str, usize, FileKind)]) -> Vec<u8> {
        let mut builder = VmuBuilder::new();
        for (name, blocks, kind) in saves {
            builder.add(Save {
                slot: 0,
                kind: *kind,
                name: (*name).to_owned(),
                dirent: Vec::new(),
                data: (0..blocks * dreamcast_vmu::BLOCK)
                    .map(|byte| u8::try_from(byte & 0xff).expect("masked to a byte"))
                    .collect(),
            });
        }
        builder.build().expect("builds")
    }

    #[test]
    fn reads_saves_and_keeps_the_colour_and_icon() {
        let image = vmu_with(&[("SONIC2___S01", 2, FileKind::Data), ("PSO_______", 1, FileKind::Data)]);
        let bundle = read(&image, &CardOptions::default()).expect("reads");

        assert_eq!(bundle.parts.len(), 2);
        let card = bundle.header.card.as_ref().unwrap();
        assert_eq!(card.format.as_str(), "vmu");
        // The data capacity is 200 blocks, not the 131072 bytes the image runs to.
        assert_eq!(card.capacity, 102_400);
        assert_eq!(card.system_area.as_ref().map(Vec::len), Some(512));

        assert_eq!(bundle.parts[0].path.as_deref(), Some("SONIC2___S01"));
        assert_eq!(bundle.parts[0].dirent.as_ref().map(Vec::len), Some(32));

        let inner = Bundle::from_slice(&bundle.parts[0].bytes().unwrap()).unwrap();
        assert_eq!(inner.parts[0].payload.len(), 1024, "two blocks");
        bundle.validate().expect("valid bundle");
    }

    #[test]
    fn a_vmu_round_trips_through_a_bundle() {
        let image = vmu_with(&[("SONIC2___S01", 2, FileKind::Data), ("PSO_______", 1, FileKind::Data)]);
        let bundle = read(&image, &CardOptions::default()).expect("reads");
        assert_eq!(write(&bundle).expect("writes"), image);
    }

    #[test]
    fn a_minigame_is_placed_at_block_zero() {
        // A mini-game executes in place from flash, so it cannot be put anywhere else. Here the
        // data save is listed first, and the writer still has to put the game at block 0. Nothing
        // in the bundle says "mini-game" except the entry the part carries verbatim.
        let image = vmu_with(&[("DATA________", 1, FileKind::Data), ("MINIGAME____", 3, FileKind::Game)]);
        let bundle = read(&image, &CardOptions::default()).expect("reads");
        let rebuilt = write(&bundle).expect("writes");

        let reread = Vmu::parse(&rebuilt).expect("reads back");
        let game = reread.saves().iter().find(|save| save.kind == FileKind::Game).expect("survived");
        assert_eq!(u16::from_le_bytes([game.dirent[2], game.dirent[3]]), 0, "a mini-game starts at block 0");
    }

    #[test]
    fn a_vmu_with_nothing_on_it_becomes_a_card_image() {
        let image = vmu_with(&[]);
        let bundle = read(&image, &CardOptions::default()).expect("an empty VMU is still a VMU");
        assert_eq!(bundle.parts.len(), 1);
        assert_eq!(bundle.parts[0].kind, one_saves::PartKind::CardImage);
        assert_eq!(write(&bundle).expect("writes"), image);
    }

    #[test]
    fn refuses_an_image_that_is_not_formatted() {
        let image = vec![0u8; dreamcast_vmu::IMAGE_LEN];
        assert!(!detect(&image));
        assert!(matches!(read(&image, &CardOptions::default()), Err(Error::NotThisFormat { .. })));
    }
}
