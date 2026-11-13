//! Nintendo 64: the header, and the save memory each cartridge carries.
//!
//! An N64 header names the game and says nothing about its save chip. Every retail game shipped
//! with one of five chips or none, and the console finds out which by asking over the Joybus or
//! by the game simply using it, so nothing in the ROM says. Neither does the save: a 4 Kbit and a
//! 16 Kbit EEPROM hold the same bytes when only the first 512 have been written. The answer is the
//! game, which is what [`cartridge`] looks up.
//!
//! The key is the game ID out of the header, `NSME`: a category (`N` for a cartridge, `C` for one
//! that expects the 64DD), two characters naming the game, and its region. Two characters name the
//! game across its releases nearly always, and where the chip differs by region or revision the
//! table says so: Castlevania keeps its save in a Controller Pak and Akumajou Dracula Mokushiroku,
//! the same game, in a 16 Kbit EEPROM; Hoshi no Kirby 64 1.0 and 1.1 have SRAM, and every later
//! Kirby 64 an EEPROM.
//!
//! Absence is two answers. A game the table holds with no chip, about half of them, keeps its saves
//! in a Controller Pak or not at all, and [`N64Cartridge::save`] is `None`. A game the table does not
//! hold is `None` from [`cartridge`] itself, and a caller that wants a chip has to find it elsewhere,
//! never assume one.
//!
//! # Where the table came from
//!
//! It was seeded from the one ares keeps (`mia/medium/nintendo-64.cpp`, ISC-licensed), then checked
//! against MiSTer's N64_ROM_Database and mupen64plus's `mupen64plus.ini`. ares keys on three
//! characters, so where two games share a code, the region now tells them apart (`NT3` is Toukon
//! Road 2 in Japan and Tony Hawk's Pro Skater 3 elsewhere, `NMS` Morita Shougi 64 and a Duke Nukem
//! beta). The remaining disagreements were settled by majority of the three, and the rows a
//! majority could not settle with confidence say "disputed": Monaco Grand Prix, Rally Challenge
//! 2000, Aidyn Chronicles and 40 Winks. A save from one of those is the evidence that ends the
//! dispute.

use one_saves_registry::{System, systems};

use crate::header::{field, found, sram};
use crate::{ByteOrder, Cartridge, Clock, RomId, Save, SaveKind, View};

/// What an N64 cartridge carries for its saves, as far as its game ID says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct N64Cartridge {
    /// The chip on the cartridge, or `None` for a game known to have none.
    pub save: Option<Save>,
    /// The Joybus clock, which only Doubutsu no Mori carries.
    pub clock: Option<Clock>,
    /// Whether the game keeps saves in a Controller Pak, beside its chip or instead of one.
    pub controller_pak: bool,
}

/// What a retail N64 cartridge carries, by its game ID and revision.
///
/// `game_id` is the header's four characters, `NSME`, or the serial built from them, `NUS-NSME`
/// or `NUS-NSME-USA`. `revision` is the header's version byte; it decides only Hoshi no Kirby 64,
/// and a lookup that needs it and was not given it answers `None` rather than picking one.
///
/// `None` for a game the table does not hold, and for a homebrew ID (`?ED?`), whose header says
/// its own chip in the byte this would read as a revision: [`identify`](crate::identify_bytes)
/// reads that.
///
/// ```
/// use rom_id::{SaveKind, n64};
///
/// let mario = n64::cartridge("NSME", None).expect("Super Mario 64");
/// assert_eq!(mario.save.map(|save| (save.kind, save.size)), Some((SaveKind::Eeprom, Some(512))));
///
/// let castlevania = n64::cartridge("NUS-ND3E-USA", None).expect("Castlevania");
/// assert_eq!((castlevania.save, castlevania.controller_pak), (None, true));
/// ```
#[must_use]
pub fn cartridge(game_id: &str, revision: Option<u8>) -> Option<N64Cartridge> {
    let id = game_id.strip_prefix("NUS-").unwrap_or(game_id);
    let id = id.split('-').next().unwrap_or(id);
    let id: [u8; 4] = id.as_bytes().try_into().ok()?;
    lookup(id, revision)
}

/// The table's answer for a header's ID.
fn lookup(id: [u8; 4], revision: Option<u8>) -> Option<N64Cartridge> {
    if &id[1..3] == b"ED" {
        return None;
    }
    let game = GAMES.iter().filter(|game| id.starts_with(game.id)).find(|game| match game.revision {
        None => true,
        Some(row) => revision.is_none_or(|asked| asked == row),
    })?;
    // A row for one revision, reached with no revision to compare: the revision decides, and
    // nobody said which.
    if game.revision.is_some() && revision.is_none() {
        return None;
    }
    Some(N64Cartridge { save: game.memory.save(), clock: clock(id), controller_pak: game.pak })
}

/// The clock a retail ID says: Doubutsu no Mori's, `NAFJ`, the one retail cartridge with one.
fn clock(id: [u8; 4]) -> Option<Clock> {
    (&id[1..3] == b"AF").then_some(Clock::Joybus)
}

/// Nintendo 64, in any of the three byte orders its dumps come in.
pub(crate) fn header(view: View<'_>, _: &[System]) -> Option<RomId> {
    let head = view.bytes(0, 0x40)?;
    let (byte_order, header): (ByteOrder, Vec<u8>) = match u32::from_be_bytes(head[..4].try_into().ok()?) {
        0x8037_1240 => (ByteOrder::Native, head.to_vec()),
        0x3780_4012 => (ByteOrder::Swapped16, head.chunks(2).flat_map(|w| [w[1], w[0]]).collect()),
        0x4012_3780 => {
            (ByteOrder::Swapped32, head.chunks(4).flat_map(|w| [w[3], w[2], w[1], w[0]]).collect())
        }
        _ => return None,
    };
    let id: [u8; 4] = header[0x3B..0x3F].try_into().ok()?;
    let flags = header[0x3F];
    // A homebrew ROM sets its game ID to `ED` and puts its save type and clock in the byte a retail
    // cartridge keeps its version in, since no retail game says either.
    let homebrew = &id[1..3] == b"ED";
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
        lookup(id, Some(flags)).map_or(Cartridge { save: None, clock: clock(id) }, |cart| Cartridge {
            save: cart.save,
            clock: cart.clock,
        })
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

/// A cartridge's save memory, as the table writes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Memory {
    /// No chip: the game saves to a Controller Pak, or not at all.
    Nothing,
    /// 4 Kbit EEPROM, 512 bytes.
    Eeprom4k,
    /// 16 Kbit EEPROM, 2 KiB.
    Eeprom16k,
    /// 256 Kbit SRAM, 32 KiB.
    Sram256k,
    /// 768 Kbit SRAM, 96 KiB, in three banks: Dezaemon 3D alone.
    Sram768k,
    /// 1 Mbit FlashRAM, 128 KiB.
    Flash1m,
}

