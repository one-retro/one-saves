//! PlayStation memory cards.
//!
//! A card is 16 blocks of 8 KiB. Block 0 is the header block: 64 frames of 128 bytes, of which
//! frame 0 is the magic, frames 1 to 15 are the directory (one entry per data block), and the
//! rest is the broken-sector list and a write-test copy of frame 0. Blocks 1 to 15 hold the
//! saves, chained through a link field so one save can span several.
//!
//! A save's directory entry is its [`dirent`](one_saves::Part::dirent), verbatim and opaque; its
//! filename is the [`path`](one_saves::Part::path), which is what tells two saves of one game
//! apart; and the directory index it occupied is the [`slot`](one_saves::Part::slot).
//!
//! What this module does *not* record is where a save's blocks sat. No console addresses a save
//! by block and every tool that moves saves between cards throws that field away, so a writer
//! allocates fresh blocks at the destination.

use one_saves::{Bundle, Card, Game, Header, Part, PartKind, Slug};

use crate::CardOptions;
use crate::error::{Error, Result, corrupt};

/// What the format is called, for error messages.
const FORMAT: &str = "PS1 memory card";

/// The card format slug a bundle carries.
pub const CARD_FORMAT: &str = "ps1-mc";

/// The system slug these saves are for.
pub const SYSTEM: &str = "psx";

const FRAME: usize = 128;
const BLOCK: usize = 8192;
/// One header block plus fifteen data blocks.
const BLOCKS: usize = 16;
/// The data capacity: what saves can occupy, which for this format is the whole card.
pub const CAPACITY: usize = BLOCK * BLOCKS;
/// How many directory entries there are, one per data block.
const SLOTS: usize = 15;

/// A directory entry's state, in the first four bytes of the entry.
mod state {
    /// In use, and the first block of its save.
    pub const FIRST: u32 = 0x51;
    /// In use, and a middle block of its save.
    pub const MIDDLE: u32 = 0x52;
    /// In use, and the last block of its save.
    pub const LAST: u32 = 0x53;
    /// Free, on a formatted card.
    pub const FREE: u32 = 0xA0;
}

/// The link field's value on the last block of a chain.
const NO_NEXT: u16 = 0xFFFF;

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]])
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

/// The XOR checksum a frame carries in its last byte.
fn frame_checksum(frame: &[u8]) -> u8 {
    frame[..FRAME - 1].iter().fold(0u8, |sum, byte| sum ^ byte)
}

/// Whether these bytes look like a PS1 card.
#[must_use]
pub fn detect(bytes: &[u8]) -> bool {
    matches!(strip_container(bytes), Some(card) if &card[..2] == b"MC")
}

/// Finds the raw 128 KiB card inside whatever container it arrived in.
///
/// The same card ships under several extensions. `.mcr`, `.mcd`, `.bin` and `.srm` are the card
/// and nothing else; `.gme` (DexDrive) and `.vgs` (VGS/Connectix) put a header in front of it;
/// `.vmp` (PSP) puts a 128-byte header in front. All of them are the same 131072 bytes once the
/// wrapper is off, which is why they are one reader rather than five.
fn strip_container(bytes: &[u8]) -> Option<&[u8]> {
    // A bare card, the common case.
    if bytes.len() == CAPACITY {
        return Some(bytes);
    }
    // DexDrive: 3904-byte header beginning "123-456-STD".
    if bytes.len() == CAPACITY + 3904 && bytes.starts_with(b"123-456-STD") {
        return Some(&bytes[3904..]);
    }
    // VGS: 64-byte header beginning "VgsM".
    if bytes.len() == CAPACITY + 64 && bytes.starts_with(b"VgsM") {
        return Some(&bytes[64..]);
    }
    // PSP virtual card: 128-byte header, magic 0x00 "PMV".
    if bytes.len() == CAPACITY + 128 && bytes[1..4] == *b"PMV" {
        return Some(&bytes[128..]);
    }
    None
}

/// One directory entry, as far as this format cares.
struct Entry {
    /// The directory index, 1 to 15, which becomes the part's `slot`.
    slot: usize,
    /// The entry's 128 bytes, verbatim.
    dirent: Vec<u8>,
    /// The filename, which becomes the part's `path`.
    filename: String,
    /// Every data block this save occupies, in chain order.
    chain: Vec<usize>,
}

