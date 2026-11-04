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
//! `internal`. An AES has no RAM of its own.
//!
//! # A MiSTer save
//!
//! The MiSTer core keeps one file per game: the cabinet's 64 KiB of backup RAM, then 8 KiB for the
//! card in its slot, every byte pair of both swapped, which [`neogeo_memcard`] swaps back. The core
//! reads a file that does not exist yet as all `0xFF`, nothing writes the RAM outside MVS mode,
//! and in MVS mode the BIOS sets up all of it. So a file whose RAM is all `0xFF` is the card
//! alone. Any other is a [`device`](one_saves::Shape::Device): the RAM in `internal`, a save of
//! the game the BIOS recorded in it, and the card in `neogeo-card`. Two MVS games, Jockey Grand
//! Prix and V-Liner, keep RAM of their own on the cartridge, and the core puts that where the card
//! would be, so a region holding something other than a card is that RAM, in `cartridge`. A region
//! nothing ever wrote is left off.
//!
//! In cart mode the core addresses the first 2 KiB of the region and nothing past it, so a
//! cartridge system's card in a MiSTer save is 2 KiB whatever its header claims, and is written
//! back that size.
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
//! it is, both numbers, so the [`path`](one_saves::Part::path) spells them as the spec does — the
//! NGH in four hex digits, a dash and the sub-number in one, `0250-1` for Metal Slug X's second —
//! and the NGH goes into the game's [`serial`](one_saves::Game::serial) as well.
//!
//! The title a game writes into its own data is what the BIOS lists the save under, so it is
//! `x.1sav.label`, which is where every other format puts what the console displays.

use neogeo_memcard::{
    CardBuilder, Container, Geometry, MISTER_BACKUP_RAM, MISTER_CARD_REGION, MISTER_CART_CARD, MemoryCard,
    Save, cabinet,
};
use one_saves::{Bundle, Game, Header, Part, PartKind, Shape, Slug};
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

/// The role of a cartridge's own RAM, which a MiSTer save keeps where the card would be.
const CARTRIDGE_ROLE: &str = "cartridge";

/// The size of the card a Neo Geo CD keeps inside itself, and so of the one the MiSTer core
/// emulates in CD mode.
const CD_CARD: usize = 8192;

/// Whether these bytes look like a Neo Geo card, bare or in a MiSTer save.
#[must_use]
pub fn detect(bytes: &[u8]) -> bool {
    neogeo_memcard::detect(bytes)
}

/// Reads a card into a bundle: one nested bundle per save, or a device for a MiSTer save whose
/// cabinet RAM holds anything.
///
/// The bundle is a Neo Geo CD's when [`CardOptions::system`] says `neogeo-cd`, or, when it names
/// neither system, when the card is an 8 KiB one in a MiSTer save whose RAM nothing wrote: the
/// core emulates the 2 KiB card a cartridge console takes in cart mode and the CD's in CD mode,
/// and leaves the RAM alone in CD mode. A bare card does not say what wrote it, and cards for the
/// cartridge consoles come in sizes up to 16 KiB, so it is a cartridge console's unless the caller
/// says otherwise, as `--from neocd` does.
pub fn read(bytes: &[u8], options: &CardOptions) -> Result<Bundle> {
    let stated = options
        .system
        .as_ref()
        .map(Slug::as_str)
        .filter(|stated| *stated == SYSTEM || *stated == NEOGEO_CD.slug);

    let Some((ram, region)) = neogeo_memcard::split_mister(bytes) else {
        let card = MemoryCard::parse(bytes).map_err(into_error)?;
        return card_bundle(&card, stated.unwrap_or(SYSTEM), options);
    };

    let ram_written = ram.iter().any(|&b| b != 0xFF);
    let system = match stated {
        Some(stated) => stated,
        None if !ram_written && header_size(&region) == Some(CD_CARD) => NEOGEO_CD.slug,
        None => SYSTEM,
    };
    if !ram_written {
        let medium = if system == NEOGEO_CD.slug { CD_CARD } else { MISTER_CART_CARD };
        let card = MemoryCard::parse_within(bytes, medium).map_err(into_error)?;
        return card_bundle(&card, system, options);
    }
    if system == NEOGEO_CD.slug {
        return Err(Error::NotConvertible(
            "this MiSTer save holds a cabinet's backup RAM, and a Neo Geo CD has none: the core leaves \
             the RAM alone in CD mode"
                .into(),
        ));
    }
    cabinet_device(bytes, ram, region, options)
}

