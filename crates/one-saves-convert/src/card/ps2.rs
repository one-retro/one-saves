//! PlayStation 2 memory cards.
//!
//! The filesystem itself lives in [`ps2_memcard`], which is a standalone crate because a PS2
//! card is a genuinely reusable format and nothing about reading one needs this container. What
//! is here is the adapter: the mapping between a card's saves and a bundle's nested parts.
//!
//! A PS2 save is a **directory** rather than a file — `BASLUS-20312/` holding `icon.sys`, an
//! icon and the game's own data — so its files are the parts of one nested bundle, told apart by
//! [`path`](one_saves::Part::path). The directory's own entry belongs to no file, and rides on
//! the outer `bundle` part alongside the slot.

use one_saves::{Bundle, Game, Header, Part, PartKind};
use ps2_memcard::{Capacity, CardBuilder, File, MemoryCard, Save};

use crate::CardOptions;
use crate::card::{card_header, civil_seconds, created_and_modified, dirent_key, slug};
use crate::detect::Format;
use crate::error::{Error, Result};
use crate::label::{label_key, shift_jis_field, value as label};

/// What the format is called, for error messages.
const FORMAT: &str = Format::Ps2Card.label();

/// The card format slug a bundle carries.
pub const CARD_FORMAT: &str = match Format::Ps2Card.card_format() {
    Some(slug) => slug,
    None => panic!("a PS2 card is a card format"),
};

/// The system slug these saves are for.
pub const SYSTEM: &str = match Format::Ps2Card.system() {
    Some(slug) => slug,
    None => panic!("a PS2 card holds saves for a system"),
};

/// Whether these bytes look like a PS2 memory card.
#[must_use]
pub fn detect(bytes: &[u8]) -> bool {
    MemoryCard::parse(bytes).is_ok()
}

/// Where a PS2 save keeps the line the browser lists it under.
///
/// In `icon.sys`, which is a file of the save's own rather than anything in the directory: 68
/// bytes of Shift-JIS at 0xc0. The browser breaks it across two lines at the offset held at 0x06,
/// which is layout rather than content — both halves are one title, so the break is not read here.
mod icon_sys {
    /// The file every PS2 save carries, and the only one this looks at.
    pub(super) const NAME: &str = "icon.sys";
    /// What it starts with, so a file of the right name and the wrong shape is not read as one.
    pub(super) const MAGIC: &[u8] = b"PS2D";
    /// Where the title sits, and how long it runs.
    pub(super) const TITLE: usize = 0xc0;
    pub(super) const TITLE_LEN: usize = 68;
}

/// The line the browser shows for a save, as `x.1sav.label` wants it.
fn title_of(save: &ps2_memcard::Save) -> Option<one_saves::dcbor::CBOR> {
    let file = save.files.iter().find(|file| file.name == icon_sys::NAME)?;
    if !file.data.starts_with(icon_sys::MAGIC) {
        return None;
    }
    let field = file.data.get(icon_sys::TITLE..icon_sys::TITLE + icon_sys::TITLE_LEN)?;
    label(shift_jis_field(field), None)
}

/// Where a PS2 directory entry keeps the times, and how it lays one out.
///
/// Two `sceMcStDateTime` structs, at 0x08 and 0x18: a reserved byte, then second, minute, hour,
/// day and month as plain binary, then a little-endian year. Confirmed against cards PCSX2 wrote,
/// whose three saves each dated a creation a few seconds before the write that followed it.
const CREATED_AT: usize = 0x08;
const MODIFIED_AT: usize = 0x18;

/// The zone a PS2 keeps its directory times in, in seconds east of UTC.
///
/// A PS2's clock runs on Japan Standard Time whatever the console's region, and the timezone a
/// later BIOS lets you set moves what the browser *shows* rather than what the filesystem writes.
/// So unlike a GameCube or a VMU, a PS2 reading can be qualified: the instant is the reading minus
/// this.
///
/// Established by controlling for the obvious alternative rather than by argument. A console left
/// at its defaults is configured as Japan, so times nine hours ahead of the host prove nothing on
/// their own. The BIOS was then set to a zone seven hours *behind* UTC and a card formatted, and
/// the entry that format wrote still read +9 — so the offset is the hardware's and not the
/// configuration's.
const JST: i32 = 9 * 3_600;

/// One `sceMcStDateTime`, as a wall-clock reading on the Unix scale.
fn entry_seconds(dirent: &[u8], at: usize) -> Option<i64> {
    let f = dirent.get(at..at + 8)?;
    civil_seconds(u16::from_le_bytes([f[6], f[7]]), f[5], f[4], f[3], f[2], f[1])
}

