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
//! comes to. The core keeps the whole file as 16-bit words with the low byte first, where the
//! 68000 reads the high byte first, so the two bytes of every word are swapped: in the RAM as much
//! as on the card. Swapping them back turns each half into the address space the 68000 sees, and
//! that is the order everything here hands out and takes in. [`split_mister`] and
//! [`join_mister`] are the container and nothing more.
//!
//! Two things about the card region are worth knowing. The core emulates a 2 KiB card in cart
//! mode and an 8 KiB one in CD mode, both inside the same 8 KiB buffer, and in cart mode it
//! addresses the first 2 KiB and nothing past it, whatever the card's header claims — which is
//! what [`MemoryCard::parse_within`] is for. And for the two carts with RAM of their own, Jockey
//! Grand Prix and V-Liner, the core puts *that* in the region instead of a card, all 8 KiB of it
//! and swapped the same way, so a save for one holds no card at all. [`MemoryCard::parse`] wants the signature and says no, which is the right
//! answer rather than a near miss; [`split_mister`] still hands the region over.

#![forbid(unsafe_code)]

mod build;
pub mod cabinet;
mod error;

pub use build::{CardBuilder, join_mister};
pub use error::{Error, Result};

/// One block: the unit the allocation table counts and a save occupies.
pub const BLOCK: usize = 64;

/// A directory entry's length.
pub const ENTRY_LEN: usize = 4;

/// The header's length: block 0, which is what a card says about itself.
///
/// It carries the card's size, the two table checksums, the holder's username, the signature and
/// the region of the console that formatted it. The rest of the reserved blocks, the directory
/// and both tables, follow from the saves.
pub const HEADER_LEN: usize = BLOCK;

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

/// How long a MiSTer save is: the cabinet's RAM, then the card region.
pub const MISTER_SAVE: usize = MISTER_BACKUP_RAM + MISTER_CARD_REGION;

/// How much of the card region the core addresses in cart mode: the 2 KiB card a cartridge system
/// takes, whatever size the card's own header claims.
pub const MISTER_CART_CARD: usize = 2048;

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
    /// A MiSTer NeoGeo core save: 64 KiB of cabinet backup RAM, then the card, with the two bytes
    /// of every 16-bit word swapped in both.
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
    /// In the order the save's chain runs, wherever on the card the blocks sit — see the module
    /// documentation. This is every block allocated, which is as much as the card knows: a game
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
        legible(&self.data[..self.data.len().min(20)])
    }
}

