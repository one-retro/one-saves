//! Reading what a ROM says about itself.
//!
//! Most consoles put a title and a product code in a fixed header, so a ROM alongside a save
//! answers questions the save cannot. This module reads that header and computes the digests a
//! catalog keys on; turning either into a canonical name is the `dat` module's job.
//!
//! Every parser here checks a magic before believing a field. A header read out of a file that is
//! not that console's ROM is worse than no header at all, because it produces a `game` map that
//! looks authoritative and is wrong.

use one_saves::{Game, HashAlgorithm, HashValue};

/// What a ROM's own header says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RomInfo {
    /// The system the ROM is for, as a registry slug.
    pub system: Option<&'static str>,
    /// The title as the header spells it, which is often padded and shouty.
    pub title: Option<String>,
    /// The product code out of the header, where the console has one.
    pub serial: Option<String>,
}

/// Identifies a ROM from its header.
///
/// Returns `None` when nothing recognises it, which is the honest answer for a headerless system
/// or a file that is not a ROM.
#[must_use]
pub fn identify(bytes: &[u8]) -> Option<RomInfo> {
    // Ordered by how strong the magic is, so a weak check never shadows a strong one.
    gba(bytes)
        .or_else(|| gameboy(bytes))
        .or_else(|| n64(bytes))
        .or_else(|| megadrive(bytes))
        .or_else(|| nes(bytes))
        .or_else(|| snes(bytes))
}

/// Trims a fixed-width header field: drop the NUL and space padding, keep the rest verbatim.
fn field(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).trim().to_owned()
}

fn non_empty(text: String) -> Option<String> {
    (!text.is_empty()).then_some(text)
}

/// The 48-byte Nintendo logo every Game Boy cartridge carries, checked by the boot ROM.
const GB_LOGO: [u8; 8] = [0xCE, 0xED, 0x66, 0x66, 0xCC, 0x0D, 0x00, 0x0B];

/// Game Boy and Game Boy Color.
fn gameboy(bytes: &[u8]) -> Option<RomInfo> {
    if bytes.len() < 0x150 || bytes[0x104..0x104 + 8] != GB_LOGO {
        return None;
    }

    // The title field shrank as Nintendo took bytes off its end for other uses, so the CGB flag
    // sits where the last title byte used to. Reading 16 bytes on a CGB cartridge would append a
    // stray 0x80 or 0xC0 to the name.
    let cgb_flag = bytes[0x143];
    let cgb = matches!(cgb_flag, 0x80 | 0xC0);
    let title_len = if cgb { 15 } else { 16 };
    let title = field(&bytes[0x134..0x134 + title_len]);

    Some(RomInfo {
        system: Some(if cgb { "gbc" } else { "gb" }),
        title: non_empty(title),
        // A Game Boy header has no product code; the title bytes are all there is, and the spec
        // says a title that is not a serial belongs in `name` rather than here.
        serial: None,
    })
}

/// Game Boy Advance.
fn gba(bytes: &[u8]) -> Option<RomInfo> {
    // 0x96 at 0xB2 is the fixed byte the BIOS checks, and it is what tells a GBA ROM from a GB one.
    if bytes.len() < 0xC0 || bytes[0xB2] != 0x96 {
        return None;
    }
    Some(RomInfo {
        system: Some("gba"),
        title: non_empty(field(&bytes[0xA0..0xAC])),
        serial: non_empty(field(&bytes[0xAC..0xB0])),
    })
}

/// Nintendo 64, in any of the three byte orders its dumps come in.
fn n64(bytes: &[u8]) -> Option<RomInfo> {
    if bytes.len() < 0x40 {
        return None;
    }
    let magic = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    // A cartridge is 16-bit words in the console's order; dumpers have shipped all three.
    let normalised: Vec<u8> = match magic {
        0x8037_1240 => bytes[..0x40].to_vec(),
        // Byte-swapped within each 16-bit word.
        0x3780_4012 => bytes[..0x40].as_chunks::<2>().0.iter().flat_map(|w| [w[1], w[0]]).collect(),
        // Wholly little-endian, within each 32-bit word.
        0x4012_3780 => {
            bytes[..0x40].as_chunks::<4>().0.iter().flat_map(|w| [w[3], w[2], w[1], w[0]]).collect()
        }
        _ => return None,
    };
    Some(RomInfo {
        system: Some("n64"),
        title: non_empty(field(&normalised[0x20..0x34])),
        serial: non_empty(field(&normalised[0x3B..0x3F])),
    })
}

/// Mega Drive and Genesis.
fn megadrive(bytes: &[u8]) -> Option<RomInfo> {
    if bytes.len() < 0x190 {
        return None;
    }
    let console = &bytes[0x100..0x110];
    if !console.starts_with(b"SEGA") {
        return None;
    }
    // The international name is the one in Latin script on a Japanese cartridge too.
    let international = field(&bytes[0x150..0x180]);
    let domestic = field(&bytes[0x120..0x150]);
    Some(RomInfo {
        system: Some("genesis"),
        title: non_empty(if international.is_empty() { domestic } else { international }),
        serial: non_empty(field(&bytes[0x183..0x18E])),
    })
}

