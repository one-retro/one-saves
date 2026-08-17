//! Reading and writing Neo Geo memory card images.
//!
//! A card is 64-byte blocks. Block 0 is the header, the next blocks are the directory, then two
//! copies of the allocation table, and everything after that is save data. How many blocks each
//! region takes is worked out from the card's own size word rather than fixed, and checked against
//! the table before anything is read — see [`Geometry`].
//!
//! ```no_run
//! use neogeo_memcard::MemoryCard;
//!
//! let card = MemoryCard::parse(&std::fs::read("card.neo")?)?;
//! for save in card.saves() {
//!     println!("NGH {:04X} save {}: {}", save.ngh, save.sub, save.title());
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # One card, two consoles
//!
//! The MVS cabinet and the AES home console take the same card, and a save written on one is read
//! by the other — which is the whole point of it. Nothing on the card records which wrote it, so
//! nothing here distinguishes them either.
//!
//! # The allocation table is a chain
//!
//! One byte per block, and the byte is the *next* block's number. [`fat::FREE`] and
//! [`fat::RESERVED`] are 0 and 2, [`fat::CHAIN_END`] is 1, and anything else is where the save
//! carries on. A directory entry names only the first block, and the chain gives the rest, so a
//! save's blocks need not be adjacent and its length is not stored anywhere.
//!
//! The three sentinels never collide with a real block number because the first block a save can
//! occupy is [`Geometry::first_data_block`], which is 5 on the smallest card and 13 on the largest
//! seen — always past 2.
//!
//! # The size word is an address span, not a byte count
//!
//! The header's size at [`SIZE_OFFSET`] is **twice** the card's length in bytes: `$1000` for a
//! 2 KiB card, `$4000` for an 8 KiB one. The card is 8-bit and the bus is 16, so what it records is
//! how much of the 68000's address space it answers to. Everything else — blocks, the directory,
//! both tables — is counted in bytes, so the size word is halved once on the way in and doubled
//! once on the way out, and nowhere else.
//!
//! # Containers
//!
//! A bare card is the card and nothing else. The MiSTer NeoGeo core writes something larger, and
//! its RTL is where the shape below is checked rather than guessed:
//!
//! | Bytes | What | Where the core says so |
//! | ----- | ---- | ---------------------- |
//! | `0x00000`–`0x0FFFF` | cabinet backup RAM, 64 KiB | `sram_wr = ~bk_lba[7]`, `rtl/mem/backup.v` |
//! | `0x10000`–`0x11FFF` | the memory card, 8 KiB | `memcard_wr = bk_lba[7]`, `rtl/mem/memcard.v` |
//!
//! The transfer runs to `bk_lba >= 'h8F` — 144 sectors of 512 bytes, which is the 73728 a save
//! comes to. The card half is written as 16-bit words whose high byte is the card's **even**
//! address and whose low byte is its **odd** one, so swapping the two bytes of every word turns the
//! file back into the address space the 68000 sees. That is what [`MemoryCard::parse`] does.
//!
//! Two things about that region are worth knowing. The core emulates a 2 KiB card in cart mode and
//! an 8 KiB one in CD mode, both inside the same 8 KiB buffer, so a cart save's card stops well
//! short of the buffer. And for carts with extra RAM the core puts *that* in the same region
//! instead of a card, so a save for one holds no card at all — [`detect`] wants the signature and
//! says no, which is the right answer rather than a near miss.

#![forbid(unsafe_code)]

mod build;
mod error;

pub use build::CardBuilder;
pub use error::{Error, Result};

/// One block: the unit the allocation table counts and a save occupies.
pub const BLOCK: usize = 64;

/// A directory entry's length.
pub const ENTRY_LEN: usize = 4;

/// A directory entry's sub-number when the slot holds nothing.
pub const FREE_ENTRY: u8 = 0xFF;

/// How many saves one game may keep.
pub const MAX_SUB: u8 = 16;

/// Where the card states its size, as a big-endian word.
///
/// The value is **twice** the card's length in bytes; see the module documentation.
pub const SIZE_OFFSET: usize = 0x0A;

/// Where the two allocation table checksums sit, FAT 1 then FAT 2.
pub const CHECKSUM_OFFSET: usize = 0x0D;