/// Reads a card into a bundle: one nested bundle per save.
pub fn read(bytes: &[u8], options: &CardOptions) -> Result<Bundle> {
    let card = strip_container(bytes).ok_or(Error::WrongLength {
        format: FORMAT,
        expected: "131072 bytes, or that behind a DexDrive, VGS or PSP header",
        found: bytes.len(),
    })?;
    if &card[..2] != b"MC" {
        return Err(Error::NotThisFormat {
            format: FORMAT,
            why: format!("the header block opens {:02x?}, not \"MC\"", &card[..2]),
        });
    }

    let entries = directory(card)?;
    let mut parts = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let payload: Vec<u8> = entry
            .chain
            .iter()
            .flat_map(|&block| &card[block * BLOCK..(block + 1) * BLOCK])
            .copied()
            .collect();

        let game = serial_from_filename(&entry.filename)
            .map(|serial| Game { serial: Some(serial), ..Game::default() });

        // Nothing is inherited across the nesting boundary, so the inner bundle repeats the
        // system and game. That is exactly what lets a save be sliced out as a byte copy and
        // still say what it belongs to.
        let inner = Bundle {
            header: Header {
                system: Some(Slug::parse(SYSTEM).expect("valid slug")),
                game: game.clone(),
                ..Header::default()
            },
            parts: vec![Part::new(0, payload)],
        };
        let inner_bytes = inner.to_vec()?;

        let mut part = Part::new(u64::try_from(index).expect("part count fits"), inner_bytes);
        part.kind = PartKind::Bundle;
        part.role = Some(options.role.clone());
        part.path = Some(entry.filename.clone());
        part.slot = Some(u64::try_from(entry.slot).expect("slot fits"));
        part.dirent = Some(entry.dirent.clone());
        // The outer game and system are an index, so a consumer can list a card's contents from
        // the head region without stepping into payloads.
        part.game = game;
        part.system = Some(Slug::parse(SYSTEM).expect("valid slug"));
        parts.push(part);
    }

    if parts.is_empty() {
        parts.push(crate::card::card_image_part(0, card, &options.role));
    }

    let bundle = Bundle {
        header: Header {
            system: Some(Slug::parse(SYSTEM).expect("valid slug")),
            card: Some(Card {
                format: Slug::parse(CARD_FORMAT).expect("valid slug"),
                capacity: CAPACITY as u64,
                system_area: system_area(card, &entries),
                unknown: one_saves::UnknownKeys::new(),
            }),
            source: options.source.clone(),
            ..Header::default()
        },
        parts,
    };
    Ok(bundle)
}

/// Walks the directory, following each used save's block chain.
fn directory(card: &[u8]) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for slot in 1..=SLOTS {
        let dirent = &card[slot * FRAME..(slot + 1) * FRAME];
        let state = u32_at(dirent, 0);
        // Only the first block of a save starts an entry; middle and last blocks are reached by
        // following the chain, and a deleted save's blocks belong to no save at all.
        if state != state::FIRST {
            continue;
        }
        let chain = follow_chain(card, slot)?;
        entries.push(Entry { slot, dirent: dirent.to_vec(), filename: filename(dirent), chain });
    }
    Ok(entries)
}

/// Follows a save's block chain from its first block.
fn follow_chain(card: &[u8], first: usize) -> Result<Vec<usize>> {
    let mut chain = vec![first];
    let mut current = first;
    loop {
        let dirent = &card[current * FRAME..(current + 1) * FRAME];
        let next = u16_at(dirent, 8);
        if next == NO_NEXT {
            return Ok(chain);
        }
        // The link is a zero-based index over the data blocks, so block 1 is written as 0.
        let block = usize::from(next) + 1;
        if block > SLOTS {
            return Err(corrupt!(FORMAT, "slot {current} links to block {block}, past the card"));
        }
        // A cycle would otherwise be an unbounded read, and a card with one is broken whatever
        // it was meant to say.
        if chain.contains(&block) {
            return Err(corrupt!(FORMAT, "the chain from slot {first} loops at block {block}"));
        }
        let state = u32_at(&card[block * FRAME..(block + 1) * FRAME], 0);
        if state != state::MIDDLE && state != state::LAST {
            return Err(corrupt!(
                FORMAT,
                "slot {current} links to block {block}, which is not a continuation (state {state:#04x})"
            ));
        }
        chain.push(block);
        current = block;
    }
}

/// The filename out of a directory entry: 20 bytes at 0x0A, NUL-terminated.
fn filename(dirent: &[u8]) -> String {
    let field = &dirent[0x0A..0x0A + 20];
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}

