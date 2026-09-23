//! Reading and writing Sega Saturn backup RAM.
//!
//! The console keeps 32 KiB of battery-backed memory inside itself, and a Backup RAM Cart in the
//! cartridge slot extends that rather than replacing it. Both hold the same filesystem at
//! different scales, and this crate reads either.
//!
//! ```no_run
//! use saturn_backup::BackupRam;
//!
//! let volume = BackupRam::parse(&std::fs::read("save.bkr")?)?;
//! for save in volume.saves() {
//!     println!("{} ({} bytes): {}", save.name, save.data.len(), save.comment);
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # The volume states its own block size
//!
//! Every other card format here has a block size fixed by the format. This one does not: the
//! console's internal memory allocates in 64-byte blocks and a 512 KiB cart in 512-byte ones. The
//! volume says which, and it says so in the signature — `BackUpRam Format` is written over and
//! over to fill block 0 exactly, so the number of times it repeats **is** the block size divided
//! by sixteen. A 64-byte block carries it four times and a 512-byte block thirty-two.
//!
//! That is why [`Geometry::of`] needs no table of sizes. It reads the repeat count off the volume
//! rather than inferring a block size from the length, which matters because the same length can
//! be a cart of one geometry or another medium entirely.
//!
//! # A block is 60 bytes, not 64
//!
//! Every block opens with a four-byte tag, which leaves [`Geometry::content`] bytes for anything
//! else. The first block of a save carries [`SAVE_TAG`]; every later block of it carries
//! [`CONTINUATION_TAG`]. A reader that took the whole block would fold four bytes of tag into the
//! payload once per block, which is invisible on a save small enough to fit one block and wrong
//! on every save larger than that.
//!
//! # The directory is inside the save
//!
//! There is no directory region. A save's own first block carries its name, its comment, when it
//! was written, how long it is, and the list of every other block it occupies:
//!
//! | Offset | Bytes | What |
//! | ------ | ----- | ---- |
//! | `0x00` | 4     | [`SAVE_TAG`] |
//! | `0x04` | 11    | the name, padded with NUL |
//! | `0x0F` | 1     | the language the game wrote it under |
//! | `0x10` | 10    | the comment the BIOS lists beside the name |
//! | `0x1A` | 4     | when it was written, in minutes since 1980-01-01 |
//! | `0x1E` | 4     | how many bytes of it are the game's |
//! | `0x22` | …     | the block list, big-endian 16-bit, terminated by `0x0000` |
//!
//! The block list runs past the end of its own block for a save of any size, continuing in the
//! next block's content area — after that block's tag. The save's data begins immediately after
//! the terminator, wherever that lands, and runs on through every listed block.
//!
//! So the payload this crate hands back is the game's bytes and nothing else: the entry is not a
//! separate record that could be carried beside them. That is what makes a writer rewrite block
//! numbers rather than copy an entry — see [`BackupBuilder`].
//!
//! # What is reserved
//!
//! Blocks 0 and 1. Block 0 is the signature; block 1 is left alone, and the console allocates
//! from block 2 on both the internal memory and a cart. Nothing in the volume says so — it is
//! what two real volumes at two block sizes do, which is why [`Geometry::FIRST_DATA_BLOCK`] is
//! stated here rather than derived.
//!
//! # Not the `.BUP` file format
//!
//! A `.BUP` file is one save with a 64-byte header in front of it, which is what a save manager
//! exports. It is a different format that happens to describe the same save, and this crate does
//! not read one.

#![forbid(unsafe_code)]

mod build;
mod error;

pub use build::BackupBuilder;
pub use error::{Error, Result};

/// The signature a formatted volume opens with, repeated to fill block 0.
pub const MAGIC: &[u8; 16] = b"BackUpRam Format";

/// The tag on the first block of a save.
pub const SAVE_TAG: [u8; 4] = [0x80, 0x00, 0x00, 0x00];

/// The tag on every block of a save after the first.
pub const CONTINUATION_TAG: [u8; 4] = [0x00, 0x00, 0x00, 0x00];

/// How many bytes of a block the tag takes.
pub const TAG_LEN: usize = 4;