impl Memory {
    fn save(self) -> Option<Save> {
        let (kind, size) = match self {
            Memory::Nothing => return None,
            Memory::Eeprom4k => (SaveKind::Eeprom, 512),
            Memory::Eeprom16k => (SaveKind::Eeprom, 2048),
            Memory::Sram256k => (SaveKind::Sram, 32 * 1024),
            Memory::Sram768k => (SaveKind::Sram, 96 * 1024),
            Memory::Flash1m => (SaveKind::Flash, 128 * 1024),
        };
        Some(Save { kind, size: Some(size), lane: None })
    }
}

/// One row of the table.
#[derive(Debug)]
struct Game {
    /// The start of the game ID the row is for: category and code, and the region where the chip
    /// differs by one.
    id: &'static [u8],
    /// The one revision the row is for, where the chip differs by one.
    revision: Option<u8>,
    memory: Memory,
    /// Whether the game keeps saves in a Controller Pak.
    pak: bool,
}

const fn game(id: &'static [u8], memory: Memory) -> Game {
    Game { id, revision: None, memory, pak: false }
}

impl Game {
    const fn pak(self) -> Game {
        Game { pak: true, ..self }
    }

    const fn revision(self, revision: u8) -> Game {
        Game { revision: Some(revision), ..self }
    }
}

use Memory::{Eeprom4k, Eeprom16k, Flash1m, Nothing, Sram256k, Sram768k};

