//! PlayStation memory cards.
//!
//! The filesystem itself lives in [`ps1_memcard`], which is a standalone crate because a PS1 card
//! is a genuinely reusable format and nothing about reading one needs this container. What is here
//! is the adapter: the mapping between a card's saves and a bundle's nested parts.
//!
//! A save's directory entry is its [`dirent`](one_saves::Part::dirent), verbatim and opaque; its
//! filename is the [`path`](one_saves::Part::path), which is what tells two saves of one game
//! apart; and the directory index it occupied is the [`slot`](one_saves::Part::slot).
//!
//! What the bundle does *not* record is where a save's blocks sat. No console addresses a save by
//! block and every tool that moves saves between cards throws that field away, so a writer
//! allocates fresh blocks at the destination.

use one_saves::{Bundle, Game};
use ps1_memcard::{CardBuilder, MemoryCard, Save};

use crate::CardOptions;
use crate::card::{card_header, card_image_part, image_only, nested_saves, save_part_with};
use crate::detect::Format;
use crate::error::{Error, Result};
use crate::label::{label_key, shift_jis_field, value as label};

/// What the format is called, for error messages.
const FORMAT: &str = Format::Ps1Card.label();

/// The card format slug a bundle carries.
pub const CARD_FORMAT: &str = match Format::Ps1Card.card_format() {
    Some(slug) => slug,
    None => panic!("a PS1 card is a card format"),
};

/// The system slug these saves are for.
pub const SYSTEM: &str = match Format::Ps1Card.system() {
    Some(slug) => slug,
    None => panic!("a PS1 card holds saves for a system"),
};

/// Whether these bytes look like a PS1 card, in any of its containers.
#[must_use]
pub fn detect(bytes: &[u8]) -> bool {
    ps1_memcard::detect(bytes)
}

/// Where a PS1 save keeps what the console shows for it, all of it in the save's first block.
mod header {
    /// What a save block starts with, so a block that is not one is not read as one.
    pub(super) const MAGIC: &[u8] = b"SC";
    /// How many icon frames the save carries, in the low nibble.
    pub(super) const FRAMES: usize = 0x02;
    /// The title, in Shift-JIS, padded to its full width.
    pub(super) const TITLE: usize = 0x04;
    pub(super) const TITLE_LEN: usize = 64;
    /// Sixteen colours, each a little-endian BGR555.
    pub(super) const CLUT: usize = 0x60;
    /// Where the frames start, each 16x16 at four bits a pixel.
    pub(super) const ICON: usize = 0x80;
    pub(super) const FRAME_LEN: usize = 128;
    pub(super) const SIDE: usize = 16;
    /// The most a save may carry, which is what the frame count is masked against.
    pub(super) const MAX_FRAMES: u8 = 3;
}

/// The title the console lists a save under.
fn title_of(data: &[u8]) -> Option<one_saves::dcbor::CBOR> {
    if !data.starts_with(header::MAGIC) {
        return None;
    }
    label(shift_jis_field(data.get(header::TITLE..header::TITLE + header::TITLE_LEN)?), None)
}

/// The icon a PS1 save carries: up to three frames of 16x16, against sixteen colours.
///
/// Four bits a pixel, and the **high** nibble is the left one of each pair. That is not what a
/// reader of the PS1's texture formats would assume, and reading it the other way round gives a
/// picture that still looks like something: every pair of pixels is swapped, which combs each
/// solid shape into stripes a pixel wide. The order here is the one that draws the Gran Turismo
/// card under `data/saves` as solid shapes rather than as a comb.
#[cfg(feature = "icon")]
fn pictures(data: &[u8]) -> Option<one_saves::dcbor::CBOR> {
    use crate::icon::{Frame, Rgba, value};

    if !data.starts_with(header::MAGIC) {
        return None;
    }
    let count = (data.get(header::FRAMES)? & 0x0f).min(header::MAX_FRAMES);
    let clut = data.get(header::CLUT..header::CLUT + 32)?;

    let mut frames = Vec::new();
    for index in 0..usize::from(count) {
        let at = header::ICON + index * header::FRAME_LEN;
        let bytes = data.get(at..at + header::FRAME_LEN)?;
        let side = header::SIDE;
        // Not tiled: a PS1 icon is scanlines, so the tile is the whole row.
        let rgba = Rgba::from_tiles(side, side, (side, 1), |pixel| {
            let byte = bytes[pixel / 2];
            let entry = if pixel % 2 == 0 { byte >> 4 } else { byte & 0x0f };
            bgr555(u16::from_le_bytes([clut[usize::from(entry) * 2], clut[usize::from(entry) * 2 + 1]]))
        });
        // A PS1 icon holds no timing of its own; the console runs the frames at its own rate.
        frames.push(Frame { png: rgba.to_png()?, hold_ms: None });
    }
    value(frames, None)
}

