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
use crate::card::{
    card_header, card_image_part, dirent_key, image_only, modified_only, nested_saves, save_part_with,
};
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

/// Where the entry points at the pictures, and what shape it says they are.
///
/// `image_offset` is where the banner and the icon frames sit inside the save's own payload;
/// `banner_flags` and `icon_format` say which of the two pixel formats each is in, two bits per
/// picture; `anim_speed` says how long each frame shows, again two bits each.
#[cfg(feature = "icon")]
mod image {
    /// The entry's `banner_flags`, whose low two bits are the banner's format.
    pub(super) const BANNER_FLAGS: usize = 0x07;
    /// Where the pictures start, as an offset into the save's payload.
    pub(super) const IMAGE_OFFSET: usize = 0x2c;
    /// Two bits per icon frame.
    pub(super) const ICON_FORMAT: usize = 0x30;
    /// Two bits per icon frame, in twelfths of a second.
    pub(super) const ANIM_SPEED: usize = 0x32;

    /// A banner is 96x32 and an icon frame 32x32, both fixed by the console.
    pub(super) const BANNER: (usize, usize) = (96, 32);
    pub(super) const ICON: (usize, usize) = (32, 32);

    /// Two bits saying how a picture is stored: a palette index, a colour, or nothing at all,
    /// which is every other value and so needs no name.
    pub(super) const CI8: u16 = 1;
    pub(super) const RGB5A3: u16 = 2;
    /// The number of entries in the palette a CI8 picture indexes into.
    pub(super) const PALETTE: usize = 256;
}

/// Decodes the pictures a GameCube save carries, as `x.1sav.icon` wants them.
///
/// The frames come out at 32x32 and the banner at 96x32, in the console's own order and colours.
/// A CI8 picture indexes a 256-entry RGB5A3 palette that follows the last frame using it; an
/// RGB5A3 picture carries its colours inline. Both are tiled, and the geometry follows the pixel
/// size: 4x4 for two bytes a pixel, 8x4 for one.
#[cfg(feature = "icon")]
fn pictures(dirent: &[u8], data: &[u8]) -> Option<one_saves::dcbor::CBOR> {
    use crate::icon::{Frame, Rgba, be16, rgb5a3, value};
    use image as gc;

    let at = usize::try_from(u32::from_be_bytes(
        dirent.get(gc::IMAGE_OFFSET..gc::IMAGE_OFFSET + 4)?.try_into().ok()?,
    ))
    .ok()?;
    let formats = u16::from_be_bytes(dirent.get(gc::ICON_FORMAT..gc::ICON_FORMAT + 2)?.try_into().ok()?);
    let speeds = u16::from_be_bytes(dirent.get(gc::ANIM_SPEED..gc::ANIM_SPEED + 2)?.try_into().ok()?);

    // The banner comes first where there is one, and the frames follow it.
    let mut cursor = at;
    let banner = match u16::from(*dirent.get(gc::BANNER_FLAGS)?) & 3 {
        gc::RGB5A3 => {
            let (w, h) = gc::BANNER;
            let bytes = data.get(cursor..cursor + w * h * 2)?;
            cursor += w * h * 2;
            Rgba::from_tiles(w, h, (4, 4), |i| rgb5a3(be16(bytes, i))).to_png()
        }
        gc::CI8 => {
            let (w, h) = gc::BANNER;
            let bytes = data.get(cursor..cursor + w * h)?.to_vec();
            cursor += w * h;
            let palette = data.get(cursor..cursor + gc::PALETTE * 2)?;
            cursor += gc::PALETTE * 2;
            Rgba::from_tiles(w, h, (8, 4), |i| rgb5a3(be16(palette, usize::from(bytes[i])))).to_png()
        }
        _ => None,
    };

    // Each frame reads its own two bits. A frame set to nothing ends the animation.
    let (w, h) = gc::ICON;
    let mut frames = Vec::new();
    for slot in 0..8 {
        // Twelfths of a second, which is the unit the console counts an icon's hold in.
        let hold = u64::from((speeds >> (2 * slot)) & 3) * 1000 / 12;
        match (formats >> (2 * slot)) & 3 {
            gc::RGB5A3 => {
                let bytes = data.get(cursor..cursor + w * h * 2)?;
                cursor += w * h * 2;
                let png = Rgba::from_tiles(w, h, (4, 4), |i| rgb5a3(be16(bytes, i))).to_png()?;
                frames.push(Frame { png, hold_ms: (hold > 0).then_some(hold) });
            }
            gc::CI8 => {
                let bytes = data.get(cursor..cursor + w * h)?.to_vec();
                cursor += w * h;
                // The palette follows the run of frames that share it, so it is read from where
                // the last of them ended rather than from after this one.
                let palette = data.get(cursor..cursor + gc::PALETTE * 2)?;
                let png = Rgba::from_tiles(w, h, (8, 4), |i| rgb5a3(be16(palette, usize::from(bytes[i]))))
                    .to_png()?;
                frames.push(Frame { png, hold_ms: (hold > 0).then_some(hold) });
            }
            _ => break,
        }
    }
    value(frames, banner)
}

/// Where a GameCube directory entry keeps the time the console last wrote the save.
///
/// Four big-endian bytes at 0x28, counting seconds from 2000-01-01. The offset is the one field
/// the entry's 64 bytes leave unclaimed by everything else that reads them — the specification's
/// own `x.1sav.dirent.gc-mc` schema enumerates 0x00, 0x04, 0x07, 0x08, 0x2c, 0x30, 0x32, 0x34,
/// 0x35 and 0x3c, and `gc-memcard` takes 0x36 and 0x38 — and it was confirmed against cards
/// Dolphin wrote, whose decoded times matched the host clock to the second.
const MODIFIED_AT: usize = 0x28;

