//! Save memory as each core keeps it in its file.
//!
//! A bundle carries a cartridge's save memory and nothing else, in the order the console's hardware
//! addresses it. Cores keep it in files of their own shapes, and the shapes disagree for two
//! systems in ways that stop a save moving between them:
//!
//! - **Mega Drive.** Most cartridges wire their SRAM to the odd byte lane of the 16-bit bus alone,
//!   so it sits at every other address. Genesis Plus GX, PicoDrive and clownmdemu keep the whole
//!   address window, the other lane filled; BlastEm and MiSTer keep the SRAM's own bytes. A MiSTer
//!   handed Genesis Plus GX's file reads garbage, and the reverse is as bad. An EEPROM is kept as
//!   its own bytes by every one of them.
//! - **GBA.** gpSP and SkyEmu pad every save to 128 KiB, SkyEmu keeps an EEPROM's eight-byte blocks
//!   back to front, VBA Next keeps an EEPROM 128 KiB into its file, and MiSTer's core reads a 4 Kbit
//!   EEPROM only from a file of 8 KiB.
//!
//! The registry records each core's [`SaveLayout`]. [`normalize`] takes a core's file to the
//! memory, and [`lay_out`] the memory to a core's file.
//!
//! Neither can always tell what a cartridge keeps from the bytes. Spreading a Mega Drive save out
//! needs to know whether it is SRAM or an EEPROM, which the cartridge's header says
//! ([`SaveMemory`], from [`rom::identify`](crate::rom::identify)) and the save does not. Without it,
//! a save of 256 bytes or less is taken for an EEPROM, which covers the 128- and 256-byte chips
//! nearly every EEPROM cartridge carries, and anything longer for SRAM on the odd lane, which is
//! most of the rest. A larger EEPROM (Frank Thomas Big Hurt Baseball's 8 KiB) needs its ROM to move
//! into an expanding core's file.

use one_saves_registry::{ExpandedLength, SaveLayout};

/// What a cartridge keeps its saves in, as far as laying them out needs to know, from its header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveMemory {
    /// SRAM on one byte lane of a 16-bit bus: most Mega Drive SRAM, on the odd lane.
    ByteLane {
        /// Whether it is on the odd lane, as nearly all of it is.
        odd: bool,
        /// Its size, half the range the header declares.
        size: Option<usize>,
    },
    /// A serial EEPROM, read a bit at a time and kept as its own bytes by every core.
    Eeprom {
        /// Its size, where the header says: a GBA's does not say whether it is 512 bytes or 8 KiB.
        size: Option<usize>,
    },
    /// Memory as wide as the bus: Mega Drive SRAM on both lanes, a GBA's SRAM or flash.
    Memory {
        /// Its size, where the header says.
        size: Option<usize>,
    },
}

impl SaveMemory {
    fn is_eeprom(self) -> bool {
        matches!(self, SaveMemory::Eeprom { .. })
    }

    fn size(self) -> Option<usize> {
        match self {
            SaveMemory::ByteLane { size, .. } | SaveMemory::Eeprom { size } | SaveMemory::Memory { size } => {
                size
            }
        }
    }
}

/// A file laid out for a core, and whether what the cartridge keeps was assumed rather than read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Laid {
    /// The file's bytes.
    pub bytes: Vec<u8>,
    /// Whether the layout depended on what the cartridge keeps, and nothing said: a Mega Drive
    /// save spread out for an expanding core with no [`SaveMemory`] given.
    pub assumed: bool,
}

/// The address window a Mega Drive cartridge's save sits in, from 0x200000.
const WINDOW: usize = 0x1_0000;

/// The longest save taken for an EEPROM where nothing says: Sega's and EA's 24C01 and Acclaim's
/// 24C02.
const SMALL_EEPROM: usize = 256;

/// What a GBA chip holds where nothing has been written.
const ERASED: u8 = 0xFF;

/// VBA Next's file: 128 KiB for a chip at the start, then 8 KiB for an EEPROM.
const VBA_NEXT_LEN: usize = 0x2_2000;
const VBA_NEXT_EEPROM: usize = 0x2_0000;