/// One BGR555 colour, in which an all-zero entry is transparent rather than black.
#[cfg(feature = "icon")]
fn bgr555(value: u16) -> [u8; 4] {
    if value == 0 {
        return [0, 0, 0, 0];
    }
    let scale = |channel: u16| u8::try_from(u32::from(channel) * 255 / 31).expect("a byte");
    [scale(value & 31), scale((value >> 5) & 31), scale((value >> 10) & 31), 255]
}

/// Reads a card into a bundle: one nested bundle per save.
pub fn read(bytes: &[u8], options: &CardOptions) -> Result<Bundle> {
    let card = MemoryCard::parse(bytes).map_err(into_error)?;

    let mut parts = Vec::new();
    for save in card.saves() {
        let game = ps1_memcard::serial_from_filename(&save.name)
            .map(|serial| Game { serial: Some(serial.to_owned()), ..Game::default() });

        // The title and the picture are both in the save's first block, so both travel with it.
        let mut inner = one_saves::Extensions::new();
        if let Some(title) = title_of(&save.data) {
            inner.insert(label_key(), title);
        }
        #[cfg(feature = "icon")]
        if let Some(icon) = pictures(&save.data) {
            inner.insert(crate::icon::icon_key(), icon);
        }
        let mut part = save_part_with(Format::Ps1Card, parts.len(), save.data.clone(), game, options, inner)?;
        part.path = Some(save.name.clone());
        part.slot = Some(u64::try_from(save.slot).expect("slot fits"));
        part.dirent = Some(save.dirent.clone());
        parts.push(part);
    }

    if parts.is_empty() {
        parts.push(card_image_part(0, card.image(), &options.role_for(Format::Ps1Card)));
    }

    Ok(Bundle {
        header: card_header(
            Format::Ps1Card,
            card.capacity(),
            card.system_area().map(<[u8]>::to_vec),
            options,
        ),
        parts,
    })
}

/// Writes a bundle back out as a raw 128 KiB card.
///
/// Everything structural is regenerated: the block chains, the directory, every checksum and the
/// header block.
pub fn write(bundle: &Bundle) -> Result<Vec<u8>> {
    // A card with nothing on it carries a whole-card image rather than nested saves, and writing
    // it back is a byte copy.
    if let Some(image) = image_only(bundle)? {
        return Ok(image);
    }

    let mut builder = CardBuilder::new();
    if let Some(area) = bundle.header.card.as_ref().and_then(|card| card.system_area.as_ref()) {
        builder.system_area(area.clone());
    }
    for save in nested_saves(bundle)? {
        builder.add(Save {
            slot: 0,
            name: save.path.unwrap_or_default(),
            dirent: save.dirent.unwrap_or_default(),
            data: save.bytes,
        });
    }
    builder.build().map_err(into_error)
}