/// Where the card holder's username sits, 16 bytes.
pub const USERNAME_OFFSET: usize = 0x10;

/// Where the signature sits.
pub const MAGIC_OFFSET: usize = 0x20;

/// Where the card states its region.
pub const REGION_OFFSET: usize = 0x30;

/// The characters of the signature, one every other byte from [`MAGIC_OFFSET`].
///
/// The bytes between them are a stepping pattern — twice the offset they sit at — so the signature
/// as stored is `4E 40 45 44 4F 48 2D 4C 47 50 45 54 4F 58 80 5C`.
pub const MAGIC: &[u8; 8] = b"NEO-GEO\x80";

/// The sizes a card comes in, in bytes: 2 KiB to 16 KiB in 2 KiB steps.
///
/// A card states twice these at [`SIZE_OFFSET`]. The two seen here are the 2 KiB one a cartridge
/// system takes and the 8 KiB one a Neo Geo CD keeps inside itself.
pub const SIZES: [usize; 8] = [2048, 4096, 6144, 8192, 10240, 12288, 14336, 16384];

/// How many bytes of cabinet backup RAM a MiSTer save carries in front of the card.
pub const MISTER_BACKUP_RAM: usize = 0x10000;

/// How much of a MiSTer save the card region takes, whatever size card is in it.
///
/// The core sizes the buffer for CD mode's 8 KiB card; a cartridge system's 2 KiB one sits at the
/// front of it and the rest is left as it was loaded.
pub const MISTER_CARD_REGION: usize = 0x2000;

/// What an allocation table entry says about a block.
///
/// Any value that is none of these three is the number of the block the save carries on in. The
/// three never collide with a real one: a save starts at [`Geometry::first_data_block`] at the
/// earliest, which is always past 2.
pub mod fat {
    /// Nothing is using this block.
    pub const FREE: u8 = 0x00;
    /// This is the last block of its save.
    pub const CHAIN_END: u8 = 0x01;
    /// The header, the directory or a table is here.
    pub const RESERVED: u8 = 0x02;
}

/// Which market the card was formatted for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    /// Japan.
    Japan,
    /// The USA.
    Usa,
    /// Europe, and Asia outside Japan.
    Europe,
    /// A byte that names none of the three, kept as it was found.
    Unknown(u8),
}

impl Region {
    /// The region a byte names.
    #[must_use]
    pub const fn from_byte(byte: u8) -> Self {
        match byte {
            0 => Region::Japan,
            1 => Region::Usa,
            2 => Region::Europe,
            other => Region::Unknown(other),
        }
    }

    /// The byte a region is written as.
    #[must_use]
    pub const fn as_byte(self) -> u8 {
        match self {
            Region::Japan => 0,
            Region::Usa => 1,
            Region::Europe => 2,
            Region::Unknown(byte) => byte,
        }
    }
}

/// How a card image arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    /// The card and nothing else, in the byte order the 68000 sees.
    Bare,
    /// A MiSTer NeoGeo core save: 64 KiB of cabinet backup RAM, then the card with the two bytes
    /// of every 16-bit word swapped.
    MiSter,
}

/// Where each region of a card sits, worked out from the card's size.
///
/// The directory holds one entry per block and the table one byte per block, which together fix
/// how much of the card is structure. [`MemoryCard::parse`] checks the result against the
/// table's own reserved marks and refuses a card the two disagree about, so a size whose geometry
/// this crate has wrong is refused rather than read through the wrong offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    /// The card's size in bytes.
    pub size: usize,
    /// How many blocks it holds in total.
    pub blocks: usize,
    /// How many directory entries there are.
    pub entries: usize,
    /// The first block of the directory.
    pub directory_block: usize,
    /// The first block of allocation table 1.
    pub fat1_block: usize,
    /// The first block of allocation table 2.
    pub fat2_block: usize,
    /// The first block a save can occupy.
    pub first_data_block: usize,
}