/// A GBA EEPROM's two sizes.
const GBA_EEPROM_SMALL: usize = 512;
const GBA_EEPROM_LARGE: usize = 0x2000;

/// The memory a core's file holds, from the file as `layout` keeps it.
///
/// What a file shows is believed only where one reading fits: a Mega Drive file is packed only
/// when one lane holds the core's fill throughout, and a GBA file is cut to its chip only when the
/// header gives the chip's size or the padding shows where it ends. Anything else comes back as
/// it was, which is what a file a core keeps as the memory does too. Normalizing memory that is
/// already normal changes nothing.
#[must_use]
pub fn normalize(
    file: &[u8],
    system: &str,
    layout: Option<SaveLayout>,
    memory: Option<SaveMemory>,
) -> Vec<u8> {
    match (system, layout) {
        ("genesis", Some(layout)) => normalize_genesis(file, layout, memory, true),
        ("gba", Some(layout)) => normalize_gba(file, layout, memory),
        _ => file.to_vec(),
    }
}

/// The memory a bundle holds, for one written before bundles held the memory alone: what
/// [`normalize`] makes of a file in `layout`, believing only what no memory could be mistaken for.
///
/// A bundle from before layouts holds the producer's file as it was, and one from since holds the
/// memory, and nothing in either says which. So a payload is taken for a file only where the file's
/// shape shows: a Mega Drive window with one lane all fill, or a GBA file of a length no chip has.
/// What a file's length alone suggests, PicoDrive's 8 KiB EEPROM file, is no more than an 8 KiB
/// SRAM's length, and is left alone.
#[must_use]
pub fn normalize_stored(
    payload: &[u8],
    system: &str,
    layout: Option<SaveLayout>,
    memory: Option<SaveMemory>,
) -> Vec<u8> {
    match (system, layout) {
        ("genesis", Some(layout)) => normalize_genesis(payload, layout, memory, false),
        ("gba", Some(layout)) => normalize_gba(payload, layout, memory),
        _ => payload.to_vec(),
    }
}

/// The file a core keeps `memory` in, as `layout` says.
///
/// `memory` is what the cartridge keeps, where its header says. See the module's notes for what is
/// assumed without it.
#[must_use]
pub fn lay_out(payload: &[u8], system: &str, layout: Option<SaveLayout>, memory: Option<SaveMemory>) -> Laid {
    match (system, layout) {
        ("genesis", Some(layout)) => lay_out_genesis(payload, layout, memory),
        ("gba", Some(layout)) => Laid { bytes: lay_out_gba(payload, layout, memory), assumed: false },
        _ => Laid { bytes: payload.to_vec(), assumed: false },
    }
}

/// Which lane holds data in a file where the other holds `fill` throughout: `Some(true)` for the
/// odd one. A file of nothing but `fill` is an empty save on the odd lane.
fn expanded_lane(bytes: &[u8], fill: u8) -> Option<bool> {
    if bytes.len() < 2 {
        return None;
    }
    let even_filled = bytes.iter().step_by(2).all(|&b| b == fill);
    let odd_filled = bytes.iter().skip(1).step_by(2).all(|&b| b == fill);
    match (even_filled, odd_filled) {
        (true, _) => Some(true),
        (false, true) => Some(false),
        (false, false) => None,
    }
}

/// A length the registry gives, in bytes.
fn len(n: u32) -> usize {
    usize::try_from(n).expect("a 32-bit length fits")
}

/// `bytes` with a trailing run of `fill` cut off.
fn trimmed(mut bytes: Vec<u8>, fill: u8) -> Vec<u8> {
    while bytes.last() == Some(&fill) {
        bytes.pop();
    }
    bytes
}

/// `bytes` cut or padded with `fill` to `len`.
fn sized(mut bytes: Vec<u8>, len: usize, fill: u8) -> Vec<u8> {
    bytes.resize(len, fill);
    bytes
}

