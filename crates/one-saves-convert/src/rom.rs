//! Reading what a ROM says about itself.
//!
//! Most consoles put a title and a product code in a fixed header, so a ROM alongside a save
//! answers questions the save cannot. [`rom_id`] reads that header; this module carries what it
//! says into a `game` map and computes the digests a catalog keys on. Turning either into a
//! canonical name is the `dat` module's job.
//!
//! Only a header counts here, never a file's name. A header read out of a file that is not that
//! console's ROM is worse than no header at all, because it produces a `game` map that looks
//! authoritative and is wrong, and `rom_id` checks a magic before it believes a field.

use one_saves::{Game, HashAlgorithm, HashValue};

/// What a ROM's own header says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RomInfo {
    /// The system the ROM is for, as a registry slug.
    pub system: Option<&'static str>,
    /// The title as the header spells it, which is often padded and shouty.
    pub title: Option<String>,
    /// The product code out of the header, where the console has one: an N64's as
    /// [`serial_of`](crate::n64::serial_of) spells it, `NUS-NSME`, which is what its Controller
    /// Pak notes name too.
    pub serial: Option<String>,
    /// The key of the clock chip the cartridge carries, where the header says, such as
    /// `x.1sav.rtc.huc3`: what [`RawOptions::clock_chip`](crate::raw::RawOptions::clock_chip)
    /// takes.
    pub clock: Option<&'static str>,
    /// The save chip an N64 cartridge carries, where the header says: only a homebrew header does,
    /// in the byte a retail one keeps its revision in.
    pub n64_save: Option<one_saves::extension::N64Save>,
    /// What a Mega Drive, GBA or Game Boy cartridge keeps its saves in, which is what laying a save
    /// out for a core takes: a Mega Drive header declares SRAM and its byte lane, or an EEPROM's two
    /// lines, a GBA ROM links the library of its chip, and a Game Boy header's type says an MBC7's
    /// EEPROM.
    pub memory: Option<crate::layout::SaveMemory>,
}

/// Identifies a ROM from its header.
///
/// Returns `None` when nothing recognises it, which is the honest answer for a headerless system
/// or a file that is not a ROM. The header is [`rom_id`]'s to read; this is the part of its answer
/// a `game` map carries, and only from a header, never from a name.
#[must_use]
pub fn identify(bytes: &[u8]) -> Option<RomInfo> {
    let id = rom_id::identify_bytes(None, bytes).filter(|id| id.evidence == rom_id::Evidence::Header)?;
    let n64 = id.system.slug == crate::n64::SYSTEM;
    Some(RomInfo {
        system: Some(id.system.slug),
        title: id.title,
        serial: if n64 { id.serial.as_deref().map(crate::n64::serial_of) } else { id.serial },
        clock: id.cartridge.clock.map(rom_id::Clock::key),
        n64_save: if n64 { id.cartridge.save.and_then(n64_save) } else { None },
        memory: match id.system.slug {
            "genesis" | "gb" | "gbc" => id.cartridge.save.map(save_memory),
            "gba" => rom_id::gba_save(bytes).map(save_memory),
            _ => None,
        },
    })
}

/// What [`layout`](crate::layout) needs of a cartridge's save memory.
fn save_memory(save: rom_id::Save) -> crate::layout::SaveMemory {
    use crate::layout::SaveMemory;
    let size = save.size.and_then(|size| usize::try_from(size).ok());
    match (save.kind, save.lane) {
        (rom_id::SaveKind::Eeprom, _) => SaveMemory::Eeprom { size },
        (_, Some(lane)) => SaveMemory::ByteLane { odd: lane == rom_id::Lane::Odd, size },
        (_, None) => SaveMemory::Memory { size },
    }
}

/// The chip a header's save memory is, by its kind and size.
fn n64_save(save: rom_id::Save) -> Option<one_saves::extension::N64Save> {
    use one_saves::extension::N64Save;
    N64Save::ALL.into_iter().find(|chip| {
        let kind = match chip {
            N64Save::Eeprom4k | N64Save::Eeprom16k => rom_id::SaveKind::Eeprom,
            N64Save::Sram256k | N64Save::Sram768k => rom_id::SaveKind::Sram,
            N64Save::Flash1m => rom_id::SaveKind::Flash,
        };
        save.kind == kind && save.size.and_then(|size| usize::try_from(size).ok()) == Some(chip.size())
    })
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
    let mut game = Game::default();
    game.rom_hashes = hashes(bytes);
    game.rom_filename = filename.map(ToOwned::to_owned);
    game.serial = info.serial;
    game.title = info.title;
    // Which system's release these hints identify, which the ROM header is exactly the thing that
    // knows. Distinct from the header's `system`, which says what will load the save.
    game.system = info.system.and_then(|slug| slug.parse().ok());
    game
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The opening bytes of the Nintendo logo a Game Boy boot ROM checks.
    const GB_LOGO: [u8; 8] = [0xCE, 0xED, 0x66, 0x66, 0xCC, 0x0D, 0x00, 0x0B];

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
        // A GBA ROM has no Nintendo logo where a Game Boy one does, and its own logo and 0x96 at
        // 0xB2 are what the BIOS checks, so the two never collide.
        let mut rom = vec![0u8; 0x200];
        rom[0x04..0x0C].copy_from_slice(&[0x24, 0xFF, 0xAE, 0x51, 0x69, 0x9A, 0xA2, 0x21]);
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
            assert_eq!(info.serial.as_deref(), Some("NUS-NSME"), "{name}");
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
