//! Reading and writing Nintendo 64DD disk images.
//!
//! A 64DD disk is 4316 blocks, numbered 0 to 4315 as the drive addresses them. A block's length
//! depends on the zone it sits in, from 19720 bytes on the outside of the disk to 9520 on the
//! inside, and the disk's type, 0 to 6, decides which zones the numbering crosses in which order.
//! The first 24 blocks are the system area: the record saying the disk's region, type and defect
//! tracks, the disk ID, and blocks the drive refuses to read. libleo numbers blocks from the 25th.
//!
//! ```no_run
//! use n64dd_disk::Disk;
//!
//! let image = std::fs::read("game.ndd")?;
//! let disk = Disk::parse(&image)?;
//! if let Some(area) = disk.ram_area() {
//!     std::fs::write("game.ram", area)?;
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Two layouts
//!
//! A **retail** dump, the form No-Intro catalogs, holds every block in block order, each at its
//! own length: 0x3DEC800 bytes. **MAME's** image holds the disk in physical order instead, head 0
//! then head 1, track by track, defect and spare tracks included: 0x435B0C0 bytes. Finding a block
//! in it takes the defect table the system area carries. [`Disk`] reads either, and converts one
//! into the other.
//!
//! # The RAM area
//!
//! A game writes its saves to the RAM area, which runs from a block the disk type fixes to the
//! disk's last. mupen64plus, Project64 and MiSTer keep that area alone as a `.ram` file, its
//! blocks in ascending order; [`Disk::ram_area`] reads it out of an image and
//! [`Disk::with_ram_area`] writes one back in. The rest of the disk is the game.
//!
//! The geometry is the one mupen64plus, ares, Project64 and MiSTer share, which LuigiBlood took
//! from the drive and libleo.

#![forbid(unsafe_code)]

use std::borrow::Cow;
use std::fmt;

/// A retail dump's length.
pub const RETAIL_LEN: usize = 0x03DE_C800;

/// The length of MAME's image of a disk.
pub const MAME_LEN: usize = 0x0435_B0C0;

/// How many blocks a disk has.
pub const BLOCKS: usize = 0x10DC;

/// How many blocks the system area takes, ahead of the first one libleo numbers.
pub const SYSTEM_BLOCKS: usize = 24;

/// The block holding the disk ID, a record repeated over the block's sectors.
pub const DISK_ID_BLOCK: usize = 14;

/// Where the disk ID starts in an image of each layout. It is in the first zone, ahead of any
/// defect track, so its place does not depend on the disk.
#[must_use]
pub fn disk_id_offset(layout: Layout) -> usize {
    match layout {
        Layout::Retail => DISK_ID_BLOCK * FIRST_ZONE_BLOCK,
        // Blocks 14 and 15 share a track, and MAME's image holds 15 first.
        Layout::Mame => (DISK_ID_BLOCK + 1) * FIRST_ZONE_BLOCK,
    }
}

/// How many sectors a block has.
pub const SECTORS_PER_BLOCK: usize = 85;

/// The length of the system record on a retail disk, which fills each sector of its block.
const SYSTEM_RECORD: usize = 0xE8;

/// The length of the system record on a development disk.
const DEV_SYSTEM_RECORD: usize = 0xC0;

/// A sector's length by physical zone: head 0's zones 0 to 7, then head 1's zones 1 to 8.
const SECTOR: [usize; 16] = [232, 216, 208, 192, 176, 160, 144, 128, 216, 208, 192, 176, 160, 144, 128, 112];