/// The product code out of a save's filename, when the filename carries one.
///
/// A PS1 save is named region code, then the disc's product code, then whatever the game adds:
/// `BASCUS-94163FF7-S01`. The middle ten characters are the serial.
///
/// Note this is the *save's* product code and not necessarily the disc that wrote it. All three
/// US discs of Final Fantasy VII carry the disc-1 code in the save name so every disc reads the
/// same saves.
fn serial_from_filename(name: &str) -> Option<String> {
    let bytes = name.as_bytes();
    if bytes.len() < 12 {
        return None;
    }
    let serial = &name[2..12];
    let shape = serial.as_bytes();
    let looks_right = shape[..4].iter().all(u8::is_ascii_uppercase)
        && shape[4] == b'-'
        && shape[5..].iter().all(u8::is_ascii_digit);
    looks_right.then(|| serial.to_owned())
}

/// The card-level bytes belonging to no save, when they carry anything a writer would not
/// regenerate.
///
/// A writer rebuilds the magic frame, the free directory entries, the broken-sector list and the
/// write-test frame from nothing, so a card whose header block holds only those needs no
/// `system_area` at all — which is why the specification's worked example omits it. A card that
/// differs anywhere else keeps its whole header block, since the bytes that differ are not ones
/// this format claims to understand.
fn system_area(card: &[u8], entries: &[Entry]) -> Option<Vec<u8>> {
    let header = &card[..BLOCK];
    let regenerated =
        build_header_block(entries, |slot| entries.iter().find(|e| e.slot == slot).map(|e| e.dirent.clone()));
    (header != regenerated.as_slice()).then(|| header.to_vec())
}