impl Geometry {
    /// The layout of a card of this many **bytes**, or `None` for a size cards do not come in.
    ///
    /// Not the header's size word, which is twice this — [`from_size_word`](Self::from_size_word)
    /// takes that.
    #[must_use]
    pub const fn of(size: usize) -> Option<Self> {
        if !size.is_multiple_of(2048) || size < 2048 || size > 16384 {
            return None;
        }
        let blocks = size / BLOCK;
        // One directory entry per block, whether or not a card could ever fill them all.
        let entries = blocks;
        let directory_blocks = (entries * ENTRY_LEN).div_ceil(BLOCK);
        let fat_blocks = blocks.div_ceil(BLOCK);
        let directory_block = 1;
        let fat1_block = directory_block + directory_blocks;
        let fat2_block = fat1_block + fat_blocks;
        Some(Geometry {
            size,
            blocks,
            entries,
            directory_block,
            fat1_block,
            fat2_block,
            first_data_block: fat2_block + fat_blocks,
        })
    }

    /// The layout a card's own size word describes.
    ///
    /// The word is an address span, so it is halved to get the card's length in bytes.
    #[must_use]
    pub const fn from_size_word(word: u16) -> Option<Self> {
        Geometry::of(word as usize / 2)
    }

    /// The size word a card of this geometry states: twice its length in bytes.
    #[must_use]
    pub fn size_word(self) -> u16 {
        u16::try_from(self.size * 2).expect("twice the largest card still fits a word")
    }

    /// How many bytes each allocation table runs to, which is one per block.
    #[must_use]
    pub const fn fat_len(self) -> usize {
        self.blocks
    }

    /// How many bytes saves can occupy: the card minus the blocks structure takes.
    #[must_use]
    pub const fn data_capacity(self) -> usize {
        (self.blocks - self.first_data_block) * BLOCK
    }

    /// The layout of the card whose saves can occupy this many bytes.
    ///
    /// The inverse of [`data_capacity`](Self::data_capacity), for a caller that recorded the usable
    /// capacity and needs the card back. `None` when no card has one.
    #[must_use]
    pub fn for_data_capacity(bytes: usize) -> Option<Self> {
        SIZES.into_iter().filter_map(Geometry::of).find(|g| g.data_capacity() == bytes)
    }
}

/// One save on a card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Save {
    /// The directory index it occupied.
    ///
    /// Read only: a [`CardBuilder`] fills the directory in the order saves are added.
    pub slot: usize,
    /// Which of the game's saves this is. A game may keep [`MAX_SUB`] of them.
    pub sub: u8,
    /// The game's NGH number, as the card stores it: binary-coded decimal, so Fatal Fury 2's
    /// NGH-047 is `0x0047` and Metal Slug X's NGH-250 is `0x0250`.
    pub ngh: u16,
    /// The directory entry's 4 bytes, verbatim.
    ///
    /// Empty when a save was built by hand rather than read off a card. Its last byte is the
    /// first block, which a builder allocates for itself.
    pub dirent: Vec<u8>,
    /// The save's bytes: every block it occupies.
    ///
    /// Blocks are contiguous, which the format leaves no alternative to — see the module
    /// documentation. This is the *allocated* run, which is as much as the card knows: a game
    /// tells the BIOS how many bytes it wants and the card does not record the answer, so a save
    /// whose data stops short of its last block gives back the whole block regardless.
    pub data: Vec<u8>,
}

impl Save {
    /// The save's title, which games write into the first 20 bytes of their data.
    ///
    /// Empty when there is nothing legible there, which is not an error: the convention is the
    /// BIOS's and a game is free to put its own bytes in that space. Metal Slug X writes
    /// `METAL SLUG X`; Fatal Fury 2 writes a binary header and no title at all.
    #[must_use]
    pub fn title(&self) -> String {
        let field = &self.data[..self.data.len().min(20)];
        let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
        let text = &field[..end];
        if text.is_empty() || !text.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
            return String::new();
        }
        String::from_utf8_lossy(text).trim_end().to_owned()
    }
}

/// A parsed memory card image.
#[derive(Debug, Clone)]
pub struct MemoryCard {
    card: Vec<u8>,
    backup_ram: Option<Vec<u8>>,
    container: Container,
    geometry: Geometry,
    saves: Vec<Save>,
}