/// Where each virtual zone ends, in blocks, by disk type.
const VZONE_END: [[u16; 16]; 7] = [
    [
        0x0124, 0x0248, 0x035A, 0x047E, 0x05A2, 0x06B4, 0x07C6, 0x08D8, 0x09EA, 0x0AB6, 0x0B82, 0x0C94,
        0x0DA6, 0x0EB8, 0x0FCA, 0x10DC,
    ],
    [
        0x0124, 0x0248, 0x035A, 0x046C, 0x057E, 0x06A2, 0x07C6, 0x08D8, 0x09EA, 0x0AFC, 0x0BC8, 0x0C94,
        0x0DA6, 0x0EB8, 0x0FCA, 0x10DC,
    ],
    [
        0x0124, 0x0248, 0x035A, 0x046C, 0x057E, 0x0690, 0x07A2, 0x08C6, 0x09EA, 0x0AFC, 0x0C0E, 0x0CDA,
        0x0DA6, 0x0EB8, 0x0FCA, 0x10DC,
    ],
    [
        0x0124, 0x0248, 0x035A, 0x046C, 0x057E, 0x0690, 0x07A2, 0x08B4, 0x09C6, 0x0AEA, 0x0C0E, 0x0D20,
        0x0DEC, 0x0EB8, 0x0FCA, 0x10DC,
    ],
    [
        0x0124, 0x0248, 0x035A, 0x046C, 0x057E, 0x0690, 0x07A2, 0x08B4, 0x09C6, 0x0AD8, 0x0BEA, 0x0D0E,
        0x0E32, 0x0EFE, 0x0FCA, 0x10DC,
    ],
    [
        0x0124, 0x0248, 0x035A, 0x046C, 0x057E, 0x0690, 0x07A2, 0x086E, 0x0980, 0x0A92, 0x0BA4, 0x0CB6,
        0x0DC8, 0x0EEC, 0x1010, 0x10DC,
    ],
    [
        0x0124, 0x0248, 0x035A, 0x046C, 0x057E, 0x0690, 0x07A2, 0x086E, 0x093A, 0x0A4C, 0x0B5E, 0x0C70,
        0x0D82, 0x0E94, 0x0FB8, 0x10DC,
    ],
];

/// The physical zone each virtual zone is, by disk type.
const VZONE_TO_PZONE: [[u8; 16]; 7] = [
    [0, 1, 2, 9, 8, 3, 4, 5, 6, 7, 15, 14, 13, 12, 11, 10],
    [0, 1, 2, 3, 10, 9, 8, 4, 5, 6, 7, 15, 14, 13, 12, 11],
    [0, 1, 2, 3, 4, 11, 10, 9, 8, 5, 6, 7, 15, 14, 13, 12],
    [0, 1, 2, 3, 4, 5, 12, 11, 10, 9, 8, 6, 7, 15, 14, 13],
    [0, 1, 2, 3, 4, 5, 6, 13, 12, 11, 10, 9, 8, 7, 15, 14],
    [0, 1, 2, 3, 4, 5, 6, 7, 14, 13, 12, 11, 10, 9, 8, 15],
    [0, 1, 2, 3, 4, 5, 6, 7, 15, 14, 13, 12, 11, 10, 9, 8],
];

/// The outermost cylinder of each of head 0's zones, which head 1's zones share.
const OUTER_CYLINDER: [u16; 8] = [0x000, 0x09E, 0x13C, 0x1D1, 0x266, 0x2FB, 0x390, 0x425];

/// The cylinder head 1 starts each of its zones at, counting inwards from it.
const HEAD1_START: [u16; 8] = [0x091, 0x12F, 0x1C4, 0x259, 0x2EE, 0x383, 0x418, 0x48A];

/// Where each physical zone starts in MAME's image.
const MAME_ZONE_START: [usize; 16] = [
    0x000_0000, 0x05F_15E0, 0x0B7_9D00, 0x108_01A0, 0x152_3720, 0x196_3D80, 0x1D4_14C0, 0x20B_BCE0,
    0x231_96E0, 0x28A_1E00, 0x2DF_5DC0, 0x329_9340, 0x36D_99A0, 0x3AB_70E0, 0x3E3_1900, 0x414_9200,
];

/// The block each disk type's RAM area starts at. A type 6 disk has none, so its area starts at
/// the end.
const RAM_START: [usize; 7] = [0x5A2, 0x7C6, 0x9EA, 0xC0E, 0xE32, 0x1010, 0x10DC];

/// The blocks a system record may sit in: 0, 1, 8 and 9 on a retail disk, and 2, 3, 10 and 11 on
/// a development one. Each is in the first zone, where every block is the same length.
const SYSTEM_SLOTS: [usize; 8] = [0, 1, 8, 9, 2, 3, 10, 11];

/// A block's length in the first zone, where the system area is.
const FIRST_ZONE_BLOCK: usize = SECTOR[0] * SECTORS_PER_BLOCK;