fn normalize_genesis(
    file: &[u8],
    layout: SaveLayout,
    memory: Option<SaveMemory>,
    from_file: bool,
) -> Vec<u8> {
    match layout {
        SaveLayout::Expanded { fill, eeprom_length, .. } => {
            let eeprom_length = eeprom_length.map(len);
            let spread = !matches!(memory, Some(SaveMemory::Eeprom { .. } | SaveMemory::Memory { .. }))
                && (memory.is_some() || file.len() > SMALL_EEPROM);
            let lane = spread.then(|| expanded_lane(file, fill)).flatten();
            let lane = match (lane, memory) {
                (Some(_), Some(SaveMemory::ByteLane { odd, .. })) => Some(odd),
                (lane, _) => lane,
            };
            if let Some(odd) = lane {
                let mut file = file.to_vec();
                if file.len() % 2 == 1 {
                    file.push(fill);
                }
                let packed: Vec<u8> = file.iter().skip(usize::from(odd)).step_by(2).copied().collect();
                // A trimmed file stops at its last write, and the core reads what is past it as its
                // fill; the header's size says how far that runs.
                return match memory.and_then(SaveMemory::size) {
                    Some(size) => sized(packed, size, fill),
                    None => packed,
                };
            }
            // An EEPROM a core pads to one size: what is past the chip is the pad, and the header
            // says where the chip ends, or failing that its last byte that is not the pad.
            if from_file && eeprom_length == Some(file.len()) || memory.is_some_and(SaveMemory::is_eeprom) {
                return match memory.and_then(SaveMemory::size) {
                    Some(size) if size < file.len() => file[..size].to_vec(),
                    None if eeprom_length == Some(file.len()) => trimmed(file.to_vec(), fill),
                    _ => file.to_vec(),
                };
            }
            file.to_vec()
        }
        SaveLayout::Packed { .. } => match memory.and_then(SaveMemory::size) {
            Some(size) if size < file.len() => file[..size].to_vec(),
            _ => file.to_vec(),
        },
        _ => file.to_vec(),
    }
}

fn lay_out_genesis(payload: &[u8], layout: SaveLayout, memory: Option<SaveMemory>) -> Laid {
    match layout {
        SaveLayout::Packed { length: Some(length) } if payload.len() < len(length) => {
            Laid { bytes: sized(payload.to_vec(), len(length), ERASED), assumed: false }
        }
        SaveLayout::Expanded { fill, length, eeprom_length } => {
            let assumed = memory.is_none();
            let memory = memory.unwrap_or(if payload.len() <= SMALL_EEPROM {
                SaveMemory::Eeprom { size: None }
            } else {
                SaveMemory::ByteLane { odd: true, size: None }
            });
            let SaveMemory::ByteLane { odd, .. } = memory else {
                // Kept as its own bytes. A core that pads an EEPROM pads it, and one that trims its
                // file trims this too.
                let bytes = match (eeprom_length, length) {
                    (Some(eeprom), _) => sized(payload.to_vec(), len(eeprom), fill),
                    (None, ExpandedLength::Trimmed) => trimmed(payload.to_vec(), ERASED),
                    (None, ExpandedLength::Fixed(length)) => sized(payload.to_vec(), len(length), fill),
                    (None, ExpandedLength::Doubled) => payload.to_vec(),
                };
                return Laid { bytes, assumed };
            };
            // The window is 64 KiB, so a lane holds 32 KiB: anything past that in the memory is a
            // file's own padding, as on a MiSTer's 64 KiB file.
            let packed = &payload[..payload.len().min(WINDOW / 2)];
            let spread: Vec<u8> =
                packed.iter().flat_map(|&byte| if odd { [fill, byte] } else { [byte, fill] }).collect();
            let bytes = match length {
                ExpandedLength::Trimmed => trimmed(spread, ERASED),
                ExpandedLength::Doubled => spread,
                ExpandedLength::Fixed(length) => sized(spread, len(length), fill),
            };
            Laid { bytes, assumed }
        }
        _ => Laid { bytes: payload.to_vec(), assumed: false },
    }
}