/// The size a card region's header claims, when the region holds a card.
fn header_size(region: &[u8]) -> Option<usize> {
    neogeo_memcard::strip_container(region).map(|(_, card, _)| {
        let word =
            u16::from_be_bytes([card[neogeo_memcard::SIZE_OFFSET], card[neogeo_memcard::SIZE_OFFSET + 1]]);
        usize::from(word) / 2
    })
}

/// A card as a bundle of its saves.
fn card_bundle(card: &MemoryCard, system: &str, options: &CardOptions) -> Result<Bundle> {
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
        // console's.
        let title = save.title();
        let mut inner = one_saves::Extensions::new();
        if let Some(value) = label(Some(title).filter(|t| !t.is_empty()), None) {
            inner.insert(value);
        }

        let mut part =
            save_part_as(Some(system), role.clone(), parts.len(), save.data.clone(), Some(game), inner)?
                .slot(u64::try_from(save.slot).expect("slot fits"))
                .dirent(save.dirent.clone());
        part.path = Some(path_of(save.ngh, save.sub));
        parts.push(part);
    }

    if parts.is_empty() {
        parts.push(card_image_part(0, card.image(), &role));
    }

    Ok(Bundle::new(
        card_header(Format::NeoGeoCard, card.capacity(), Some(card.system_area().to_vec()), options)
            .system(Some(slug(system))),
        parts,
    ))
}

/// A save's `path`: its NGH number in four hex digits, a dash, its sub-number in one, lowercase.
fn path_of(ngh: u16, sub: u8) -> String {
    format!("{ngh:04x}-{sub:x}")
}

/// The NGH number and sub-number a `path` spells, for a save that arrived without an entry.
fn path_numbers(path: &str) -> Option<(u16, u8)> {
    let (ngh, sub) = path.split_once('-')?;
    if ngh.len() != 4 || sub.len() != 1 {
        return None;
    }
    Some((u16::from_str_radix(ngh, 16).ok()?, u8::from_str_radix(sub, 16).ok()?))
}

/// A MiSTer save whose cabinet RAM holds something, as the device it is.
///
/// The RAM is a save of the game the BIOS recorded in it, which on the core's one-slot board is
/// the file's. Beside it the region holds the card, a cartridge's own RAM, or nothing anyone
/// wrote, which is left off.
fn cabinet_device(bytes: &[u8], ram: Vec<u8>, region: Vec<u8>, options: &CardOptions) -> Result<Bundle> {
    let game = recorded_game(&ram);
    let component = |id: u64, role: Slug, inner: &Bundle, game: Option<Game>| -> Result<Part> {
        Ok(Part::new(id, inner.to_vec()?).kind(PartKind::Bundle).role(role).game(game).system(slug(SYSTEM)))
    };
    let save = |payload: Vec<u8>| {
        Bundle::new(
            Header::new(Shape::Save).system(slug(SYSTEM)).game(game.clone()).source(options.source.clone()),
            vec![Part::new(0, payload)],
        )
    };

    // In the order the MiSTer core writes them: the RAM, then whatever is in the card's place.
    let mut parts = vec![component(0, slug(INTERNAL_ROLE), &save(ram), game.clone())?];
    if header_size(&region).is_some() {
        // One machine has one `internal`, and the RAM is it.
        let role = options.role_for(Format::NeoGeoCard);
        if role.as_str() == INTERNAL_ROLE {
            return Err(Error::NotConvertible(
                "this MiSTer save holds a cabinet's backup RAM, which is the `internal` socket, so the \
                 card beside it is not"
                    .into(),
            ));
        }
        let card = MemoryCard::parse_within(bytes, MISTER_CART_CARD).map_err(into_error)?;
        parts.push(component(1, role, &card_bundle(&card, SYSTEM, options)?, None)?);
    } else if region.iter().any(|&b| b != 0xFF) {
        parts.push(component(1, slug(CARTRIDGE_ROLE), &save(region), game)?);
    }
    Ok(Bundle::new(Header::new(Shape::Device).system(slug(SYSTEM)).source(options.source.clone()), parts))
}