/// What can be wrong with a disk image.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The image is neither a retail dump's length nor MAME's.
    WrongLength(usize),
    /// No block where a system record sits holds one.
    NoSystemArea,
    /// The system record's defect table runs past itself.
    Corrupt(String),
    /// A RAM area of the wrong length for the disk's type.
    WrongRamArea {
        /// How long the disk's type makes it.
        expected: usize,
        /// How long the one given is.
        found: usize,
    },
    /// A development disk, whose system area this does not convert between layouts.
    Development,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::WrongLength(len) => write!(
                f,
                "a 64DD disk image is {RETAIL_LEN} bytes as a retail dump or {MAME_LEN} as MAME's, and this is {len}"
            ),
            Error::NoSystemArea => f.write_str("no block of this image holds the disk's system record"),
            Error::Corrupt(why) => write!(f, "this disk's system area is corrupt: {why}"),
            Error::WrongRamArea { expected, found } => {
                write!(f, "this disk's RAM area is {expected} bytes, and the one given is {found}")
            }
            Error::Development => f.write_str(
                "this is a development disk, whose system area is laid out apart from a retail one's",
            ),
        }
    }
}

impl std::error::Error for Error {}

/// The result of reading a disk image.
pub type Result<T> = std::result::Result<T, Error>;

/// How an image lays the disk out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// Every block in block order, as a retail dump holds it.
    Retail,
    /// The disk in physical order, defect tracks included, as MAME's image holds it.
    Mame,
}

impl Layout {
    /// The layout an image this long has.
    #[must_use]
    pub fn of_len(len: usize) -> Option<Self> {
        match len {
            RETAIL_LEN => Some(Layout::Retail),
            MAME_LEN => Some(Layout::Mame),
            _ => None,
        }
    }

    /// How long an image in this layout is.
    #[must_use]
    pub fn image_len(self) -> usize {
        match self {
            Layout::Retail => RETAIL_LEN,
            Layout::Mame => MAME_LEN,
        }
    }
}

/// Which market a disk was pressed for, from the first word of its system record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    /// `E848D316`.
    Japan,
    /// `2263EE56`.
    Usa,
    /// All zeros: a development disk.
    Development,
}

/// The disk's system record: its region, its type and where its defect tracks are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemArea {
    /// The disk's region.
    pub region: Region,
    /// The disk's type, 0 to 6.
    pub disk_type: u8,
    /// Where each physical zone's list of defect tracks ends in [`defects`](Self::defects).
    pub defect_ends: [u8; 16],
    /// Each zone's defect tracks, as cylinders counted from the zone's outermost one.
    pub defects: [u8; 192],
}

impl SystemArea {
    /// Reads one system record, as the first sector of a system block holds it: the region, a
    /// disk type and an IPL the drive could load, or `None` for bytes that are no such record.
    ///
    /// What a probe of the image's first few hundred bytes can check; [`Disk::parse`] checks too
    /// that the record repeats over every sector of its block, as the drive writes it.
    #[must_use]
    pub fn from_record(record: &[u8]) -> Option<Self> {
        let region = match u32::from_be_bytes(record.get(..4)?.try_into().ok()?) {
            0xE848_D316 => Region::Japan,
            0x2263_EE56 => Region::Usa,
            0 => Region::Development,
            _ => return None,
        };
        let record =
            record.get(..if region == Region::Development { DEV_SYSTEM_RECORD } else { SYSTEM_RECORD })?;
        // The low nibble is the type; a retail disk sets 0x10 over it.
        if record[5] & 0xEF > 6 {
            return None;
        }
        let ipl_blocks = u16::from_be_bytes([record[6], record[7]]);
        let ipl_address = u32::from_be_bytes(record[0x1C..0x20].try_into().ok()?);
        if !(1..0x10C3).contains(&ipl_blocks) || !(0x8000_0000..0x8080_0000).contains(&ipl_address) {
            return None;
        }
        Some(SystemArea {
            region,
            disk_type: record[5] & 0x0F,
            defect_ends: record[8..0x18].try_into().ok()?,
            defects: record[0x20..0xE0].try_into().ok()?,
        })
    }

