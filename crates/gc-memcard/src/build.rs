//! Writing a card image out of a set of saves.

use crate::{
    BAT_BLOCK, BAT_MIRROR_BLOCK, BLOCK, CHAIN_END, DIR_BLOCK, DIR_MIRROR_BLOCK, ENCODING_OFFSET, ENTRY_LEN,
    Error, KNOWN_SIZES, MAP_OFFSET, MAX_ENTRIES, RESERVED_BLOCKS, Result, Save, checksum,
    stamp_directory_checksum, stamp_table_checksum,
};

/// Builds a card image from saves.
///
/// The directory, the allocation table, both their mirrors and every checksum are regenerated.
/// Only a save entry's first block and block count are rewritten; the rest of it is the game's and
/// comes through unchanged.
#[derive(Debug, Clone, Default)]
pub struct CardBuilder {
    saves: Vec<Save>,
    header: Option<Vec<u8>>,
    capacity: Option<usize>,
    shift_jis: bool,
}

impl CardBuilder {
    /// A builder for an empty card, sized to whatever the saves need.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Keeps a header block read off another card, so the card's flash serial survives.
    ///
    /// A block that is not [`BLOCK`] bytes is ignored, since it is not a header block.
    pub fn header_block(&mut self, bytes: impl Into<Vec<u8>>) -> &mut Self {
        self.header = Some(bytes.into());
        self
    }

    /// Asks for a card of this **data** capacity in bytes, as [`MemoryCard::capacity`] reports it.
    ///
    /// A capacity no card comes in is ignored rather than honoured: a console would not accept
    /// one. Without this the smallest card the saves fit on is used.
    ///
    /// [`MemoryCard::capacity`]: crate::MemoryCard::capacity
    pub fn capacity(&mut self, bytes: usize) -> &mut Self {
        self.capacity = Some(bytes);
        self
    }

    /// Formats the card for Shift-JIS names, as a Japanese console does, rather than ASCII.
    ///
    /// Only a card that brings no [`header_block`](Self::header_block) of its own is formatted, so
    /// this says nothing about one that does: its header already names its character set.
    pub fn shift_jis(&mut self, shift_jis: bool) -> &mut Self {
        self.shift_jis = shift_jis;
        self
    }

    /// Adds a save. Its `slot` is ignored: the directory is filled in the order saves are added.
    pub fn add(&mut self, save: Save) -> &mut Self {
        self.saves.push(save);
        self
    }

    /// Writes the card out.
    pub fn build(&self) -> Result<Vec<u8>> {
        if self.saves.len() > MAX_ENTRIES {
            return Err(Error::TooManySaves { given: self.saves.len(), available: MAX_ENTRIES });
        }

        // Take the stated size when it is one cards come in, otherwise the smallest that fits: a
        // capacity nothing makes is not a card a console would accept.
        let needed: usize = self.saves.iter().map(|save| save.data.len().div_ceil(BLOCK)).sum();
        let stated = self.capacity.map(|capacity| capacity / BLOCK + RESERVED_BLOCKS);
        let blocks = stated
            .filter(|size| KNOWN_SIZES.contains(size))
            .or_else(|| KNOWN_SIZES.into_iter().find(|size| size - RESERVED_BLOCKS >= needed))
            .ok_or(Error::Full { needed, available: KNOWN_SIZES[KNOWN_SIZES.len() - 1] - RESERVED_BLOCKS })?;

        if needed > blocks - RESERVED_BLOCKS {
            return Err(Error::Full { needed, available: blocks - RESERVED_BLOCKS });
        }

        let mut card = vec![0xFFu8; blocks * BLOCK];
        let mut directory = vec![0xFFu8; BLOCK];
        let mut table = vec![0u8; BLOCK];

        let mut next_block = RESERVED_BLOCKS;
        for (slot, save) in self.saves.iter().enumerate() {
            let count = save.data.len().div_ceil(BLOCK);
            let first = next_block;

            for step in 0..count {
                let current = first + step;
                let from = step * BLOCK;
                let to = ((step + 1) * BLOCK).min(save.data.len());
                card[current * BLOCK..current * BLOCK + (to - from)].copy_from_slice(&save.data[from..to]);

                let link =
                    if step + 1 == count { CHAIN_END } else { u16::try_from(current + 1).expect("fits") };
                let at = MAP_OFFSET + (current - RESERVED_BLOCKS) * 2;
                table[at..at + 2].copy_from_slice(&link.to_be_bytes());
            }

            let mut entry = directory_entry(save);
            entry.resize(ENTRY_LEN, 0);
            entry[0x36..0x38].copy_from_slice(&u16::try_from(first).expect("block fits").to_be_bytes());
            entry[0x38..0x3A].copy_from_slice(&u16::try_from(count).expect("count fits").to_be_bytes());
            directory[slot * ENTRY_LEN..(slot + 1) * ENTRY_LEN].copy_from_slice(&entry);

            next_block += count;
        }

        // The table's own head: an update counter, how many blocks are free, and the last one
        // handed out. Both copies of each region start at the same counter, since neither is stale.
        let free = u16::try_from(blocks - next_block).expect("free count fits");
        table[4..6].copy_from_slice(&0u16.to_be_bytes());
        table[6..8].copy_from_slice(&free.to_be_bytes());
        table[8..10].copy_from_slice(&u16::try_from(next_block - 1).expect("block fits").to_be_bytes());
        directory[BLOCK - 6..BLOCK - 4].copy_from_slice(&0u16.to_be_bytes());

        stamp_directory_checksum(&mut directory);
        stamp_table_checksum(&mut table);

        let header = match &self.header {
            Some(area) if area.len() == BLOCK => area.clone(),
            _ => default_header(blocks, self.shift_jis),
        };
        card[..BLOCK].copy_from_slice(&header);
        card[DIR_BLOCK * BLOCK..(DIR_BLOCK + 1) * BLOCK].copy_from_slice(&directory);
        card[DIR_MIRROR_BLOCK * BLOCK..(DIR_MIRROR_BLOCK + 1) * BLOCK].copy_from_slice(&directory);
        card[BAT_BLOCK * BLOCK..(BAT_BLOCK + 1) * BLOCK].copy_from_slice(&table);
        card[BAT_MIRROR_BLOCK * BLOCK..(BAT_MIRROR_BLOCK + 1) * BLOCK].copy_from_slice(&table);
        Ok(card)
    }
}