/// 2000-01-01T00:00:00 as a Unix second, which is what the entry counts from.
const GC_EPOCH: i64 = 946_684_800;

/// The entry's modify time, as a wall-clock reading on the Unix scale.
///
/// A GameCube has no notion of a zone: the IPL sets a date and a time of day and nothing else, so
/// what comes back fixes when the console thought it was and not which instant that was. That is
/// exactly the reading `x.1sav.dirent` asks for, and it is why no offset rides beside it.
fn modified_at(dirent: &[u8]) -> Option<i64> {
    let field = dirent.get(MODIFIED_AT..MODIFIED_AT + 4)?;
    let seconds = u32::from_be_bytes(field.try_into().expect("four bytes"));
    // A zero is an entry that never had the field written rather than midnight on new year 2000.
    (seconds != 0).then(|| GC_EPOCH + i64::from(seconds))
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

        // The picture travels with the save, so it goes in the header of the bundle that *is* the
        // save rather than on the card's part naming it.
        #[allow(unused_mut)]
        let mut inner = one_saves::Extensions::new();
        #[cfg(feature = "icon")]
        if let Some(icon) = pictures(&save.dirent, &save.data) {
            inner.insert(crate::icon::icon_key(), icon);
        }
        let mut part = save_part_with(Format::GcCard, parts.len(), save.data.clone(), game, options, inner)?;
        part.path = Some(save.filename.clone());
        part.slot = Some(u64::try_from(save.slot).expect("slot fits"));
        part.dirent = Some(save.dirent.clone());
        // The directory's own record of the save, which is the card's rather than the save's, so
        // it rides on the part beside the raw entry rather than in the nested bundle's header.
        if let Some(seconds) = modified_at(&save.dirent) {
            part.extensions.insert(dirent_key(), modified_only(seconds, None));
        }
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

    #[test]
    fn a_save_whose_entry_is_dated_carries_the_reading_on_its_part() {
        // The decoder is checked below; this is the wiring, which the other cards here cannot
        // reach because a builder-written entry has no time in it and `modified_at` then reports
        // absence rather than midnight in 2000.
        let mut dirent = vec![0u8; 64];
        dirent[..6].copy_from_slice(b"GALE01");
        dirent[6] = 0xFF;
        dirent[8..22].copy_from_slice(b"SuperSmashBros");
        // What a real card held, to the second.
        dirent[MODIFIED_AT..MODIFIED_AT + 4].copy_from_slice(&842_979_996u32.to_be_bytes());

        let mut builder = CardBuilder::new();
        builder.capacity((64 - gc_memcard::RESERVED_BLOCKS) * gc_memcard::BLOCK);
        builder.add(Save {
            slot: 0,
            game_code: "GALE".into(),
            maker_code: "01".into(),
            filename: "SuperSmashBros".into(),
            dirent,
            data: vec![7u8; gc_memcard::BLOCK],
        });
        let bundle = read(&builder.build().expect("builds"), &CardOptions::default()).expect("reads");

        let value = bundle.parts[0]
            .extensions
            .get(&dirent_key())
            .expect("a dated entry puts its reading on the part");
        let map = value.as_map().expect("a dirent map");
        assert!(map.get::<u64, one_saves::dcbor::CBOR>(0).is_none(), "a GameCube dates no creation");
        let arm = map.get::<u64, one_saves::dcbor::CBOR>(1).expect("the write time");
        let pair = arm.as_array().expect("a reading, and an offset only where one is known");
        assert_eq!(pair.len(), 1, "a GameCube reading is unqualified: the console has no zone");

        let one_saves::dcbor::CBORCase::Tagged(_, seconds) = pair[0].as_case() else {
            panic!("a reading is a tagged epoch")
        };
        assert_eq!(i64::try_from(seconds.clone()).expect("seconds"), 1_789_664_796);

        // And a save the card never dated puts no key on its part at all.
        let plain =
            read(&card_with(64, &[("GAFE01", "sunshine", 1)]), &CardOptions::default()).expect("reads");
        assert!(!plain.parts[0].extensions.contains_key(&dirent_key()));
    }

    #[test]
    fn the_entry_time_is_the_wall_clock_the_console_showed() {
        // Both values came off real cards: the raw field, and the reading it decodes to. A
        // GameCube counts from 2000-01-01, so the conversion is the epoch and nothing else.
        for (raw, reading) in [
            // Super Smash Bros. Melee, whose card said 2026-09-17 17:06:36.
            (842_979_996u32, 1_789_664_796i64),
            // F-Zero GX, 2026-08-20 14:51:35.
            (840_552_695, 1_787_237_495),
        ] {
            let mut dirent = vec![0u8; 64];
            dirent[MODIFIED_AT..MODIFIED_AT + 4].copy_from_slice(&raw.to_be_bytes());
            assert_eq!(modified_at(&dirent), Some(reading), "{raw}");
        }

        // Those readings are seven hours off the instant each card was actually written at, which
        // is the whole reason no offset rides beside them: the console's clock was set in a menu
        // with no notion of a zone, and nothing in the entry says which one it was.

        // A field nothing ever wrote is absence rather than midnight on new year 2000.
        assert_eq!(modified_at(&[0u8; 64]), None);
        // And an entry too short to hold the field has none to read.
        assert_eq!(modified_at(&[0u8; 8]), None);
    }
}