/// Every game, by its code. A row for one region or revision comes before the row for the rest of
/// the game, and the first row that matches answers.
#[rustfmt::skip]
static GAMES: &[Game] = &[
    game(b"N22", Nothing).pak(),                  // Ready 2 Rumble Boxing - Round 2
    game(b"N2M", Nothing).pak(),                  // Madden Football 2002
    game(b"N2P", Nothing).pak(),                  // Rampage 2 - Universal Tour
    game(b"N2V", Nothing).pak(),                  // Chameleon Twist 2 (U + E)
    game(b"N32", Nothing).pak(),                  // Army Men - Sarge's Heroes 2
    game(b"N3D", Eeprom16k),                      // Doraemon 3: Nobita no Machi SOS!
    game(b"N3HJ", Sram256k).pak(),                // Ganbare! Nippon! Olympics 2000
    game(b"N3H", Nothing).pak(),                  // International Track & Field 2000
    game(b"N3P", Nothing).pak(),                  // Triple Play 2000
    game(b"N3T", Nothing).pak(),                  // Tony Hawk's Pro Skater 3
    game(b"N4W", Eeprom4k).pak(),                 // 40 Winks (disputed)
    game(b"N64", Nothing).pak(),                  // Kira to Kaiketsu! 64 Tanteidan
    game(b"N7I", Nothing).pak(),                  // FIFA Soccer 64 [FIFA 64 (E)]
    game(b"N8I", Nothing).pak(),                  // FIFA - Road to World Cup 98 [World Cup e no Michi (J)]
    game(b"N8M", Nothing).pak(),                  // Madden Football 64
    game(b"N8W", Nothing).pak(),                  // World Cup '98
    game(b"N9B", Nothing).pak(),                  // NBA Live '99
    game(b"N9C", Nothing).pak(),                  // Nascar '99
    game(b"N9F", Nothing).pak(),                  // FIFA 99
    game(b"N9H", Nothing).pak(),                  // NHL '99
    game(b"N9M", Nothing).pak(),                  // Madden Football '99
    game(b"NA2", Sram256k).pak(),                 // Virtual Pro Wrestling 2
    game(b"NAB", Eeprom4k).pak(),                 // Air Boarder 64
    game(b"NAC", Nothing).pak(),                  // Army Men - Air Combat
    game(b"NAD", Eeprom4k),                       // Worms Armageddon (U)
    game(b"NAF", Flash1m).pak(),                  // Doubutsu no Mori
    game(b"NAG", Eeprom4k).pak(),                 // AeroGauge
    game(b"NAH", Nothing).pak(),                  // Asteroids Hyper 64
    game(b"NAI", Nothing).pak(),                  // Midway's Greatest Arcade Hits Volume 1
    game(b"NAL", Sram256k),                       // Super Smash Bros. [Nintendo All-Star! Dairantou Smash Brothers (J)]
    game(b"NAM", Nothing).pak(),                  // Army Men - Sarge's Heroes
    game(b"NAR", Nothing).pak(),                  // Armorines - Project S.W.A.R.M.
    game(b"NAS", Nothing).pak(),                  // All-Star Baseball 2001
    game(b"NAY", Nothing).pak(),                  // Aidyn Chronicles: The First Mage (disputed)
    game(b"NB2", Nothing).pak(),                  // NBA In the Zone '99 [NBA Pro '99 (E)]
    game(b"NB3", Nothing).pak(),                  // Bust-A-Move '99 [Bust-A-Move 3 DX (E)]
    game(b"NB4", Nothing).pak(),                  // Bass Masters 2000
    game(b"NB5", Sram256k),                       // Biohazard 2 (J)
    game(b"NB6", Eeprom4k).pak(),                 // Super B-Daman: Battle Phoenix 64
    game(b"NB7", Eeprom16k),                      // Banjo-Tooie [Banjo to Kazooie no Daiboken 2 (J)]
    game(b"NB8", Nothing).pak(),                  // Beetle Adventure Racing (J)
    game(b"NB9", Nothing).pak(),                  // NBA Jam '99
    game(b"NBA", Nothing).pak(),                  // NBA In the Zone '98 [NBA Pro '98 (E)]
    game(b"NBC", Eeprom4k).pak(),                 // Blast Corps
    game(b"NBD", Eeprom4k),                       // Bomberman Hero [Mirian Ojo o Sukue! (J)]
    game(b"NBE", Nothing).pak(),                  // All-Star Baseball 2000
    game(b"NBF", Nothing).pak(),                  // Bio F.R.E.A.K.S.
    game(b"NBH", Eeprom4k),                       // Body Harvest
    game(b"NBI", Nothing).pak(),                  // NFL Blitz 2000
    game(b"NBJ", Nothing).pak(),                  // Bakushou Jinsei 64 - Mezase! Resort Ou
    game(b"NBK", Eeprom4k),                       // Banjo-Kazooie [Banjo to Kazooie no Daiboken (J)]
    game(b"NBL", Nothing).pak(),                  // Buck Bumble
    game(b"NBM", Eeprom4k).pak(),                 // Bomberman 64 [Baku Bomberman (J)]
    game(b"NBN", Eeprom4k).pak(),                 // Bakuretsu Muteki Bangaioh
    game(b"NBO", Nothing).pak(),                  // Bottom of the 9th
    game(b"NBP", Nothing).pak(),                  // Blues Brothers 2000
    game(b"NBQ", Nothing).pak(),                  // Battletanx - Global Assault
    game(b"NBR", Nothing).pak(),                  // Milo's Astro Lanes
    game(b"NBS", Nothing).pak(),                  // All-Star Baseball '99
    game(b"NBU", Nothing).pak(),                  // Bust-A-Move 2 - Arcade Edition
    game(b"NBV", Eeprom4k).pak(),                 // Bomberman 64: The Second Attack! [Baku Bomberman 2 (J)]
    game(b"NBW", Nothing).pak(),                  // Super Bowling
    game(b"NBX", Nothing).pak(),                  // Battletanx
    game(b"NBY", Nothing).pak(),                  // Bug's Life, A
    game(b"NBZ", Nothing).pak(),                  // NFL Blitz
    game(b"NC2", Nothing),                        // Clay Fighter - Sculptor's Cut
    game(b"NCB", Nothing),                        // Charlie Blast's Territory
    game(b"NCC", Flash1m),                        // Command & Conquer
    game(b"NCD", Nothing).pak(),                  // Carmageddon 64
    game(b"NCE", Nothing).pak(),                  // Nuclear Strike 64
    game(b"NCF", Nothing),                        // Clay Fighter 63 1/3
    game(b"NCG", Eeprom4k).pak(),                 // Choro Q 64 II - Hacha Mecha Grand Prix Race (J)
    game(b"NCH", Eeprom4k),                       // Chopper Attack
    game(b"NCK", Flash1m),                        // NBA Courtside 2 featuring Kobe Bryant
    game(b"NCL", Nothing).pak(),                  // California Speed
    game(b"NCO", Nothing).pak(),                  // Jeremy McGrath Supercross 2000
    game(b"NCR", Eeprom4k).pak(),                 // Penny Racers [Choro Q 64 (J)]
    game(b"NCS", Nothing).pak(),                  // S.C.A.R.S.
    game(b"NCT", Eeprom4k),                       // Chameleon Twist
    game(b"NCU", Eeprom4k).pak(),                 // Cruis'n USA
    game(b"NCV", Flash1m),                        // Cubivore [English translation/patch of Doubutsu Banchou (J)]
    game(b"NCW", Eeprom16k),                      // Cruis'n World
    game(b"NCX", Eeprom4k),                       // Custom Robo
    game(b"NCY", Nothing),                        // South Park - Chef's Luv Shack
    game(b"NCZ", Eeprom16k),                      // Custom Robo V2
    game(b"ND2", Eeprom16k),                      // Doraemon 2: Nobita to Hikari no Shinden
    game(b"ND3J", Eeprom16k),                     // Akumajou Dracula Mokushiroku
    game(b"ND3", Nothing).pak(),                  // Castlevania
    game(b"ND4J", Eeprom16k),                     // Akumajou Dracula Mokushiroku Gaiden: Legend of Cornell
    game(b"ND4", Nothing).pak(),                  // Castlevania: Legacy of Darkness
    game(b"ND6", Eeprom16k),                      // Densha de Go! 64
    game(b"NDA", Flash1m).pak(),                  // Derby Stallion 64
    game(b"NDC", Nothing).pak(),                  // SD Hiryuu no Ken Densetsu (J)
    game(b"NDE", Nothing).pak(),                  // Destruction Derby 64
    game(b"NDF", Nothing),                        // Dance Dance Revolution - Disney Dancing Museum
    game(b"NDH", Nothing).pak(),                  // Duel Heroes
    game(b"NDKJ", Eeprom4k),                      // Space Dynamites
    game(b"NDK", Nothing),                        // Dark Rift
    game(b"NDM", Nothing).pak(),                  // Doom 64
    game(b"NDN", Nothing).pak(),                  // Duke Nukem 64
    game(b"NDO", Eeprom16k),                      // Donkey Kong 64
    game(b"NDP", Flash1m),                        // Dinosaur Planet (Unlicensed)
    game(b"NDQ", Nothing).pak(),                  // Donald Duck: Goin' Quackers
    game(b"NDR", Eeprom4k),                       // Doraemon: Nobita to 3tsu no Seireiseki
    game(b"NDS", Nothing).pak(),                  // J.League Dynamite Soccer 64
    game(b"NDT", Nothing).pak(),                  // South Park
    game(b"NDU", Eeprom4k),                       // Duck Dodgers starring Daffy Duck
    game(b"NDW", Nothing).pak(),                  // Daikatana, John Romero's
    game(b"NDY", Eeprom4k).pak(),                 // Diddy Kong Racing
    game(b"CDZ", Sram768k),                       // Dezaemon 3D
    game(b"NDZ", Nothing).pak(),                  // Duke Nukem - Zero Hour
    game(b"NEA", Eeprom4k),                       // PGA European Tour
    game(b"NEG", Nothing).pak(),                  // Extreme-G
    game(b"NEL", Nothing),                        // Sesame Street - Elmo's Letter Adventure
    game(b"NEN", Nothing),                        // Sesame Street - Elmo's Number Journey
    game(b"NEP", Eeprom16k),                      // Star Wars Episode I: Racer
    game(b"NER", Eeprom4k),                       // Aero Fighters Assault [Sonic Wings Assault (J)]
    game(b"NET", Nothing).pak(),                  // Quest 64 [Eltale Monsters (J) Holy Magic Century (E)]
    game(b"NEV", Eeprom16k),                      // Neon Genesis Evangelion
    game(b"NF2", Eeprom4k),                       // F-1 World Grand Prix II
    game(b"NF9", Nothing).pak(),                  // Fox Sports College Hoops '99
    game(b"NFB", Nothing).pak(),                  // NFL Blitz 2001
    game(b"NFD", Nothing).pak(),                  // Flying Dragon
    game(b"NFF", Nothing).pak(),                  // Fighting Force 64
    game(b"NFG", Eeprom4k).pak(),                 // Fighter Destiny 2
    game(b"NFH", Eeprom4k),                       // In-Fisherman Bass Hunter 64
    game(b"NFL", Nothing).pak(),                  // Madden Football 2001
    game(b"NFO", Nothing).pak(),                  // Forsaken 64
    game(b"NFQ", Nothing).pak(),                  // Razor Freestyle Scooter
    game(b"NFR", Nothing).pak(),                  // F-1 Racing Championship
    game(b"NFS", Nothing).pak(),                  // Famista 64
    game(b"NFU", Eeprom16k),                      // Conker's Bad Fur Day
    game(b"NFW", Eeprom4k),                       // F-1 World Grand Prix
    game(b"NFX", Eeprom4k),                       // Star Fox 64 [Lylat Wars (E)]
    game(b"NFY", Eeprom4k).pak(),                 // Kakutou Denshou: F-Cup Maniax
    game(b"CFZ", Sram256k),                       // F-Zero X (J)
    game(b"NFZ", Sram256k),                       // F-Zero X (U + E)
    game(b"NG2", Nothing).pak(),                  // Extreme-G XG2
    game(b"NG5", Nothing).pak(),                  // Ganbare Goemon - Neo Momoyama Bakufu no Odori [Mystical Ninja Starring Goemon]
    game(b"NG6", Sram256k),                       // Ganmare Goemon: Dero Dero Douchuu Obake Tenkomori
    game(b"NGA", Nothing).pak(),                  // Deadly Arts [G.A.S.P!! Fighter's NEXTream (E + J)]
    game(b"NGB", Nothing).pak(),                  // Top Gear Hyper Bike
    game(b"NGC", Eeprom16k).pak(),                // GT 64: Championship Edition
    game(b"NGD", Nothing).pak(),                  // Gauntlet Legends (J)
    game(b"NGE", Eeprom4k),                       // GoldenEye 007
    game(b"NGF", Eeprom4k),                       // 007 - Goldfinger
    game(b"NGL", Eeprom4k).pak(),                 // Getter Love!!
    game(b"NGM", Nothing).pak(),                  // Goemon's Great Adventure [Mystical Ninja 2 Starring Goemon]
    game(b"NGN", Nothing).pak(),                  // Golden Nugget 64
    game(b"NGP", Sram256k).pak(),                 // Goemon: Mononoke Sugoroku
    game(b"NGRE", Nothing).pak(),                 // Top Gear Rally
    game(b"NGR", Eeprom4k).pak(),                 // Top Gear Rally
    game(b"NGS", Nothing).pak(),                  // Jikkyou G1 Stable
    game(b"NGT", Eeprom16k).pak(),                // City Tour GrandPrix - Zen Nihon GT Senshuken
    game(b"NGU", Eeprom4k),                       // Tsumi to Batsu: Hoshi no Keishousha (Sin and Punishment)
    game(b"NGV", Eeprom4k),                       // Glover
    game(b"NGX", Nothing).pak(),                  // Gauntlet Legends
    game(b"NH5", Nothing).pak(),                  // Nagano Winter Olympics '98 [Hyper Olympics in Nagano (J)]
    game(b"NH9", Nothing).pak(),                  // NHL Breakaway '99
    game(b"NHA", Eeprom4k).pak(),                 // Bomberman 64: Arcade Edition (J)
    game(b"NHB", Nothing),                        // Nintama Rantarou 64 Game Gallery
    game(b"NHC", Nothing).pak(),                  // Hercules - The Legendary Journeys
    game(b"NHF", Eeprom4k),                       // 64 Hanafuda: Tenshi no Yakusoku
    game(b"NHG", Nothing).pak(),                  // F-1 Pole Position 64 / Human Grand Prix
    game(b"NHH", Eeprom4k).pak(),                 // Mario Kart 64 (Super W00ting Hack)
    game(b"NHK", Nothing).pak(),                  // Hiryuu no Ken Twin
    game(b"NHL", Nothing).pak(),                  // NHL Breakaway '98
    game(b"NHM", Nothing).pak(),                  // Mia Hamm Soccer 64
    game(b"NHN", Nothing).pak(),                  // Olympic Hockey Nagano '98
    game(b"NHO", Nothing).pak(),                  // NHL Blades of Steel '99 [NHL Pro '99 (E)]
    game(b"NHP", Eeprom4k),                       // Heiwa Pachinko World 64
    game(b"NHS", Nothing).pak(),                  // Hamster Monogatari 64
    game(b"NHT", Nothing).pak(),                  // Hydro Thunder
    game(b"NHV", Nothing).pak(),                  // Hybrid Heaven (U + E)
    game(b"NHW", Nothing).pak(),                  // Hot Wheels Turbo Racing
    game(b"NHX", Nothing).pak(),                  // Hexen
    game(b"NHYJ", Sram256k).pak(),                // Hybrid Heaven
    game(b"NHY", Nothing).pak(),                  // Hybrid Heaven
    game(b"NIB", Sram256k),                       // Itoi Shigesato no Bass Tsuri No. 1 Kettei Ban!
    game(b"NIC", Eeprom4k),                       // Indy Racing 2000
    game(b"NIJ", Eeprom4k),                       // Indiana Jones and the Infernal Machine
    game(b"NIM", Eeprom16k),                      // Ide Yosuke no Mahjong Juku
    game(b"NIR", Eeprom4k),                       // Utchan Nanchan no Hono no Challenger: Denryuu Ira Ira Bou
    game(b"NIS", Nothing).pak(),                  // International Superstar Soccer 2000
    game(b"NIV", Nothing).pak(),                  // Space Invaders
    game(b"NJ2", Nothing).pak(),                  // Wonder Project J2 - Koruro no Mori no Jozet (J)
    game(b"NJ3", Nothing).pak(),                  // Jikkyou World Soccer 3
    game(b"NJ5", Sram256k).pak(),                 // Jikkyou Powerful Pro Yakyuu 5
    game(b"NJA", Nothing).pak(),                  // NBA Jam 2000
    game(b"NJE", Nothing).pak(),                  // J.League Eleven Beat 1997
    game(b"NJF", Flash1m),                        // Jet Force Gemini [Star Twins (J)]
    game(b"NJG", Sram256k),                       // Jinsei Game 64
    game(b"NJK", Eeprom4k),                       // Viewpoint 2064 (Master)
    game(b"NJL", Nothing).pak(),                  // J.League Live 64
    game(b"NJM", Eeprom4k),                       // Earthworm Jim 3D
    game(b"NJO", Nothing),                        // Jeopardy!
    game(b"NJP", Nothing).pak(),                  // International Superstar Soccer 64 [Jikkyo J-League Perfect Striker (J)]
    game(b"NJQ", Nothing),                        // Batman Beyond - Return of the Joker [Batman of the Future - Return of the Joker (E)]
    game(b"NK2", Eeprom4k),                       // Snowboard Kids 2 [Chou Snobow Kids (J)]
    game(b"NK4J", Sram256k).revision(0),          // Hoshi no Kirby 64 1.0
    game(b"NK4J", Sram256k).revision(1),          // Hoshi no Kirby 64 1.1
    game(b"NK4", Eeprom16k),                      // Kirby 64: The Crystal Shards
    game(b"NKA", Eeprom4k).pak(),                 // Fighters Destiny [Fighting Cup (J)]
    game(b"NKE", Nothing),                        // Knife Edge - Nose Gunner
    game(b"NKG", Sram256k).pak(),                 // Major League Baseball featuring Ken Griffey Jr.
    game(b"NKI", Eeprom4k).pak(),                 // Killer Instinct Gold
    game(b"NKJ", Flash1m),                        // Ken Griffey Jr.'s Slugfest
    game(b"NKK", Nothing).pak(),                  // Knockout Kings 2000
    game(b"NKM", Nothing).pak(),                  // Pro Mahjong Kiwame 64 (J)
    game(b"NKR", Nothing).pak(),                  // Rakuga Kids (E)
    game(b"NKT", Eeprom4k).pak(),                 // Mario Kart 64
    game(b"NL2", Nothing).pak(),                  // Top Gear Rally 2
    game(b"CLB", Eeprom4k),                       // Mario Party (NTSC)
    game(b"NLB", Eeprom4k),                       // Mario Party (PAL)
    game(b"NLC", Nothing).pak(),                  // Automobili Lamborghini [Super Speed Race 64 (J)]
    game(b"NLG", Nothing).pak(),                  // LEGO Racers
    game(b"NLL", Eeprom4k),                       // Last Legion UX
    game(b"NLR", Eeprom4k),                       // Lode Runner 3-D
    game(b"NM3", Nothing),                        // Monster Truck Madness 64
    game(b"NM4", Nothing).pak(),                  // Mortal Kombat 4
    game(b"NM6", Flash1m),                        // Mega Man 64
    game(b"NM8", Eeprom16k),                      // Mario Tennis
    game(b"NM9", Nothing).pak(),                  // Harukanaru Augusta Masters 98
    game(b"NMA", Nothing).pak(),                  // Jangou Simulation Mahjong Do 64
    game(b"NMB", Nothing).pak(),                  // Mike Piazza's Strike Zone
    game(b"CMD", Nothing),                        // Nintendo 64 Modem
    game(b"NMD", Nothing).pak(),                  // Madden Football 2000
    game(b"NME", Nothing),                        // Mace - The Dark Age
    game(b"NMF", Sram256k),                       // Mario Golf 64
    game(b"NMGD", Nothing).pak(),                 // Racing Simulation 2
    game(b"NMG", Eeprom4k).pak(),                 // Monaco Grand Prix (disputed)
    game(b"NMH", Nothing),                        // Mahjong Hourouki Classic
    game(b"NMI", Eeprom4k),                       // Mission: Impossible
    game(b"NMJ", Nothing).pak(),                  // Mahjong 64
    game(b"NMK", Nothing),                        // Mortal Kombat Trilogy
    game(b"NML", Eeprom4k),                       // Mickey's Speedway USA [Mickey no Racing Challenge USA (J)]
    game(b"NMM", Nothing).pak(),                  // Mahjong Master
    game(b"NMN", Nothing),                        // Mini Racers (Prototype)
    game(b"NMO", Eeprom4k),                       // Monopoly
    game(b"NMP", Nothing),                        // Mario no Photopie
    game(b"NMQ", Flash1m),                        // Paper Mario
    game(b"NMR", Eeprom4k).pak(),                 // Multi-Racing Championship
    game(b"NMSJ", Eeprom4k).pak(),                // Morita Shougi 64
    game(b"NMS", Nothing).pak(),                  // Duke Nukem 64 (beta)
    game(b"NMT", Nothing),                        // Magical Tetris Challenge
    game(b"NMU", Eeprom4k).pak(),                 // Big Mountain 2000
    game(b"NMV", Eeprom16k),                      // Mario Party 3
    game(b"NMW", Eeprom4k),                       // Mario Party 2
    game(b"NMX", Eeprom16k).pak(),                // Excitebike 64
    game(b"NMY", Nothing).pak(),                  // Mortal Kombat Mythologies - Sub-Zero
    game(b"NMZ", Eeprom4k).pak(),                 // Zool - Majou Tsukai Densetsu (J)
    game(b"NN2", Nothing).pak(),                  // Nascar 2000
    game(b"NN6", Eeprom4k),                       // Dr. Mario 64
    game(b"NNA", Eeprom4k),                       // Star Wars Episode I: Battle for Naboo
    game(b"NNB", Eeprom16k).pak(),                // Kobe Bryant in NBA Courtside
    game(b"NNC", Nothing).pak(),                  // Nightmare Creatures
    game(b"NNL", Nothing).pak(),                  // NBA Live 2000
    game(b"NNM", Nothing).pak(),                  // Namco Museum 64
    game(b"NNR", Nothing).pak(),                  // Pro Mahjong Tsuwamono 64 - Jansou Battle ni Chousen (J)
    game(b"NNS", Nothing).pak(),                  // Beetle Adventure Racing
    game(b"NO2", Nothing).pak(),                  // 64 Oozumou 2
    game(b"NO7", Nothing).pak(),                  // The World Is Not Enough
    game(b"NOB", Sram256k).pak(),                 // Ogre Battle 64: Person of Lordly Caliber
    game(b"NOF", Nothing).pak(),                  // Offroad Challenge
    game(b"NOH", Nothing),                        // Transformers Beast Wars - Transmetals
    game(b"NOM", Nothing).pak(),                  // Onegai Monsters
    game(b"NOS", Eeprom4k).pak(),                 // 64 Oozumou
    game(b"NOW", Nothing).pak(),                  // Brunswick Circuit Pro Bowling
    game(b"CP2", Flash1m),                        // Pocket Monsters Stadium 2 (J)
    game(b"NP2", Eeprom4k).pak(),                 // Chou Kuukan Night Pro Yakyuu King 2 (J)
    game(b"NP3", Flash1m),                        // Pokemon Stadium 2 [Pocket Monsters Stadium - Kin Gin (J)]
    game(b"NP4", Sram256k).pak(),                 // Jikkyou Powerful Pro Yakyuu 4
    game(b"NP6", Sram256k).pak(),                 // Jikkyou Powerful Pro Yakyuu 6
    game(b"NP9", Nothing).pak(),                  // Ms. Pac-Man - Maze Madness
    game(b"NPA", Sram256k).pak(),                 // Jikkyou Powerful Pro Yakyuu 2000
    game(b"NPB", Nothing).pak(),                  // Puzzle Bobble 64 (J)
    game(b"NPC", Nothing).pak(),                  // Pachinko 365 Nichi (J)
    game(b"NPD", Eeprom16k).pak(),                // Perfect Dark
    game(b"NPE", Sram256k).pak(),                 // Jikkyou Powerful Pro Yakyuu Basic Ban 2001
    game(b"NPF", Flash1m),                        // Pokemon Snap [Pocket Monsters Snap (J)]
    game(b"NPG", Eeprom4k),                       // Hey You, Pikachu! [Pikachu Genki Dechu (J)]
    game(b"NPH", Flash1m),                        // Pokémon Snap Station
    game(b"NPK", Nothing).pak(),                  // Chou Kuukan Night Pro Yakyuu King (J)
    game(b"NPL", Nothing).pak(),                  // Power League 64 (J)
    game(b"NPM", Sram256k).pak(),                 // Premier Manager 64
    game(b"NPN", Flash1m),                        // Pokemon Puzzle League
    game(b"NPO", Flash1m),                        // Pokemon Stadium
    game(b"NPP", Eeprom16k).pak(),                // Parlor! Pro 64: Pachinko Jikki Simulation Game
    game(b"NPQ", Nothing),                        // Powerpuff Girls, The - Chemical X-Traction
    game(b"NPR", Nothing).pak(),                  // South Park Rally
    game(b"CPS", Sram256k),                       // Pocket Monsters Stadium (J)
    game(b"NPS", Sram256k).pak(),                 // Jikkyou J.League 1999: Perfect Striker 2
    game(b"NPT", Eeprom4k),                       // Puyo Puyon Party
    game(b"NPU", Nothing).pak(),                  // Power Rangers - Lightspeed Rescue
    game(b"NPW", Eeprom4k),                       // Pilotwings 64
    game(b"NPX", Nothing).pak(),                  // Polaris SnoCross
    game(b"BPY", Eeprom4k),                       // Pyoro 64
    game(b"NPY", Eeprom4k),                       // Puyo Puyo Sun 64
    game(b"NPZ", Nothing).pak(),                  // Susume! Taisen Puzzle Dama Toukon! Marumata Chou (J)
    game(b"NQ2", Nothing).pak(),                  // Quake 2
    game(b"NQ8", Nothing).pak(),                  // NFL Quarterback Club '98
    game(b"NQ9", Nothing).pak(),                  // NFL Quarterback Club '99
    game(b"NQB", Nothing).pak(),                  // NFL Quarterback Club 2000
    game(b"NQC", Nothing).pak(),                  // NFL Quarterback Club 2001
    game(b"NQK", Nothing).pak(),                  // Quake 64
    game(b"NR2", Nothing).pak(),                  // Rush 2 - Extreme Racing USA
    game(b"NR3", Nothing).pak(),                  // Stunt Racer 64
    game(b"NR6", Nothing).pak(),                  // Tom Clancy's Rainbow Six
    game(b"NR7", Eeprom16k),                      // Robot Poncots 64: 7tsu no Umi no Caramel
    game(b"NRA", Eeprom4k).pak(),                 // Rally '99 (J)
    game(b"NRC", Eeprom4k),                       // Top Gear Overdrive
    game(b"NRD", Nothing).pak(),                  // Ready 2 Rumble Boxing
    game(b"NRE", Sram256k),                       // Resident Evil 2
    game(b"NRG", Nothing),                        // Rugrats - Scavenger Hunt [Treasure Hunt (E)]
    game(b"NRH", Flash1m),                        // Rockman Dash - Hagane no Boukenshin (J)
    game(b"NRI", Sram256k).pak(),                 // New Tetris, The
    game(b"NRK", Nothing).pak(),                  // Rugrats in Paris - The Movie
    game(b"NRO", Nothing).pak(),                  // Road Rash 64
    game(b"NRP", Nothing).pak(),                  // Rampage - World Tour
    game(b"NRR", Nothing).pak(),                  // Roadster's Trophy
    game(b"NRS", Eeprom4k),                       // Star Wars: Rogue Squadron [Shutsugeki! Rogue Chuutai (J)]
    game(b"NRT", Nothing).pak(),                  // Rat Attack
    game(b"NRU", Nothing).pak(),                  // San Francisco Rush 2049
    game(b"NRV", Nothing).pak(),                  // Re-Volt
    game(b"NRW", Nothing).pak(),                  // Turok: Rage Wars
    game(b"NRX", Nothing).pak(),                  // Robotron 64
    game(b"NRZ", Eeprom16k),                      // Ridge Racer 64
    game(b"NS2", Nothing).pak(),                  // Simcity 2000
    game(b"NS3", Eeprom4k).pak(),                 // AI Shougi 3
    game(b"NS4", Sram256k).pak(),                 // Super Robot Taisen 64
    game(b"NS6", Eeprom4k),                       // Star Soldier: Vanishing Earth
    game(b"NSA", Eeprom4k),                       // Sonic Wings Assault (J)
    game(b"NSB", Nothing).pak(),                  // Twisted Edge - Extreme Snowboarding [King Hill 64 - Extreme Snowboarding (J)]
    game(b"NSC", Eeprom4k),                       // Starshot: Space Circus Fever
    game(b"NSD", Nothing).pak(),                  // Shadow Man
    game(b"NSF", Nothing).pak(),                  // San Francisco Rush - Extreme Racing
    game(b"NSG", Nothing).pak(),                  // Shadowgate 64 - Trials Of The Four Towers
    game(b"NSH", Nothing).pak(),                  // Saikyou Habu Shougi (J)
    game(b"NSI", Sram256k).pak(),                 // Fushigi no Dungeon: Fuurai no Shiren 2
    game(b"NSJ", Nothing),                        // J.League Tactics Soccer
    game(b"NSK", Nothing).pak(),                  // Snowboard Kids [Snobow Kids (J)]
    game(b"NSL", Nothing).pak(),                  // Spider-Man
    game(b"NSM", Eeprom4k),                       // Super Mario 64
    game(b"NSN", Eeprom4k).pak(),                 // Snow Speeder (J)
    game(b"NSO", Nothing).pak(),                  // NBA Showtime - NBA on NBC
    game(b"NSP", Nothing).pak(),                  // Superman
    game(b"NSQ", Flash1m),                        // StarCraft 64
    game(b"NSS", Eeprom4k),                       // Super Robot Spirits
    game(b"NST", Nothing).pak(),                  // Eikou no Saint Andrews
    game(b"NSU", Eeprom4k),                       // Rocket: Robot on Wheels
    game(b"NSV", Eeprom4k),                       // Space Station Silicon Valley
    game(b"NSW", Eeprom4k),                       // Star Wars: Shadows of the Empire [Teikoku no Kage (J)]
    game(b"NSX", Nothing).pak(),                  // Supercross 2000
    game(b"NSY", Nothing).pak(),                  // Scooby-Doo! - Classic Creep Capers
    game(b"NSZ", Nothing).pak(),                  // NFL Blitz - Special Edition
    game(b"NT2", Nothing).pak(),                  // Turok 2 - Seeds of Evil [Violence Killer - Turok New Generation (J)]
    game(b"NT3J", Sram256k).pak(),                // Shin Nihon Pro Wrestling: Toukon Road 2
    game(b"NT3", Nothing).pak(),                  // Tony Hawk's Pro Skater 3
    game(b"NT4", Nothing).pak(),                  // CyberTiger
    game(b"NT6", Eeprom4k),                       // Tetris 64
    game(b"NT9", Flash1m),                        // Tigger's Honey Hunt
    game(b"NTA", Nothing).pak(),                  // Disney's Tarzan
    game(b"NTB", Eeprom4k),                       // Transformers: Beast Wars Metals 64 (J)
    game(b"NTC", Eeprom4k),                       // 64 Trump Collection
    game(b"NTD", Nothing).pak(),                  // O.D.T. (Unreleased)
    game(b"NTE", Sram256k),                       // 1080 Snowboarding
    game(b"NTF", Nothing).pak(),                  // Tony Hawk's Pro Skater
    game(b"NTH", Nothing).pak(),                  // Toy Story 2 - Buzz Lightyear to the Rescue!
    game(b"NTI", Nothing).pak(),                  // WWF: Attitude
    game(b"NTJ", Eeprom4k),                       // Tom & Jerry in Fists of Fury
    game(b"NTK", Nothing).pak(),                  // Turok 3 - Shadow of Oblivion
    game(b"NTM", Eeprom4k),                       // Mischief Makers [Yuke Yuke!! Trouble Makers (J)]
    game(b"NTN", Eeprom4k),                       // All Star Tennis '99
    game(b"NTO", Nothing).pak(),                  // Shin Nihon Pro Wrestling - Toukon Road - Brave Spirits (J)
    game(b"NTP", Eeprom4k),                       // Tetrisphere
    game(b"NTQ", Nothing).pak(),                  // Tony Hawk's Pro Skater 2
    game(b"NTR", Eeprom4k).pak(),                 // Top Gear Rally (J + E)
    game(b"NTS", Nothing).pak(),                  // Centre Court Tennis [Let's Smash (J)]
    game(b"NTT", Nothing).pak(),                  // Tonic Trouble
    game(b"NTU", Nothing).pak(),                  // Turok: Dinosaur Hunter [Turok: Jikuu Senshi (J)]
    game(b"NTW", Eeprom4k).pak(),                 // 64 de Hakken!! Tamagotchi
    game(b"NTX", Eeprom4k),                       // Taz Express
    game(b"NUB", Eeprom16k).pak(),                // PD Ultraman Battle Collection 64
    game(b"NUM", Sram256k),                       // Nushi Zuri 64: Shiokaze ni Notte
    game(b"NUT", Sram256k).pak(),                 // Nushi Zuri 64
    game(b"NV2", Nothing).pak(),                  // Chameleon Twist 2 (J)
    game(b"NV3", Nothing).pak(),                  // Micro Machines 64 Turbo
    game(b"NV8", Nothing).pak(),                  // Vigilante 8
    game(b"NVB", Sram256k),                       // Bass Rush - ECOGEAR PowerWorm Championship (J)
    game(b"NVC", Nothing).pak(),                  // Virtual Chess 64
    game(b"NVG", Nothing).pak(),                  // Vigilante 8 - Second Offense
    game(b"NVL", Eeprom4k),                       // V-Rally Edition '99
    game(b"NVP", Sram256k).pak(),                 // Virtual Pro Wrestling 64
    game(b"NVR", Nothing).pak(),                  // Virtual Pool 64
    game(b"NVY", Eeprom4k),                       // V-Rally Edition '99 (J)
    game(b"NW2", Sram256k),                       // WCW-nWo Revenge
    game(b"NW3", Nothing).pak(),                  // WCW: Nitro
    game(b"NW4", Flash1m).pak(),                  // WWF No Mercy
    game(b"NW8", Nothing).pak(),                  // Wayne Gretzky's 3D Hockey '98
    game(b"NWA", Nothing),                        // War Gods
    game(b"NWB", Nothing).pak(),                  // Iggy's Reckin' Balls [Iggy-kun no Bura Bura Poyon (J)]
    game(b"NWC", Eeprom4k),                       // Wild Choppers
    game(b"NWD", Nothing).pak(),                  // Winback - Covert Operations
    game(b"NWF", Nothing),                        // Wheel of Fortune
    game(b"NWG", Nothing).pak(),                  // Wayne Gretzky's 3D Hockey
    game(b"NWI", Nothing).pak(),                  // ECW Hardcore Revolution
    game(b"NWK", Nothing).pak(),                  // Michael Owens WLS 2000 [World League Soccer 2000 (E) / Telefoot Soccer 2000 (F)]
    game(b"NWL", Sram256k),                       // Waialae Country Club: True Golf Classics
    game(b"NWM", Nothing).pak(),                  // WCW: Mayhem
    game(b"NWN", Nothing).pak(),                  // WCW vs. nWo - World Tour
    game(b"NWO", Nothing).pak(),                  // World Driver Championship
    game(b"NWP", Nothing).pak(),                  // Wipeout 64
    game(b"NWQ", Eeprom4k).pak(),                 // Rally Challenge 2000 (disputed)
    game(b"NWR", Eeprom4k).pak(),                 // Wave Race 64
    game(b"NWS", Nothing).pak(),                  // International Superstar Soccer '98 [Jikkyo World Soccer - World Cup France '98 (J)]
    game(b"NWTJ", Eeprom4k),                      // Wetrix
    game(b"NWT", Nothing).pak(),                  // Wetrix
    game(b"NWU", Eeprom4k),                       // Worms Armageddon (E)
    game(b"NWV", Nothing).pak(),                  // WCW: Backstage Assault
    game(b"NWW", Nothing).pak(),                  // WWF: War Zone
    game(b"NWX", Sram256k).pak(),                 // WWF WrestleMania 2000
    game(b"NWZ", Nothing).pak(),                  // NBA In the Zone 2000
    game(b"NX2", Nothing).pak(),                  // Gex 64 - Enter the Gecko
    game(b"NX3", Nothing).pak(),                  // Gex 3 - Deep Cover Gecko
    game(b"NX7", Eeprom16k).pak(),                // GoldenEye - X
    game(b"NXC", Nothing).pak(),                  // Xeno Crisis (Aftermarket)
    game(b"NXF", Nothing).pak(),                  // Xena Warrior Princess - The Talisman of Fate
    game(b"NXG", Nothing).pak(),                  // NBA Hangtime
    game(b"NXO", Eeprom4k),                       // Cruis'n Exotica
    game(b"NY2", Nothing).pak(),                  // Rayman 2 - The Great Escape
    game(b"NYK", Eeprom4k),                       // Yakouchuu II: Satsujin Kouro
    game(b"NYP", Nothing).pak(),                  // Paperboy
    game(b"NYS", Eeprom16k),                      // Yoshi's Story
    game(b"NYW", Sram256k).pak(),                 // Harvest Moon 64
    game(b"CZL", Sram256k),                       // Legend of Zelda: Ocarina of Time [Zelda no Densetsu - Toki no Ocarina (J)]
    game(b"NZL", Sram256k),                       // Legend of Zelda: Ocarina of Time (E)
    game(b"NZO", Nothing).pak(),                  // Battlezone - Rise of the Black Dogs
    game(b"NZS", Flash1m),                        // Legend of Zelda: Majora's Mask [Zelda no Densetsu - Mujura no Kamen (J)]
];

