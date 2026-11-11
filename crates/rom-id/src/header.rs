//! The headers: where in a file each console's sits, and one parser for each.
//!
//! Every parser checks what the console or its dumper checks before it believes a field. A title
//! read out of a file that is not that console's ROM is worse than none, since it looks
//! authoritative and is wrong.

use std::ops::Range;

use one_saves_registry::{System, systems};

use crate::{ByteOrder, Cartridge, Clock, Evidence, Lane, RomId, Save, SaveKind, View};

/// How much a header's check proves on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Strength {
    /// A logo, a magic word of four bytes or more, or a checksum the console verifies: enough to
    /// overrule the file's name.
    Strong,
    /// Two bytes, or a footer with only its shape to check: believed only where the name does not
    /// point at another system.
    Weak,
}

/// A parser: the file, and the systems its name allows, which only the Master System's needs.
pub(crate) type Parser = fn(View<'_>, &[System]) -> Option<RomId>;

/// Every parser, strongest first, so a weak check never shadows a strong one.
pub(crate) static PARSERS: &[(Strength, Parser)] = &[
    (Strength::Strong, n64),
    (Strength::Strong, ines),
    (Strength::Strong, unif),
    (Strength::Strong, fds),
    (Strength::Strong, lynx),
    (Strength::Strong, atari_7800),
    (Strength::Strong, c64_cartridge),
    (Strength::Strong, ncsd),
    (Strength::Strong, nds),
    (Strength::Strong, gba),
    (Strength::Strong, gameboy),
    (Strength::Strong, neogeo_pocket),
    (Strength::Strong, mega_drive),
    (Strength::Strong, master_system),
    (Strength::Strong, vectrex),
    (Strength::Weak, snes),
    (Strength::Weak, colecovision),
    (Strength::Weak, wonderswan),
];

/// The front of the file: every header that sits in its first 512 bytes, which is most of them.
const FRONT: u64 = 0x200;
/// Where a Master System or Game Gear header can sit, depending on the cartridge's size.
const SEGA_8BIT_HEADERS: [u64; 3] = [0x7FF0, 0x3FF0, 0x1FF0];
/// Where a Super Nintendo header sits: LoROM, HiROM, ExHiROM.
const SNES_HEADERS: [u64; 3] = [0x7FC0, 0xFFC0, 0x40_FFC0];
/// The copier header some SNES dumps carry, which pushes the real one back by its length.
const SNES_COPIER: u64 = 0x200;
/// The end of the file, where a WonderSwan keeps its header.
const TAIL: u64 = 16;
/// Where a 32X cartridge keeps the header its own boot code reads, past the Mega Drive's.
const MARS_HEADER: u64 = 0x3C0;

/// Every range a parser reads, clipped to the file and merged.
pub(crate) fn windows(len: u64) -> Vec<Range<u64>> {
    let mut ranges = Vec::with_capacity(12);
    ranges.push(0..FRONT);
    ranges.push(MARS_HEADER..MARS_HEADER + 16);
    ranges.extend(SEGA_8BIT_HEADERS.map(|at| at..at + 16));
    for base in [0, SNES_COPIER] {
        ranges.extend(SNES_HEADERS.map(|at| base + at..base + at + 0x20));
    }
    ranges.push(len.saturating_sub(TAIL)..len);

    let mut clipped: Vec<Range<u64>> = ranges
        .into_iter()
        .map(|range| range.start.min(len)..range.end.min(len))
        .filter(|r| !r.is_empty())
        .collect();
    clipped.sort_by_key(|range| range.start);
    let mut merged: Vec<Range<u64>> = Vec::with_capacity(clipped.len());
    for range in clipped {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

/// A ROM a header identified, before the header's own fields are filled in.
fn found(system: System, view: View<'_>) -> RomId {
    RomId {
        system,
        evidence: Evidence::Header,
        title: None,
        serial: None,
        revision: None,
        cartridge: Cartridge::default(),
        payload: 0..view.size(),
        byte_order: ByteOrder::Native,
    }
}

/// A fixed-width header field: up to the first NUL, trimmed, or `None` when that leaves nothing.
fn field(bytes: &[u8]) -> Option<String> {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let text = String::from_utf8_lossy(&bytes[..end]).trim().to_owned();
    (!text.is_empty()).then_some(text)
}

/// Battery-backed SRAM of a stated size.
fn sram(size: u32) -> Save {
    Save { kind: SaveKind::Sram, size: Some(size), lane: None }
}

/// Nintendo 64, in any of the three byte orders its dumps come in.
fn n64(view: View<'_>, _: &[System]) -> Option<RomId> {
    let head = view.bytes(0, 0x40)?;
    let (byte_order, header): (ByteOrder, Vec<u8>) = match u32::from_be_bytes(head[..4].try_into().ok()?) {
        0x8037_1240 => (ByteOrder::Native, head.to_vec()),
        0x3780_4012 => (ByteOrder::Swapped16, head.chunks(2).flat_map(|w| [w[1], w[0]]).collect()),
        0x4012_3780 => {
            (ByteOrder::Swapped32, head.chunks(4).flat_map(|w| [w[3], w[2], w[1], w[0]]).collect())
        }
        _ => return None,
    };
    // A homebrew ROM sets its game ID to `ED` and puts its save type and clock in the byte a retail
    // cartridge keeps its version in, since no retail game says either. Doubutsu no Mori, `NAFJ`,
    // is the one retail cartridge with a clock.
    let homebrew = &header[0x3C..0x3E] == b"ED";
    let flags = header[0x3F];
    let cartridge = if homebrew {
        let save = match flags >> 4 {
            1 => Some(Save { kind: SaveKind::Eeprom, size: Some(512), lane: None }),
            2 => Some(Save { kind: SaveKind::Eeprom, size: Some(2048), lane: None }),
            3 => Some(sram(32 * 1024)),
            4 => Some(sram(96 * 1024)),
            5 => Some(Save { kind: SaveKind::Flash, size: Some(128 * 1024), lane: None }),
            _ => None,
        };
        Cartridge { save, clock: (flags & 1 != 0).then_some(Clock::Joybus) }
    } else {
        Cartridge { save: None, clock: (&header[0x3C..0x3E] == b"AF").then_some(Clock::Joybus) }
    };
    Some(RomId {
        title: field(&header[0x20..0x34]),
        serial: field(&header[0x3B..0x3F]),
        revision: (!homebrew).then_some(flags),
        cartridge,
        byte_order,
        ..found(systems::N64, view)
    })
}

/// NES, in the iNES or NES 2.0 container.
///
/// The header describes the cartridge's wiring and says nothing about the game, so there is no
/// title or code to read; that needs a hash and a catalog, over the bytes after the header.
fn ines(view: View<'_>, _: &[System]) -> Option<RomId> {
    let header = view.bytes(0, 16)?;
    if &header[..4] != b"NES\x1A" {
        return None;
    }
    // Only NES 2.0 states the size of the battery-backed RAM; iNES says there is some.
    let nes2 = header[7] & 0x0C == 0x08;
    let size = if nes2 { (header[10] >> 4 != 0).then(|| 64 << (header[10] >> 4)) } else { None };
    let save = (header[6] & 0x02 != 0).then_some(Save { kind: SaveKind::Sram, size, lane: None });
    Some(RomId {
        cartridge: Cartridge { save, clock: None },
        payload: 16..view.size(),
        ..found(systems::NES, view)
    })
}

/// Famicom Disk System, as a raw disk or behind fwNES's header.
fn fds(view: View<'_>, _: &[System]) -> Option<RomId> {
    let header = view.bytes(0, 16)?;
    let payload = if &header[..4] == b"FDS\x1A" {
        16..view.size()
    } else if &header[..15] == b"\x01*NINTENDO-HVC*" {
        0..view.size()
    } else {
        return None;
    };
    Some(RomId { payload, ..found(systems::FDS, view) })
}

/// NES, in the UNIF container, which unlicensed and multicart dumps use for boards iNES had no
/// number for.
///
/// Its ROM is split across chunks rather than kept in one run, so the payload is the whole file;
/// no catalog hashes a UNIF anyway.
fn unif(view: View<'_>, _: &[System]) -> Option<RomId> {
    (view.bytes(0, 4)? == b"UNIF").then(|| found(systems::NES, view))
}

/// Atari Lynx, in Handy's `LYNX` container.
fn lynx(view: View<'_>, _: &[System]) -> Option<RomId> {
    let header = view.bytes(0, 64)?;
    if &header[..5] != b"LYNX\0" {
        return None;
    }
    Some(RomId {
        title: field(&header[0x0A..0x2A]),
        payload: 64..view.size(),
        ..found(systems::ATARI_LYNX, view)
    })
}

/// Atari 7800, in the `.a78` container.
fn atari_7800(view: View<'_>, _: &[System]) -> Option<RomId> {
    let header = view.bytes(0, 128)?;
    if &header[1..10] != b"ATARI7800" {
        return None;
    }
    Some(RomId {
        title: field(&header[0x11..0x31]),
        payload: 128..view.size(),
        ..found(systems::ATARI_7800, view)
    })
}

/// Commodore 64 cartridge, in VICE's `.crt` container, which is itself the format catalogs hash.
fn c64_cartridge(view: View<'_>, _: &[System]) -> Option<RomId> {
    let header = view.bytes(0, 0x40)?;
    if &header[..16] != b"C64 CARTRIDGE   " {
        return None;
    }
    Some(RomId { title: field(&header[0x20..0x40]), ..found(systems::C64, view) })
}

/// Nintendo 3DS cartridge image, whose NCSD header opens 0x100 bytes in.
fn ncsd(view: View<'_>, _: &[System]) -> Option<RomId> {
    (view.bytes(0x100, 4)? == b"NCSD").then(|| found(systems::_3DS, view))
}

/// Nintendo DS, whose header carries the logo's CRC and its own, both of which the console checks.
fn nds(view: View<'_>, _: &[System]) -> Option<RomId> {
    let header = view.bytes(0, 0x160)?;
    if u16::from_le_bytes([header[0x15C], header[0x15D]]) != 0xCF56
        || crc16(&header[..0x15E]) != u16::from_le_bytes([header[0x15E], header[0x15F]])
    {
        return None;
    }
    Some(RomId {
        title: field(&header[..0x0C]),
        serial: field(&header[0x0C..0x10]),
        revision: Some(header[0x1E]),
        ..found(systems::NDS, view)
    })
}

/// CRC-16/MODBUS, which is what a DS header's checksums are.
fn crc16(bytes: &[u8]) -> u16 {
    bytes.iter().fold(0xFFFF, |crc, &byte| {
        (0..8).fold(crc ^ u16::from(byte), |crc, _| if crc & 1 == 0 { crc >> 1 } else { crc >> 1 ^ 0xA001 })
    })
}

/// The opening bytes of the logo a GBA's BIOS checks at 0x04.
const GBA_LOGO: [u8; 8] = [0x24, 0xFF, 0xAE, 0x51, 0x69, 0x9A, 0xA2, 0x21];

/// The GBA games whose cartridges carry a Seiko S-3511A, by the first three letters of their game
/// code: Pokémon Ruby, Sapphire and Emerald, the three Boktai, Rockman EXE 4.5, Sennen Kazoku and
/// the two Legendz. The header says nothing about a clock, so the list is the only way to know;
/// it is the one mGBA keeps.
const GBA_CLOCKS: [&str; 10] = ["AXV", "AXP", "BPE", "U3I", "U32", "U33", "BR4", "BKA", "BLJ", "BLV"];

/// Game Boy Advance.
fn gba(view: View<'_>, _: &[System]) -> Option<RomId> {
    let header = view.bytes(0, 0xC0)?;
    // 0x96 at 0xB2 is the fixed byte the BIOS checks beside the logo.
    if header[0x04..0x0C] != GBA_LOGO || header[0xB2] != 0x96 {
        return None;
    }
    let serial = field(&header[0xAC..0xB0]);
    let clock = serial
        .as_deref()
        .and_then(|code| code.get(..3))
        .is_some_and(|game| GBA_CLOCKS.contains(&game))
        .then_some(Clock::S3511a);
    Some(RomId {
        title: field(&header[0xA0..0xAC]),
        serial,
        revision: Some(header[0xBC]),
        cartridge: Cartridge { save: None, clock },
        ..found(systems::GBA, view)
    })
}

/// The save chip a GBA ROM was built for, by the library it links: Nintendo's SDK leaves the
/// library's name and version in the ROM, `EEPROM_V124`, `SRAM_V113`, `FLASH1M_V103`.
///
/// It reads the whole ROM, which identifying one never does: a [`Probe`](crate::Probe) reads a few
/// windows and has to reach the same answer. So this is asked for apart, by a caller holding the
/// ROM in memory. An EEPROM's size is not in its name: 512 bytes and 8 KiB link the same library.
///
/// ```
/// let mut rom = vec![0u8; 0x400];
/// rom[0x200..0x20B].copy_from_slice(b"EEPROM_V124");
/// assert_eq!(rom_id::gba_save(&rom).map(|save| save.kind), Some(rom_id::SaveKind::Eeprom));
/// ```
#[must_use]
pub fn gba_save(rom: &[u8]) -> Option<Save> {
    const LIBRARIES: [(&[u8], SaveKind, Option<u32>); 5] = [
        (b"FLASH1M_V", SaveKind::Flash, Some(128 * 1024)),
        (b"FLASH512_V", SaveKind::Flash, Some(64 * 1024)),
        (b"FLASH_V", SaveKind::Flash, Some(64 * 1024)),
        (b"SRAM", SaveKind::Sram, Some(32 * 1024)),
        (b"EEPROM_V", SaveKind::Eeprom, None),
    ];
    let mut found = [false; LIBRARIES.len()];
    for at in 0..rom.len() {
        if !matches!(rom[at], b'E' | b'F' | b'S') {
            continue;
        }
        let rest = &rom[at..];
        // A version follows the name: three digits.
        let versioned = |name: &[u8]| {
            rest.starts_with(name)
                && rest.get(name.len()..name.len() + 3).is_some_and(|d| d.iter().all(u8::is_ascii_digit))
        };
        for (seen, (name, ..)) in found.iter_mut().zip(LIBRARIES) {
            // SRAM's library is SRAM_V or SRAM_F_V.
            *seen |= if name == b"SRAM" {
                versioned(b"SRAM_V") || versioned(b"SRAM_F_V")
            } else {
                versioned(name)
            };
        }
    }
    LIBRARIES.iter().zip(found).find(|(_, seen)| *seen).map(|((_, kind, size), _)| Save {
        kind: *kind,
        size: *size,
        lane: None,
    })
}

/// The opening bytes of the Nintendo logo every Game Boy cartridge carries at 0x104, checked by the
/// boot ROM.
const GB_LOGO: [u8; 8] = [0xCE, 0xED, 0x66, 0x66, 0xCC, 0x0D, 0x00, 0x0B];

/// Game Boy and Game Boy Color.
fn gameboy(view: View<'_>, named: &[System]) -> Option<RomId> {
    // 0x100 to 0x160: the header, and the four bytes past it TPP1 extends it by.
    let header = view.bytes(0x100, 0x60)?;
    if header[0x04..0x0C] != GB_LOGO {
        return None;
    }
    // The title field shrank as Nintendo took bytes off its end for other uses, so the CGB flag
    // sits where the last title byte used to. Reading 16 bytes on a CGB cartridge would append a
    // stray 0x80 or 0xC0 to the name.
    let cgb = matches!(header[0x43], 0x80 | 0xC0);
    let title_len = if cgb { 15 } else { 16 };
    // 0xC0 is a Color-only cartridge. 0x80 runs on both, so it is a Game Boy game where its name
    // says so — No-Intro files Pokémon Yellow there — and a Color one otherwise.
    let named_gb = named.contains(&systems::GB) && !named.contains(&systems::GBC);
    let gbc = match header[0x43] {
        0xC0 => true,
        0x80 => !named_gb,
        _ => false,
    };
    Some(RomId {
        title: field(&header[0x34..0x34 + title_len]),
        revision: Some(header[0x4C]),
        cartridge: gameboy_cartridge(header),
        ..found(if gbc { systems::GBC } else { systems::GB }, view)
    })
}

/// What a Game Boy cartridge holds, from its type and RAM size bytes. The types are the ones
/// Gameboy_MiSTer's `cart.v` and SameBoy decode.
fn gameboy_cartridge(header: &[u8]) -> Cartridge {
    // TPP1 sets the type, RAM size and destination bytes to values no other cartridge uses, and
    // states its real RAM size and features past the end of the usual header.
    if header[0x47] == 0xBC && header[0x49] == 0xC1 && header[0x4A] == 0x65 {
        let (ram, features) = (header[0x52], header[0x53]);
        let save = (features & 0x08 != 0 && ram != 0).then(|| sram(8192 << (ram - 1).min(8)));
        return Cartridge { save, clock: (features & 0x04 != 0).then_some(Clock::Tpp1) };
    }
    let ram = match header[0x49] {
        1 => Some(2 * 1024),
        2 => Some(8 * 1024),
        3 => Some(32 * 1024),
        4 => Some(128 * 1024),
        5 => Some(64 * 1024),
        _ => None,
    };
    let battery_ram = ram.map(sram);
    match header[0x47] {
        // MBC1, ROM, MMM01, MBC3, MBC5, MBC5 with rumble, the Pocket Camera and HuC-1, each with
        // RAM and a battery.
        0x03 | 0x09 | 0x0D | 0x13 | 0x1B | 0x1E | 0xFC | 0xFF => Cartridge { save: battery_ram, clock: None },
        // MBC2 keeps 512 four-bit cells of its own, whatever the RAM byte says.
        0x06 => Cartridge { save: Some(sram(512)), clock: None },
        // MBC3 with its timer, with or without RAM.
        0x0F | 0x10 => Cartridge { save: battery_ram, clock: Some(Clock::Mbc3) },
        // MBC7 keeps its saves in a 256-byte EEPROM.
        0x22 => Cartridge {
            save: Some(Save { kind: SaveKind::Eeprom, size: Some(256), lane: None }),
            clock: None,
        },
        // TAMA5, whose header states no memory at all.
        0xFD => Cartridge { save: None, clock: Some(Clock::Tc8521) },
        0xFE => Cartridge { save: battery_ram, clock: Some(Clock::Huc3) },
        _ => Cartridge::default(),
    }
}

/// Neo Geo Pocket and Pocket Color, whose BIOS checks the copyright line the header opens with.
fn neogeo_pocket(view: View<'_>, _: &[System]) -> Option<RomId> {
    let header = view.bytes(0, 0x30)?;
    if &header[..28] != b"COPYRIGHT BY SNK CORPORATION" && &header[..28] != b" LICENSED BY SNK CORPORATION" {
        return None;
    }
    // 0x10 says the game uses the Color's palette; 0x00 is a monochrome game.
    let system = if header[0x23] == 0x10 { systems::NGPC } else { systems::NGP };
    Some(RomId { title: field(&header[0x24..0x30]), ..found(system, view) })
}

/// Mega Drive, Genesis and 32X.
fn mega_drive(view: View<'_>, _: &[System]) -> Option<RomId> {
    let raw = view.bytes(0x100, 0x100)?;
    // Triple Play 96 and its Gold Edition hold their first 2 MiB byte-swapped, and their dumps keep
    // it so, which puts the header in swapped pairs. Only the header is read that way: the rest of
    // the file is not swapped, and the file as it is is what a catalog hashes.
    let swapped: Vec<u8>;
    let header = if raw.starts_with(b"ESAG") {
        swapped = raw.chunks(2).flat_map(|pair| [pair[1], pair[0]]).collect();
        &swapped[..]
    } else {
        raw
    };
    let console = &header[..16];
    // TMSS takes `SEGA` one byte in as well, and Namco's and Masaya's games put a space first.
    let sega = console.starts_with(b"SEGA") || console[1..].starts_with(b"SEGA");
    // The Pico shares the header and is no Mega Drive; the registry has no system for it, so it is
    // left to the name rather than filed under the wrong one.
    if !sega || console.windows(4).any(|w| w == b"PICO") {
        return None;
    }
    // Most 32X games say so in the console name, but the protos, the SDK builds and some retail
    // games (FIFA Soccer '96) give the Mega Drive's there. All of them carry the 32X's own header,
    // whose module name is the one the boot code checks for.
    let mars = view.bytes(MARS_HEADER, 15).is_some_and(|name| name == b"MARS CHECK MODE");
    let system =
        if mars || console.windows(3).any(|w| w == b"32X") { systems::SEGA_32X } else { systems::GENESIS };
    // The international name is the one in Latin script on a Japanese cartridge too.
    let title = field(&header[0x50..0x80]).or_else(|| field(&header[0x20..0x50]));
    Some(RomId {
        title,
        serial: field(&header[0x83..0x8E]),
        cartridge: Cartridge { save: backup_ram(&header[0xB0..0xBC]), clock: None },
        ..found(system, view)
    })
}

/// The backup RAM a Mega Drive header declares at 0x1B0: `RA`, a type byte, and the address range.
///
/// Bit 6 of the type says the RAM is kept by a battery, and bit 4 that it sits on one byte lane
/// only, odd or even addresses, so it holds half the range it spans.
fn backup_ram(record: &[u8]) -> Option<Save> {
    if &record[..2] != b"RA" || record[2] & 0x40 == 0 {
        return None;
    }
    let start = u32::from_be_bytes(record[4..8].try_into().ok()?);
    let end = u32::from_be_bytes(record[8..12].try_into().ok()?);
    let span = end.checked_sub(start)?.checked_add(1)?;
    // A serial EEPROM is two lines on the bus rather than a range of memory, and a cartridge
    // carrying one declares the word they sit in: Evander Holyfield's and Frank Thomas's
    // 200001-200001. Its size is the chip's, which the header does not give.
    if span <= 2 {
        return Some(Save { kind: SaveKind::Eeprom, size: None, lane: None });
    }
    // Bit 3 says which lane, the odd one when set.
    let lane = (record[2] & 0x10 != 0).then(|| if record[2] & 0x08 != 0 { Lane::Odd } else { Lane::Even });
    Some(Save { lane, ..sram(if lane.is_some() { span.div_ceil(2) } else { span }) })
}

/// Master System and Game Gear, which share the `TMR SEGA` header at one of three places.
///
/// The region nibble says which console the game is for. A Game Gear game that runs in the Master
/// System mode says Master System there, so where the name says Game Gear and only that, the name
/// is believed.
fn master_system(view: View<'_>, named: &[System]) -> Option<RomId> {
    SEGA_8BIT_HEADERS.into_iter().find_map(|at| {
        let header = view.bytes(at, 16)?;
        if &header[..8] != b"TMR SEGA" {
            return None;
        }
        let gear = matches!(header[15] >> 4, 5..=7)
            || (named.contains(&systems::SEGA_GG) && !named.contains(&systems::MASTERSYSTEM));
        let system = if gear { systems::SEGA_GG } else { systems::MASTERSYSTEM };
        Some(RomId { revision: Some(header[14] & 0x0F), ..found(system, view) })
    })
}

/// Vectrex, whose cartridges open with the copyright line the BIOS checks.
fn vectrex(view: View<'_>, _: &[System]) -> Option<RomId> {
    (view.bytes(0, 5)? == b"g GCE").then(|| found(systems::VECTREX, view))
}

/// Super Nintendo, whose header sits at one of three places and may be behind a copier header.
///
/// The checksum and its complement have to add up to all ones, which is what tells the header from
/// whatever sits at the other offsets. Two bytes prove little on their own, which is why this is a
/// weak check.
fn snes(view: View<'_>, _: &[System]) -> Option<RomId> {
    // A copier header shifts everything, and is detected by the file's length rather than by
    // anything in it.
    let base = if view.size() % 1024 == SNES_COPIER { SNES_COPIER } else { 0 };
    SNES_HEADERS.into_iter().find_map(|at| {
        let header = view.bytes(base + at, 0x20)?;
        let complement = u16::from_le_bytes([header[0x1C], header[0x1D]]);
        let checksum = u16::from_le_bytes([header[0x1E], header[0x1F]]);
        if complement ^ checksum != 0xFFFF {
            return None;
        }
        let (map, chips, ram) = (header[0x15], header[0x16], header[0x18]);
        // The two clocks, as Main_MiSTer's loader tells them: the S-RTC on an ExHiROM board, the
        // RTC-4513 beside an SPC7110.
        let clock = match (map, chips) {
            (0x35, 0x55) => Some(Clock::Srtc),
            (0x3A, 0xF9) => Some(Clock::Rtc4513),
            _ => None,
        };
        // The low nibble of the chip byte is the board: a battery on 2, 5, 6 and 9.
        let battery = matches!(chips & 0x0F, 2 | 5 | 6 | 9);
        let save = (battery && (1..=8).contains(&ram)).then(|| sram(1024 << ram));
        Some(RomId {
            title: field(&header[..21]),
            revision: Some(header[0x1B]),
            cartridge: Cartridge { save, clock },
            payload: base..view.size(),
            ..found(systems::SNES, view)
        })
    })
}

/// ColecoVision, whose BIOS starts a cartridge that opens with either byte order of `AA 55`.
fn colecovision(view: View<'_>, _: &[System]) -> Option<RomId> {
    matches!(view.bytes(0, 2)?, [0xAA, 0x55] | [0x55, 0xAA]).then(|| found(systems::COLECOVISION, view))
}

/// WonderSwan and WonderSwan Color, whose header is the last ten bytes of the ROM, behind the jump
/// the console starts at.
fn wonderswan(view: View<'_>, _: &[System]) -> Option<RomId> {
    let tail = view.tail(TAIL, 16)?;
    let footer = &tail[6..];
    // The far jump every cartridge's reset vector is, then the fields as WonderSwan_MiSTer reads
    // them: the Color flag, the save type, and the clock flag.
    if tail[0] != 0xEA || footer[1] > 1 || footer[7] & 0xFE != 0 {
        return None;
    }
    let save = match footer[5] {
        0x00 => None,
        0x01 => Some(sram(8 * 1024)),
        0x02 => Some(sram(32 * 1024)),
        0x03 => Some(sram(128 * 1024)),
        0x04 => Some(sram(256 * 1024)),
        0x05 => Some(sram(512 * 1024)),
        0x10 => Some(Save { kind: SaveKind::Eeprom, size: Some(128), lane: None }),
        0x20 => Some(Save { kind: SaveKind::Eeprom, size: Some(2048), lane: None }),
        0x50 => Some(Save { kind: SaveKind::Eeprom, size: Some(1024), lane: None }),
        _ => return None,
    };
    let system = if footer[1] == 1 { systems::WONDERSWAN_COLOR } else { systems::WONDERSWAN };
    Some(RomId {
        revision: Some(footer[3]),
        cartridge: Cartridge { save, clock: (footer[7] == 1).then_some(Clock::S3511a) },
        ..found(system, view)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Probe, identify_bytes};

    /// Identifies `rom` both ways — whole, and through the windows a probe asks for — and checks
    /// they agree, so no parser can read past what [`windows`] lists.
    fn identify(name: Option<&str>, rom: &[u8]) -> Option<RomId> {
        let mut probe = Probe::new(rom.len() as u64);
        for window in probe.windows() {
            probe.supply(
                window.start,
                rom[usize::try_from(window.start).unwrap()..usize::try_from(window.end).unwrap()].to_vec(),
            );
        }
        let whole = identify_bytes(name, rom);
        assert_eq!(probe.identify(name), whole, "the windows hold everything the parsers read");
        whole
    }

    fn gb_rom(kind: u8, ram: u8, cgb: u8) -> Vec<u8> {
        let mut rom = vec![0u8; 0x8000];
        rom[0x104..0x10C].copy_from_slice(&GB_LOGO);
        rom[0x134..0x13F].copy_from_slice(b"POKEMON SLV");
        rom[0x143] = cgb;
        rom[0x147] = kind;
        rom[0x149] = ram;
        rom
    }

    #[test]
    fn the_windows_are_a_kilobyte_whatever_the_rom() {
        for len in [0, 15, 0x200, 0x8000, 0x10_0000, 0x80_0000, 64 << 20] {
            let windows = windows(len);
            let total: u64 = windows.iter().map(|w| w.end - w.start).sum();
            assert!(total <= 1024, "{len}: {total} bytes");
            assert!(windows.iter().all(|w| w.end <= len), "{len}: nothing past the end");
            assert!(windows.windows(2).all(|pair| pair[0].end < pair[1].start), "{len}: merged and sorted");
        }
    }

    #[test]
    fn a_game_boy_cartridge_says_which_clock_it_has() {
        let crystal = identify(None, &gb_rom(0x10, 3, 0x80)).expect("a Game Boy Color ROM");
        assert_eq!(crystal.system, systems::GBC);
        assert_eq!(crystal.title.as_deref(), Some("POKEMON SLV"));
        assert_eq!(crystal.cartridge, Cartridge { save: Some(sram(32 * 1024)), clock: Some(Clock::Mbc3) });

        // A cartridge for both is a Game Boy game only where its name says so.
        assert_eq!(
            identify(Some("Pokemon - Yellow Version.gb"), &gb_rom(0x1B, 3, 0x80)).unwrap().system,
            systems::GB
        );
        assert_eq!(
            identify(Some("Pokemon - Silver Version.gbc"), &gb_rom(0x10, 3, 0x80)).unwrap().system,
            systems::GBC
        );
        assert_eq!(
            identify(Some("Shantae.gb"), &gb_rom(0x1B, 3, 0xC0)).unwrap().system,
            systems::GBC,
            "Color only"
        );

        let robopon = identify(None, &gb_rom(0xFE, 3, 0x00)).unwrap();
        assert_eq!(robopon.system, systems::GB);
        assert_eq!(robopon.cartridge.clock, Some(Clock::Huc3));
        assert_eq!(identify(None, &gb_rom(0xFD, 0, 0)).unwrap().cartridge.clock, Some(Clock::Tc8521));
        // An MBC3 without its timer is no clock, and a cartridge with no battery keeps no save.
        assert_eq!(identify(None, &gb_rom(0x13, 2, 0)).unwrap().cartridge.clock, None);
        assert_eq!(identify(None, &gb_rom(0x12, 2, 0)).unwrap().cartridge.save, None);
        assert_eq!(identify(None, &gb_rom(0x06, 0, 0)).unwrap().cartridge.save, Some(sram(512)));
    }

    #[test]
    fn tpp1_states_its_clock_past_the_usual_header() {
        let mut rom = gb_rom(0xBC, 0xC1, 0);
        rom[0x14A] = 0x65;
        rom[0x152] = 2;
        rom[0x153] = 0x0C;
        let cartridge = identify(None, &rom).unwrap().cartridge;
        assert_eq!(cartridge, Cartridge { save: Some(sram(16 * 1024)), clock: Some(Clock::Tpp1) });
        rom[0x153] = 0x08;
        assert_eq!(identify(None, &rom).unwrap().cartridge.clock, None, "the header did not ask for one");
    }

    #[test]
    fn a_gba_clock_comes_from_the_game_code() {
        let mut rom = vec![0u8; 0x200];
        rom[0x04..0x0C].copy_from_slice(&GBA_LOGO);
        rom[0xA0..0xAC].copy_from_slice(b"POKEMON EMER");
        rom[0xAC..0xB0].copy_from_slice(b"BPEE");
        rom[0xB2] = 0x96;
        let emerald = identify(None, &rom).expect("a GBA ROM");
        assert_eq!((emerald.system, emerald.serial.as_deref()), (systems::GBA, Some("BPEE")));
        assert_eq!(emerald.cartridge.clock, Some(Clock::S3511a));
        rom[0xAC..0xB0].copy_from_slice(b"BPRE");
        assert_eq!(identify(None, &rom).unwrap().cartridge.clock, None, "FireRed has none");
        // Without the logo the fixed byte proves nothing.
        rom[0x04] = 0;
        assert_eq!(identify(None, &rom), None);
    }

    #[test]
    fn an_n64_header_reads_the_same_in_all_three_byte_orders() {
        let mut native = vec![0u8; 0x1000];
        native[..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        native[0x20..0x2E].copy_from_slice(b"DOUBUTSUNOMORI");
        native[0x3B..0x3F].copy_from_slice(b"NAFJ");
        let swapped16: Vec<u8> = native.chunks(2).flat_map(|w| [w[1], w[0]]).collect();
        let swapped32: Vec<u8> = native.chunks(4).flat_map(|w| [w[3], w[2], w[1], w[0]]).collect();
        for (rom, order) in [
            (&native, ByteOrder::Native),
            (&swapped16, ByteOrder::Swapped16),
            (&swapped32, ByteOrder::Swapped32),
        ] {
            let id = identify(None, rom).expect("an N64 ROM");
            assert_eq!((id.title.as_deref(), id.serial.as_deref()), (Some("DOUBUTSUNOMORI"), Some("NAFJ")));
            assert_eq!((id.byte_order, id.cartridge.clock), (order, Some(Clock::Joybus)));
        }
    }

    #[test]
    fn a_homebrew_n64_header_says_its_save_and_clock() {
        let mut rom = vec![0u8; 0x1000];
        rom[..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        rom[0x3B..0x3E].copy_from_slice(b"NED");
        rom[0x3F] = 0x31;
        let id = identify(None, &rom).unwrap();
        assert_eq!(id.cartridge, Cartridge { save: Some(sram(32 * 1024)), clock: Some(Clock::Joybus) });
        assert_eq!(id.revision, None, "that byte is the flags on a homebrew ROM");
    }

    #[test]
    fn an_ines_dump_hashes_without_its_header() {
        let mut rom = vec![0u8; 16 + 0x8000];
        rom[..4].copy_from_slice(b"NES\x1A");
        rom[6] = 0x02;
        let id = identify(Some("Zelda.nes"), &rom).unwrap();
        assert_eq!((id.system, id.payload.clone()), (systems::NES, 16..rom.len() as u64));
        assert_eq!(
            id.cartridge.save,
            Some(Save { kind: SaveKind::Sram, size: None, lane: None }),
            "iNES says no size"
        );
        // NES 2.0 says how much.
        rom[7] = 0x08;
        rom[10] = 0x70;
        assert_eq!(identify(None, &rom).unwrap().cartridge.save, Some(sram(8 * 1024)));
    }

    #[test]
    fn a_snes_header_names_the_clock_chip() {
        let mut rom = vec![0u8; 0x40_0000];
        let at = 0xFFC0;
        rom[at..at + 21].copy_from_slice(b"DAIKAIJYUMONOGATARI2 ");
        rom[at + 0x15] = 0x35;
        rom[at + 0x16] = 0x55;
        rom[at + 0x18] = 3;
        rom[at + 0x1C..at + 0x1E].copy_from_slice(&0x1234u16.to_le_bytes());
        rom[at + 0x1E..at + 0x20].copy_from_slice(&(!0x1234u16).to_le_bytes());
        let id = identify(None, &rom).expect("a SNES ROM");
        assert_eq!(id.cartridge, Cartridge { save: Some(sram(8 * 1024)), clock: Some(Clock::Srtc) });

        // A copier header in front moves the header and the payload by its length.
        let mut copied = vec![0u8; 0x200];
        copied.extend_from_slice(&rom);
        let id = identify(None, &copied).unwrap();
        assert_eq!((id.system, id.payload.start), (systems::SNES, 0x200));
    }

    #[test]
    fn a_weak_header_does_not_overrule_the_name() {
        // ColecoVision's magic is two bytes, which a 2600 dump can open with by chance.
        let mut rom = vec![0u8; 0x2000];
        rom[..2].copy_from_slice(&[0xAA, 0x55]);
        assert_eq!(identify(None, &rom).map(|id| id.system), Some(systems::COLECOVISION));
        assert_eq!(
            identify(Some("Pitfall.a26"), &rom).map(|id| (id.system, id.evidence)),
            Some((systems::ATARI_2600, Evidence::Name))
        );
    }

    #[test]
    fn a_sega_8bit_header_tells_the_game_gear_by_its_region() {
        let mut rom = vec![0u8; 0x8000];
        rom[0x7FF0..0x7FF8].copy_from_slice(b"TMR SEGA");
        rom[0x7FFF] = 0x4C;
        assert_eq!(identify(None, &rom).unwrap().system, systems::MASTERSYSTEM);
        rom[0x7FFF] = 0x6C;
        assert_eq!(identify(None, &rom).unwrap().system, systems::SEGA_GG);
        // A Game Gear game in Master System mode says so, and its name says otherwise.
        rom[0x7FFF] = 0x4C;
        assert_eq!(identify(Some("Castle of Illusion.gg"), &rom).unwrap().system, systems::SEGA_GG);
    }

    #[test]
    fn a_gba_rom_names_its_save_chip_by_the_library_it_links() {
        let mut rom = vec![0u8; 0x400];
        rom[0x04..0x0C].copy_from_slice(&GBA_LOGO);
        rom[0xA0..0xAC].copy_from_slice(b"KLONOA      ");
        rom[0xAC..0xB0].copy_from_slice(b"AKLE");
        rom[0xB2] = 0x96;
        assert_eq!(gba_save(&rom), None, "no library, no chip");
        rom[0x200..0x20B].copy_from_slice(b"EEPROM_V124");
        let eeprom = Save { kind: SaveKind::Eeprom, size: None, lane: None };
        assert_eq!(gba_save(&rom), Some(eeprom));
        // Identifying the ROM reads windows of it, and says nothing of the chip.
        assert_eq!(identify(None, &rom).unwrap().cartridge.save, None);
        rom[0x300..0x30C].copy_from_slice(b"FLASH1M_V103");
        let flash = Save { kind: SaveKind::Flash, size: Some(128 * 1024), lane: None };
        assert_eq!(gba_save(&rom), Some(flash), "the larger chip wins");
        // A name with no version after it is text, not the library.
        rom[0x300..0x30C].copy_from_slice(b"FLASH1M_Vxyz");
        assert_eq!(gba_save(&rom), Some(eeprom));
    }

    #[test]
    fn a_mega_drive_header_declares_its_backup_ram() {
        let mut rom = vec![0x20u8; 0x200];
        rom[0x100..0x110].copy_from_slice(b"SEGA GENESIS    ");
        rom[0x150..0x160].copy_from_slice(b"PHANTASY STAR IV");
        rom[0x1B0..0x1BC].copy_from_slice(b"RA\xF8\x20\x00\x20\x00\x01\x00\x20\x3F\xFF");
        let id = identify(None, &rom).unwrap();
        assert_eq!((id.system, id.title.as_deref()), (systems::GENESIS, Some("PHANTASY STAR IV")));
        let odd = Save { lane: Some(Lane::Odd), ..sram(8 * 1024) };
        assert_eq!(id.cartridge.save, Some(odd), "odd bytes only, so half the span");
        // Evander Holyfield's: an EEPROM's two lines, declared as the one word they sit in.
        rom[0x1B0..0x1BC].copy_from_slice(b"RA\xE8\x40\x00\x20\x00\x01\x00\x20\x00\x01");
        let eeprom = Save { kind: SaveKind::Eeprom, size: None, lane: None };
        assert_eq!(identify(None, &rom).unwrap().cartridge.save, Some(eeprom));
        rom[0x1B0..0x1BC].copy_from_slice(b"RA\xF8\x20\x00\x20\x00\x01\x00\x20\x3F\xFF");
        rom[0x100..0x110].copy_from_slice(b"SEGA 32X        ");
        assert_eq!(identify(None, &rom).unwrap().system, systems::SEGA_32X);
        // FIFA Soccer '96 calls itself a Mega Drive, and only the 32X header past it says otherwise.
        rom[0x100..0x110].copy_from_slice(b"SEGA MEGA DRIVE ");
        rom.resize(0x400, 0);
        rom[0x3C0..0x3D0].copy_from_slice(b"MARS CHECK MODE ");
        assert_eq!(identify(Some("FIFA Soccer '96.32x"), &rom).unwrap().system, systems::SEGA_32X);
    }

    #[test]
    fn a_mega_drive_header_can_start_late_or_read_swapped() {
        let mut rom = vec![0x20u8; 0x200];
        // Phelios: `SEGA` one byte in, which TMSS allows.
        rom[0x100..0x110].copy_from_slice(b" SEGA MEGA DRIVE");
        rom[0x150..0x156].copy_from_slice(b"PHELIO");
        let id = identify(Some("Phelios (USA).md"), &rom).unwrap();
        assert_eq!((id.system, id.evidence), (systems::GENESIS, Evidence::Header));

        // Triple Play 96, as its dump has it: the header in swapped pairs.
        rom[0x100..0x110].copy_from_slice(b"ESAGG NESESI    ");
        rom[0x150..0x168].copy_from_slice(b"rTpielP al yaBesabll    ");
        let id = identify(Some("Triple Play 96 (USA).md"), &rom).unwrap();
        assert_eq!((id.system, id.title.as_deref()), (systems::GENESIS, Some("Triple Play Baseball")));
        assert_eq!(id.byte_order, ByteOrder::Native, "the dump is hashed as it is");
    }

    #[test]
    fn a_unif_is_a_nes_dump() {
        let mut rom = vec![0u8; 0x400];
        rom[..4].copy_from_slice(b"UNIF");
        let id = identify(Some("200-in-1 - Elfland (Unl) [U][!].unif"), &rom).unwrap();
        assert_eq!((id.system, id.evidence), (systems::NES, Evidence::Header));
    }

    #[test]
    fn a_wonderswan_footer_says_its_save_and_clock() {
        let mut rom = vec![0u8; 0x10_0000];
        let end = rom.len();
        rom[end - 16] = 0xEA;
        rom[end - 9] = 1;
        rom[end - 5] = 0x02;
        rom[end - 3] = 1;
        let id = identify(Some("Dokodemo Hamster 3.wsc"), &rom).expect("a WonderSwan ROM");
        assert_eq!(id.system, systems::WONDERSWAN_COLOR);
        assert_eq!(id.cartridge, Cartridge { save: Some(sram(32 * 1024)), clock: Some(Clock::S3511a) });
    }

    #[test]
    fn containers_and_checked_headers_name_their_systems() {
        let mut lynx = vec![0u8; 0x1000];
        lynx[..5].copy_from_slice(b"LYNX\0");
        lynx[0x0A..0x13].copy_from_slice(b"CHIPSCHAL");
        let id = identify(None, &lynx).unwrap();
        assert_eq!(
            (id.system, id.payload.start, id.title.as_deref()),
            (systems::ATARI_LYNX, 64, Some("CHIPSCHAL"))
        );

        let mut a78 = vec![0u8; 0x1000];
        a78[1..10].copy_from_slice(b"ATARI7800");
        assert_eq!(
            identify(None, &a78).map(|id| (id.system, id.payload.start)),
            Some((systems::ATARI_7800, 128))
        );

        let mut ngp = vec![0u8; 0x1000];
        ngp[..28].copy_from_slice(b"COPYRIGHT BY SNK CORPORATION");
        ngp[0x23] = 0x10;
        assert_eq!(identify(None, &ngp).unwrap().system, systems::NGPC);

        let mut nds = vec![0u8; 0x1000];
        nds[..9].copy_from_slice(b"POKEMON D");
        nds[0x0C..0x10].copy_from_slice(b"ADAE");
        nds[0x15C..0x15E].copy_from_slice(&0xCF56u16.to_le_bytes());
        let crc = crc16(&nds[..0x15E]);
        nds[0x15E..0x160].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(
            identify(None, &nds).map(|id| (id.system, id.serial)),
            Some((systems::NDS, Some("ADAE".into())))
        );
        nds[0x15E] ^= 1;
        assert_eq!(identify(None, &nds), None, "a header whose own CRC fails is not one");

        let mut vectrex = vec![0u8; 0x2000];
        vectrex[..10].copy_from_slice(b"g GCE 1982");
        assert_eq!(identify(None, &vectrex).unwrap().system, systems::VECTREX);
    }

    #[test]
    fn crc16_is_modbus() {
        assert_eq!(crc16(b"123456789"), 0x4B37);
    }
}