impl MemoryCard {
    /// Reads a card image, bare or in a MiSTer save.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let (container, card, backup_ram) = strip_container(bytes).ok_or(Error::WrongLength(bytes.len()))?;
        if !has_magic(&card) {
            return Err(Error::NotAMemoryCard);
        }

        let word = u16::from_be_bytes([card[SIZE_OFFSET], card[SIZE_OFFSET + 1]]);
        let geometry = Geometry::from_size_word(word).ok_or(Error::UnknownSize(word as usize))?;
        if card.len() < geometry.size {
            return Err(Error::Corrupt(format!(
                "the card states {} bytes and the image holds {}",
                geometry.size,
                card.len()
            )));
        }
        let card = card[..geometry.size].to_vec();

        // The table's reserved marks and the computed layout are two statements about the same
        // thing. Trusting the offsets when they disagree would read a directory that is not there.
        let fat = &card[geometry.fat1_block * BLOCK..][..geometry.fat_len()];
        let stated = fat.iter().take_while(|&&v| v == fat::RESERVED).count();
        if stated != geometry.first_data_block {
            return Err(Error::UnexpectedGeometry { computed: geometry.first_data_block, stated });
        }

        let saves = read_directory(&card, geometry)?;
        Ok(MemoryCard { card, backup_ram, container, geometry, saves })
    }

    /// Every save on the card, in directory order.
    #[must_use]
    pub fn saves(&self) -> &[Save] {
        &self.saves
    }

    /// Takes the saves, for a caller that is going to rebuild rather than read on.
    #[must_use]
    pub fn into_saves(self) -> Vec<Save> {
        self.saves
    }

    /// The card itself, in the byte order the 68000 sees, with any container taken off.
    #[must_use]
    pub fn image(&self) -> &[u8] {
        &self.card
    }

    /// What the image arrived in.
    #[must_use]
    pub fn container(&self) -> Container {
        self.container
    }

    /// The cabinet's backup RAM, when the container carried one.
    ///
    /// This is a *different* save from the card — an MVS cabinet keeps its own — and it is here so
    /// that reading a MiSTer file does not quietly drop half of it.
    #[must_use]
    pub fn backup_ram(&self) -> Option<&[u8]> {
        self.backup_ram.as_deref()
    }

    /// Where the card's regions sit.
    #[must_use]
    pub fn geometry(&self) -> Geometry {
        self.geometry
    }

    /// The card's capacity in bytes, structure included, as the card itself states it.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.geometry.size
    }

    /// Everything before the first data block: the header, the directory and both tables.
    ///
    /// A builder regenerates the entries and the tables and passes the rest through, so keeping
    /// this is what carries a card's username, region and free-slot bytes across a round trip.
    #[must_use]
    pub fn system_area(&self) -> &[u8] {
        &self.card[..self.geometry.first_data_block * BLOCK]
    }

    /// The card holder's username, as the card stores it.
    #[must_use]
    pub fn username(&self) -> &[u8] {
        &self.card[USERNAME_OFFSET..USERNAME_OFFSET + 16]
    }

    /// Which market the card was formatted for.
    #[must_use]
    pub fn region(&self) -> Region {
        Region::from_byte(self.card[REGION_OFFSET])
    }
}

/// The 8-bit sum an allocation table is checked by.
#[must_use]
pub fn fat_checksum(fat: &[u8]) -> u8 {
    fat.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte))
}

/// Whether the signature is where it should be.
#[must_use]
fn has_magic(card: &[u8]) -> bool {
    card.len() > MAGIC_OFFSET + 15 && MAGIC.iter().enumerate().all(|(i, &c)| card[MAGIC_OFFSET + i * 2] == c)
}

/// Whether these bytes look like a card, in any of the containers one ships in.
#[must_use]
pub fn detect(bytes: &[u8]) -> bool {
    matches!(strip_container(bytes), Some((_, card, _)) if has_magic(&card))
}