/// Whether a GBA save is an EEPROM's: what the header says, or failing that the two sizes no other
/// chip has.
fn gba_eeprom(payload: &[u8], memory: Option<SaveMemory>) -> bool {
    match memory {
        Some(memory) => memory.is_eeprom(),
        None => matches!(payload.len(), GBA_EEPROM_SMALL | GBA_EEPROM_LARGE),
    }
}

/// An EEPROM's size, cut from a run that may be longer: the header's, or 512 bytes where all that
/// follows them is erased.
fn gba_eeprom_size(region: &[u8], memory: Option<SaveMemory>) -> usize {
    memory.and_then(SaveMemory::size).unwrap_or_else(|| {
        let large = region.len().min(GBA_EEPROM_LARGE);
        if region.len() > GBA_EEPROM_SMALL && region[GBA_EEPROM_SMALL..large].iter().all(|&b| b == ERASED) {
            GBA_EEPROM_SMALL
        } else {
            large
        }
    })
}

/// Each eight-byte block back to front, which undoes itself.
fn reverse_blocks(bytes: &[u8]) -> Vec<u8> {
    bytes.chunks(8).flat_map(|block| block.iter().rev().copied()).collect()
}

fn normalize_gba(file: &[u8], layout: SaveLayout, memory: Option<SaveMemory>) -> Vec<u8> {
    match layout {
        SaveLayout::Padded { length, reversed_eeprom } if file.len() == len(length) => match memory {
            Some(memory) if memory.is_eeprom() => {
                let chip = &file[..gba_eeprom_size(file, Some(memory)).min(file.len())];
                if reversed_eeprom { reverse_blocks(chip) } else { chip.to_vec() }
            }
            Some(memory) => match memory.size() {
                Some(size) if size < file.len() => file[..size].to_vec(),
                _ => file.to_vec(),
            },
            // Every chip fits the file, so the padding alone cannot say which this is.
            None => file.to_vec(),
        },
        SaveLayout::VbaNext if file.len() == VBA_NEXT_LEN => {
            let (front, eeprom) = file.split_at(VBA_NEXT_EEPROM);
            let erased = |bytes: &[u8]| bytes.iter().all(|&b| b == ERASED);
            if memory.is_some_and(SaveMemory::is_eeprom)
                || (memory.is_none() && erased(front) && !erased(eeprom))
            {
                eeprom[..gba_eeprom_size(eeprom, memory)].to_vec()
            } else {
                // The EEPROM's 8 KiB are VBA Next's own padding for any other chip.
                let size = memory.and_then(SaveMemory::size).unwrap_or(VBA_NEXT_EEPROM);
                front[..size.min(front.len())].to_vec()
            }
        }
        SaveLayout::Chip { .. } => match memory {
            Some(memory) if memory.is_eeprom() && file.len() == GBA_EEPROM_LARGE => {
                file[..gba_eeprom_size(file, Some(memory))].to_vec()
            }
            _ => file.to_vec(),
        },
        _ => file.to_vec(),
    }
}