/// How long a save's name may be.
pub const NAME_LEN: usize = 11;

/// How long a save's comment may be.
pub const COMMENT_LEN: usize = 10;

/// How many bytes of a save's first block come before its block list.
pub const ENTRY_LEN: usize = 0x22;

/// The epoch a save's timestamp counts minutes from: 1980-01-01 00:00 UTC, as seconds.
pub const EPOCH_1980: i64 = 315_532_800;

/// The block sizes a volume is laid out in, smallest first.
///
/// The console's internal memory uses the first and a 512 KiB cart the fourth. The rest are here
/// because the block size scales with the medium and these are the steps it scales in; a volume
/// states which it uses, so this list is only ever a search space for
/// [`Geometry::for_data_capacity`], never an authority.
pub const BLOCK_SIZES: [usize; 6] = [64, 128, 256, 512, 1024, 2048];

/// The block size these bytes state, from how many times the signature repeats.
///
/// This is the one thing that settles a volume's geometry, and it works on the reserved region
/// alone as well as on a whole volume — which is what lets a writer recover the block size from a
/// card map's `system_area`.
#[must_use]
pub fn block_size(bytes: &[u8]) -> Option<usize> {
    let repeats = (0..bytes.len() / MAGIC.len())
        .take_while(|i| &bytes[i * MAGIC.len()..][..MAGIC.len()] == MAGIC)
        .count();
    (repeats > 0).then_some(repeats * MAGIC.len())
}

/// Where each region of a volume sits, worked out from the volume itself.
///
/// The block size is read off the signature rather than guessed from the length; see the module
/// documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    /// The volume's length in bytes.
    pub size: usize,
    /// The block size in bytes, which the volume states.
    pub block: usize,
    /// How many blocks it holds in total.
    pub blocks: usize,
}

impl Geometry {
    /// The first block a save can occupy. Blocks 0 and 1 are the volume's.
    pub const FIRST_DATA_BLOCK: usize = 2;

    /// The layout of the volume in these bytes, or `None` when they are not one.
    ///
    /// The block size is however many times [`MAGIC`] repeats at the front, times sixteen. A
    /// volume whose length is not a whole number of those blocks is not one this can lay out.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Option<Self> {
        let block = block_size(bytes)?;
        if !bytes.len().is_multiple_of(block) {
            return None;
        }
        let blocks = bytes.len() / block;
        if blocks <= Geometry::FIRST_DATA_BLOCK {
            return None;
        }
        Some(Geometry { size: bytes.len(), block, blocks })
    }

    /// The geometry whose saves can occupy this many bytes.
    ///
    /// The inverse of [`data_capacity`](Self::data_capacity), for a caller that recorded the
    /// usable capacity and nothing else. Unlike every other format here that inverse is not free:
    /// the block size is the volume's rather than the format's, so a capacity can in principle
    /// describe more than one geometry. Requiring the volume's own length to be a power of two
    /// settles it — every real one is — and an ambiguous capacity is `None` rather than a guess.
    ///
    /// Prefer [`block_size`] on a `system_area` where there is one: that is what the volume
    /// actually said, and this is what is left when nobody kept it.
    #[must_use]
    pub fn for_data_capacity(bytes: usize) -> Option<Self> {
        let mut found = None;
        for block in BLOCK_SIZES {
            if bytes == 0 || !bytes.is_multiple_of(block) {
                continue;
            }
            let blocks = bytes / block + Geometry::FIRST_DATA_BLOCK;
            let size = blocks * block;
            if !size.is_power_of_two() {
                continue;
            }
            if found.is_some() {
                return None;
            }
            found = Some(Geometry { size, block, blocks });
        }
        found
    }

    /// The layout of a volume of this length with this block size, for a builder starting fresh.
    #[must_use]
    pub const fn new(size: usize, block: usize) -> Option<Self> {
        if block == 0 || !block.is_multiple_of(MAGIC.len()) || !size.is_multiple_of(block) {
            return None;
        }
        let blocks = size / block;
        if blocks <= Geometry::FIRST_DATA_BLOCK {
            return None;
        }
        Some(Geometry { size, block, blocks })
    }

    /// How many bytes of a block are not its tag.
    #[must_use]
    pub const fn content(self) -> usize {
        self.block - TAG_LEN
    }

    /// How many bytes saves can occupy: every block past the reserved ones, tags and all.
    ///
    /// Tags included on purpose. This is the figure a card map's `capacity` carries, and that
    /// field is the medium's size rather than a budget for payloads — the same way a PS1 card
    /// states 131072 with its own header inside that.
    #[must_use]
    pub const fn data_capacity(self) -> usize {
        (self.blocks - Geometry::FIRST_DATA_BLOCK) * self.block
    }

    /// How many blocks a save of this many bytes needs, its entry and block list included.
    ///
    /// The list is part of what has to fit, and how long it is depends on how many blocks there
    /// are, so this settles at a fixed point rather than dividing once.
    #[must_use]
    pub const fn blocks_for(self, data: usize) -> usize {
        let mut blocks = 1;
        loop {
            // The entry and the list live in the same run as the data: the list holds every block
            // but the first, and the terminator closes it.
            let overhead = ENTRY_LEN - TAG_LEN + blocks * 2;
            let need = overhead + data;
            let have = blocks * self.content();
            if have >= need {
                return blocks;
            }
            blocks += 1;
        }
    }
}