/// The game the BIOS recorded in a cabinet's RAM, by its NGH number and its name, when it recorded
/// exactly one.
///
/// A real cabinet has up to six slots, and its RAM is then several games' rather than any one's.
fn recorded_game(ram: &[u8]) -> Option<Game> {
    match cabinet::games(ram).as_slice() {
        [one] => {
            let game = Game::default().serial(format!("NGH-{:04X}", one.ngh));
            Some(if one.name.is_empty() { game } else { game.title(one.name.clone()) })
        }
        _ => None,
    }
}

/// Writes a bundle back out as a bare card image, or a device as the MiSTer save it was read from.
///
/// The directory, both allocation tables, their checksums, the card's size and its signature are
/// regenerated; the rest of the header comes back from `system_area`, so the card's username and
/// region survive. The card is the size its `capacity` says.
///
/// A card read out of a MiSTer save comes back **bare**, the same way a PS1 card read out of a
/// DexDrive file does: the container is the producer's, not the card's. A **device** is the
/// exception, since it is everything the MiSTer file held: that comes back as the MiSTer save.
pub fn write(bundle: &Bundle) -> Result<Vec<u8>> {
    if *bundle.shape() == Shape::Device {
        return write_cabinet(bundle);
    }

    // A card with nothing on it carries a whole-card image rather than nested saves, and writing
    // it back is a byte copy.
    if let Some(image) = image_only(bundle)? {
        return Ok(image);
    }
    build(bundle, card_size(bundle)?)
}

/// The card a bundle's saves go on, built this many bytes.
///
/// The size is the medium's rather than necessarily the card's the saves came off, so a card can
/// land on a smaller one, provided its saves fit.
fn build(bundle: &Bundle, size: usize) -> Result<Vec<u8>> {
    let mut builder = CardBuilder::new(size).map_err(into_error)?;
    if let Some(area) = bundle.header.card.as_ref().and_then(|card| card.system_area.as_ref()) {
        builder.system_area(area.clone());
    }
    builder.container(Container::Bare);

    for save in nested_saves(bundle)? {
        let dirent = save.dirent.unwrap_or_default();
        // The entry carries the sub-number and the NGH. A save that arrived without one falls back
        // to what its path spells, then to its serial and the first sub-number.
        let (sub, ngh) = match dirent.len() {
            neogeo_memcard::ENTRY_LEN => (dirent[0], u16::from_be_bytes([dirent[1], dirent[2]])),
            _ => match save.path.as_deref().and_then(path_numbers) {
                Some((ngh, sub)) => (sub, ngh),
                None => (0, ngh_from_serial(&save.inner).unwrap_or_default()),
            },
        };
        builder.add(Save { slot: 0, sub, ngh, dirent, data: save.bytes });
    }
    builder.build().map_err(into_error)
}

/// How large a card bundle's card is: its `capacity`, read as [`card_capacity`] reads it.
fn card_size(bundle: &Bundle) -> Result<usize> {
    let recorded = bundle
        .header
        .card
        .as_ref()
        .map(|card| card.capacity)
        .ok_or_else(|| Error::NotConvertible("this bundle does not say how big its card is".into()))?;
    let capacity = usize::try_from(card_capacity(recorded)).unwrap_or(usize::MAX);
    Geometry::of(capacity)
        .map(|geometry| geometry.size)
        .ok_or_else(|| Error::NotConvertible(format!("no Neo Geo card is {recorded} bytes")))
}

/// The whole card's size, out of the `capacity` a Neo Geo card bundle recorded.
///
/// That is the card's size, structure included. Bundles before the spec settled it recorded the
/// room left for saves instead, which no card size equals, so one of those is turned into the
/// size of the card it describes. Anything else comes back as it was.
pub(crate) fn card_capacity(recorded: u64) -> u64 {
    usize::try_from(recorded)
        .ok()
        .filter(|&bytes| Geometry::of(bytes).is_none())
        .and_then(Geometry::for_data_capacity)
        .map_or(recorded, |geometry| geometry.size as u64)
}