#[cfg(test)]
mod tests {
    use super::*;

    /// What the table says of a game: `Err` where it does not hold the game, `Ok(None)` where it
    /// holds it with no chip.
    fn chip(id: &str, revision: Option<u8>) -> Result<Option<(SaveKind, u32)>, ()> {
        let cart = cartridge(id, revision).ok_or(())?;
        Ok(cart.save.map(|save| (save.kind, save.size.unwrap())))
    }

    const EEPROM_4K: Option<(SaveKind, u32)> = Some((SaveKind::Eeprom, 512));
    const EEPROM_16K: Option<(SaveKind, u32)> = Some((SaveKind::Eeprom, 2048));
    const SRAM_256K: Option<(SaveKind, u32)> = Some((SaveKind::Sram, 32 * 1024));

    #[test]
    fn every_chip_is_found_by_its_game() {
        assert_eq!(chip("NSME", None), Ok(EEPROM_4K), "Super Mario 64");
        assert_eq!(chip("NB7E", None), Ok(EEPROM_16K), "Banjo-Tooie");
        assert_eq!(chip("NZLE", None), Ok(SRAM_256K), "Ocarina of Time");
        assert_eq!(chip("CDZJ", None), Ok(Some((SaveKind::Sram, 96 * 1024))), "Dezaemon 3D");
        assert_eq!(chip("NZSE", None), Ok(Some((SaveKind::Flash, 128 * 1024))), "Majora's Mask");
    }