/// One save on a volume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Save {
    /// The block its entry sits in, which is the first block it occupies.
    ///
    /// Read only: a [`BackupBuilder`] allocates for itself.
    pub block: usize,
    /// The name the console lists it under, such as `PANDRA_3_01`.
    pub name: String,
    /// The comment beside the name, such as `AZEL#1Lv01`.
    pub comment: String,
    /// The language byte the game wrote, kept as found.
    pub language: u8,
    /// When it was written, in minutes since 1980-01-01. [`Save::written_at`] puts it on the
    /// Unix scale.
    pub date: u32,
    /// The save's bytes: exactly what the game asked to store, with no tag and no entry in them.
    pub data: Vec<u8>,
    /// The entry's own bytes, verbatim: everything from the name to the start of the block list.
    ///
    /// [`ENTRY_LEN`] minus [`TAG_LEN`] bytes, or empty for a save built by hand rather than read
    /// off a volume. The fields above are this decoded, and this is kept beside them because
    /// decoding loses two things a round trip needs: the language byte, which nothing else here
    /// exposes, and whatever a game left in the padding of a field it did not fill. NiGHTS writes
    /// `A-life\0` into ten bytes of comment and leaves an `0xEE` in the eighth.
    ///
    /// Not position-dependent, which is the point: every field in it — name, language, comment,
    /// date, length — is the same wherever the save sits. Only the block list that follows it
    /// moves, so a writer can keep this whole and rebuild only the list.
    pub entry: Vec<u8>,
}

impl Save {
    /// When the save was written, in whole epoch seconds.
    ///
    /// Read as the console showed it. The Saturn keeps a wall clock with no notion of a zone, so
    /// this fixes a date and a time of day and says nothing about where on Earth that was.
    #[must_use]
    pub fn written_at(&self) -> i64 {
        EPOCH_1980 + i64::from(self.date) * 60
    }
}

/// A parsed backup RAM volume.
#[derive(Debug, Clone)]
pub struct BackupRam {
    image: Vec<u8>,
    geometry: Geometry,
    saves: Vec<Save>,
}