fn lay_out_gba(payload: &[u8], layout: SaveLayout, memory: Option<SaveMemory>) -> Vec<u8> {
    let eeprom = gba_eeprom(payload, memory);
    match layout {
        SaveLayout::Chip { eeprom_min } if eeprom && payload.len() < len(eeprom_min) => {
            sized(payload.to_vec(), len(eeprom_min), ERASED)
        }
        SaveLayout::Padded { length, reversed_eeprom } => {
            let chip = if eeprom && reversed_eeprom { reverse_blocks(payload) } else { payload.to_vec() };
            if chip.len() < len(length) { sized(chip, len(length), ERASED) } else { chip }
        }
        SaveLayout::VbaNext if eeprom => {
            let mut file = vec![ERASED; VBA_NEXT_EEPROM];
            file.extend(sized(payload.to_vec(), GBA_EEPROM_LARGE, ERASED));
            file
        }
        SaveLayout::VbaNext if payload.len() < VBA_NEXT_LEN => sized(payload.to_vec(), VBA_NEXT_LEN, ERASED),
        _ => payload.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GPGX: SaveLayout =
        SaveLayout::Expanded { fill: 0xFF, length: ExpandedLength::Trimmed, eeprom_length: None };
    const PICODRIVE: SaveLayout =
        SaveLayout::Expanded { fill: 0x00, length: ExpandedLength::Doubled, eeprom_length: Some(8192) };
    const CLOWNMDEMU: SaveLayout =
        SaveLayout::Expanded { fill: 0x00, length: ExpandedLength::Fixed(65_536), eeprom_length: None };
    const MISTER_MD: SaveLayout = SaveLayout::Packed { length: Some(65_536) };
    const GPSP: SaveLayout = SaveLayout::Padded { length: 131_072, reversed_eeprom: false };
    const SKYEMU: SaveLayout = SaveLayout::Padded { length: 131_072, reversed_eeprom: true };
    const MISTER_GBA: SaveLayout = SaveLayout::Chip { eeprom_min: 8192 };

    /// An odd-lane SRAM of `len` bytes, its last byte not 0xFF.
    fn sram(len: usize) -> Vec<u8> {
        (0..len).map(|i| u8::try_from(i * 7 % 251).unwrap()).collect()
    }

    #[test]
    fn a_genesis_plus_gx_file_is_its_odd_bytes_and_goes_back_byte_for_byte() {
        let memory = sram(1000);
        let file = lay_out(&memory, "genesis", Some(GPGX), None);
        assert!(file.assumed, "nothing said it was SRAM");
        assert_eq!(file.bytes.len(), 2000, "ending at the last byte that is not 0xFF, on the odd lane");
        assert!(file.bytes.iter().step_by(2).all(|&b| b == 0xFF));
        assert_eq!(normalize(&file.bytes, "genesis", Some(GPGX), None), memory);
        // With the header's size, the run past the last write is the core's fill.
        let header = SaveMemory::ByteLane { odd: true, size: Some(1024) };
        let mut sized_memory = memory.clone();
        sized_memory.resize(1024, 0xFF);
        assert_eq!(normalize(&file.bytes, "genesis", Some(GPGX), Some(header)), sized_memory);
        assert_eq!(lay_out(&sized_memory, "genesis", Some(GPGX), Some(header)).bytes, file.bytes);
    }

    #[test]
    fn picodrive_and_clownmdemu_fill_the_even_lane_with_zeros() {
        let memory = sram(8192);
        let pico = lay_out(&memory, "genesis", Some(PICODRIVE), None).bytes;
        assert_eq!(pico.len(), 16_384);
        assert_eq!(normalize(&pico, "genesis", Some(PICODRIVE), None), memory);
        let clown = lay_out(&memory, "genesis", Some(CLOWNMDEMU), None).bytes;
        assert_eq!(clown.len(), 65_536);
        assert_eq!(&normalize(&clown, "genesis", Some(CLOWNMDEMU), None)[..8192], &memory[..]);
    }

    #[test]
    fn an_eeprom_is_never_spread_out() {
        let eeprom = sram(128);
        // Genesis Plus GX keeps it as it is, trimmed of a trailing erased run.
        assert_eq!(lay_out(&eeprom, "genesis", Some(GPGX), None).bytes, eeprom);
        assert_eq!(normalize(&eeprom, "genesis", Some(GPGX), None), eeprom);
        // PicoDrive pads it to 8 KiB with zeros, which come off again.
        let pico = lay_out(&eeprom, "genesis", Some(PICODRIVE), None).bytes;
        assert_eq!((pico.len(), &pico[..128]), (8192, &eeprom[..]));
        assert_eq!(normalize(&pico, "genesis", Some(PICODRIVE), None), eeprom);
        // A larger one is SRAM to the size rule, and an EEPROM to its header.
        let big = sram(8192);
        let header = Some(SaveMemory::Eeprom { size: Some(8192) });
        assert_eq!(lay_out(&big, "genesis", Some(GPGX), header).bytes, big);
        assert_ne!(lay_out(&big, "genesis", Some(GPGX), None).bytes, big);
    }

    #[test]
    fn a_mister_file_is_the_memory_padded_to_64_kib() {
        let memory = sram(32_768);
        let mister = lay_out(&memory, "genesis", Some(MISTER_MD), None).bytes;
        assert_eq!((mister.len(), &mister[..32_768]), (65_536, &memory[..]));
        // Its own padding comes off where the header says how much is the SRAM.
        let header = SaveMemory::ByteLane { odd: true, size: Some(32_768) };
        assert_eq!(normalize(&mister, "genesis", Some(MISTER_MD), Some(header)), memory);
        assert_eq!(normalize(&mister, "genesis", Some(MISTER_MD), None), mister, "and stays otherwise");
        // Spread out, the window holds 32 KiB of it.
        assert_eq!(lay_out(&mister, "genesis", Some(PICODRIVE), None).bytes.len(), 65_536);
    }

    #[test]
    fn memory_that_is_already_normal_stays_as_it_is() {
        let memory = sram(32_768);
        for layout in [GPGX, PICODRIVE, CLOWNMDEMU, MISTER_MD] {
            assert_eq!(normalize(&memory, "genesis", Some(layout), None), memory, "{layout:?}");
        }
        assert_eq!(normalize(&memory, "snes", Some(GPGX), None), memory, "another system has no layout");
    }

    #[test]
    fn a_gba_eeprom_goes_where_each_core_keeps_one() {
        let eeprom: Vec<u8> = (0..512u16).map(|i| u8::try_from(i % 256).unwrap()).collect();
        let mister = lay_out(&eeprom, "gba", Some(MISTER_GBA), None).bytes;
        assert_eq!((mister.len(), &mister[..512]), (8192, &eeprom[..]));
        assert!(mister[512..].iter().all(|&b| b == 0xFF));
        let header = Some(SaveMemory::Eeprom { size: None });
        assert_eq!(normalize(&mister, "gba", Some(MISTER_GBA), header), eeprom, "erased past 512 bytes");

        let gpsp = lay_out(&eeprom, "gba", Some(GPSP), None).bytes;
        assert_eq!(gpsp.len(), 131_072);
        assert_eq!(normalize(&gpsp, "gba", Some(GPSP), header), eeprom);
        assert_eq!(normalize(&gpsp, "gba", Some(GPSP), None), gpsp, "nothing says which chip");

        let skyemu = lay_out(&eeprom, "gba", Some(SKYEMU), None).bytes;
        assert_eq!(&skyemu[..8], &[7, 6, 5, 4, 3, 2, 1, 0], "each block back to front");
        assert_eq!(normalize(&skyemu, "gba", Some(SKYEMU), header), eeprom);

        let vba_next = lay_out(&eeprom, "gba", Some(SaveLayout::VbaNext), None).bytes;
        assert_eq!((vba_next.len(), &vba_next[0x2_0000..0x2_0200]), (0x2_2000, &eeprom[..]));
        assert_eq!(
            normalize(&vba_next, "gba", Some(SaveLayout::VbaNext), None),
            eeprom,
            "found by its place"
        );
    }

    #[test]
    fn other_gba_chips_sit_at_the_start() {
        let flash = sram(65_536);
        let vba_next = lay_out(&flash, "gba", Some(SaveLayout::VbaNext), None).bytes;
        assert_eq!((vba_next.len(), &vba_next[..65_536]), (0x2_2000, &flash[..]));
        let header = Some(SaveMemory::Memory { size: Some(65_536) });
        assert_eq!(normalize(&vba_next, "gba", Some(SaveLayout::VbaNext), header), flash);
        assert_eq!(normalize(&vba_next, "gba", Some(SaveLayout::VbaNext), None).len(), 0x2_0000);
        assert_eq!(lay_out(&flash, "gba", Some(MISTER_GBA), None).bytes, flash, "no EEPROM, no padding");
    }
}