/// NES, in the iNES container.
fn nes(bytes: &[u8]) -> Option<RomInfo> {
    // An iNES header describes the cartridge's wiring and says nothing about the game, so there
    // is no title or code to read. Identification here needs a hash and a catalog.
    (bytes.len() >= 16 && bytes.starts_with(b"NES\x1A")).then_some(RomInfo {
        system: Some("nes"),
        title: None,
        serial: None,
    })
}

/// Super Nintendo, whose header sits at one of two places and may be behind a copier header.
fn snes(bytes: &[u8]) -> Option<RomInfo> {
    // A 512-byte copier header shifts everything, and is detected by the file's length rather
    // than by anything in it.
    let base = usize::from(bytes.len() % 1024 == 512) * 512;

    // LoROM and HiROM put the header in different places, and the checksum pair is what decides:
    // it and its complement have to add up to all ones.
    for offset in [0x7FC0, 0xFFC0] {
        let at = base + offset;
        if bytes.len() < at + 32 {
            continue;
        }
        let header = &bytes[at..at + 32];
        let complement = u16::from_le_bytes([header[0x1C], header[0x1D]]);
        let checksum = u16::from_le_bytes([header[0x1E], header[0x1F]]);
        if complement ^ checksum != 0xFFFF {
            continue;
        }
        return Some(RomInfo { system: Some("snes"), title: non_empty(field(&header[..21])), serial: None });
    }
    None
}

/// Every digest a retro catalog might key on.
///
/// All four are computed in one pass over the bytes, since a caller reaching for one usually
/// wants whichever its catalog carries and cannot know which that is in advance.
#[must_use]
pub fn hashes(bytes: &[u8]) -> Vec<HashValue> {
    // All three crates re-export the same `Digest` trait from `digest`, so one import serves.
    use sha2::Digest as _;

    let sha256 = sha2::Sha256::digest(bytes);
    let sha1 = sha1::Sha1::digest(bytes);
    let md5 = md5::Md5::digest(bytes);
    let crc32 = crc32fast::hash(bytes);

    // Ascending by tag number, which is the order a `rom_hashes` array holds.
    vec![
        HashValue::new(HashAlgorithm::Sha256, sha256.to_vec()).expect("sha256 is 32 bytes"),
        HashValue::new(HashAlgorithm::Sha1, sha1.to_vec()).expect("sha1 is 20 bytes"),
        HashValue::new(HashAlgorithm::Crc32, crc32.to_be_bytes().to_vec()).expect("crc32 is 4 bytes"),
        HashValue::new(HashAlgorithm::Md5, md5.to_vec()).expect("md5 is 16 bytes"),
    ]
}