    /// Reads the record a block of the system area holds, checking it repeats over the block's
    /// sectors as the drive writes it.
    fn parse(block: &[u8], record_len: usize) -> Option<Self> {
        let area = SystemArea::from_record(block)?;
        if (area.region == Region::Development) != (record_len == DEV_SYSTEM_RECORD) {
            return None;
        }
        let record = &block[..record_len];
        (1..SECTORS_PER_BLOCK)
            .all(|sector| block.get(sector * record_len..(sector + 1) * record_len) == Some(record))
            .then_some(area)
    }

    /// Where a block sits on the disk: its physical zone, its cylinder and which of the track's
    /// two blocks it is, skipping the zone's defect tracks.
    fn physical(&self, block: usize) -> Result<(usize, u16, usize)> {
        let disk_type = usize::from(self.disk_type);
        let vzone = virtual_zone(self.disk_type, block);
        let pzone = usize::from(VZONE_TO_PZONE[disk_type][vzone]);
        let head1 = pzone > 7;
        let first = if vzone == 0 { 0 } else { usize::from(VZONE_END[disk_type][vzone - 1]) };
        let step = u16::try_from((block - first) >> 1).expect("a zone is shorter than that");
        let (outer, mut cylinder) = if head1 {
            (OUTER_CYLINDER[pzone - 8], HEAD1_START[pzone - 8] - step)
        } else {
            (OUTER_CYLINDER[pzone], OUTER_CYLINDER[pzone] + step)
        };
        let from = if pzone == 0 { 0 } else { usize::from(self.defect_ends[pzone - 1]) };
        let to = usize::from(self.defect_ends[pzone]);
        let defects = self
            .defects
            .get(from..to.max(from))
            .ok_or_else(|| Error::Corrupt(format!("zone {pzone}'s defect list runs past the table")))?;
        for &defect in defects {
            if u16::from(defect) + outer > cylinder {
                break;
            }
            cylinder += 1;
        }
        Ok((pzone, cylinder, usize::from(matches!(block & 3, 1 | 2))))
    }

    /// Where a block starts in MAME's image.
    fn mame_offset(&self, block: usize) -> Result<usize> {
        let (pzone, cylinder, which) = self.physical(block)?;
        let head = usize::from(pzone > 7);
        // The zone the cylinder falls in, on head 0's numbering, as MAME's image orders them.
        let zone = (1..8).rev().find(|&zone| cylinder >= OUTER_CYLINDER[zone]).unwrap_or(0);
        let index = zone + head * 8;
        let block_len = SECTOR[index] * SECTORS_PER_BLOCK;
        let track = usize::from(cylinder - OUTER_CYLINDER[zone]);
        Ok(MAME_ZONE_START[index] + track * 2 * block_len + which * block_len)
    }
}

/// The virtual zone a block is in, for a disk type.
fn virtual_zone(disk_type: u8, block: usize) -> usize {
    VZONE_END[usize::from(disk_type)]
        .iter()
        .position(|&end| block < usize::from(end))
        .expect("a block of the disk")
}

/// A block's length on a disk of this type.
#[must_use]
pub fn block_len(disk_type: u8, block: usize) -> usize {
    let pzone = usize::from(VZONE_TO_PZONE[usize::from(disk_type)][virtual_zone(disk_type, block)]);
    SECTOR[pzone] * SECTORS_PER_BLOCK
}

/// Where every block starts in a retail dump of a disk of this type, and where the last one ends.
#[must_use]
pub fn retail_offsets(disk_type: u8) -> Vec<usize> {
    let mut offsets = Vec::with_capacity(BLOCKS + 1);
    let mut at = 0;
    for block in 0..BLOCKS {
        offsets.push(at);
        at += block_len(disk_type, block);
    }
    offsets.push(at);
    offsets
}

/// Where a disk type's RAM area starts, as the drive numbers blocks, and how long it is in bytes.
/// `None` for a type 6 disk, which has none.
#[must_use]
pub fn ram_area_of(disk_type: u8) -> Option<(usize, usize)> {
    let start = *RAM_START.get(usize::from(disk_type))?;
    let len: usize = (start..BLOCKS).map(|block| block_len(disk_type, block)).sum();
    (len > 0).then_some((start, len))
}

/// A 64DD disk image, in either layout.
#[derive(Debug, Clone)]
pub struct Disk<'a> {
    image: &'a [u8],
    layout: Layout,
    system: SystemArea,
}