/// A cabinet's backup RAM and what sat beside it, written back out as the MiSTer save they were
/// read from.
///
/// A card goes in at the 2 KiB the core addresses in cart mode. A part that is not there leaves
/// its half as a fresh file holds it, all `0xFF`.
fn write_cabinet(device: &Bundle) -> Result<Vec<u8>> {
    let mut ram = None;
    let mut region = None;
    for part in device.parts.iter().filter(|part| part.kind == PartKind::Bundle) {
        let inner = Bundle::from_slice(&part.bytes()?)?;
        let role = part.role.as_ref().map_or("primary", Slug::as_str);
        let beside = match (role, inner.shape()) {
            (INTERNAL_ROLE, Shape::Save) => {
                ram = Some(concatenated(&inner)?);
                continue;
            }
            (_, Shape::Card) => card_on_cartridge_medium(&inner)?,
            (CARTRIDGE_ROLE, Shape::Save) => concatenated(&inner)?,
            _ => {
                return Err(Error::NotConvertible(format!(
                    "a MiSTer save holds a cabinet's RAM and a card or a cartridge's RAM beside it, and this \
                     device's `{role}` is neither"
                )));
            }
        };
        if region.replace(beside).is_some() {
            return Err(Error::NotConvertible(
                "a MiSTer save holds one card or one cartridge's RAM beside the cabinet's, and this device \
                 holds two"
                    .into(),
            ));
        }
    }

    let ram = ram.unwrap_or_default();
    if !ram.is_empty() && ram.len() != MISTER_BACKUP_RAM {
        return Err(Error::NotConvertible(format!(
            "a cabinet's backup RAM is {MISTER_BACKUP_RAM} bytes in a MiSTer save, and this one is {}",
            ram.len()
        )));
    }
    let region = region.unwrap_or_default();
    if region.len() > MISTER_CARD_REGION {
        return Err(Error::NotConvertible(format!(
            "a MiSTer save has {MISTER_CARD_REGION} bytes beside the cabinet's RAM, and this cartridge's RAM is {}",
            region.len()
        )));
    }
    neogeo_memcard::join_mister(&ram, &region).map_err(into_error)
}

/// A card bundle as the 2 KiB card a MiSTer save in cart mode holds.
///
/// Built from its saves at that size, whatever size the card they came off was. A whole-card image
/// is the card it was and goes in only if it fits: one copied off a larger card has a header
/// promising room past the end, which the BIOS fills by writing over that header.
fn card_on_cartridge_medium(card: &Bundle) -> Result<Vec<u8>> {
    if card.header.card.as_ref().is_none_or(|map| map.format.as_str() != CARD_FORMAT) {
        return Err(Error::NotConvertible("the card this device holds is not a Neo Geo card".into()));
    }
    let Some(image) = image_only(card)? else {
        return build(card, MISTER_CART_CARD);
    };
    let size = card_size(card)?;
    if size > MISTER_CART_CARD || image.len() > MISTER_CART_CARD {
        return Err(Error::NotConvertible(format!(
            "this card is a whole {size}-byte image, and a MiSTer save's cartridge card is {MISTER_CART_CARD} \
             bytes; an image does not shrink, so take its saves instead"
        )));
    }
    Ok(image)
}

/// A save's bytes: its parts', end to end.
fn concatenated(save: &Bundle) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    for part in &save.parts {
        bytes.extend_from_slice(&part.bytes()?);
    }
    Ok(bytes)
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

    #[test]
    fn a_path_spells_the_ngh_and_the_sub_number() {
        assert_eq!(path_of(0x0201, 0), "0201-0");
        assert_eq!(path_of(0x0250, 15), "0250-f", "lowercase");
        assert_eq!(path_numbers("0250-f"), Some((0x0250, 15)));
        for unspelled in ["METAL SLUG X", "250-1", "0250-10", "0250"] {
            assert_eq!(path_numbers(unspelled), None, "{unspelled}");
        }
    }
}