/// Reads a card into a bundle: one nested bundle per save, one part per file in it.
pub fn read(bytes: &[u8], options: &CardOptions) -> Result<Bundle> {
    let card = MemoryCard::parse(bytes).map_err(|e| into_error(&e))?;
    let saves = card.saves().map_err(|e| into_error(&e))?;

    let mut parts = Vec::new();
    for (index, save) in saves.iter().enumerate() {
        let game = Some(Game { serial: serial_from_name(&save.name), ..Game::default() });
        // The directory's record of the save: when it was made and when it was last written, both
        // qualified by the zone a PS2 keeps them in.
        let times = entry_seconds(&save.dirent, CREATED_AT)
            .zip(entry_seconds(&save.dirent, MODIFIED_AT))
            .map(|(created, modified)| created_and_modified(created, modified, Some(JST)));

        // Each file is a part of the nested bundle, carrying its own directory entry. That is
        // what a PS2 save needs and a PS1 save does not: it is a directory with an entry of its
        // own and an entry per file.
        let inner_parts: Vec<Part> = save
            .files
            .iter()
            .enumerate()
            .map(|(file_index, file)| {
                let mut part = Part::new(u64::try_from(file_index).expect("fits"), file.data.clone());
                part.path = Some(file.name.clone());
                part.dirent = Some(file.dirent.clone());
                part
            })
            .collect();

        if inner_parts.is_empty() {
            // A bundle holds at least one part, so a save with no files cannot be represented.
            continue;
        }

        // The title travels with the save, so it goes in the save's own header.
        let mut extensions = one_saves::Extensions::new();
        if let Some(title) = title_of(save) {
            extensions.insert(label_key(), title);
        }
        let inner = Bundle {
            header: Header {
                // A PS2 save is a directory, so this nested bundle has a part per file — still
                // one game's state, and so still a save.
                shape: one_saves::Shape::Save,
                system: Some(slug(SYSTEM)),
                game: game.clone(),
                extensions,
                ..Header::default()
            },
            parts: inner_parts,
        };

        let mut part = Part::new(u64::try_from(parts.len()).expect("fits"), inner.to_vec()?);
        part.kind = PartKind::Bundle;
        part.role = Some(options.role_for(Format::Ps2Card));
        part.path = Some(save.name.clone());
        part.slot = Some(u64::try_from(index).expect("fits"));
        // The directory's own entry, carrying its mode bits and timestamps, belongs to no file.
        part.dirent = Some(save.dirent.clone());
        if let Some(times) = times {
            part.extensions.insert(dirent_key(), times);
        }
        part.game = game;
        part.system = Some(slug(SYSTEM));
        parts.push(part);
    }

    if parts.is_empty() {
        parts.push(crate::card::card_image_part(0, bytes, &options.role_for(Format::Ps2Card)));
    }

    // The capacity is the card's data capacity, which is not the length of a dump of it: an 8 MB
    // card dumps to 8650752 bytes because every page carries a 16-byte spare area. Nothing on a
    // PS2 card belongs to no save, so there is no system area to keep.
    Ok(Bundle { header: card_header(Format::Ps2Card, card.capacity(), None, options), parts })
}

/// Writes a bundle back out as a card image.
///
/// The image is plain data pages by default. A hardware dump with spare areas is
/// [`write_with_spare`].
pub fn write(bundle: &Bundle) -> Result<Vec<u8>> {
    // A card with nothing on it carries a whole-card image rather than nested saves, and writing
    // it back is a byte copy.
    if let Some(image) = crate::card::image_only(bundle)? {
        return Ok(image);
    }
    builder_for(bundle)?.build().map_err(|e| into_error(&e))
}

/// Writes a bundle back out as a hardware dump, spare areas and ECC included.
pub fn write_with_spare(bundle: &Bundle) -> Result<Vec<u8>> {
    if let Some(image) = crate::card::image_only(bundle)? {
        return Ok(image);
    }
    builder_for(bundle)?.build_with_spare().map_err(|e| into_error(&e))
}

fn builder_for(bundle: &Bundle) -> Result<CardBuilder> {
    let saves = crate::card::nested_saves(bundle)?;

    // A card that came from a bundle keeps its stated capacity where that is a size cards come
    // in; anything else gets the standard 8 MB card, since a capacity nothing makes is not a
    // card a console would accept.
    let capacity = bundle
        .header
        .card
        .as_ref()
        .and_then(|card| usize::try_from(card.capacity).ok())
        .and_then(Capacity::from_bytes)
        .unwrap_or(Capacity::Mb8);

    let mut builder = CardBuilder::new(capacity);
    for save in &saves {
        let name = save
            .path
            .clone()
            .ok_or_else(|| Error::NotConvertible("a save has no path to name its directory".into()))?;

        let files = save
            .inner
            .parts
            .iter()
            .map(|part| {
                Ok(File {
                    name: part.path.clone().unwrap_or_else(|| name.clone()),
                    dirent: part.dirent.clone().unwrap_or_default(),
                    data: part.bytes()?.into_owned(),
                })
            })
            .collect::<Result<Vec<_>>>()?;

        builder.add(Save { name, dirent: save.dirent.clone().unwrap_or_default(), files });
    }
    Ok(builder)
}