/// The directory entry a save gets: its own, or one carrying its codes and filename.
fn directory_entry(save: &Save) -> Vec<u8> {
    if !save.dirent.is_empty() {
        return save.dirent.clone();
    }
    let mut entry = vec![0u8; ENTRY_LEN];
    // A code field a save did not state is written as `-`, which is what the console shows for a
    // save whose game it cannot name.
    entry[..6].fill(b'-');
    let code = save.game_code.as_bytes();
    entry[..code.len().min(4)].copy_from_slice(&code[..code.len().min(4)]);
    let maker = save.maker_code.as_bytes();
    entry[4..4 + maker.len().min(2)].copy_from_slice(&maker[..maker.len().min(2)]);
    entry[6] = 0xFF;
    let name = save.filename.as_bytes();
    let len = name.len().min(32);
    entry[8..8 + len].copy_from_slice(&name[..len]);
    entry[0x3A..0x3C].copy_from_slice(&0xFFFFu16.to_be_bytes());
    entry
}

/// A freshly formatted header block, as Dolphin formats one.
///
/// The first 32 bytes say where and when the card was formatted: a serial worked out from the
/// console's flash ID and the moment, the moment itself, and the console's clock bias, language
/// and video mode. A card made here knows none of those, so it says what a console would with
/// all of them zero, serial included: worked out the console's way, so the two still agree.
pub(crate) fn default_header(blocks: usize, shift_jis: bool) -> Vec<u8> {
    let mut header = vec![0xFFu8; BLOCK];
    let format_time = 0u64;
    header[..12].copy_from_slice(&serial(format_time, [0; 12]));
    header[0x0C..0x14].copy_from_slice(&format_time.to_be_bytes());
    header[0x14..0x20].fill(0);
    // The slot it was formatted in, 0 for A. The console's card library refuses any other value
    // as a broken card, and the size after it is what Dolphin checks the file's length against.
    header[0x20..0x22].copy_from_slice(&0u16.to_be_bytes());
    // The size the console reports, in megabits.
    let megabits = u16::try_from(blocks * BLOCK / (1024 * 1024 / 8)).unwrap_or(4);
    header[0x22..0x24].copy_from_slice(&megabits.to_be_bytes());
    header[ENCODING_OFFSET..ENCODING_OFFSET + 2].copy_from_slice(&u16::from(shift_jis).to_be_bytes());
    let (sum, inverse) = checksum(&header[..0x1FC]);
    header[0x1FC..0x1FE].copy_from_slice(&sum.to_be_bytes());
    header[0x1FE..0x200].copy_from_slice(&inverse.to_be_bytes());
    header
}