    #[test]
    fn a_game_with_no_chip_is_not_a_game_nobody_knows() {
        let trilogy = cartridge("NMKE", None).expect("Mortal Kombat Trilogy is in the table");
        assert_eq!(trilogy.save, None);
        assert_eq!(cartridge("NQQE", None), None, "no game has that code");
    }

    #[test]
    fn the_region_decides_where_the_releases_differ() {
        assert_eq!(chip("ND3J", None), Ok(EEPROM_16K), "Akumajou Dracula Mokushiroku");
        assert_eq!(chip("ND3E", None), Ok(None), "Castlevania saves to a pak");
        assert_eq!(chip("NT3J", None), Ok(SRAM_256K), "Toukon Road 2");
        assert_eq!(chip("NT3E", None), Ok(None), "Tony Hawk's Pro Skater 3 shares its code");
        assert_eq!(chip("NGRE", None), Ok(None), "Top Gear Rally in America");
        assert_eq!(chip("NGRP", None), Ok(EEPROM_4K), "and in Europe");
    }

    #[test]
    fn the_revision_decides_hoshi_no_kirby_64_and_is_not_guessed() {
        assert_eq!(chip("NK4J", Some(0)), Ok(SRAM_256K));
        assert_eq!(chip("NK4J", Some(1)), Ok(SRAM_256K));
        assert_eq!(chip("NK4J", Some(2)), Ok(EEPROM_16K));
        assert_eq!(chip("NK4J", None), Err(()), "1.0 and 1.2 disagree, so the revision is needed");
        assert_eq!(chip("NK4E", None), Ok(EEPROM_16K), "every Kirby 64 outside Japan agrees");
    }