impl BackupRam {
    /// Reads a volume: the console's internal memory, or a Backup RAM Cart.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let geometry = match Geometry::of(bytes) {
            Some(geometry) => geometry,
            // Told apart so the caller learns which it was: a volume that opens with the
            // signature and does not divide into blocks is a different complaint from one that
            // never carried a signature at all.
            None if starts_with_magic(bytes) => return Err(Error::WrongLength(bytes.len())),
            None => return Err(Error::NotBackupRam),
        };

        let mut saves = Vec::new();
        for block in Geometry::FIRST_DATA_BLOCK..geometry.blocks {
            if tag_of(bytes, geometry, block) != SAVE_TAG {
                continue;
            }
            saves.push(read_save(bytes, geometry, block)?);
        }
        Ok(BackupRam { image: bytes.to_vec(), geometry, saves })
    }

    /// Every save on the volume, in block order.
    #[must_use]
    pub fn saves(&self) -> &[Save] {
        &self.saves
    }

    /// Takes the saves, for a caller that is going to rebuild rather than read on.
    #[must_use]
    pub fn into_saves(self) -> Vec<Save> {
        self.saves
    }

    /// The volume as it was read.
    #[must_use]
    pub fn image(&self) -> &[u8] {
        &self.image
    }

    /// Where the volume's blocks sit.
    #[must_use]
    pub fn geometry(&self) -> Geometry {
        self.geometry
    }

    /// How many bytes saves can occupy.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.geometry.data_capacity()
    }

    /// The reserved blocks: the signature and the block after it.
    ///
    /// A builder passes these through rather than reformatting, so whatever a console left in
    /// block 1 survives a round trip.
    #[must_use]
    pub fn system_area(&self) -> &[u8] {
        &self.image[..Geometry::FIRST_DATA_BLOCK * self.geometry.block]
    }
}

/// Whether these bytes open with the signature, whatever else is wrong with them.
#[must_use]
fn starts_with_magic(bytes: &[u8]) -> bool {
    bytes.len() >= MAGIC.len() && &bytes[..MAGIC.len()] == MAGIC
}

/// Whether these bytes are a backup RAM volume this crate can lay out.
#[must_use]
pub fn detect(bytes: &[u8]) -> bool {
    Geometry::of(bytes).is_some()
}

/// One block's tag.
fn tag_of(bytes: &[u8], geometry: Geometry, block: usize) -> [u8; 4] {
    let at = block * geometry.block;
    [bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]
}

/// A block's content: everything after its tag.
fn content_of(bytes: &[u8], geometry: Geometry, block: usize) -> &[u8] {
    &bytes[block * geometry.block + TAG_LEN..(block + 1) * geometry.block]
}

/// Reads the save whose entry sits in this block.
///
/// The block list is read from the same stream the data is, because the two are continuous: the
/// list runs until its terminator and the data starts at the next byte, wherever in whichever
/// block that lands.
fn read_save(bytes: &[u8], geometry: Geometry, first: usize) -> Result<Save> {
    let entry = &bytes[first * geometry.block..];
    let name = text(&entry[TAG_LEN..TAG_LEN + NAME_LEN]);
    let language = entry[0x0F];
    let comment = text(&entry[0x10..0x10 + COMMENT_LEN]);
    let date = u32::from_be_bytes([entry[0x1A], entry[0x1B], entry[0x1C], entry[0x1D]]);
    let size = u32::from_be_bytes([entry[0x1E], entry[0x1F], entry[0x20], entry[0x21]]) as usize;

    // The stream is this block's content from the end of the entry, then each listed block's
    // content in turn. Blocks join it as the list names them, which is what lets the list run
    // past the end of the block holding its own first half.
    let mut stream = content_of(bytes, geometry, first)[ENTRY_LEN - TAG_LEN..].to_vec();
    let mut listed = vec![first];
    let mut at = 0usize;
    loop {
        if at + 2 > stream.len() {
            return Err(Error::Corrupt(format!(
                "the block list in block {first} runs off the end of the blocks it names"
            )));
        }
        let next = usize::from(u16::from_be_bytes([stream[at], stream[at + 1]]));
        at += 2;
        if next == 0 {
            break;
        }
        if next < Geometry::FIRST_DATA_BLOCK || next >= geometry.blocks {
            return Err(Error::Corrupt(format!("the save in block {first} reaches block {next}")));
        }
        // A repeat would otherwise be an unbounded read, and a volume with one is broken whatever
        // it was meant to say.
        if listed.contains(&next) {
            return Err(Error::Corrupt(format!("the save in block {first} names block {next} twice")));
        }
        listed.push(next);
        stream.extend_from_slice(content_of(bytes, geometry, next));
    }

    if at + size > stream.len() {
        return Err(Error::Corrupt(format!(
            "the save in block {first} states {size} bytes and its blocks hold {}",
            stream.len().saturating_sub(at)
        )));
    }
    Ok(Save {
        block: first,
        name,
        comment,
        language,
        date,
        data: stream[at..at + size].to_vec(),
        entry: entry[TAG_LEN..ENTRY_LEN].to_vec(),
    })
}