/// The product code out of a save directory's name.
///
/// A PS2 save directory is named region code, then the disc's product code, the same way a PS1
/// save file is: `BASLUS-20312` is `BA` and `SLUS-20312`.
fn serial_from_name(name: &str) -> Option<String> {
    if name.len() < 12 {
        return None;
    }
    let serial = &name[2..12];
    let shape = serial.as_bytes();
    let looks_right = shape[..4].iter().all(u8::is_ascii_uppercase)
        && shape[4] == b'-'
        && shape[5..].iter().all(u8::is_ascii_digit);
    looks_right.then(|| serial.to_owned())
}

fn into_error(source: &ps2_memcard::Error) -> Error {
    match source {
        ps2_memcard::Error::NotAMemoryCard => {
            Error::NotThisFormat { format: FORMAT, why: "no memory card magic".into() }
        }
        ps2_memcard::Error::TooShort(len) => {
            Error::WrongLength { format: FORMAT, expected: "a whole number of pages", found: *len }
        }
        other => Error::Corrupt { format: FORMAT, why: other.to_string() },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card_with_saves() -> Vec<u8> {
        let mut builder = CardBuilder::new(Capacity::Mb8);
        builder.add(Save {
            name: "BASLUS-20312".into(),
            dirent: vec![0u8; ps2_memcard::ENTRY],
            files: vec![
                File { name: "icon.sys".into(), dirent: vec![0u8; ps2_memcard::ENTRY], data: vec![1u8; 964] },
                File {
                    name: "list.ico".into(),
                    dirent: vec![0u8; ps2_memcard::ENTRY],
                    data: vec![2u8; 15168],
                },
                File {
                    name: "BASLUS-20312".into(),
                    dirent: vec![0u8; ps2_memcard::ENTRY],
                    data: vec![3u8; 40960],
                },
            ],
        });
        builder.build().expect("builds")
    }

    #[test]
    fn a_save_is_a_directory_so_its_files_are_the_nested_parts() {
        // The worked example from the Memory Cards specification.
        let bundle = read(&card_with_saves(), &CardOptions::default()).expect("reads");

        assert_eq!(bundle.header.system.as_ref().unwrap().as_str(), "ps2");
        let card = bundle.header.card.as_ref().unwrap();
        assert_eq!(card.format.as_str(), "ps2-mc");
        // The data capacity, not the 8650752 a hardware dump of the same card runs to.
        assert_eq!(card.capacity, 8_388_608);

        assert_eq!(bundle.parts.len(), 1);
        let outer = &bundle.parts[0];
        assert_eq!(outer.path.as_deref(), Some("BASLUS-20312"));
        assert_eq!(outer.game.as_ref().unwrap().serial.as_deref(), Some("SLUS-20312"));
        // The directory's own entry rides on the outer part.
        assert_eq!(outer.dirent.as_ref().map(Vec::len), Some(512));

        let inner = Bundle::from_slice(&outer.bytes().unwrap()).expect("inner bundle");
        assert_eq!(inner.parts.len(), 3);
        assert_eq!(inner.parts[0].path.as_deref(), Some("icon.sys"));
        assert_eq!(inner.parts[2].payload.len(), 40960);
        // Each file's entry is on its own inner part.
        assert_eq!(inner.parts[1].dirent.as_ref().map(Vec::len), Some(512));

        bundle.validate().expect("a card is a valid bundle");
    }

    #[test]
    fn a_card_round_trips_through_a_bundle() {
        let bundle = read(&card_with_saves(), &CardOptions::default()).expect("reads");
        let rebuilt = write(&bundle).expect("writes");

        // Not a byte-for-byte copy — a writer regenerates the allocation table and the tree — so
        // the check is that the saves survive, which is what the representation promises.
        let again = read(&rebuilt, &CardOptions::default()).expect("reads what it wrote");
        assert_eq!(again.parts.len(), bundle.parts.len());
        let before = Bundle::from_slice(&bundle.parts[0].bytes().unwrap()).unwrap();
        let after = Bundle::from_slice(&again.parts[0].bytes().unwrap()).unwrap();
        for (a, b) in before.parts.iter().zip(&after.parts) {
            assert_eq!((&a.path, a.bytes().unwrap()), (&b.path, b.bytes().unwrap()));
        }
    }

    #[test]
    fn a_hardware_dump_reads_the_same_as_a_plain_image() {
        let bundle = read(&card_with_saves(), &CardOptions::default()).expect("reads");
        let dump = write_with_spare(&bundle).expect("writes a dump");
        assert_eq!(dump.len(), 8_650_752, "a dump is longer than the card's capacity");

        let from_dump = read(&dump, &CardOptions::default()).expect("reads the dump");
        assert_eq!(from_dump.header.card.as_ref().unwrap().capacity, 8_388_608);
        assert_eq!(from_dump.parts.len(), bundle.parts.len());
    }

    #[test]
    fn reads_a_serial_only_out_of_a_name_shaped_like_one() {
        assert_eq!(serial_from_name("BASLUS-20312").as_deref(), Some("SLUS-20312"));
        assert_eq!(serial_from_name("BESLES-50213").as_deref(), Some("SLES-50213"));
        assert_eq!(serial_from_name("SHORT"), None);
    }
}