/// Builds a header block: the magic frame, a directory, and the trailing frames.
fn build_header_block(entries: &[Entry], dirent_for: impl Fn(usize) -> Option<Vec<u8>>) -> Vec<u8> {
    let mut block = vec![0u8; BLOCK];

    // Frame 0: the magic, with its XOR checksum.
    block[0] = b'M';
    block[1] = b'C';
    block[FRAME - 1] = frame_checksum(&block[..FRAME]);

    // Frames 1 to 15: the directory. A slot with no save is written as free.
    for slot in 1..=SLOTS {
        let frame = &mut block[slot * FRAME..(slot + 1) * FRAME];
        if let Some(dirent) = dirent_for(slot) {
            frame.copy_from_slice(&dirent);
        } else {
            {
                frame[..4].copy_from_slice(&state::FREE.to_le_bytes());
                frame[8..10].copy_from_slice(&NO_NEXT.to_le_bytes());
                frame[FRAME - 1] = frame_checksum(frame);
            }
        }
    }

    // Frames 16 to 35: the broken-sector list, every entry marked unused.
    for frame_index in 16..36 {
        let frame = &mut block[frame_index * FRAME..(frame_index + 1) * FRAME];
        frame[..4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        frame[8..10].copy_from_slice(&NO_NEXT.to_le_bytes());
        frame[FRAME - 1] = frame_checksum(frame);
    }

    // Frame 63: a copy of the magic frame, which the console uses as a write test.
    let (head, tail) = block.split_at_mut(63 * FRAME);
    tail[..FRAME].copy_from_slice(&head[..FRAME]);

    let _ = entries;
    block
}

/// Writes a bundle back out as a raw 128 KiB card.
///
/// Everything structural is regenerated: the block chains, the directory, every checksum and the
/// header block. A save's `dirent` plus its payload is sufficient input, which is what `.mcs`
/// demonstrates in production.
pub fn write(bundle: &Bundle) -> Result<Vec<u8>> {
    // A card with nothing on it carries a whole-card image rather than nested saves, and writing
    // it back is a byte copy.
    if let Some(image) = crate::card::image_only(bundle)? {
        return Ok(image);
    }
    let saves = crate::card::nested_saves(bundle)?;

    let mut card = vec![0u8; CAPACITY];
    let mut dirents: Vec<(usize, Vec<u8>)> = Vec::new();
    let mut next_block = 1usize;

    for save in &saves {
        let blocks = save.bytes.len().div_ceil(BLOCK);
        if blocks == 0 {
            return Err(Error::NotConvertible(format!("save {:?} is empty", save.path)));
        }
        if next_block + blocks - 1 > SLOTS {
            return Err(Error::NotConvertible(format!(
                "these saves need {} blocks, and a card has {SLOTS}",
                next_block - 1 + blocks
            )));
        }

        // The allocator packs saves from the front. A writer that places them differently from
        // where they were has not done anything wrong: block placement is an allocator's
        // business, which is why it was never recorded.
        let first = next_block;
        for (index, block) in (first..first + blocks).enumerate() {
            let start = block * BLOCK;
            let from = index * BLOCK;
            let to = ((index + 1) * BLOCK).min(save.bytes.len());
            card[start..start + (to - from)].copy_from_slice(&save.bytes[from..to]);

            // Each block gets a directory entry: the save's own on the first, a continuation on
            // the rest.
            let mut dirent = if index == 0 {
                save.dirent.clone().unwrap_or_else(|| default_dirent(save))
            } else {
                vec![0u8; FRAME]
            };
            dirent.resize(FRAME, 0);

            let state = match (index, blocks - 1) {
                (0, _) => state::FIRST,
                (i, last) if i == last => state::LAST,
                _ => state::MIDDLE,
            };
            dirent[..4].copy_from_slice(&state.to_le_bytes());
            // The size field counts every block the save occupies, on every entry of the chain.
            let size = u32::try_from(blocks * BLOCK).expect("a card's worth of bytes fits");
            dirent[4..8].copy_from_slice(&size.to_le_bytes());
            let link = if index + 1 == blocks {
                NO_NEXT
            } else {
                // The link is a zero-based index over the data blocks, so block 1 is written as 0.
                u16::try_from(block).expect("block index fits")
            };
            dirent[8..10].copy_from_slice(&link.to_le_bytes());
            if index > 0 {
                // A continuation entry carries no name; the name is on the first block.
                dirent[0x0A..0x0A + 20].fill(0);
            }
            dirent[FRAME - 1] = frame_checksum(&dirent);
            dirents.push((block, dirent));
        }
        next_block += blocks;
    }

    // A card that arrived with a system area keeps it, so whatever a writer does not understand
    // survives the round trip; otherwise the header block is built from nothing.
    let header = match bundle.header.card.as_ref().and_then(|card| card.system_area.as_ref()) {
        Some(area) if area.len() == BLOCK => {
            let mut header = area.clone();
            for slot in 1..=SLOTS {
                let frame = &mut header[slot * FRAME..(slot + 1) * FRAME];
                if let Some((_, dirent)) = dirents.iter().find(|(block, _)| *block == slot) {
                    frame.copy_from_slice(dirent);
                } else {
                    {
                        frame.fill(0);
                        frame[..4].copy_from_slice(&state::FREE.to_le_bytes());
                        frame[8..10].copy_from_slice(&NO_NEXT.to_le_bytes());
                        frame[FRAME - 1] = frame_checksum(frame);
                    }
                }
            }
            header
        }
        _ => build_header_block(&[], |slot| {
            dirents.iter().find(|(block, _)| *block == slot).map(|(_, d)| d.clone())
        }),
    };
    card[..BLOCK].copy_from_slice(&header);
    Ok(card)
}

/// A directory entry for a save that arrived without one.
fn default_dirent(save: &crate::card::NestedSave) -> Vec<u8> {
    let mut dirent = vec![0u8; FRAME];
    if let Some(path) = &save.path {
        let name = path.as_bytes();
        let len = name.len().min(20);
        dirent[0x0A..0x0A + len].copy_from_slice(&name[..len]);
    }
    dirent
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a card with the given saves, as a console would have left it.
    fn card_with(saves: &[(&str, usize)]) -> Vec<u8> {
        let mut card = vec![0u8; CAPACITY];
        card[0] = b'M';
        card[1] = b'C';
        card[FRAME - 1] = frame_checksum(&card[..FRAME]);

        for slot in 1..=SLOTS {
            let frame = &mut card[slot * FRAME..(slot + 1) * FRAME];
            frame[..4].copy_from_slice(&state::FREE.to_le_bytes());
            frame[8..10].copy_from_slice(&NO_NEXT.to_le_bytes());
            frame[FRAME - 1] = frame_checksum(frame);
        }
        for frame_index in 16..36 {
            let frame = &mut card[frame_index * FRAME..(frame_index + 1) * FRAME];
            frame[..4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
            frame[8..10].copy_from_slice(&NO_NEXT.to_le_bytes());
            frame[FRAME - 1] = frame_checksum(frame);
        }
        let (head, tail) = card.split_at_mut(63 * FRAME);
        tail[..FRAME].copy_from_slice(&head[..FRAME]);

        let mut block = 1usize;
        for (name, blocks) in saves {
            for index in 0..*blocks {
                let current = block + index;
                // Patterned payload, so an off-by-one that copied the wrong block shows up.
                for byte in 0..BLOCK {
                    card[current * BLOCK + byte] =
                        u8::try_from((byte + current * 7) & 0xff).expect("masked to a byte");
                }
                let frame = &mut card[current * FRAME..(current + 1) * FRAME];
                let state = match (index, blocks - 1) {
                    (0, _) => state::FIRST,
                    (i, last) if i == last => state::LAST,
                    _ => state::MIDDLE,
                };
                frame[..4].copy_from_slice(&state.to_le_bytes());
                frame[4..8].copy_from_slice(
                    &u32::try_from(blocks * BLOCK).expect("a card's worth fits").to_le_bytes(),
                );
                let link =
                    if index + 1 == *blocks { NO_NEXT } else { u16::try_from(current).expect("block fits") };
                frame[8..10].copy_from_slice(&link.to_le_bytes());
                if index == 0 {
                    frame[0x0A..0x0A + name.len()].copy_from_slice(name.as_bytes());
                }
                frame[FRAME - 1] = frame_checksum(frame);
            }
            block += blocks;
        }
        card
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
        // A freshly formatted header block is entirely regenerable, so there is nothing to keep.
        assert_eq!(card_map.system_area, None);
        // The card holds three saves and no header game, because none of them describes the card.
        assert_eq!(bundle.parts.len(), 3);
        assert!(bundle.header.game.is_none());

        assert_eq!(bundle.parts[0].path.as_deref(), Some("BASCUS-94163FF7-S01"));
        assert_eq!(bundle.parts[0].slot, Some(1));
        assert_eq!(bundle.parts[0].dirent.as_ref().map(Vec::len), Some(128));
        assert_eq!(bundle.parts[2].game.as_ref().unwrap().serial.as_deref(), Some("SLUS-00594"));

        // The two Final Fantasy VII entries carry byte-identical game maps and are told apart
        // only by path and slot.
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
        assert_eq!(rebuilt.len(), CAPACITY);
        assert_eq!(rebuilt, card, "a card a writer regenerates should match what it read");
    }

    #[test]
    fn a_formatted_card_with_nothing_on_it_becomes_a_card_image() {
        // A real card with all fifteen slots free. It is still a card and worth carrying, and a
        // whole-card image is the only part it can have, since there is no save to nest.
        let card = card_with(&[]);
        let bundle = read(&card, &CardOptions::default()).expect("an empty card is still a card");

        assert_eq!(bundle.parts.len(), 1);
        assert_eq!(bundle.parts[0].kind, one_saves::PartKind::CardImage);
        assert_eq!(bundle.parts[0].payload.len(), CAPACITY as u64);
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
        bundle.parts.push(crate::card::card_image_part(1, &card, &role));

        // Still rebuilt from the nested save, not short-circuited to the image.
        assert_eq!(crate::card::image_only(&bundle).unwrap(), None);
        assert_eq!(write(&bundle).expect("writes"), card);
    }

    #[test]
    fn strips_the_containers_the_same_card_ships_in() {
        let card = card_with(&[("BASLUS-00594", 1)]);

        let mut dexdrive = vec![0u8; 3904];
        dexdrive[..11].copy_from_slice(b"123-456-STD");
        dexdrive.extend_from_slice(&card);
        assert_eq!(strip_container(&dexdrive), Some(&card[..]));

        let mut vgs = vec![0u8; 64];
        vgs[..4].copy_from_slice(b"VgsM");
        vgs.extend_from_slice(&card);
        assert_eq!(strip_container(&vgs), Some(&card[..]));

        let mut psp = vec![0u8; 128];
        psp[1..4].copy_from_slice(b"PMV");
        psp.extend_from_slice(&card);
        assert_eq!(strip_container(&psp), Some(&card[..]));

        assert!(detect(&dexdrive) && detect(&vgs) && detect(&psp) && detect(&card));
    }

    #[test]
    fn refuses_a_chain_that_loops() {
        let mut card = card_with(&[("BASLUS-00594", 2)]);
        // Point the second block back at the first, which no console would have written.
        card[2 * FRAME + 8..2 * FRAME + 10].copy_from_slice(&0u16.to_le_bytes());
        card[2 * FRAME + FRAME - 1] = frame_checksum(&card[2 * FRAME..3 * FRAME]);
        let err = read(&card, &CardOptions::default()).unwrap_err();
        assert!(matches!(err, Error::Corrupt { .. }), "{err}");
    }

    #[test]
    fn reads_a_serial_only_out_of_a_name_shaped_like_one() {
        assert_eq!(serial_from_filename("BASCUS-94163FF7-S01").as_deref(), Some("SCUS-94163"));
        assert_eq!(serial_from_filename("BASLUS-00594").as_deref(), Some("SLUS-00594"));
        assert_eq!(serial_from_filename("SHORT"), None);
        assert_eq!(serial_from_filename("not-a-serial-here"), None);
    }
}