/// A fixed-width text field, up to its first NUL.
///
/// Read as ASCII, which is what the console's own character set is over this range. A byte that
/// is not is kept as the replacement character rather than dropping the field: a name half-read
/// still tells two saves apart.
fn text(field: &[u8]) -> String {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_block_size_is_read_off_the_signature_and_not_the_length() {
        // The two real geometries: the console's 32 KiB in 64-byte blocks, a 512 KiB cart in
        // 512-byte ones. Same filesystem, and nothing but the repeat count says which.
        let internal = formatted(32_768, 64);
        let cart = formatted(524_288, 512);
        assert_eq!(Geometry::of(&internal).map(|g| g.block), Some(64));
        assert_eq!(Geometry::of(&cart).map(|g| g.block), Some(512));
        assert_eq!(Geometry::of(&internal).map(|g| g.blocks), Some(512));
        assert_eq!(Geometry::of(&cart).map(|g| g.blocks), Some(1024));
    }

    #[test]
    fn a_capacity_names_one_geometry_or_none() {
        // The two real volumes, recovered from capacity alone. The block size is the volume's
        // rather than the format's, so this is the inverse the specification flags as the open
        // question for 0.3 — a power-of-two length is what closes it.
        let internal = Geometry::for_data_capacity(32_640).expect("the console's own memory");
        assert_eq!((internal.size, internal.block, internal.blocks), (32_768, 64, 512));
        let cart = Geometry::for_data_capacity(523_264).expect("a 512 KiB cart");
        assert_eq!((cart.size, cart.block, cart.blocks), (524_288, 512, 1024));

        // Both round-trip against the figure they came from.
        for geometry in [internal, cart] {
            assert_eq!(Geometry::for_data_capacity(geometry.data_capacity()), Some(geometry));
        }
        assert_eq!(Geometry::for_data_capacity(0), None);
        assert_eq!(Geometry::for_data_capacity(1000), None, "no volume holds this");
    }

    #[test]
    fn the_reserved_region_states_the_block_size_on_its_own() {
        // A writer gets the reserved blocks back and nothing else, so the signature has to carry
        // the block size that far.
        for (size, block) in [(32_768usize, 64usize), (524_288, 512)] {
            let volume = formatted(size, block);
            let reserved = &volume[..Geometry::FIRST_DATA_BLOCK * block];
            assert_eq!(block_size(reserved), Some(block));
        }
        assert_eq!(block_size(&[0u8; 64]), None);
    }

    #[test]
    fn something_that_is_not_a_volume_is_refused() {
        assert!(matches!(BackupRam::parse(&[1, 2, 3]), Err(Error::NotBackupRam)));
        assert!(matches!(BackupRam::parse(&vec![0u8; 32_768]), Err(Error::NotBackupRam)));
        assert!(!detect(&[1, 2, 3]));
        // The signature is there and the length is not a whole number of blocks.
        let mut ragged = formatted(32_768, 64);
        ragged.truncate(32_768 - 8);
        assert!(matches!(BackupRam::parse(&ragged), Err(Error::WrongLength(_))));
    }

    #[test]
    fn a_block_holds_sixty_bytes_and_the_count_allows_for_its_own_list() {
        let g = Geometry::new(32_768, 64).expect("a real geometry");
        assert_eq!(g.content(), 60);
        // One block carries 60 bytes less the 30-byte entry and a 2-byte list: 28 bytes.
        assert_eq!(g.blocks_for(28), 1);
        assert_eq!(g.blocks_for(29), 2);
        // The real save: 1276 bytes over 23 blocks, which is what the console allocated.
        assert_eq!(g.blocks_for(1276), 23);
    }

    /// A formatted volume of `size` bytes with this block size, and nothing saved on it.
    pub(crate) fn formatted(size: usize, block: usize) -> Vec<u8> {
        let mut out = vec![0u8; size];
        for offset in (0..block).step_by(MAGIC.len()) {
            out[offset..offset + MAGIC.len()].copy_from_slice(MAGIC);
        }
        out
    }
}