impl<'a> Disk<'a> {
    /// Reads an image: its layout from its length, and its system record from the first block of
    /// the system area that holds one.
    pub fn parse(image: &'a [u8]) -> Result<Self> {
        let layout = Layout::of_len(image.len()).ok_or(Error::WrongLength(image.len()))?;
        // Every block a system record sits in is in the first zone, at the same place in both
        // layouts but for 2 and 3, and 10 and 11, which MAME's image holds the other way round.
        // Trying each slot finds the record wherever it is.
        let system = SYSTEM_SLOTS
            .iter()
            .find_map(|&slot| {
                let block = &image[slot * FIRST_ZONE_BLOCK..(slot + 1) * FIRST_ZONE_BLOCK];
                let record = if slot & 2 == 0 { SYSTEM_RECORD } else { DEV_SYSTEM_RECORD };
                SystemArea::parse(block, record)
            })
            .ok_or(Error::NoSystemArea)?;
        let disk = Disk { image, layout, system };
        // A defect table that sends a block off the end of MAME's image is no table at all. It is
        // checked whichever layout this image is, since a retail one converts into MAME's too, and
        // the table is the system record's to state: what a file says, not what a disk can hold.
        for block in 0..BLOCKS {
            let at = disk.system.mame_offset(block)?;
            if at + block_len(disk.system.disk_type, block) > MAME_LEN {
                return Err(Error::Corrupt(format!(
                    "its defect table puts block {block} past the end of the disk"
                )));
            }
        }
        Ok(disk)
    }

    /// The image's layout.
    #[must_use]
    pub fn layout(&self) -> Layout {
        self.layout
    }

    /// The disk's system record.
    #[must_use]
    pub fn system_area(&self) -> &SystemArea {
        &self.system
    }

    /// The disk's type, 0 to 6.
    #[must_use]
    pub fn disk_type(&self) -> u8 {
        self.system.disk_type
    }

    /// Where a block starts in this image.
    fn offset(&self, block: usize, retail: &[usize]) -> usize {
        match self.layout {
            Layout::Retail => retail[block],
            Layout::Mame => self.system.mame_offset(block).expect("checked when the image was read"),
        }
    }