    #[test]
    fn a_serial_reads_as_its_game_id() {
        for serial in ["NKTE", "NUS-NKTE", "NUS-NKTE-USA"] {
            let kart = cartridge(serial, None).expect(serial);
            assert_eq!((kart.save.map(|save| save.size), kart.controller_pak), (Some(Some(512)), true));
        }
        assert_eq!(cartridge("NKT", None), None, "a game ID without its region");
        assert_eq!(cartridge("NUS-NKTEX", None), None);
    }

    #[test]
    fn doubutsu_no_mori_has_the_clock() {
        let mori = cartridge("NAFJ", None).unwrap();
        assert_eq!(mori.clock, Some(Clock::Joybus));
        assert_eq!(mori.save.map(|save| save.kind), Some(SaveKind::Flash));
    }

    #[test]
    fn a_homebrew_id_is_left_to_its_header() {
        assert_eq!(cartridge("NEDE", None), None);
    }

    #[test]
    fn every_row_can_be_reached() {
        for (at, row) in GAMES.iter().enumerate() {
            assert!(matches!(row.id.len(), 3 | 4), "{row:?}");
            assert!(row.id.iter().all(u8::is_ascii_alphanumeric), "{row:?}");
            assert!(row.revision.is_none() || row.id.len() == 4, "a revision without a region: {row:?}");
            let shadow = GAMES[..at].iter().find(|earlier| {
                row.id.starts_with(earlier.id)
                    && (earlier.revision.is_none() || earlier.revision == row.revision)
            });
            assert!(shadow.is_none(), "{row:?} is never reached past {shadow:?}");
        }
    }
}
