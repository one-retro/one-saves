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
//! # And a CD that keeps its card inside
//!
//! A Neo Geo CD takes no card. It keeps its saves in 8 KiB of memory of its own, laid out as a
//! card, so the same reader reads it, and what changes is the system, `neogeo-cd`, and the socket,
//! `internal`. Nothing on the card says which it is; see [`read`] for how that is decided.
//!
//! # What identifies a save
//!
//! Not a filename: a card has none. A save is a game's NGH number and which of that game's saves
//! it is, so the [`path`](one_saves::Part::path) is the title the game wrote into its own data and
//! the NGH goes into the game's [`serial`](one_saves::Game::serial). Two saves for one game are
//! told apart by [`slot`](one_saves::Part::slot), as they are on every other card.
//!
//! That title is also written as `x.1sav.label`, which is where every other format puts what the
//! console displays. It is the same string in both places on purpose: `path` identifies the save
//! within the card and the label says what to call it, and here one string happens to do both.

use neogeo_memcard::{CardBuilder, Container, MemoryCard, Save};
use one_saves::{Bundle, Game, Part, PartKind};
use one_saves_registry::systems::NEOGEO_CD;

use crate::CardOptions;
use crate::card::{card_header, card_image_part, image_only, nested_saves, save_part_as, slug};
use crate::detect::Format;
use crate::error::{Error, Result};
use crate::label::value as label;

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

/// The role of memory built into the machine: a cabinet's backup RAM, as distinct from the card
/// in its slot, and the card a Neo Geo CD keeps inside itself.
const INTERNAL_ROLE: &str = "internal";

/// The size of the card a Neo Geo CD keeps inside itself, and so of the one the MiSTer core
/// emulates in CD mode.
const CD_CARD: usize = 8192;

/// Whether these bytes look like a Neo Geo card, bare or in a MiSTer save.
#[must_use]
pub fn detect(bytes: &[u8]) -> bool {
    neogeo_memcard::detect(bytes)
}

/// Reads a card into a bundle: one nested bundle per save.
///
/// The bundle is a Neo Geo CD's when [`CardOptions::system`] says `neogeo-cd`, or, when it names
/// neither system, when the card is an 8 KiB one in a MiSTer save: the core emulates the 2 KiB
/// card a cartridge console takes in cart mode and the CD's in CD mode. A bare card does not say
/// what wrote it, and cards for the cartridge consoles come in sizes up to 16 KiB, so it is a
/// cartridge console's unless the caller says otherwise, as `--from neocd` does.
pub fn read(bytes: &[u8], options: &CardOptions) -> Result<Bundle> {
    let card = MemoryCard::parse(bytes).map_err(into_error)?;
    let system = match options.system.as_ref().map(one_saves::Slug::as_str) {
        Some(stated) if stated == SYSTEM || stated == NEOGEO_CD.slug => stated,
        _ if card.container() == Container::MiSter && card.geometry().size == CD_CARD => NEOGEO_CD.slug,
        _ => SYSTEM,
    };
    let role = match &options.role {
        Some(role) => role.clone(),
        None if system == NEOGEO_CD.slug => slug(INTERNAL_ROLE),
        None => options.role_for(Format::NeoGeoCard),
    };

    let mut parts = Vec::new();
    for save in card.saves() {
        // The NGH number is what a card is keyed on and what the BIOS looks a save up by, so it is
        // the serial. It is binary-coded decimal on the card, which is also how it is printed on a
        // cartridge: NGH-047.
        let game = Game::default().serial(format!("NGH-{:04X}", save.ngh));

        // The title is what the BIOS lists the save under, so it is a label like every other
        // console's. It doubles as the `path` because a Neo Geo card has no filenames — but a
        // consumer after a name to show should not have to know that, which is what the key is for.
        let title = save.title();
        let mut inner = one_saves::Extensions::new();
        if let Some(value) = label(Some(title.clone()).filter(|t| !t.is_empty()), None) {
            inner.insert(value);
        }

        let mut part =
            save_part_as(Some(system), role.clone(), parts.len(), save.data.clone(), Some(game), inner)?
                .slot(u64::try_from(save.slot).expect("slot fits"))
                .dirent(save.dirent.clone());
        part.path = (!title.is_empty()).then_some(title);
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
        let part = Part::new(u64::try_from(parts.len()).expect("part count fits"), ram.to_vec())
            .kind(PartKind::Aux)
            .role(slug(INTERNAL_ROLE))
            .system(slug(SYSTEM));
        parts.push(part);
    }

    Ok(Bundle::new(
        card_header(
            Format::NeoGeoCard,
            card.geometry().data_capacity(),
            Some(card.system_area().to_vec()),
            options,
        )
        .system(Some(slug(system))),
        parts,
    ))
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