/// The serial a console writes when it formats a card, from its flash ID and the moment.
///
/// Each byte is the flash ID's plus the next step of a linear congruential generator seeded with
/// the format time, as Dolphin and the console's own library both work it out.
fn serial(format_time: u64, flash_id: [u8; 12]) -> [u8; 12] {
    let step = |rand: u64| rand.wrapping_mul(0x41C6_4E6D).wrapping_add(0x3039) >> 16;
    let mut rand = format_time;
    let mut serial = [0u8; 12];
    for (byte, id) in serial.iter_mut().zip(flash_id) {
        rand = step(rand);
        *byte = id.wrapping_add(rand.to_le_bytes()[0]);
        rand = step(rand) & 0x7FFF;
    }
    serial
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MemoryCard, tests::card_with};

    fn round_trip(card: &[u8]) -> Vec<u8> {
        let parsed = MemoryCard::parse(card).expect("reads");
        let mut builder = CardBuilder::new();
        builder.header_block(parsed.header_block().to_vec()).capacity(parsed.capacity());
        for save in parsed.saves() {
            builder.add(save.clone());
        }
        builder.build().expect("writes")
    }

    #[test]
    fn a_card_a_console_wrote_is_rebuilt_byte_for_byte() {
        let card = card_with(64, &[("GAFE01", "super_mario_sunshine", 2), ("GM4E01", "PSO_SYSTEM", 1)]);
        assert_eq!(round_trip(&card), card);
    }

    #[test]
    fn a_save_built_by_hand_gets_an_entry_carrying_its_codes() {
        let mut builder = CardBuilder::new();
        builder.add(Save {
            slot: 0,
            game_code: "GAFE".into(),
            maker_code: "01".into(),
            filename: "super_mario_sunshine".into(),
            dirent: Vec::new(),
            data: vec![5u8; BLOCK],
        });
        let card = builder.build().expect("writes");

        let parsed = MemoryCard::parse(&card).expect("reads what it wrote");
        assert_eq!(parsed.saves().len(), 1);
        assert_eq!(parsed.saves()[0].game_code, "GAFE");
        assert_eq!(parsed.saves()[0].maker_code, "01");
        assert_eq!(parsed.saves()[0].filename, "super_mario_sunshine");
        assert_eq!(parsed.saves()[0].data, vec![5u8; BLOCK]);
    }

    #[test]
    fn a_capacity_no_card_comes_in_is_ignored_rather_than_honoured() {
        let mut builder = CardBuilder::new();
        builder.capacity(12_345).add(Save {
            slot: 0,
            game_code: "GAFE".into(),
            maker_code: "01".into(),
            filename: "x".into(),
            dirent: Vec::new(),
            data: vec![0u8; BLOCK],
        });
        let card = builder.build().expect("writes");
        // Falls back to the smallest card the save fits on rather than making up a size.
        assert_eq!(card.len(), 64 * BLOCK);
    }

    #[test]
    fn refuses_saves_that_do_not_fit_the_card_they_asked_for() {
        let mut builder = CardBuilder::new();
        builder.capacity((64 - RESERVED_BLOCKS) * BLOCK).add(Save {
            slot: 0,
            game_code: "GAFE".into(),
            maker_code: "01".into(),
            filename: "big".into(),
            dirent: Vec::new(),
            data: vec![0u8; 100 * BLOCK],
        });
        assert!(matches!(builder.build(), Err(Error::Full { needed: 100, available: 59 })));
    }

    /// The serial is Dolphin's working: a card it formatted in slot A at 0x3277D33F carries this
    /// one, and the flash ID it formats slot A with is the text `DOLPHINSLOTA`.
    #[test]
    fn a_serial_is_worked_out_as_dolphin_works_it_out() {
        assert_eq!(
            serial(0x3277_D33F, *b"DOLPHINSLOTA"),
            [0x60, 0x4D, 0x8E, 0x20, 0x32, 0x9D, 0x43, 0xCE, 0x5D, 0xBA, 0x33, 0xEA]
        );
    }

    /// A card that brings no header of its own gets the one Dolphin writes when it formats.
    ///
    /// Dolphin lays the fields out as slot at 0x20, size in megabits at 0x22 and encoding at 0x24,
    /// and refuses a card whose size field does not match its length. This header once had each
    /// field one place early, which this crate read back without complaint since it never looks.
    #[test]
    fn a_header_made_up_for_a_card_says_what_dolphin_checks_where_it_looks() {
        for (blocks, megabits) in [(59usize, 4u16), (2043, 128)] {
            let mut builder = CardBuilder::new();
            builder.capacity(blocks * BLOCK);
            let card = builder.build().expect("an empty card writes");
            assert_eq!(card.len(), (blocks + RESERVED_BLOCKS) * BLOCK);

            let header = &card[..BLOCK];
            let field = |at: usize| u16::from_be_bytes([header[at], header[at + 1]]);
            assert_eq!(field(0x20), 0, "formatted in slot A");
            assert_eq!(field(0x22), megabits, "the size, in megabits");
            assert_eq!(field(0x24), 0, "ASCII");
            assert!(header[0x26..0x1FC].iter().all(|&byte| byte == 0xFF), "unused, as Dolphin checks");
            assert_eq!(checksum(&header[..0x1FC]), (field(0x1FC), field(0x1FE)));
            assert!(!crate::is_shift_jis(header));
        }

        // A Japanese console's card differs in the one field, and its checksum.
        let card = CardBuilder::new().shift_jis(true).build().expect("writes");
        let header = &card[..BLOCK];
        assert_eq!(&header[0x24..0x26], &[0, 1]);
        assert!(crate::is_shift_jis(header));
        let stored = (
            u16::from_be_bytes([header[0x1FC], header[0x1FD]]),
            u16::from_be_bytes([header[0x1FE], header[0x1FF]]),
        );
        assert_eq!(checksum(&header[..0x1FC]), stored);

        // A card that brings its own header keeps it, whatever it is asked to format as.
        let kept = CardBuilder::new().header_block(header.to_vec()).shift_jis(false).build().expect("writes");
        assert!(crate::is_shift_jis(&kept[..BLOCK]));
    }
}
