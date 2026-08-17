//! Neo Geo memory cards.
//!
//! The filesystem itself lives in [`neogeo_memcard`], which is a standalone crate because a Neo Geo
//! card is a genuinely reusable format and nothing about reading one needs this container. What is
//! here is the adapter: the mapping between a card's saves and a bundle's nested parts.
//!
//! # One card, two consoles
//!
//! The MVS cabinet and the AES home console take the same card and read each other's saves, so
//! there is one system slug — `neogeo` — and not one per machine. What differs is the *socket*: a
//! card is [`neogeo-card`](one_saves_registry::role), and a cabinet's own battery-backed RAM is
//! `internal`. A MiSTer save carries both, and both come across.
//!
//! # What identifies a save
//!
//! Not a filename: a card has none. A save is a game's NGH number and which of that game's saves
//! it is, so the [`path`](one_saves::Part::path) is the title the game wrote into its own data and
//! the NGH goes into the game's [`serial`](one_saves::Game::serial). Two saves for one game are
//! told apart by [`slot`](one_saves::Part::slot), as they are on every other card.

use neogeo_memcard::{CardBuilder, Container, MemoryCard, Save};
use one_saves::{Bundle, Game, Part, PartKind};

use crate::CardOptions;
use crate::card::{card_header, card_image_part, image_only, nested_saves, save_part, slug};
use crate::detect::Format;
use crate::error::{Error, Result};

/// What the format is called, for error messages.
const FORMAT: &str = Format::NeoGeoCard.label();

/// The card format slug a bundle carries.
pub const CARD_FORMAT: &str = match Format::NeoGeoCard.card_format() {
    Some(slug) => slug,
    None => panic!("a Neo Geo card is a card format"),
};

/// The system slug these saves are for.
pub const SYSTEM: &str = match Format::NeoGeoCard.system() {
    Some(slug) => slug,
    None => panic!("a Neo Geo card holds saves for a system"),
};

/// The role a cabinet's own backup RAM takes, as distinct from the card's.
const BACKUP_RAM_ROLE: &str = "internal";

/// Whether these bytes look like a Neo Geo card, bare or in a MiSTer save.
#[must_use]
pub fn detect(bytes: &[u8]) -> bool {
    neogeo_memcard::detect(bytes)
}

/// Reads a card into a bundle: one nested bundle per save.
pub fn read(bytes: &[u8], options: &CardOptions) -> Result<Bundle> {
    let card = MemoryCard::parse(bytes).map_err(into_error)?;
    let role = options.role_for(Format::NeoGeoCard);

    let mut parts = Vec::new();
    for save in card.saves() {
        // The NGH number is what a card is keyed on and what the BIOS looks a save up by, so it is
        // the serial. It is binary-coded decimal on the card, which is also how it is printed on a
        // cartridge: NGH-047.
        let game = Some(Game { serial: Some(format!("NGH-{:04X}", save.ngh)), ..Game::default() });

        let mut part = save_part(Format::NeoGeoCard, parts.len(), save.data.clone(), game, options)?;
        let title = save.title();
        part.path = (!title.is_empty()).then_some(title);
        part.slot = Some(u64::try_from(save.slot).expect("slot fits"));
        part.dirent = Some(save.dirent.clone());
        parts.push(part);
    }

    if parts.is_empty() {
        parts.push(card_image_part(0, card.image(), &role));
    }

    // A cabinet keeps its own saves beside the card's, and a MiSTer file carries both. Dropping
    // half of what a file holds is not a conversion, so it rides along in its own socket. An
    // erased one says nothing and is left off.
    if let Some(ram) = card.backup_ram()
        && ram.iter().any(|&b| b != 0xFF)
    {
        let mut part = Part::new(u64::try_from(parts.len()).expect("part count fits"), ram.to_vec());
        part.kind = PartKind::Aux;
        part.role = Some(slug(BACKUP_RAM_ROLE));
        part.system = Some(slug(SYSTEM));
        parts.push(part);
    }

    Ok(Bundle {
        header: card_header(
            Format::NeoGeoCard,
            card.geometry().data_capacity(),
            Some(card.system_area().to_vec()),
            options,
        ),
        parts,
    })
}

/// Writes a bundle back out as a bare card image.
///
/// The directory, both allocation tables and their checksums are regenerated; everything else in
/// the reserved region comes back from `system_area`, so the card's signature, username and region
/// survive.
///
/// A card read out of a MiSTer save comes back **bare**, the same way a PS1 card read out of a
/// DexDrive file does: the container is the producer's, not the card's. A cabinet's backup RAM
/// therefore does not come back here — it is a part of its own in the bundle, and pulling it out
/// is `--part`'s job rather than this one's.
pub fn write(bundle: &Bundle) -> Result<Vec<u8>> {
    // A card with nothing on it carries a whole-card image rather than nested saves, and writing
    // it back is a byte copy.
    if let Some(image) = image_only(bundle)? {
        return Ok(image);
    }

    let capacity = bundle
        .header
        .card
        .as_ref()
        .and_then(|card| usize::try_from(card.capacity).ok())
        .ok_or_else(|| Error::NotConvertible("this bundle does not say how big its card is".into()))?;
    let size = neogeo_memcard::Geometry::for_data_capacity(capacity)
        .ok_or_else(|| Error::NotConvertible(format!("no card holds {capacity} bytes of saves")))?
        .size;

    let mut builder = CardBuilder::new(size).map_err(into_error)?;
    if let Some(area) = bundle.header.card.as_ref().and_then(|card| card.system_area.as_ref()) {
        builder.system_area(area.clone());
    }

    builder.container(Container::Bare);

    for save in nested_saves(bundle)? {
        let dirent = save.dirent.unwrap_or_default();
        // The entry carries the sub-number and the NGH; a save that arrived without one falls back
        // to what the bundle says, and to the first sub-number when it says nothing.
        let (sub, ngh) = match dirent.len() {
            neogeo_memcard::ENTRY_LEN => (dirent[0], u16::from_be_bytes([dirent[1], dirent[2]])),
            _ => (0, ngh_from_serial(&save.inner).unwrap_or_default()),
        };
        builder.add(Save { slot: 0, sub, ngh, dirent, data: save.bytes });
    }
    builder.build().map_err(into_error)
}

/// The NGH number out of a nested save's `serial`, for a save that arrived without an entry.
fn ngh_from_serial(inner: &Bundle) -> Option<u16> {
    let serial = inner.header.game.as_ref()?.serial.as_ref()?;
    u16::from_str_radix(serial.strip_prefix("NGH-")?, 16).ok()
}

fn into_error(source: neogeo_memcard::Error) -> Error {
    match source {
        neogeo_memcard::Error::WrongLength(found) => Error::WrongLength {
            format: FORMAT,
            expected: "2 KiB to 16 KiB of card, bare or behind a MiSTer save's 64 KiB of backup RAM",
            found,
        },
        neogeo_memcard::Error::NotAMemoryCard => Error::NotThisFormat {
            format: FORMAT,
            why: "this image does not carry the `NEO-GEO` signature".into(),
        },
        neogeo_memcard::Error::Corrupt(why) => Error::Corrupt { format: FORMAT, why },
        other => Error::NotConvertible(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn something_that_is_not_a_card_is_refused_in_this_format_s_words() {
        assert!(matches!(read(&[1, 2, 3], &CardOptions::default()), Err(Error::WrongLength { .. })));
    }
}
