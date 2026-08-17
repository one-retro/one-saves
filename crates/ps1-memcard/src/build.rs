//! Writing a card image out of a set of saves.

use crate::{BLOCK, CAPACITY, Error, FRAME, MAGIC, NO_NEXT, Result, SLOTS, Save, frame_checksum, state};

/// Builds a card image from saves.
///
/// Everything structural is regenerated: the block chains, the directory and every frame checksum.
/// A save's `dirent` plus its bytes is sufficient input, which is what the `.mcs` single-save
/// format demonstrates in production.
#[derive(Debug, Clone, Default)]
pub struct CardBuilder {
    saves: Vec<Save>,
    system_area: Option<Vec<u8>>,
}

impl CardBuilder {
    /// A builder for an empty card.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Keeps a header block read off another card, so whatever a writer does not understand
    /// survives the round trip.
    ///
    /// Only the directory frames are rewritten; everything else in the block is passed through. A
    /// block that is not [`BLOCK`] bytes is ignored, since it is not a header block.
    pub fn system_area(&mut self, bytes: impl Into<Vec<u8>>) -> &mut Self {
        self.system_area = Some(bytes.into());
        self
    }

    /// Adds a save. Its `slot` is ignored: a builder allocates its own blocks.
    pub fn add(&mut self, save: Save) -> &mut Self {
        self.saves.push(save);
        self
    }

    /// Writes the card out, all 128 KiB of it.
    pub fn build(&self) -> Result<Vec<u8>> {
        let mut card = vec![0u8; CAPACITY];
        let mut dirents: Vec<(usize, Vec<u8>)> = Vec::new();
        let mut next_block = 1usize;

        for save in &self.saves {
            let blocks = save.data.len().div_ceil(BLOCK);
            if blocks == 0 {
                return Err(Error::EmptySave(save.name.clone()));
            }
            if next_block + blocks - 1 > SLOTS {
                return Err(Error::Full { needed: next_block - 1 + blocks, available: SLOTS });
            }

            // The allocator packs saves from the front. A writer that places them differently from
            // where they were has not done anything wrong: block placement is an allocator's
            // business, which is why it was never recorded.
            let first = next_block;
            for (index, block) in (first..first + blocks).enumerate() {
                let start = block * BLOCK;
                let from = index * BLOCK;
                let to = ((index + 1) * BLOCK).min(save.data.len());
                card[start..start + (to - from)].copy_from_slice(&save.data[from..to]);

                // Each block gets a directory entry: the save's own on the first, a continuation
                // on the rest.
                let mut dirent = if index == 0 { first_dirent(save) } else { vec![0u8; FRAME] };
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
                    // The link is a zero-based index over the data blocks, so block 1 is written
                    // as 0.
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

        let lookup = |slot: usize| dirents.iter().find(|(block, _)| *block == slot).map(|(_, d)| d.clone());
        let header = match &self.system_area {
            Some(area) if area.len() == BLOCK => {
                let mut header = area.clone();
                for slot in 1..=SLOTS {
                    let frame = &mut header[slot * FRAME..(slot + 1) * FRAME];
                    if let Some(dirent) = lookup(slot) {
                        frame.copy_from_slice(&dirent);
                    } else {
                        free_frame(frame);
                    }
                }
                header
            }
            _ => header_block(lookup),
        };
        card[..BLOCK].copy_from_slice(&header);
        Ok(card)
    }
}

/// The directory entry a save's first block gets: its own, or one carrying just the name.
fn first_dirent(save: &Save) -> Vec<u8> {
    if !save.dirent.is_empty() {
        return save.dirent.clone();
    }
    let mut dirent = vec![0u8; FRAME];
    let name = save.name.as_bytes();
    let len = name.len().min(20);
    dirent[0x0A..0x0A + len].copy_from_slice(&name[..len]);
    dirent
}

/// Writes a free directory entry, as a formatted card carries in every empty slot.
fn free_frame(frame: &mut [u8]) {
    frame.fill(0);
    frame[..4].copy_from_slice(&state::FREE.to_le_bytes());
    frame[8..10].copy_from_slice(&NO_NEXT.to_le_bytes());
    frame[FRAME - 1] = frame_checksum(frame);
}

/// Builds a header block from nothing: the magic frame, a directory, and the trailing frames.
///
/// `dirent_for` supplies the entry for a used slot and `None` for a free one.
pub(crate) fn header_block(dirent_for: impl Fn(usize) -> Option<Vec<u8>>) -> Vec<u8> {
    let mut block = vec![0u8; BLOCK];

    // Frame 0: the magic, with its XOR checksum.
    block[..MAGIC.len()].copy_from_slice(MAGIC);
    block[FRAME - 1] = frame_checksum(&block[..FRAME]);

    // Frames 1 to 15: the directory. A slot with no save is written as free.
    for slot in 1..=SLOTS {
        let frame = &mut block[slot * FRAME..(slot + 1) * FRAME];
        if let Some(dirent) = dirent_for(slot) {
            frame.copy_from_slice(&dirent);
        } else {
            free_frame(frame);
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
    block
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MemoryCard, tests::card_with};

    /// Reads a card and writes it straight back, the way a converter does.
    fn round_trip(card: &[u8]) -> Vec<u8> {
        let parsed = MemoryCard::parse(card).expect("reads");
        let mut builder = CardBuilder::new();
        if let Some(area) = parsed.system_area() {
            builder.system_area(area.to_vec());
        }
        for save in parsed.saves() {
            builder.add(save.clone());
        }
        builder.build().expect("writes")
    }

    #[test]
    fn a_card_a_console_wrote_is_rebuilt_byte_for_byte() {
        let card = card_with(&[("BASCUS-94163FF7-S01", 1), ("BASLUS-00594", 2)]);
        assert_eq!(round_trip(&card), card);
    }

    #[test]
    fn a_formatted_card_with_nothing_on_it_rebuilds_as_formatted() {
        let card = card_with(&[]);
        assert_eq!(round_trip(&card), card);
    }

    #[test]
    fn a_save_built_by_hand_gets_an_entry_carrying_its_name() {
        let mut builder = CardBuilder::new();
        builder.add(Save {
            slot: 0,
            name: "BASLUS-00594".into(),
            dirent: Vec::new(),
            data: vec![7u8; BLOCK],
        });
        let card = builder.build().expect("writes");

        let parsed = MemoryCard::parse(&card).expect("reads what it wrote");
        assert_eq!(parsed.saves().len(), 1);
        assert_eq!(parsed.saves()[0].name, "BASLUS-00594");
        assert_eq!(parsed.saves()[0].slot, 1, "a builder packs from the front");
        assert_eq!(parsed.saves()[0].data, vec![7u8; BLOCK]);
    }

    #[test]
    fn refuses_more_saves_than_a_card_holds() {
        let mut builder = CardBuilder::new();
        for index in 0..16 {
            builder.add(Save {
                slot: 0,
                name: format!("SAVE{index}"),
                dirent: Vec::new(),
                data: vec![0u8; BLOCK],
            });
        }
        assert!(matches!(builder.build(), Err(Error::Full { needed: 16, available: 15 })));
    }

    #[test]
    fn refuses_a_save_with_no_bytes() {
        let mut builder = CardBuilder::new();
        builder.add(Save { slot: 0, name: "EMPTY".into(), dirent: Vec::new(), data: Vec::new() });
        assert!(matches!(builder.build(), Err(Error::EmptySave(_))));
    }
}