fn into_error(source: ps1_memcard::Error) -> Error {
    match source {
        ps1_memcard::Error::WrongLength(found) => Error::WrongLength {
            format: FORMAT,
            expected: "131072 bytes, or that behind a DexDrive, VGS or PSP header",
            found,
        },
        ps1_memcard::Error::NotAMemoryCard => {
            Error::NotThisFormat { format: FORMAT, why: "the header block does not open with \"MC\"".into() }
        }
        ps1_memcard::Error::Corrupt(why) => Error::Corrupt { format: FORMAT, why },
        other => Error::NotConvertible(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use one_saves::PartKind;

    /// Builds a card holding the given saves, each of the given block count.
    fn card_with(saves: &[(&str, usize)]) -> Vec<u8> {
        let mut builder = CardBuilder::new();
        for (name, blocks) in saves {
            builder.add(Save {
                slot: 0,
                name: (*name).to_owned(),
                dirent: Vec::new(),
                // Patterned payload, so an off-by-one that copied the wrong block shows up.
                data: (0..blocks * ps1_memcard::BLOCK)
                    .map(|byte| u8::try_from(byte & 0xff).expect("masked to a byte"))
                    .collect(),
            });
        }
        builder.build().expect("builds")
    }

    #[test]
    fn reads_a_card_of_three_saves_two_for_one_game() {
        // The worked example from the Memory Cards specification.
        let card = card_with(&[("BASCUS-94163FF7-S01", 1), ("BASCUS-94163FF7-S03", 1), ("BASLUS-00594", 1)]);
        let bundle = read(&card, &CardOptions::default()).expect("reads");

        assert_eq!(bundle.header.system.as_ref().unwrap().as_str(), "psx");
        let card_map = bundle.header.card.as_ref().expect("is a card");
        assert_eq!(card_map.format.as_str(), "ps1-mc");
        assert_eq!(card_map.capacity, 131_072);
        // The card holds three saves and no header game, because none of them describes the card.
        assert_eq!(bundle.parts.len(), 3);
        assert!(bundle.header.game.is_none());

        assert_eq!(bundle.parts[0].path.as_deref(), Some("BASCUS-94163FF7-S01"));
        assert_eq!(bundle.parts[0].slot, Some(1));
        assert_eq!(bundle.parts[0].dirent.as_ref().map(Vec::len), Some(128));
        assert_eq!(bundle.parts[2].game.as_ref().unwrap().serial.as_deref(), Some("SLUS-00594"));

        // The two Final Fantasy VII entries carry byte-identical game maps and are told apart only
        // by path and slot.
        assert_eq!(bundle.parts[0].game, bundle.parts[1].game);
        assert_ne!(bundle.parts[0].path, bundle.parts[1].path);
        bundle.validate().expect("a card is a valid bundle");
    }

    #[test]
    fn a_save_spanning_two_blocks_is_one_part_of_both() {
        let card = card_with(&[("BASLUS-00594", 2)]);
        let bundle = read(&card, &CardOptions::default()).expect("reads");
        assert_eq!(bundle.parts.len(), 1);

        let inner = Bundle::from_slice(&bundle.parts[0].bytes().unwrap()).expect("inner bundle");
        // Every block the save occupied, and the fact that they were not adjacent is not recorded.
        assert_eq!(inner.parts[0].payload.len(), 16384);
    }

    #[test]
    fn a_card_round_trips_through_a_bundle() {
        let card = card_with(&[("BASCUS-94163FF7-S01", 1), ("BASLUS-00594", 2)]);
        let bundle = read(&card, &CardOptions::default()).expect("reads");
        let rebuilt = write(&bundle).expect("writes");
        assert_eq!(rebuilt.len(), ps1_memcard::CAPACITY);
        assert_eq!(rebuilt, card, "a card a writer regenerates should match what it read");
    }

    #[test]
    fn a_formatted_card_with_nothing_on_it_becomes_a_card_image() {
        // A real card with all fifteen slots free. It is still a card and worth carrying, and a
        // whole-card image is the only part it can have, since there is no save to nest.
        let card = card_with(&[]);
        let bundle = read(&card, &CardOptions::default()).expect("an empty card is still a card");

        assert_eq!(bundle.parts.len(), 1);
        assert_eq!(bundle.parts[0].kind, PartKind::CardImage);
        assert_eq!(bundle.parts[0].payload.len(), ps1_memcard::CAPACITY as u64);
        assert_eq!(bundle.header.card.as_ref().unwrap().format.as_str(), "ps1-mc");
        bundle.validate().expect("valid bundle");

        // Writing it back is a byte copy rather than a rebuild.
        assert_eq!(write(&bundle).expect("writes"), card);
    }

    #[test]
    fn an_image_kept_beside_the_splits_is_not_what_a_writer_rebuilds_from() {
        // The spec allows both: the image as a byte-exact archive, the splits for portability.
        // The splits are the source; the image is the archive.
        let card = card_with(&[("BASLUS-00594", 1)]);
        let mut bundle = read(&card, &CardOptions::default()).expect("reads");
        let role = bundle.parts[0].role.clone().unwrap();
        bundle.parts.push(card_image_part(1, &card, &role));

        // Still rebuilt from the nested save, not short-circuited to the image.
        assert_eq!(image_only(&bundle).unwrap(), None);
        assert_eq!(write(&bundle).expect("writes"), card);
    }

    #[test]
    fn a_container_reads_the_same_as_the_bare_card() {
        let card = card_with(&[("BASLUS-00594", 1)]);
        let mut vgs = vec![0u8; 64];
        vgs[..4].copy_from_slice(b"VgsM");
        vgs.extend_from_slice(&card);

        let bundle = read(&vgs, &CardOptions::default()).expect("reads");
        assert_eq!(bundle.parts[0].path.as_deref(), Some("BASLUS-00594"));
        // The wrapper is not part of the card, so writing back gives the bare bytes.
        assert_eq!(write(&bundle).expect("writes"), card);
    }

    #[test]
    fn something_that_is_not_a_card_is_refused_in_this_format_s_words() {
        assert!(matches!(
            read(&[1, 2, 3], &CardOptions::default()),
            Err(Error::WrongLength { format: FORMAT, .. })
        ));
        assert!(matches!(
            read(&vec![0u8; ps1_memcard::CAPACITY], &CardOptions::default()),
            Err(Error::NotThisFormat { .. })
        ));
    }
}
