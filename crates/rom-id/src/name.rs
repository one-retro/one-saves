//! What a ROM's name says: the systems its extension allows, narrowed by the folder it sits in.

use one_saves_registry::{System, system_by_any_name, systems};

/// Every extension a cartridge dump is commonly given, and the systems it can mean.
///
/// Most name one system. The ones that do not — `.bin` above all — are why the folder matters: a
/// `.bin` under `genesis/` is a Mega Drive dump, and one under `atari2600/` is not.
static EXTENSIONS: &[(&str, &[System])] = &[
    ("32x", &[systems::SEGA_32X]),
    ("3ds", &[systems::_3DS]),
    ("a26", &[systems::ATARI_2600]),
    ("a52", &[systems::ATARI_5200]),
    ("a78", &[systems::ATARI_7800]),
    ("agb", &[systems::GBA]),
    (
        "bin",
        &[
            systems::GENESIS,
            systems::ATARI_2600,
            systems::ATARI_7800,
            systems::COLECOVISION,
            systems::INTELLIVISION,
            systems::ODYSSEY2,
            systems::VECTREX,
            systems::SEGA_32X,
        ],
    ),
    ("bs", &[systems::SNES]),
    ("cgb", &[systems::GBC]),
    ("col", &[systems::COLECOVISION]),
    ("crt", &[systems::C64]),
    ("fds", &[systems::FDS]),
    ("fig", &[systems::SNES]),
    ("gb", &[systems::GB]),
    ("gba", &[systems::GBA]),
    ("gbc", &[systems::GBC]),
    ("gen", &[systems::GENESIS]),
    ("gg", &[systems::SEGA_GG]),
    ("int", &[systems::INTELLIVISION]),
    ("j64", &[systems::ATARI_JAGUAR]),
    ("jag", &[systems::ATARI_JAGUAR]),
    ("lnx", &[systems::ATARI_LYNX]),
    ("lyx", &[systems::ATARI_LYNX]),
    ("md", &[systems::GENESIS]),
    ("mx1", &[systems::MSX]),
    ("mx2", &[systems::MSX2]),
    ("n64", &[systems::N64]),
    ("nds", &[systems::NDS]),
    ("neo", &[systems::NEOGEO]),
    ("nes", &[systems::NES]),
    ("ngc", &[systems::NGPC]),
    ("ngp", &[systems::NGP]),
    ("o2", &[systems::ODYSSEY2]),
    ("pce", &[systems::PCENGINE]),
    ("rom", &[systems::MSX, systems::MSX2, systems::COLECOVISION, systems::ATARI_7800]),
    ("sc", &[systems::SG_1000]),
    ("sfc", &[systems::SNES]),
    ("sg", &[systems::SG_1000]),
    ("sgb", &[systems::GB]),
    ("sgx", &[systems::SUPERGRAFX]),
    ("smc", &[systems::SNES]),
    ("smd", &[systems::GENESIS]),
    ("sms", &[systems::MASTERSYSTEM]),
    ("swc", &[systems::SNES]),
    ("unf", &[systems::NES]),
    ("unif", &[systems::NES]),
    ("v64", &[systems::N64]),
    ("vb", &[systems::VIRTUALBOY]),
    ("vboy", &[systems::VIRTUALBOY]),
    ("vec", &[systems::VECTREX]),
    ("ws", &[systems::WONDERSWAN]),
    ("wsc", &[systems::WONDERSWAN_COLOR]),
    ("z64", &[systems::N64]),
];

/// Extensions that are never a cartridge dump, whatever folder they sit in: archives, saves, disc
/// images, and the text and pictures a collection keeps beside its games. Without this a
/// `systeminfo.txt` under `gb/` would be a Game Boy game by its folder.
static NOT_DUMPS: &[&str] = &[
    "1saves", "7z", "bmp", "ccd", "cdi", "chd", "cso", "cue", "dat", "db", "eep", "fla", "gdi", "gif", "gz",
    "htm", "html", "iso", "jpeg", "jpg", "json", "m3u", "md5", "mpk", "nfo", "pbp", "pdf", "png", "rar",
    "sav", "sfv", "sha1", "sqlite", "sra", "srm", "state", "sub", "txt", "xml", "zip",
];

/// The systems a file's name allows, most likely first.
///
/// The extension decides, and the folder the file sits in narrows it: a `.bin` under `genesis/` is
/// the Mega Drive alone. A folder is read through the registry, by slug or by any name it lists
/// for a system, so `megadrive/` and `MD/` both work. A folder that names a system the extension
/// does not allow is ignored rather than believed, and a file with no extension this crate knows
/// takes the folder's system alone — unless its extension says it is no dump at all, like an
/// archive, a save or a text file.
///
/// ```
/// use one_saves_registry::systems;
///
/// assert_eq!(rom_id::candidates("roms/genesis/Sonic.bin"), [systems::GENESIS]);
/// assert_eq!(rom_id::candidates("Tetris.gb"), [systems::GB]);
/// assert!(rom_id::candidates("Sonic.bin").len() > 1);
/// ```
#[must_use]
pub fn candidates(name: &str) -> Vec<System> {
    let mut parts = name.rsplit(['/', '\\']).filter(|part| !part.is_empty());
    let file = parts.next().unwrap_or_default();
    let extension = file.rsplit_once('.').map(|(_, extension)| extension);
    if extension.is_some_and(|extension| NOT_DUMPS.iter().any(|known| known.eq_ignore_ascii_case(extension)))
    {
        return Vec::new();
    }
    let folder = parts.next().and_then(system_by_any_name).copied();
    let allowed = extension
        .and_then(|extension| EXTENSIONS.iter().find(|(known, _)| known.eq_ignore_ascii_case(extension)))
        .map_or(&[][..], |(_, systems)| *systems);
    match folder {
        Some(folder) if allowed.contains(&folder) || allowed.is_empty() => vec![folder],
        _ => allowed.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_extension_names_the_system() {
        assert_eq!(candidates("Pokemon - Crystal Version (USA, Europe) (Rev 1).gbc"), [systems::GBC]);
        assert_eq!(candidates("SUPER MARIO 64.Z64"), [systems::N64], "the case of an extension is nobody's");
        assert_eq!(candidates("no extension"), []);
        assert_eq!(candidates("archive.zip"), [], "an archive is not a dump");
    }

    #[test]
    fn the_folder_narrows_a_shared_extension() {
        assert_eq!(candidates("/Volumes/Games/ROMs/genesis/Sonic.bin"), [systems::GENESIS]);
        assert_eq!(candidates(r"C:\roms\megadrive\Sonic.bin"), [systems::GENESIS], "by an alias too");
        assert_eq!(candidates("roms/2600/Pitfall.bin"), [systems::ATARI_2600]);
        assert_eq!(candidates("Sonic.bin").first(), Some(&systems::GENESIS));
    }

    #[test]
    fn a_folder_the_extension_rules_out_is_not_believed() {
        // A Game Boy dump filed under the wrong folder is still a Game Boy dump.
        assert_eq!(candidates("roms/snes/Tetris.gb"), [systems::GB]);
        // And a file the extension says nothing about takes the folder's word.
        assert_eq!(candidates("roms/pcengine/Bonk"), [systems::PCENGINE]);
    }

    #[test]
    fn what_is_never_a_dump_takes_no_folder() {
        assert_eq!(candidates("roms/gb/systeminfo.txt"), []);
        assert_eq!(candidates("roms/gba/Pokemon - Emerald Version (USA, Europe).zip"), []);
        assert_eq!(candidates("roms/gba/Pokemon - Emerald Version (USA, Europe).SAV"), []);
    }
}