/// The text a fixed-width field holds up to its first NUL, the spaces padding it either side
/// trimmed, or nothing when any of it is not printable ASCII.
pub(crate) fn legible(field: &[u8]) -> String {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    let text = &field[..end];
    if text.is_empty() || !text.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
        return String::new();
    }
    String::from_utf8_lossy(text).trim().to_owned()
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
    /// Reads a card image, bare or in a MiSTer save, as large as its header says it is.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        Self::read(bytes, None)
    }

    /// Reads a card off a medium that holds `medium` bytes of it, whatever its header claims.
    ///
    /// A MiSTer save in cart mode is the case: the core addresses the first
    /// [`MISTER_CART_CARD`] bytes of its card region, and a card whose header promises more
    /// wraps every address past that back to the start. The header still lays out the directory
    /// and the tables, so they are read where it puts them, but the card is the medium:
    /// [`capacity`](Self::capacity) and [`image`](Self::image) are that long, and a save whose
    /// chain runs past the end of it is refused as broken, since the console would have written
    /// it over the start of the card.
    pub fn parse_within(bytes: &[u8], medium: usize) -> Result<Self> {
        if Geometry::of(medium).is_none() {
            return Err(Error::UnknownSize(medium));
        }
        Self::read(bytes, Some(medium))
    }

    fn read(bytes: &[u8], medium: Option<usize>) -> Result<Self> {
        let (container, card, backup_ram) = strip_container(bytes).ok_or(Error::WrongLength(bytes.len()))?;
        if !has_magic(&card) {
            return Err(Error::NotAMemoryCard);
        }

        let word = u16::from_be_bytes([card[SIZE_OFFSET], card[SIZE_OFFSET + 1]]);
        let geometry = Geometry::from_size_word(word).ok_or(Error::UnknownSize(word as usize))?;
        let medium = medium.unwrap_or(geometry.size);
        if card.len() < medium {
            return Err(Error::Corrupt(format!(
                "the card is {medium} bytes and the image holds {}",
                card.len()
            )));
        }
        if geometry.first_data_block * BLOCK > medium {
            return Err(Error::Corrupt(format!(
                "the card's header claims {} bytes, and its directory and tables alone run past the {medium} \
                 the medium holds",
                geometry.size
            )));
        }
        let card = card[..medium].to_vec();

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
    ///
    /// As long as the medium it was read off: see [`capacity`](Self::capacity).
    #[must_use]
    pub fn image(&self) -> &[u8] {
        &self.card
    }

    /// What the image arrived in.
    #[must_use]
    pub fn container(&self) -> Container {
        self.container
    }

    /// The cabinet's backup RAM, when the container carried one, in the byte order the 68000 sees.
    ///
    /// This is a *different* save from the card — an MVS cabinet keeps its own — and it is here so
    /// that reading a MiSTer file does not quietly drop half of it. [`cabinet`] reads what the
    /// BIOS keeps in it.
    #[must_use]
    pub fn backup_ram(&self) -> Option<&[u8]> {
        self.backup_ram.as_deref()
    }

    /// Where the card's regions sit, as its header lays them out.
    #[must_use]
    pub fn geometry(&self) -> Geometry {
        self.geometry
    }

    /// The card's capacity in bytes, structure included.
    ///
    /// What the header states, unless the card was read off a smaller medium with
    /// [`parse_within`](Self::parse_within), in which case it is the medium's.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.card.len()
    }

    /// The header block: what the card says about itself.
    ///
    /// A builder keeps it and rewrites only the fields that follow from the card it builds, the
    /// size, both checksums and the signature, so keeping this is what carries a card's username
    /// and region across a round trip. See [`HEADER_LEN`].
    #[must_use]
    pub fn system_area(&self) -> &[u8] {
        &self.card[..HEADER_LEN]
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
///
/// A MiSTer save counts as well when an MVS BIOS set up its cabinet RAM, card or no card: the
/// RAM is half of what the file holds, and the region beside it may be a cart's own RAM or a card
/// nothing has formatted yet.
#[must_use]
pub fn detect(bytes: &[u8]) -> bool {
    matches!(strip_container(bytes), Some((_, card, _)) if has_magic(&card))
        || split_mister(bytes).is_some_and(|(ram, _)| cabinet::is_set_up(&ram))
}

/// Finds the card inside whatever container it arrived in, and the backup RAM beside it.
///
/// Both come back in the byte order the 68000 sees, whatever the container did to them.
#[must_use]
pub fn strip_container(bytes: &[u8]) -> Option<(Container, Vec<u8>, Option<Vec<u8>>)> {
    // A MiSTer save is the cabinet's backup RAM and then the card, word-swapped. It is checked
    // first because its card region is a plausible bare card length on its own.
    if bytes.len() > MISTER_BACKUP_RAM {
        let card = swap_pairs(&bytes[MISTER_BACKUP_RAM..]);
        if has_magic(&card) {
            return Some((Container::MiSter, card, Some(swap_pairs(&bytes[..MISTER_BACKUP_RAM]))));
        }
    }
    if has_magic(bytes) {
        return Some((Container::Bare, bytes.to_vec(), None));
    }
    None
}

/// A MiSTer save's two halves, the cabinet's backup RAM and the card region, each in the byte
/// order the 68000 sees. `None` for bytes that are not [`MISTER_SAVE`] long.
///
/// Nothing is asked of what either half holds. This is the container and only the container, for
/// a caller after a region that is not a card, as a Jockey Grand Prix save's is.
#[must_use]
pub fn split_mister(bytes: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    (bytes.len() == MISTER_SAVE)
        .then(|| (swap_pairs(&bytes[..MISTER_BACKUP_RAM]), swap_pairs(&bytes[MISTER_BACKUP_RAM..])))
}

/// The two bytes of every 16-bit word swapped, which turns the MiSTer core's order into the
/// 68000's and back.
pub(crate) fn swap_pairs(bytes: &[u8]) -> Vec<u8> {
    (0..bytes.len() & !1).map(|i| bytes[i ^ 1]).collect()
}

/// Walks the directory, following each used entry's chain.
fn read_directory(card: &[u8], geometry: Geometry) -> Result<Vec<Save>> {
    let directory = &card[geometry.directory_block * BLOCK..][..geometry.entries * ENTRY_LEN];
    let fat = &card[geometry.fat1_block * BLOCK..][..geometry.fat_len()];
    // The header lays the card out, but the medium is what there is: a block past its end is one
    // the console would have found back at the start.
    let blocks = geometry.blocks.min(card.len() / BLOCK);

    let mut saves = Vec::new();
    for index in 0..geometry.entries {
        let entry = &directory[index * ENTRY_LEN..][..ENTRY_LEN];
        if entry[0] == FREE_ENTRY {
            continue;
        }
        let mut data = Vec::new();
        for block in follow_chain(fat, entry[3] as usize, geometry.first_data_block, blocks, index)? {
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
///
/// Every block has to lie in `first_data_block..blocks`.
fn follow_chain(
    fat: &[u8],
    first: usize,
    first_data_block: usize,
    blocks: usize,
    index: usize,
) -> Result<Vec<usize>> {
    let mut chain = Vec::new();
    let mut current = first;
    loop {
        if current < first_data_block || current >= blocks {
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