/// Finds the card inside whatever container it arrived in, and the backup RAM beside it.
///
/// The card comes back in the byte order the 68000 sees, whatever the container did to it.
#[must_use]
pub fn strip_container(bytes: &[u8]) -> Option<(Container, Vec<u8>, Option<Vec<u8>>)> {
    // A MiSTer save is the cabinet's backup RAM and then the card, word-swapped. It is checked
    // first because its card region is a plausible bare card length on its own.
    if bytes.len() > MISTER_BACKUP_RAM {
        let region = &bytes[MISTER_BACKUP_RAM..];
        let swapped: Vec<u8> = (0..region.len() & !1).map(|i| region[i ^ 1]).collect();
        if has_magic(&swapped) {
            return Some((Container::MiSter, swapped, Some(bytes[..MISTER_BACKUP_RAM].to_vec())));
        }
    }
    if has_magic(bytes) {
        return Some((Container::Bare, bytes.to_vec(), None));
    }
    None
}

/// Walks the directory, following each used entry's chain.
fn read_directory(card: &[u8], geometry: Geometry) -> Result<Vec<Save>> {
    let directory = &card[geometry.directory_block * BLOCK..][..geometry.entries * ENTRY_LEN];
    let fat = &card[geometry.fat1_block * BLOCK..][..geometry.fat_len()];

    let mut saves = Vec::new();
    for index in 0..geometry.entries {
        let entry = &directory[index * ENTRY_LEN..][..ENTRY_LEN];
        if entry[0] == FREE_ENTRY {
            continue;
        }
        let mut data = Vec::new();
        for block in follow_chain(fat, entry[3] as usize, geometry, index)? {
            data.extend_from_slice(&card[block * BLOCK..(block + 1) * BLOCK]);
        }
        saves.push(Save {
            slot: index,
            sub: entry[0],
            ngh: u16::from_be_bytes([entry[1], entry[2]]),
            dirent: entry.to_vec(),
            data,
        });
    }
    Ok(saves)
}

/// Every block a save occupies, from the one its entry names to the end of its chain.
fn follow_chain(fat: &[u8], first: usize, geometry: Geometry, index: usize) -> Result<Vec<usize>> {
    let mut chain = Vec::new();
    let mut current = first;
    loop {
        if current < geometry.first_data_block || current >= geometry.blocks {
            return Err(Error::Corrupt(format!("entry {index} reaches block {current}")));
        }
        // A cycle would otherwise be an unbounded read, and a card with one is broken whatever it
        // was meant to say.
        if chain.contains(&current) {
            return Err(Error::Corrupt(format!("the chain from block {first} loops at {current}")));
        }
        chain.push(current);
        match fat[current] {
            fat::CHAIN_END => return Ok(chain),
            fat::FREE | fat::RESERVED => {
                return Err(Error::Corrupt(format!(
                    "the chain from block {first} runs into block {current}, which no save holds"
                )));
            }
            next => current = next as usize,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_geometry_of_every_size_reserves_what_it_should() {
        for size in SIZES {
            let g = Geometry::of(size).expect("a size cards come in");
            assert_eq!(g.blocks, size / BLOCK);
            assert_eq!(g.entries, g.blocks);
            assert!(g.first_data_block > g.fat2_block);
            assert_eq!(Geometry::from_size_word(g.size_word()), Some(g), "the word round-trips");
        }
        assert_eq!(Geometry::of(3000), None);
        assert_eq!(Geometry::of(32768), None);

        // The two geometries real cards confirm: the 2 KiB card a cartridge system takes, and the
        // 8 KiB one a Neo Geo CD keeps inside itself.
        let cart = Geometry::of(2048).expect("a real size");
        assert_eq!((cart.blocks, cart.entries, cart.first_data_block), (32, 32, 5));
        assert_eq!(cart.size_word(), 0x1000);
        let cd = Geometry::of(8192).expect("a real size");
        assert_eq!((cd.blocks, cd.entries, cd.first_data_block), (128, 128, 13));
        assert_eq!((cd.directory_block, cd.fat1_block, cd.fat2_block), (1, 9, 11));
        assert_eq!(cd.size_word(), 0x4000);
    }

    #[test]
    fn something_that_is_not_a_card_is_refused() {
        assert!(matches!(MemoryCard::parse(&[1, 2, 3]), Err(Error::WrongLength(3))));
        assert!(!detect(&[1, 2, 3]));
        assert!(!detect(&vec![0u8; 4096]));
    }
}