    /// A block's bytes, as the image holds them.
    #[must_use]
    pub fn block(&self, block: usize) -> &'a [u8] {
        let at = match self.layout {
            Layout::Retail => retail_offsets(self.system.disk_type)[block],
            Layout::Mame => self.system.mame_offset(block).expect("checked when the image was read"),
        };
        &self.image[at..at + block_len(self.system.disk_type, block)]
    }

    /// The disk ID's record: the first 0xE8 bytes of its block, which open with the game's code.
    #[must_use]
    pub fn disk_id(&self) -> &'a [u8] {
        &self.block(DISK_ID_BLOCK)[..SYSTEM_RECORD]
    }

    /// The RAM area, its blocks in ascending order, or `None` for a type 6 disk, which has none.
    #[must_use]
    pub fn ram_area(&self) -> Option<Vec<u8>> {
        let (start, len) = ram_area_of(self.system.disk_type)?;
        let retail = retail_offsets(self.system.disk_type);
        if self.layout == Layout::Retail {
            return Some(self.image[retail[start]..retail[start] + len].to_vec());
        }
        let mut area = Vec::with_capacity(len);
        for block in start..BLOCKS {
            let at = self.offset(block, &retail);
            area.extend_from_slice(&self.image[at..at + block_len(self.system.disk_type, block)]);
        }
        Some(area)
    }

    /// A copy of the image, in its own layout, holding `area` as its RAM area.
    pub fn with_ram_area(&self, area: &[u8]) -> Result<Vec<u8>> {
        let (start, len) = ram_area_of(self.system.disk_type)
            .ok_or(Error::WrongRamArea { expected: 0, found: area.len() })?;
        if area.len() != len {
            return Err(Error::WrongRamArea { expected: len, found: area.len() });
        }
        let retail = retail_offsets(self.system.disk_type);
        let mut image = self.image.to_vec();
        let mut from = 0;
        for block in start..BLOCKS {
            let block_len = block_len(self.system.disk_type, block);
            let at = self.offset(block, &retail);
            image[at..at + block_len].copy_from_slice(&area[from..from + block_len]);
            from += block_len;
        }
        Ok(image)
    }

    /// The disk as a retail dump holds it, which is the form No-Intro catalogs and a disk is
    /// hashed in. A retail image is itself; MAME's is put in block order, its defect and spare
    /// tracks left out.
    pub fn to_retail(&self) -> Result<Cow<'a, [u8]>> {
        if self.layout == Layout::Retail {
            return Ok(Cow::Borrowed(self.image));
        }
        if self.system.region == Region::Development {
            return Err(Error::Development);
        }
        let retail = retail_offsets(self.system.disk_type);
        let mut out = vec![0u8; RETAIL_LEN];
        for block in 0..BLOCKS {
            let len = block_len(self.system.disk_type, block);
            let at = self.offset(block, &retail);
            out[retail[block]..retail[block] + len].copy_from_slice(&self.image[at..at + len]);
        }
        Ok(Cow::Owned(out))
    }

    /// The disk as MAME's image holds it, its defect and spare tracks zero, as ares converts one.
    pub fn to_mame(&self) -> Result<Cow<'a, [u8]>> {
        if self.layout == Layout::Mame {
            return Ok(Cow::Borrowed(self.image));
        }
        if self.system.region == Region::Development {
            return Err(Error::Development);
        }
        let retail = retail_offsets(self.system.disk_type);
        let mut out = vec![0u8; MAME_LEN];
        for (block, &from) in retail.iter().enumerate().take(BLOCKS) {
            let len = block_len(self.system.disk_type, block);
            let at = self.system.mame_offset(block)?;
            out[at..at + len].copy_from_slice(&self.image[from..from + len]);
        }
        Ok(Cow::Owned(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A retail disk of `disk_type`, its system record in blocks 0, 1, 8 and 9, twelve defect
    /// tracks in each zone, and every other block filled with its own number so a block out of
    /// place shows.
    pub(crate) fn retail_disk(disk_type: u8) -> Vec<u8> {
        let offsets = retail_offsets(disk_type);
        let mut image = vec![0u8; RETAIL_LEN];
        for block in 0..BLOCKS {
            let tag = u16::try_from(block).unwrap().to_be_bytes();
            for (index, byte) in image[offsets[block]..offsets[block + 1]].iter_mut().enumerate() {
                *byte = tag[index % 2] ^ u8::try_from(index % 251).unwrap();
            }
        }
        let mut record = vec![0u8; SYSTEM_RECORD];
        record[..4].copy_from_slice(&0xE848_D316u32.to_be_bytes());
        record[4] = 0x10;
        record[5] = 0x10 | disk_type;
        record[6..8].copy_from_slice(&0x0018u16.to_be_bytes());
        for zone in 0..16 {
            record[8 + zone] = u8::try_from((zone + 1) * 12).unwrap();
            // Spread over the zone, as a pressing's defects are, and ascending as the drive lists
            // them. None falls on the system area's twelve tracks, which a disk that reads at all
            // has whole.
            let first = if zone == 0 { 12 } else { zone % 5 };
            for defect in 0..12 {
                record[0x20 + zone * 12 + defect] = u8::try_from(first + defect * 8).unwrap();
            }
        }
        record[0x18..0x1C].fill(0xFF);
        record[0x1C..0x20].copy_from_slice(&0x8000_0400u32.to_be_bytes());
        for block in [0, 1, 8, 9] {
            for sector in 0..SECTORS_PER_BLOCK {
                let at = offsets[block] + sector * SYSTEM_RECORD;
                image[at..at + SYSTEM_RECORD].copy_from_slice(&record);
            }
        }
        image
    }

    #[test]
    fn every_type_s_blocks_fill_a_retail_dump_exactly() {
        for disk_type in 0..7 {
            assert_eq!(retail_offsets(disk_type)[BLOCKS], RETAIL_LEN, "type {disk_type}");
        }
        // The spec's table of RAM areas, block for block and byte for byte.
        let areas: Vec<_> = (0..7).map(ram_area_of).collect();
        assert_eq!(
            areas,
            [
                Some((1418 + 24, 38_444_480)),
                Some((1966 + 24, 29_501_120)),
                Some((2514 + 24, 21_303_040)),
                Some((3062 + 24, 13_850_240)),
                Some((3610 + 24, 7_142_720)),
                Some((4088 + 24, 1_942_080)),
                None,
            ]
        );
    }

    #[test]
    fn a_disk_goes_into_mame_s_order_and_back_unchanged() {
        for disk_type in 0..6 {
            let image = retail_disk(disk_type);
            let disk = Disk::parse(&image).expect("reads");
            assert_eq!((disk.layout(), disk.disk_type()), (Layout::Retail, disk_type));
            let mame = disk.to_mame().unwrap().into_owned();
            let again = Disk::parse(&mame).expect("reads MAME's image");
            assert_eq!(again.layout(), Layout::Mame);
            for block in [0, 13, 14, 15, 24, 1000, BLOCKS - 1] {
                assert_eq!(again.block(block), disk.block(block), "type {disk_type} block {block}");
            }
            assert_eq!(again.ram_area(), disk.ram_area(), "type {disk_type}");
            assert!(again.to_retail().unwrap() == image, "type {disk_type}");
        }
    }

    #[test]
    fn mame_s_order_holds_two_blocks_a_track_and_swaps_every_other_pair() {
        let image = retail_disk(0);
        let disk = Disk::parse(&image).unwrap();
        // Blocks 2 and 3 share a track, and the drive reads 3 first; 0 and 1 stay in order.
        assert_eq!(disk.system.mame_offset(0), Ok(0));
        assert_eq!(disk.system.mame_offset(1), Ok(FIRST_ZONE_BLOCK));
        assert_eq!(disk.system.mame_offset(3), Ok(2 * FIRST_ZONE_BLOCK));
        assert_eq!(disk.system.mame_offset(2), Ok(3 * FIRST_ZONE_BLOCK));
    }

    #[test]
    fn a_ram_area_is_written_back_where_it_came_from() {
        let image = retail_disk(5);
        let mame = Disk::parse(&image).unwrap().to_mame().unwrap().into_owned();
        for copy in [&image, &mame] {
            let disk = Disk::parse(copy).unwrap();
            let area = vec![0x5A; 1_942_080];
            let written = disk.with_ram_area(&area).expect("writes");
            let again = Disk::parse(&written).unwrap();
            assert_eq!(again.ram_area().as_deref(), Some(&area[..]));
            assert_eq!(again.block(100), disk.block(100), "and nothing outside it");
            assert_eq!(
                disk.with_ram_area(&area[1..]),
                Err(Error::WrongRamArea { expected: 1_942_080, found: 1_942_079 })
            );
        }
    }

    #[test]
    fn the_disk_id_sits_where_each_layout_puts_it() {
        let image = retail_disk(2);
        let disk = Disk::parse(&image).unwrap();
        let mame = disk.to_mame().unwrap().into_owned();
        assert_eq!(disk_id_offset(Layout::Retail), 0x43670);
        assert_eq!(disk_id_offset(Layout::Mame), 0x48378);
        assert_eq!(&image[0x43670..0x43670 + 0xE8], disk.disk_id());
        assert_eq!(&mame[0x48378..0x48378 + 0xE8], disk.disk_id());
        assert_eq!(SystemArea::from_record(&image[..0x200]).as_ref(), Some(disk.system_area()));
    }

    /// A system record whose defect table pushes the last zone's blocks past the disk's last
    /// track, which a retail image can state as easily as MAME's, is refused before anything is
    /// read through it.
    #[test]
    fn a_defect_table_that_runs_off_the_disk_is_refused_in_either_layout() {
        let mut image = retail_disk(0);
        let offsets = retail_offsets(0);
        for block in [0, 1, 8, 9] {
            for sector in 0..SECTORS_PER_BLOCK {
                let record = &mut image[offsets[block] + sector * SYSTEM_RECORD..][..SYSTEM_RECORD];
                // Every one of the 192 defects in the innermost zone, at its first cylinder.
                record[8..0x17].fill(0);
                record[0x17] = 192;
                record[0x20..0xE0].fill(0);
            }
        }
        assert!(matches!(Disk::parse(&image), Err(Error::Corrupt(_))));
    }

    #[test]
    fn an_image_of_no_disk_is_refused() {
        assert_eq!(Disk::parse(&[0; 16]).unwrap_err(), Error::WrongLength(16));
        assert_eq!(Disk::parse(&vec![0; RETAIL_LEN]).unwrap_err(), Error::NoSystemArea);
    }
}