/// Builds the `game` hints a ROM supports: its header fields, its filename, and its digests.
#[must_use]
pub fn game_from_rom(bytes: &[u8], filename: Option<&str>) -> Game {
    let info = identify(bytes).unwrap_or_default();
    Game {
        game_id: std::collections::BTreeMap::new(),
        rom_hashes: hashes(bytes),
        rom_filename: filename.map(ToOwned::to_owned),
        serial: info.serial,
        title: info.title,
        // Which system's release these hints identify, which the ROM header is exactly the thing
        // that knows. Distinct from the header's `system`, which says what will load the save.
        system: info.system.and_then(|slug| slug.parse().ok()),
        unknown: one_saves::UnknownKeys::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gb_rom(title: &[u8], cgb: u8) -> Vec<u8> {
        let mut rom = vec![0u8; 0x8000];
        rom[0x104..0x104 + 8].copy_from_slice(&GB_LOGO);
        rom[0x134..0x134 + title.len()].copy_from_slice(title);
        rom[0x143] = cgb;
        rom
    }

    #[test]
    fn reads_a_game_boy_title() {
        let rom = gb_rom(b"POKEMON RED", 0x00);
        let info = identify(&rom).expect("a Game Boy ROM");
        assert_eq!(info.system, Some("gb"));
        assert_eq!(info.title.as_deref(), Some("POKEMON RED"));
    }

    #[test]
    fn a_colour_cartridge_does_not_take_the_flag_byte_as_a_title_character() {
        // The title field shrank to make room for the CGB flag, so reading 16 bytes would put a
        // stray 0x80 on the end of every Game Boy Color name.
        let mut rom = gb_rom(b"ZELDA DIN", 0xC0);
        rom[0x134..0x134 + 15].copy_from_slice(b"ZELDA DIN\0\0\0\0\0\0");
        let info = identify(&rom).expect("a Game Boy Color ROM");
        assert_eq!(info.system, Some("gbc"));
        assert_eq!(info.title.as_deref(), Some("ZELDA DIN"));
    }

    #[test]
    fn reads_a_gba_code_and_prefers_it_over_the_game_boy_parser() {
        // A GBA ROM has no Nintendo logo where a Game Boy one does, and 0x96 at 0xB2 is what the
        // BIOS checks, so the two never collide.
        let mut rom = vec![0u8; 0x200];
        rom[0xA0..0xAC].copy_from_slice(b"POKEMON RUBY");
        rom[0xAC..0xB0].copy_from_slice(b"AXVE");
        rom[0xB2] = 0x96;
        let info = identify(&rom).expect("a GBA ROM");
        assert_eq!(info.system, Some("gba"));
        assert_eq!(info.title.as_deref(), Some("POKEMON RUBY"));
        assert_eq!(info.serial.as_deref(), Some("AXVE"));
    }

    #[test]
    fn reads_an_n64_header_in_all_three_byte_orders() {
        let mut native = vec![0u8; 0x40];
        native[..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        native[0x20..0x20 + 14].copy_from_slice(b"SUPER MARIO 64");
        native[0x3B..0x3F].copy_from_slice(b"NSME");

        let byteswapped: Vec<u8> = native.as_chunks::<2>().0.iter().flat_map(|w| [w[1], w[0]]).collect();
        let little: Vec<u8> =
            native.as_chunks::<4>().0.iter().flat_map(|w| [w[3], w[2], w[1], w[0]]).collect();

        for (name, rom) in [("z64", &native), ("v64", &byteswapped), ("n64", &little)] {
            let info = identify(rom).unwrap_or_else(|| panic!("{name} should parse"));
            assert_eq!(info.system, Some("n64"), "{name}");
            assert_eq!(info.title.as_deref(), Some("SUPER MARIO 64"), "{name}");
            assert_eq!(info.serial.as_deref(), Some("NSME"), "{name}");
        }
    }

    #[test]
    fn reads_a_mega_drive_serial() {
        let mut rom = vec![0x20u8; 0x200];
        rom[0x100..0x110].copy_from_slice(b"SEGA MEGA DRIVE ");
        rom[0x150..0x160].copy_from_slice(b"SONIC THE HEDGEH");
        rom[0x160..0x180].fill(0x20);
        rom[0x183..0x18E].copy_from_slice(b"GM 00001009");
        let info = identify(&rom).expect("a Mega Drive ROM");
        assert_eq!(info.system, Some("genesis"));
        assert_eq!(info.serial.as_deref(), Some("GM 00001009"));
    }

    #[test]
    fn finds_the_snes_header_only_where_the_checksum_agrees() {
        // The checksum and its complement have to add to all ones, which is what tells the real
        // header from the 32 bytes that happen to sit at the other candidate offset.
        let mut rom = vec![0u8; 0x10000];
        let at = 0x7FC0;
        rom[at..at + 21].copy_from_slice(b"SUPER MARIOWORLD     ");
        rom[at + 0x1C..at + 0x1E].copy_from_slice(&0x1234u16.to_le_bytes());
        rom[at + 0x1E..at + 0x20].copy_from_slice(&(!0x1234u16).to_le_bytes());

        let info = identify(&rom).expect("a SNES ROM");
        assert_eq!(info.system, Some("snes"));
        assert_eq!(info.title.as_deref(), Some("SUPER MARIOWORLD"));

        // Break the pair and the header stops being believed.
        rom[at + 0x1E] ^= 0xFF;
        assert_eq!(identify(&rom), None);
    }

    #[test]
    fn declines_a_file_that_is_not_a_rom() {
        assert_eq!(identify(&[0u8; 4096]), None);
        assert_eq!(identify(b"just some bytes"), None);
    }

    #[test]
    fn hashes_come_out_in_ascending_tag_order() {
        let hashes = hashes(b"");
        let tags: Vec<u64> = hashes.iter().map(HashValue::tag).collect();
        let mut sorted = tags.clone();
        sorted.sort_unstable();
        assert_eq!(tags, sorted, "a rom_hashes array is ascending by tag");

        // Known-good digests of the empty input, so a wired-up-wrong algorithm shows here.
        assert_eq!(
            hashes[0].to_string(),
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(hashes[1].to_string(), "sha1:da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(hashes[2].to_string(), "crc32:00000000");
        assert_eq!(hashes[3].to_string(), "md5:d41d8cd98f00b204e9800998ecf8427e");
    }

    #[test]
    fn a_game_map_from_a_rom_carries_everything_it_can() {
        let rom = gb_rom(b"POKEMON RED", 0x00);
        let game = game_from_rom(&rom, Some("pokemon_red.gb"));
        assert_eq!(game.title.as_deref(), Some("POKEMON RED"));
        assert_eq!(game.rom_filename.as_deref(), Some("pokemon_red.gb"));
        assert_eq!(game.rom_hashes.len(), 4);
        assert!(!game.is_empty());
    }
}
