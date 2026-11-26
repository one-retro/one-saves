//! Nintendo DS game card saves, and the files they arrive in.
//!
//! A DS card keeps its saves in one serial chip, and a bundle carries every byte of it: 512 bytes
//! of EEPROM up to 1 MiB of flash. The chip's length is all there is to tell one chip from another,
//! so a DS save carries no key naming it, and its length has to be right. Nothing in the card's
//! header says it, and nothing in a file always does, so [`read`] takes the chip from the game where
//! the caller knows it ([`NdsOptions::chip`], which `rom_id::nds` answers from the game code), and
//! otherwise only from a file whose length it can trust.
//!
//! # The files
//!
//! | Written by | What the file holds | Read as |
//! | ---------- | ------------------- | ------- |
//! | melonDS, melonDS DS, NooDS, a flash cartridge, a card reader | the chip | the chip |
//! | DeSmuME (`.dsv`) | the chip padded with 0xFF to the size DeSmuME worked out, then a 122-byte footer | the chip; 256 KiB with its upper half erased needs the game, since DeSmuME keeps a 1 Mbit EEPROM that way |
//! | SkyEmu (`.srm`) | the chip, then 0x00 to 8 MiB | needs the game |
//! | Action Replay DS (`.duc`, `.dss`) | a header, then the chip, sometimes compressed | the chip; 256 KiB needs the game, since the converters that wrote most of these write every chip at that |
//! | no$gba | a header, then the chip, sometimes compressed | the chip |
//! | `DSXS` | a header, then the chip gzipped | the chip |
//! | DraStic's save state | the chip in a record of its own, inside the state | the chip, at the size the record gives |
//! | DeSmuME's save state | the chip inside the core's data | through [`crate::state`], which lifts it out |
//!
//! A file shorter than its chip is filled out with 0xFF, which is what an erased chip reads. A file
//! longer than its chip is cut, but only where what follows the chip is padding, 0xFF or the
//! producer's own fill, or the chip again as a reader that reads past its end gets it: anything
//! else past the end is data no card holds. A file whose chip cannot be told is carried as it is, as
//! an `aux` part named by [`NdsOptions::file_name`], rather than as a save of the wrong chip. So is
//! a NAND card's, which the format does not cover yet.
//!
//! SkyEmu also keeps the console clock's registers in a `.rtc` beside the save. The clock is the
//! console's, and SkyEmu sets it from the host's every frame, so the file is no part of the save and
//! nothing here reads it.

use one_saves::{Bundle, Game, Header, Part, PartKind, Shape, Slug, Source};
use one_saves_registry::SaveLayout;

use crate::error::{Error, Result};

/// The system these saves are for.
pub const SYSTEM: &str = "nds";

/// What a chip holds where nothing has been written.
const ERASED: u8 = 0xFF;

/// Every length a DS card's save chip comes in: EEPROM of 4, 64 and 512 Kbit and 1 Mbit, FRAM of
/// 256 Kbit, and flash of 2, 4 and 8 Mbit.
pub const CHIPS: [usize; 8] = [512, 8192, 32_768, 65_536, 131_072, 262_144, 524_288, 1_048_576];

/// Whether a length is a DS save chip's.
#[must_use]
pub fn is_chip(len: usize) -> bool {
    CHIPS.contains(&len)
}

/// The length that says nothing on its own in some files: the converters that write Action Replay
/// files write every chip at it, and DeSmuME a 1 Mbit EEPROM.
const AMBIGUOUS: usize = 256 * 1024;

/// The largest a file inflates to before it is refused: a NAND card's 64 MiB, which is the most a
/// DS card keeps, with room for a state around it. A DS save is never more, so a file that claims
/// more is not one.
#[cfg(feature = "nds-zlib")]
const MAX_INFLATED: u64 = 80 << 20;

/// What a save arrived in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    /// Nothing: the chip, or the chip as the producer pads it.
    Bare,
    /// DeSmuME's `.dsv`, the chip and its footer.
    Desmume,
    /// An Action Replay DS file. Version 1 opens with `ARDS000000000001` and a 500-byte header;
    /// version 2 with a header of 0xA4 bytes. Some version 1 files hold the chip compressed.
    ActionReplay {
        /// The header's version, 1 or 2.
        version: u8,
        /// Whether the chip was run-length compressed.
        compressed: bool,
    },
    /// no$gba's backup file, `NocashGbaBackupMediaSavDataFile`.
    NoGba,
    /// A `DSXS` file: a 256-byte header, then the chip gzipped.
    Dsxs,
    /// A DraStic save state, which holds the chip in a record of its own.
    DrasticState,
    /// A DeSmuME save state, which holds the chip inside the core's data.
    DesmumeState,
}

impl Container {
    /// What to call it in a message.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Container::Bare => "DS save",
            Container::Desmume => "DeSmuME DS save",
            Container::ActionReplay { compressed: false, .. } => "Action Replay DS save",
            Container::ActionReplay { compressed: true, .. } => "Action Replay DS save, compressed",
            Container::NoGba => "no$gba DS save",
            Container::Dsxs => "DSXS DS save",
            Container::DrasticState => "DS save, from a DraStic state",
            Container::DesmumeState => "DS save, from a DeSmuME state",
        }
    }
}

/// A save taken out of whatever it arrived in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened {
    /// The save memory as the file holds it: the chip, or the chip padded or cut short.
    pub data: Vec<u8>,
    /// What it arrived in.
    pub container: Container,
    /// The chip's length, where the file itself records it reliably, as a DraStic state does.
    pub chip: Option<usize>,
    /// The game code the file records, as a DraStic state and a DeSmuME state can.
    pub game_code: Option<String>,
    /// What a player typed about the save into the file, as an Action Replay DS asks for.
    pub note: Option<String>,
}

impl Opened {
    /// A save that arrived in nothing.
    fn bare(data: &[u8]) -> Self {
        Opened { data: data.to_vec(), container: Container::Bare, chip: None, game_code: None, note: None }
    }
}

/// Whether bytes are a DS save in a file of its own making, by the signature it opens or ends with.
///
/// A bare chip carries nothing to know it by, and is not recognised here.
#[must_use]
pub fn is_container(bytes: &[u8]) -> bool {
    bytes.starts_with(ARDS_V1)
        || is_ards_v2(bytes)
        || bytes.starts_with(NOGBA_ID)
        || bytes.starts_with(DSXS_ID)
        || bytes.starts_with(DRASTIC_ID)
        || dsv(bytes).is_some()
}

/// Takes a save out of whatever it arrived in. A file that is none of them is the chip, as it is.
///
/// # Errors
///
/// [`Error::Corrupt`] for a file that opens as one of them and does not hold together;
/// [`Error::Unsupported`] for a compressed one in a build without `nds-zlib`.
pub fn open(bytes: &[u8]) -> Result<Opened> {
    let outer = if bytes.starts_with(ARDS_V1) || is_ards_v2(bytes) {
        action_replay(bytes)?
    } else if bytes.starts_with(DSXS_ID) {
        dsxs(bytes)?
    } else if bytes.starts_with(DRASTIC_ID) {
        return drastic(bytes);
    } else {
        Opened::bare(bytes)
    };
    // An Action Replay file sometimes wraps another tool's file whole: DeSmuME's, or no$gba's.
    if let Some((data, _)) = dsv(&outer.data) {
        let data = data.to_vec();
        let container = if outer.container == Container::Bare { Container::Desmume } else { outer.container };
        return Ok(Opened { data, container, ..outer });
    }
    if outer.data.starts_with(NOGBA_ID) {
        let data = no_gba(&outer.data)?;
        let container = if outer.container == Container::Bare { Container::NoGba } else { outer.container };
        return Ok(Opened { data, container, ..outer });
    }
    Ok(outer)
}

/// How to wrap a DS save.
#[derive(Debug, Clone, Default)]
pub struct NdsOptions {
    /// Who produced the file.
    pub source: Option<Source>,
    /// What is known about the game.
    pub game: Option<Game>,
    /// A human-readable note about the bundle. Without one, what the player typed into the file.
    pub description: Option<String>,
    /// When the bundle was assembled, in whole epoch seconds.
    pub bundled_at: Option<i64>,
    /// How long the card's save memory is, from the game: what `rom_id::nds` answers for its game
    /// code, from its ROM's header or a catalog. NAND's length too, which is carried as `aux`.
    pub chip: Option<usize>,
    /// How the producer keeps the chip in its file, for one the registry does not list. Otherwise
    /// the layout is the registry's, for the core [`source`](Self::source) names.
    pub layout: Option<SaveLayout>,
    /// The file's name, which an `aux` part is named by where the chip cannot be told.
    pub file_name: Option<String>,
}

/// A DS save read into a bundle, and what was found along the way.
#[derive(Debug, Clone)]
pub struct Read {
    /// The bundle: a save of the chip, or the file as an `aux` part.
    pub bundle: Bundle,
    /// What the save arrived in.
    pub container: Container,
    /// The chip the save part holds, or `None` where the file went as `aux`.
    pub chip: Option<usize>,
    /// Why the file went as `aux`, where it did.
    pub why_aux: Option<String>,
}

/// Reads a DS save, in whatever it arrived in, into a save bundle.
///
/// # Errors
///
/// What [`open`] finds wrong with the file, and an empty file.
pub fn read(bytes: &[u8], options: &NdsOptions) -> Result<Read> {
    if bytes.is_empty() {
        return Err(Error::NotConvertible("this save is zero bytes".into()));
    }
    let opened = open(bytes)?;
    read_opened(bytes, &opened, options)
}

/// Reads a save already taken out of what it arrived in: a DeSmuME state's, lifted out by
/// [`crate::state`]. `file` is what goes as `aux` where the chip cannot be told.
///
/// # Errors
///
/// An empty save.
pub fn read_opened(file: &[u8], opened: &Opened, options: &NdsOptions) -> Result<Read> {
    if opened.data.is_empty() {
        return Err(Error::NotConvertible("this file holds a DS save of zero bytes".into()));
    }
    let layout = options.layout.or_else(|| layout_of(options.source.as_ref()));
    let known = options.chip.or(opened.chip);
    let memory = known.map(|size| crate::layout::SaveMemory::Memory { size: Some(size) });
    let data = crate::layout::normalize(&opened.data, SYSTEM, layout, memory);

    let padded_with = match layout {
        Some(SaveLayout::Padded { fill, .. }) => Some(fill),
        _ => None,
    };
    let verdict = match known {
        Some(chip) if !is_chip(chip) => Err(format!(
            "the card keeps its saves in {} of NAND, which this version of the format carries only as \
             the file it came in",
            size_of(chip)
        )),
        Some(chip) => fit(&data, chip, padded_with).map(|chip_bytes| (chip, chip_bytes)),
        None => told_by_length(&data, opened.container, layout, keeps_chip(options.source.as_ref()))
            .map(|chip| (chip, data.clone())),
    };

    let mut header = Header::new(Shape::Save)
        .bundled_at(options.bundled_at)
        .system(slug(SYSTEM))
        .game(options.game.clone().or_else(|| game_of(opened.game_code.as_deref())))
        .source(options.source.clone());
    header.description = options.description.clone().or_else(|| opened.note.clone());

    let (part, chip, why_aux) = match verdict {
        Ok((chip, bytes)) => (Part::new(0, bytes), Some(chip), None),
        Err(why) => {
            let name = options.file_name.clone().unwrap_or_else(|| "save".to_owned());
            let part = Part::new(0, file.to_vec())
                .kind(PartKind::Aux)
                .path(name)
                .content_type("application/octet-stream");
            (part, None, Some(why))
        }
    };
    Ok(Read { bundle: Bundle::new(header, vec![part]), container: opened.container, chip, why_aux })
}

/// The libretro cores that write a DS card's chip as it is, at its own length, whatever the game
/// has written: the ones whose file alone says the chip.
const KEEP_THE_CHIP: [&str; 3] = ["melonds", "melonds-ds", "noods"];

/// Whether `source` names a producer that keeps the chip at its own length.
fn keeps_chip(source: Option<&Source>) -> bool {
    source.and_then(|source| source.core.as_ref()).is_some_and(|core| KEEP_THE_CHIP.contains(&core.as_str()))
}

/// The chip a file's length tells, where nothing else does, or why it tells none.
///
/// Only a file its tool keeps at the chip's own length says the chip. Every other tool's file says
/// it only where no smaller chip could have made it: a file whose upper half is erased, or is its
/// lower half again, is also what a chip of half its length makes, filled out or read past its end.
fn told_by_length(
    data: &[u8],
    container: Container,
    layout: Option<SaveLayout>,
    keeps_chip: bool,
) -> std::result::Result<usize, String> {
    let len = data.len();
    let ask = "pass --rom, or the game's code with --nds-game, to say which chip the card has";
    if let Some(SaveLayout::Padded { length, .. }) = layout
        && usize::try_from(length).ok() == Some(len)
    {
        return Err(format!(
            "its producer pads every chip to {}, so its length says nothing; {ask}",
            size_of(len)
        ));
    }
    if !is_chip(len) {
        return Err(format!("{} is no DS save chip's length; {ask}", size_of(len)));
    }
    if keeps_chip && container == Container::Bare {
        return Ok(len);
    }
    let what = match container {
        Container::Bare if layout == Some(SaveLayout::Desmume) => Container::Desmume.label(),
        Container::Bare => "a file from a tool not known to keep the chip at its length,",
        other => other.label(),
    };
    if len == AMBIGUOUS && matches!(container, Container::ActionReplay { .. }) {
        return Err(format!("{what} {} can be any chip padded or cut to that; {ask}", size_of(len)));
    }
    let (lower, upper) = data.split_at(len / 2);
    // DeSmuME and an Action Replay pad with 0xFF; a tool nothing is known of may pad with zeros.
    let pads_with_zeros = container == Container::Bare && layout != Some(SaveLayout::Desmume);
    let erased = upper.iter().all(|&b| b == ERASED) || (pads_with_zeros && upper.iter().all(|&b| b == 0));
    if len > CHIPS[0] && (erased || upper == lower) {
        let why = if erased { "its upper half is erased" } else { "its upper half repeats its lower" };
        return Err(format!("{what} {} can be a chip half that long, since {why}; {ask}", size_of(len)));
    }
    Ok(len)
}

/// `data` as `chip` bytes: filled out with 0xFF where it stops short, and cut where what follows
/// the chip is padding or the chip again. Anything else past the chip is data, and refused.
fn fit(data: &[u8], chip: usize, fill: Option<u8>) -> std::result::Result<Vec<u8>, String> {
    if data.len() <= chip {
        let mut bytes = data.to_vec();
        bytes.resize(chip, ERASED);
        return Ok(bytes);
    }
    let (head, tail) = data.split_at(chip);
    let padding = |byte: u8| tail.iter().all(|&b| b == byte);
    let repeated = tail.chunks(chip).all(|run| run == &head[..run.len()]);
    if padding(ERASED) || fill.is_some_and(padding) || repeated {
        return Ok(head.to_vec());
    }
    Err(format!(
        "it holds data past the end of the card's {} chip ({} in all), which no card holds",
        size_of(chip),
        size_of(data.len())
    ))
}

/// A length to show: bytes, or KiB and MiB where they are whole.
fn size_of(len: usize) -> String {
    match len {
        len if len >= 1 << 20 && len.is_multiple_of(1 << 20) => format!("{} MiB", len >> 20),
        len if len >= 1 << 10 && len.is_multiple_of(1 << 10) => format!("{} KiB", len >> 10),
        len => format!("{len} bytes"),
    }
}

/// The game a game code names, as the `game` map carries it.
fn game_of(code: Option<&str>) -> Option<Game> {
    Some(Game::default().serial(code?))
}

/// How the producer `source` names keeps a DS save, as the registry records it.
fn layout_of(source: Option<&Source>) -> Option<SaveLayout> {
    let source = source?;
    let core = one_saves_registry::core(source.core.as_ref()?.as_str())?;
    let app = source.app.as_ref().and_then(|app| one_saves_registry::app(app.as_str()));
    core.layout_in(app, SYSTEM)
}

/// Parses a slug the specification defines, which is always well-formed.
fn slug(text: &str) -> Slug {
    Slug::parse(text).expect("a spec slug is well-formed")
}

/// A file that opens as `format` and does not hold together.
fn corrupt(format: &'static str, why: impl Into<String>) -> Error {
    Error::Corrupt { format, why: why.into() }
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

/// Text in a fixed field, up to its first NUL, trimmed; `None` where that leaves nothing.
fn text(field: &[u8], utf16: bool) -> Option<String> {
    let text = if utf16 {
        let units: Vec<u16> = field.as_chunks::<2>().0.iter().map(|&pair| u16::from_le_bytes(pair)).collect();
        let end = units.iter().position(|&unit| unit == 0).unwrap_or(units.len());
        String::from_utf16_lossy(&units[..end])
    } else {
        let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
        field[..end].iter().map(|&b| char::from(b)).collect()
    };
    let text = text.trim().to_owned();
    (!text.is_empty() && !text.chars().any(char::is_control)).then_some(text)
}

// --- DeSmuME --------------------------------------------------------------------------------------

/// The text DeSmuME's footer opens with.
const DSV_TEXT: &[u8] = b"|<--Snip above here to create a raw sav by excluding this DeSmuME savedata footer:";
/// The cookie it closes with.
const DSV_COOKIE: &[u8] = b"|-DESMUME SAVE-|";
/// The footer's length: the text, six 32-bit words, and the cookie.
const DSV_FOOTER: usize = DSV_TEXT.len() + 24 + DSV_COOKIE.len();

/// The sizes DeSmuME pads a chip out to (`saveSizes` in its `mc.cpp`): no 1 Mbit size among them.
const DESMUME_SIZES: [usize; 13] = [
    512,
    8 << 10,
    32 << 10,
    64 << 10,
    256 << 10,
    512 << 10,
    1 << 20,
    2 << 20,
    4 << 20,
    8 << 20,
    16 << 20,
    32 << 20,
    64 << 20,
];

/// The sizes of DeSmuME's save types, by number (`save_types` in its `mc.cpp`): 0 is its
/// autodetection, and FRAM comes after the EEPROMs.
const DESMUME_TYPES: [usize; 14] = [
    0,
    512,
    8 << 10,
    64 << 10,
    32 << 10,
    256 << 10,
    512 << 10,
    1 << 20,
    2 << 20,
    4 << 20,
    8 << 20,
    16 << 20,
    32 << 20,
    64 << 20,
];

/// DeSmuME's footer, as it reads one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DsvFooter {
    /// How far the game has written, which is not the chip's size.
    pub size: u32,
    /// How many bytes precede the footer: the chip, padded to a size DeSmuME has.
    pub pad_size: u32,
    /// DeSmuME's save type, by its number.
    pub kind: u32,
    /// How many bytes an address takes.
    pub addr_size: u32,
    /// The memory's size, as DeSmuME took it.
    pub mem_size: u32,
}

/// The data before a DeSmuME footer, and the footer, where `file` ends in one.
///
/// DeSmuME wants the cookie at the very end; another tool's copy padded with 0xFF after it, as
/// one inside an Action Replay file is, is taken too.
#[must_use]
pub fn dsv(file: &[u8]) -> Option<(&[u8], DsvFooter)> {
    let trailing = file.iter().rev().take_while(|&&b| b == ERASED).count();
    let file = &file[..file.len() - trailing];
    let body = file.strip_suffix(DSV_COOKIE)?;
    let at = file.len().checked_sub(DSV_FOOTER)?;
    if !file[at..].starts_with(DSV_TEXT) {
        return None;
    }
    let words = &body[body.len() - 24..];
    let word = |i: usize| u32_at(words, i * 4);
    let footer = DsvFooter {
        size: word(0)?,
        pad_size: word(1)?,
        kind: word(2)?,
        addr_size: word(3)?,
        mem_size: word(4)?,
    };
    // Version 0 is the only one DeSmuME reads, and it says what precedes the footer.
    (word(5)? == 0 && usize::try_from(footer.pad_size).ok() == Some(at)).then_some((&file[..at], footer))
}

/// The `.dsv` DeSmuME's own import makes of a chip: the chip padded with 0xFF to the next size
/// DeSmuME has, which takes a 1 Mbit EEPROM to 256 KiB, then its footer.
///
/// The footer's first field is how far the game has written, which DeSmuME counts as the game runs
/// and a bundle does not keep. The whole chip is written there, as DeSmuME's import writes it and
/// as a file DeSmuME reads back without complaint.
#[must_use]
pub fn desmume_file(chip: &[u8]) -> Vec<u8> {
    let len = chip.len();
    let pad = DESMUME_SIZES.iter().copied().find(|&size| size >= len).unwrap_or(len);
    let kind = DESMUME_TYPES.iter().position(|&size| size == pad).unwrap_or(0);
    let addr_size: u32 = match len {
        512 => 1,
        0x2000 | 0x8000 | 0x1_0000 => 2,
        0x2_0000 | 0x4_0000 | 0x8_0000 | 0x10_0000 | 0x20_0000 | 0x80_0000 => 3,
        _ => u32::MAX,
    };
    let word = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    let mut file = chip.to_vec();
    file.resize(pad, ERASED);
    file.extend_from_slice(DSV_TEXT);
    for value in [word(len), word(pad), word(kind), addr_size, word(pad), 0] {
        file.extend_from_slice(&value.to_le_bytes());
    }
    file.extend_from_slice(DSV_COOKIE);
    file
}

// --- Action Replay DS ---------------------------------------------------------------------------

/// What a version 1 Action Replay file opens with.
const ARDS_V1: &[u8] = b"ARDS000000000001";
/// The length of its header.
const ARDS_V1_HEADER: usize = 500;
/// The length of a version 2 file's header.
const ARDS_V2_HEADER: usize = 0xA4;

/// The header text converters write in place of what a player would type.
const CONVERTERS: [&str; 3] = ["Converted Savegame", "Converted Saves", "Savegame"];

/// Whether a file is a version 2 Action Replay file: four zero bytes, 0xC0 at 0xA3, and the size
/// its header gives at 0xA1 matching what follows it.
fn is_ards_v2(bytes: &[u8]) -> bool {
    bytes.len() > ARDS_V2_HEADER
        && bytes[..4] == [0; 4]
        && bytes[0xA3] == 0xC0
        && (usize::from(bytes[0xA1]) << 8 | usize::from(bytes[0xA2]) << 16) == bytes.len() - ARDS_V2_HEADER
}

/// An Action Replay DS file's save, as DeSmuME's `import_duc` reads one, and a compressed one too.
fn action_replay(bytes: &[u8]) -> Result<Opened> {
    const FORMAT: &str = "Action Replay DS save";
    let (version, payload, note) = if bytes.starts_with(ARDS_V1) {
        if bytes.len() <= ARDS_V1_HEADER {
            return Err(corrupt(FORMAT, "it holds a header and no save"));
        }
        // What a player typed on the device: the game, the save's name and a description, in
        // UTF-16 at fixed places. A converter's header holds its own text there instead.
        let fields: Vec<String> = [(0x10, 0x40), (0x50, 0x40), (0x90, 0xC8)]
            .iter()
            .filter_map(|&(at, len)| text(&bytes[at..at + len], true))
            .collect();
        let converted = fields.first().is_some_and(|game| CONVERTERS.contains(&game.as_str()))
            || fields.get(1).is_some_and(|name| name.starts_with("DualScene"));
        let note = (!converted && !fields.is_empty()).then(|| fields.join(" / "));
        (1, &bytes[ARDS_V1_HEADER..], note)
    } else {
        (2, &bytes[ARDS_V2_HEADER..], None)
    };
    let (data, compressed) = match unrle(payload) {
        Some(data) => (data, true),
        None => (payload.to_vec(), false),
    };
    Ok(Opened {
        data,
        container: Container::ActionReplay { version, compressed },
        chip: None,
        game_code: None,
        note,
    })
}

/// The block-compressed payload the oldest Action Replay files hold, or `None` for one that is not:
/// a 32-bit total and block count, then per block its two lengths, an escape byte, and run-length
/// data in which the escape and a zero is the escape itself and the escape, a count and a byte is
/// that byte, count and one times.
///
/// Taken for compressed only where every block comes out at its stated length, the lengths add up
/// to the total, and nothing but zeros follows: a raw save that happened to open with a plausible
/// count would fail one of those.
fn unrle(payload: &[u8]) -> Option<Vec<u8>> {
    let total = usize::try_from(u32_at(payload, 0)?).ok()?;
    let blocks = u32_at(payload, 4)?;
    if !(1..=4096).contains(&blocks) || !(1..=16 << 20).contains(&total) {
        return None;
    }
    let be = |at: usize| -> Option<usize> {
        usize::try_from(u32::from_be_bytes(payload.get(at..at.checked_add(4)?)?.try_into().ok()?)).ok()
    };
    let mut out = Vec::with_capacity(total);
    let mut at = 8;
    for _ in 0..blocks {
        let (out_len, in_len) = (be(at)?, be(at + 4)?);
        let block = payload.get(at + 8..at.checked_add(8)?.checked_add(in_len)?)?;
        let (&escape, mut rest) = block.split_first()?;
        let start = out.len();
        while let Some((&byte, after)) = rest.split_first() {
            rest = after;
            if byte != escape {
                out.push(byte);
                continue;
            }
            match rest {
                [0, after @ ..] => {
                    out.push(escape);
                    rest = after;
                }
                [count, value, after @ ..] => {
                    out.resize(out.len() + usize::from(*count) + 1, *value);
                    rest = after;
                }
                _ => return None,
            }
            if out.len() > total {
                return None;
            }
        }
        if out.len() - start != out_len {
            return None;
        }
        at += 8 + in_len;
    }
    (out.len() == total && payload[at..].iter().all(|&b| b == 0)).then_some(out)
}

// --- no$gba ---------------------------------------------------------------------------------------

/// What no$gba's backup file opens with.
const NOGBA_ID: &[u8] = b"NocashGbaBackupMediaSavDataFile";

/// A no$gba backup file's save, as DeSmuME's `no_gba_unpackSAV` reads it: method 0 holds it as it
/// is, method 1 run-length coded.
fn no_gba(bytes: &[u8]) -> Result<Vec<u8>> {
    const FORMAT: &str = "no$gba backup file";
    if bytes.len() < 0x50 || bytes[0x1F] != 0x1A || &bytes[0x40..0x44] != b"SRAM" {
        return Err(corrupt(FORMAT, "its header is not no$gba's"));
    }
    let size_at = |at: usize| u32_at(bytes, at).and_then(|n| usize::try_from(n).ok());
    match u32_at(bytes, 0x44) {
        Some(0) => {
            let len = size_at(0x48).ok_or_else(|| corrupt(FORMAT, "it is cut short"))?;
            bytes
                .get(0x4C..0x4C + len)
                .map(<[u8]>::to_vec)
                .ok_or_else(|| corrupt(FORMAT, "it holds less than its header says"))
        }
        Some(1) => {
            let len = size_at(0x4C).ok_or_else(|| corrupt(FORMAT, "it is cut short"))?;
            if len > 64 << 20 {
                return Err(corrupt(FORMAT, "it says it holds more than a DS card keeps"));
            }
            let mut out = Vec::with_capacity(len);
            let mut rest = &bytes[0x50..];
            let short = || corrupt(FORMAT, "its coded data stops short");
            loop {
                let (&code, after) = rest.split_first().ok_or_else(short)?;
                rest = after;
                match code {
                    0 => break,
                    0x80 => {
                        let [value, lo, hi, after @ ..] = rest else { return Err(short()) };
                        out.resize(out.len() + usize::from(u16::from_le_bytes([*lo, *hi])), *value);
                        rest = after;
                    }
                    0x81.. => {
                        let (&value, after) = rest.split_first().ok_or_else(short)?;
                        out.resize(out.len() + usize::from(code - 0x80), value);
                        rest = after;
                    }
                    literal => {
                        let (run, after) = rest.split_at_checked(usize::from(literal)).ok_or_else(short)?;
                        out.extend_from_slice(run);
                        rest = after;
                    }
                }
                if out.len() > len {
                    return Err(corrupt(FORMAT, "it unpacks past the size its header gives"));
                }
            }
            if out.len() == len {
                Ok(out)
            } else {
                Err(corrupt(FORMAT, format!("it unpacks to {} bytes, and its header says {len}", out.len())))
            }
        }
        Some(method) => {
            Err(corrupt(FORMAT, format!("it is packed by method {method}, which no$gba has not")))
        }
        None => Err(corrupt(FORMAT, "it is cut short")),
    }
}

// --- DSXS -----------------------------------------------------------------------------------------

/// What a `DSXS` file opens with.
const DSXS_ID: &[u8] = b"DSXS";
/// Where its gzip member starts.
const DSXS_HEADER: usize = 0x100;

/// A `DSXS` file's save: the gzip member after its 256-byte header, whose own checksum checks it.
///
/// Nothing documents the format; its layout was read from the one file the corpus holds, which
/// names the game, the save and a description in ASCII at 0x10, 0x40 and 0x70.
fn dsxs(bytes: &[u8]) -> Result<Opened> {
    const FORMAT: &str = "DSXS save";
    let member = bytes.get(DSXS_HEADER..).filter(|member| member.starts_with(&[0x1F, 0x8B]));
    let member = member.ok_or_else(|| corrupt(FORMAT, "it holds no gzip data where the save goes"))?;
    let fields: Vec<String> = [(0x10, 0x30), (0x40, 0x30), (0x70, 0x90)]
        .iter()
        .filter_map(|&(at, len)| text(&bytes[at..at + len], false))
        .collect();
    let data = gunzip(member, FORMAT)?;
    Ok(Opened {
        data,
        container: Container::Dsxs,
        chip: None,
        game_code: None,
        note: (!fields.is_empty()).then(|| fields.join(" / ")),
    })
}

// --- DraStic --------------------------------------------------------------------------------------

/// What a DraStic save state opens with.
const DRASTIC_ID: &[u8] = b"DraStic-SaveState";
/// Where the inflated state keeps the card's save memory: a record of a 32-bit kind, the address
/// mask, a zero word and the address width, then the chip.
const DRASTIC_SAVE: usize = 0x3_0000;

/// The chip a DraStic save state holds.
///
/// DraStic is closed. Its layout was read from the two states the corpus holds: the game code at
/// 0x34, the zlib stream's length at 0x40 and the stream from 0x44, and in the inflated state at
/// 0x30000 the record above, whose mask gives the chip's size. The record is checked before its
/// bytes are believed, so a state laid out otherwise is refused rather than misread.
fn drastic(bytes: &[u8]) -> Result<Opened> {
    const FORMAT: &str = "DraStic save state";
    let code = bytes.get(0x34..0x38).and_then(|code| text(code, false));
    let len = u32_at(bytes, 0x40).and_then(|n| usize::try_from(n).ok());
    let stream = len.and_then(|len| bytes.get(0x44..0x44usize.checked_add(len)?));
    let stream =
        stream.ok_or_else(|| corrupt(FORMAT, "its compressed data runs past the end of the file"))?;
    let state = inflate(stream, FORMAT)?;
    let record = state.get(DRASTIC_SAVE..DRASTIC_SAVE + 13);
    let record = record.ok_or_else(|| corrupt(FORMAT, "it is too short to hold the card's save memory"))?;
    let mask = u32_at(record, 4).unwrap_or(0);
    let size = usize::try_from(mask).ok().and_then(|mask| mask.checked_add(1)).unwrap_or(0);
    if !size.is_power_of_two()
        || !(512..=8 << 20).contains(&size)
        || u32_at(record, 8) != Some(0)
        || !(1..=3).contains(&record[12])
    {
        return Err(corrupt(FORMAT, "it holds no save memory where DraStic keeps it"));
    }
    let chip = state.get(DRASTIC_SAVE + 13..DRASTIC_SAVE + 13 + size);
    let chip = chip.ok_or_else(|| corrupt(FORMAT, "its save memory runs past the end of the state"))?;
    Ok(Opened {
        data: chip.to_vec(),
        container: Container::DrasticState,
        chip: Some(size),
        game_code: code,
        note: None,
    })
}

// --- Inflating ------------------------------------------------------------------------------------

#[cfg(feature = "nds-zlib")]
fn gunzip(member: &[u8], format: &'static str) -> Result<Vec<u8>> {
    bounded(flate2::read::GzDecoder::new(member), format)
}

#[cfg(feature = "nds-zlib")]
fn inflate(stream: &[u8], format: &'static str) -> Result<Vec<u8>> {
    bounded(flate2::read::ZlibDecoder::new(stream), format)
}

/// Everything a decoder gives, refused past [`MAX_INFLATED`], so a file that inflates without end
/// costs no more than a DS save could.
#[cfg(feature = "nds-zlib")]
fn bounded(decoder: impl std::io::Read, format: &'static str) -> Result<Vec<u8>> {
    use std::io::Read as _;
    let mut out = Vec::new();
    decoder
        .take(MAX_INFLATED + 1)
        .read_to_end(&mut out)
        .map_err(|e| corrupt(format, format!("its compressed data does not inflate: {e}")))?;
    if out.len() as u64 > MAX_INFLATED {
        return Err(corrupt(format, "it inflates to more than a DS save could be"));
    }
    Ok(out)
}

#[cfg(not(feature = "nds-zlib"))]
fn gunzip(_: &[u8], format: &'static str) -> Result<Vec<u8>> {
    Err(Error::Unsupported { format, feature: "nds-zlib" })
}

#[cfg(not(feature = "nds-zlib"))]
fn inflate(_: &[u8], format: &'static str) -> Result<Vec<u8>> {
    Err(Error::Unsupported { format, feature: "nds-zlib" })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chip(len: usize) -> Vec<u8> {
        (0..len).map(|i| u8::try_from(i * 7 % 251).unwrap()).collect()
    }

    fn read_with(bytes: &[u8], chip: Option<usize>, source: Option<&str>) -> Read {
        let source = source.map(|core| Source::default().core(one_saves::Name::parse(core).ok()));
        read(bytes, &NdsOptions { chip, source, file_name: Some("game.sav".into()), ..NdsOptions::default() })
            .expect("reads")
    }

    fn payload(read: &Read) -> &[u8] {
        &read.bundle.parts[0].payload
    }

    #[test]
    fn a_chip_as_it_is_is_its_own_length() {
        for len in CHIPS {
            let bytes = chip(len);
            let read = read_with(&bytes, None, None);
            assert_eq!((read.chip, payload(&read)), (Some(len), &bytes[..]), "{len}");
            read.bundle.validate().expect("a DS save of a chip's length");
        }
    }

    #[test]
    fn a_desmume_file_goes_round_and_its_footer_comes_off() {
        let bytes = chip(8192);
        let file = desmume_file(&bytes);
        assert_eq!(file.len(), 8192 + DSV_FOOTER);
        let (data, footer) = dsv(&file).expect("its own footer");
        assert_eq!((data, footer.pad_size, footer.kind, footer.addr_size), (&bytes[..], 8192, 2, 2));
        let read = read_with(&file, None, Some("desmume"));
        assert_eq!((read.container, payload(&read)), (Container::Desmume, &bytes[..]));
    }

    #[test]
    fn a_1_mbit_eeprom_goes_to_desmume_at_256_kib_and_comes_back_by_its_game() {
        let mut eeprom = chip(131_072);
        eeprom[..4].copy_from_slice(b"SORA");
        let file = desmume_file(&eeprom);
        let (_, footer) = dsv(&file).unwrap();
        assert_eq!((footer.pad_size, footer.kind, footer.addr_size), (262_144, 5, 3), "as DeSmuME keeps it");

        // Without the game, 256 KiB with nothing above its first half is either chip.
        let unknown = read_with(&file, None, Some("desmume"));
        assert_eq!((unknown.chip, unknown.bundle.parts[0].kind.clone()), (None, PartKind::Aux));
        assert_eq!(payload(&unknown), &file[..], "the file as it came");
        unknown.bundle.validate().expect("an aux part");

        let known = read_with(&file, Some(131_072), Some("desmume"));
        assert_eq!(payload(&known), &eeprom[..]);
    }

    #[test]
    fn skyemu_pads_every_chip_to_8_mib_and_only_the_game_says_where_it_ends() {
        let flash = chip(262_144);
        let mut file = flash.clone();
        file.resize(8 << 20, 0);
        let unknown = read_with(&file, None, Some("skyemu"));
        assert!(unknown.why_aux.as_deref().unwrap().contains("8 MiB"));
        let known = read_with(&file, Some(262_144), Some("skyemu"));
        assert_eq!(payload(&known), &flash[..]);
    }

    #[test]
    fn a_short_file_fills_out_with_0xff_and_a_long_one_is_cut_only_past_padding() {
        let half = chip(262_144);
        let read = read_with(&half, Some(524_288), None);
        assert_eq!(&payload(&read)[..262_144], &half[..]);
        assert!(payload(&read)[262_144..].iter().all(|&b| b == 0xFF));

        let twice = [half.clone(), half.clone()].concat();
        assert_eq!(payload(&read_with(&twice, Some(262_144), None)), &half[..], "the chip again");

        let other = [half.clone(), vec![0x42; 1000]].concat();
        let read = read_with(&other, Some(262_144), None);
        assert_eq!(read.chip, None, "data past the chip is not padding");
        assert!(read.why_aux.unwrap().contains("past the end"));
    }

    #[test]
    fn only_a_tool_that_keeps_the_chip_says_it_by_length_where_half_a_chip_could_have_made_the_file() {
        let half = chip(524_288);
        let erased = [half.clone(), vec![ERASED; 524_288]].concat();
        let zeros = [half.clone(), vec![0; 524_288]].concat();
        let mirrored = [half.clone(), half.clone()].concat();
        for (why, file) in [("erased", &erased), ("zeros", &zeros), ("the chip again", &mirrored)] {
            let unknown = read_with(file, None, None);
            assert_eq!(unknown.chip, None, "{why}: a tool nothing is known of");
            assert!(payload(&unknown) == &file[..], "{why}: the file as it came");
            assert!(unknown.why_aux.as_deref().unwrap().contains("half"), "{why}");
            // melonDS writes the chip at its own length, so its 1 MiB is 8 Mbit flash.
            assert_eq!(read_with(file, None, Some("melonds")).chip, Some(1_048_576), "{why}");
            // And the game says it outright, but zeros past it from a tool nothing is known of may be
            // the game's.
            let known = read_with(file, Some(524_288), None);
            assert_eq!(known.chip, (why != "zeros").then_some(524_288), "{why}");
        }
        // Erased past a DeSmuME chip is DeSmuME's padding, and zeros there are the game's.
        let desmume = read_with(&desmume_file(&zeros), None, Some("desmume"));
        assert_eq!(desmume.chip, Some(1_048_576));
        let desmume = read_with(&desmume_file(&erased), None, Some("desmume"));
        assert_eq!(desmume.chip, None);
        // The smallest chip has no smaller one to be mistaken for.
        assert_eq!(read_with(&[0xFF; 512], None, None).chip, Some(512));
    }

    #[test]
    fn zeros_past_the_chip_are_padding_only_from_a_tool_that_pads_with_them() {
        let bytes = chip(8192);
        let zeros = [bytes.clone(), vec![0; 8192]].concat();
        let unknown = read_with(&zeros, Some(8192), None);
        assert_eq!(unknown.chip, None, "zeros may be the game's");
        assert!(unknown.why_aux.unwrap().contains("past the end"));
        let skyemu = [bytes.clone(), vec![0; (8 << 20) - 8192]].concat();
        assert_eq!(payload(&read_with(&skyemu, Some(8192), Some("skyemu"))), &bytes[..], "SkyEmu's fill");
    }

    #[test]
    fn a_nand_card_goes_as_the_file() {
        let read = read_with(&chip(8 << 20), Some(8 << 20), None);
        assert_eq!(read.chip, None);
        assert!(read.why_aux.unwrap().contains("NAND"));
    }

    #[test]
    fn an_action_replay_file_s_header_comes_off_and_a_player_s_note_stays() {
        let mut file = vec![0u8; ARDS_V1_HEADER];
        file[..16].copy_from_slice(ARDS_V1);
        for (at, text) in [(0x10, "Bomberman"), (0x50, "Level 10-6")] {
            for (i, unit) in text.encode_utf16().enumerate() {
                file[at + 2 * i..at + 2 * i + 2].copy_from_slice(&unit.to_le_bytes());
            }
        }
        let bytes = chip(512);
        file.extend_from_slice(&bytes);
        let read = read_with(&file, None, None);
        assert_eq!(read.container, Container::ActionReplay { version: 1, compressed: false });
        assert_eq!(payload(&read), &bytes[..]);
        assert_eq!(read.bundle.header.description.as_deref(), Some("Bomberman / Level 10-6"));

        // A converter's 256 KiB says nothing about the chip.
        let mut converted = file[..ARDS_V1_HEADER].to_vec();
        converted.extend(chip(262_144));
        assert_eq!(read_with(&converted, None, None).chip, None);
        assert_eq!(read_with(&converted, Some(262_144), None).chip, Some(262_144));
    }

    #[test]
    fn a_compressed_action_replay_payload_unpacks_exactly() {
        // One block: a literal, the escape itself, and a run of five 0xFF.
        let block = [0xEE, 0x41, 0xEE, 0x00, 0xEE, 0x04, 0xFF];
        let mut payload = Vec::new();
        payload.extend_from_slice(&7u32.to_le_bytes());
        payload.extend_from_slice(&1u32.to_le_bytes());
        payload.extend_from_slice(&7u32.to_be_bytes());
        payload.extend_from_slice(&u32::try_from(block.len()).unwrap().to_be_bytes());
        payload.extend_from_slice(&block);
        payload.extend_from_slice(&[0; 5]);
        assert_eq!(unrle(&payload), Some(vec![0x41, 0xEE, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]));
        payload[20] = 0x05;
        assert_eq!(unrle(&payload), None, "a block that comes out at another length is no block");
        assert_eq!(unrle(&chip(512)), None);
    }

    #[test]
    fn a_no_gba_file_unpacks_both_ways() {
        let mut file = NOGBA_ID.to_vec();
        file.push(0x1A);
        file.resize(0x40, 0);
        file.extend_from_slice(b"SRAM");
        file.extend_from_slice(&1u32.to_le_bytes());
        file.extend_from_slice(&0u32.to_le_bytes());
        file.extend_from_slice(&515u32.to_le_bytes());
        // Two literals, 0x80 with a 16-bit count of 0x1FF, then 0x82 for two more.
        file.extend_from_slice(&[0x02, 0x10, 0x20, 0x80, 0xFF, 0xFF, 0x01, 0x82, 0x33, 0x00]);
        let data = no_gba(&file).expect("unpacks");
        assert_eq!(data.len(), 515);
        assert_eq!((&data[..2], data[2], data[513]), (&[0x10, 0x20][..], 0xFF, 0x33));
    }

    #[test]
    fn a_container_is_known_by_its_signature_and_a_chip_by_nothing() {
        assert!(is_container(&desmume_file(&chip(512))));
        assert!(!is_container(&chip(512)));
        let mut v2 = vec![0u8; ARDS_V2_HEADER + 0x2000];
        v2[0xA1] = 0x20;
        v2[0xA3] = 0xC0;
        assert!(is_container(&v2));
        v2.push(0);
        assert!(!is_container(&v2), "the size its header gives is what follows it");
    }

    // --- Every chip through every layout ----------------------------------------------------------

    const SKYEMU: SaveLayout = SaveLayout::Padded { length: 8 << 20, fill: 0, reversed_eeprom: false };

    /// The chip, as each kind of producer keeps it: as it is, in DeSmuME's `.dsv`, and padded to
    /// SkyEmu's 8 MiB.
    fn layouts() -> [(&'static str, Option<SaveLayout>); 3] {
        [("bare", None), ("desmume", Some(SaveLayout::Desmume)), ("skyemu", Some(SKYEMU))]
    }

    fn read_in(file: &[u8], chip: Option<usize>, layout: Option<SaveLayout>) -> Read {
        let options = NdsOptions { chip, layout, file_name: Some("save".into()), ..NdsOptions::default() };
        read(file, &options).expect("reads")
    }

    #[test]
    fn every_chip_goes_out_to_every_layout_and_back_byte_for_byte() {
        for len in CHIPS {
            let bytes = chip(len);
            for (name, layout) in layouts() {
                let file = crate::layout::lay_out(&bytes, SYSTEM, layout, None).bytes;
                let read = read_in(&file, Some(len), layout);
                assert_eq!((read.chip, payload(&read)), (Some(len), &bytes[..]), "{name} {len}");
                read.bundle.validate().unwrap_or_else(|e| panic!("{name} {len}: {e}"));
                let again = crate::layout::lay_out(payload(&read), SYSTEM, layout, None).bytes;
                assert_eq!(again, file, "{name} {len}: the file it came in");
            }
        }
    }

    #[test]
    fn without_the_game_each_layout_tells_the_chips_it_can_and_keeps_the_rest_whole() {
        for len in CHIPS {
            let bytes = chip(len);
            for (name, layout) in layouts() {
                let file = crate::layout::lay_out(&bytes, SYSTEM, layout, None).bytes;
                let read = read_in(&file, None, layout);
                // A bare chip is its own length. DeSmuME keeps a 1 Mbit EEPROM at 256 KiB with the
                // upper half erased, which 2 Mbit flash could be too; SkyEmu's 8 MiB says nothing.
                let told = match name {
                    "bare" => true,
                    "desmume" => len != 131_072,
                    _ => false,
                };
                if told {
                    assert_eq!((read.chip, payload(&read)), (Some(len), &bytes[..]), "{name} {len}");
                } else {
                    let part = &read.bundle.parts[0];
                    assert_eq!((read.chip, &part.kind), (None, &PartKind::Aux), "{name} {len}");
                    assert_eq!(part.payload, file, "{name} {len}: the file as it came");
                    assert!(read.why_aux.is_some(), "{name} {len}");
                }
                read.bundle.validate().unwrap_or_else(|e| panic!("{name} {len}: {e}"));
            }
        }
    }

    // --- Files that do not hold together ----------------------------------------------------------

    fn is_corrupt(result: Result<impl std::fmt::Debug>, why: &str) {
        match result {
            Err(Error::Corrupt { why: found, .. }) => assert!(found.contains(why), "{found:?}, not {why:?}"),
            other => panic!("expected it refused as corrupt ({why}), got {other:?}"),
        }
    }

    #[test]
    fn an_empty_file_is_no_save() {
        assert!(matches!(read(&[], &NdsOptions::default()), Err(Error::NotConvertible(_))));
    }

    #[test]
    fn an_action_replay_header_with_no_save_after_it_is_refused() {
        let mut file = ARDS_V1.to_vec();
        file.resize(ARDS_V1_HEADER, 0);
        is_corrupt(read(&file, &NdsOptions::default()), "no save");
    }

    #[test]
    fn a_version_2_action_replay_file_s_header_comes_off() {
        let bytes = chip(8192);
        let mut file = vec![0u8; ARDS_V2_HEADER];
        file[0xA1] = 0x20;
        file[0xA3] = 0xC0;
        file.extend_from_slice(&bytes);
        let read = read_with(&file, None, None);
        assert_eq!(read.container, Container::ActionReplay { version: 2, compressed: false });
        assert_eq!((read.chip, payload(&read)), (Some(8192), &bytes[..]));
    }

    #[test]
    fn an_action_replay_file_wrapping_a_desmume_file_gives_the_chip_inside() {
        let bytes = chip(8192);
        let mut file = ARDS_V1.to_vec();
        file.resize(ARDS_V1_HEADER, 0);
        file.extend(desmume_file(&bytes));
        file.resize(file.len() + 300, ERASED);
        let read = read_with(&file, None, None);
        assert_eq!(read.container, Container::ActionReplay { version: 1, compressed: false });
        assert_eq!((read.chip, payload(&read)), (Some(8192), &bytes[..]));
    }

    /// A compressed payload's header: its total, one block of `out_len` from `block`.
    fn rle(total: u32, out_len: u32, block: &[u8]) -> Vec<u8> {
        let mut payload = total.to_le_bytes().to_vec();
        payload.extend_from_slice(&1u32.to_le_bytes());
        payload.extend_from_slice(&out_len.to_be_bytes());
        payload.extend_from_slice(&u32::try_from(block.len()).unwrap().to_be_bytes());
        payload.extend_from_slice(block);
        payload
    }

    #[test]
    fn a_payload_that_only_looks_compressed_is_taken_as_it_is() {
        let good = rle(3, 3, &[0xEE, 0xEE, 0x02, 0x41]);
        assert_eq!(unrle(&good), Some(vec![0x41; 3]), "a count of two is three bytes");
        for (why, payload) in [
            ("a run past the total", rle(2, 3, &[0xEE, 0xEE, 0x02, 0x41])),
            ("an escape at the block's end", rle(1, 1, &[0xEE, 0x41, 0xEE])),
            ("an escape and one byte", rle(1, 1, &[0xEE, 0x41, 0xEE, 0x05])),
            ("a block longer than the payload", rle(3, 3, &[0xEE, 0xEE, 0x02, 0x41])[..15].to_vec()),
            ("a block with no escape byte", rle(0, 0, &[])),
            ("something after the blocks", [good.clone(), vec![1]].concat()),
            ("no blocks", [3u32.to_le_bytes(), 0u32.to_le_bytes()].concat()),
            ("a total of nothing", rle(0, 0, &[0xEE])),
            ("4097 blocks", [3u32.to_le_bytes(), 4097u32.to_le_bytes()].concat()),
            ("a block count and nothing else", [3u32.to_le_bytes(), 4096u32.to_le_bytes()].concat()),
        ] {
            assert_eq!(unrle(&payload), None, "{why}");
        }
    }

    /// A no$gba backup file's header up to its method and the word after it.
    fn no_gba_header(method: u32) -> Vec<u8> {
        let mut file = NOGBA_ID.to_vec();
        file.push(0x1A);
        file.resize(0x40, 0);
        file.extend_from_slice(b"SRAM");
        file.extend_from_slice(&method.to_le_bytes());
        file
    }

    #[test]
    fn a_no_gba_file_stored_as_it_is_reads_through_open() {
        let bytes = chip(512);
        let mut file = no_gba_header(0);
        file.extend_from_slice(&512u32.to_le_bytes());
        file.extend_from_slice(&bytes);
        let read = read_with(&file, None, None);
        assert_eq!((read.container, payload(&read)), (Container::NoGba, &bytes[..]));
    }

    #[test]
    fn a_no_gba_file_that_does_not_hold_together_is_refused() {
        let coded = |size: u32, data: &[u8]| {
            let mut file = no_gba_header(1);
            file.extend_from_slice(&0u32.to_le_bytes());
            file.extend_from_slice(&size.to_le_bytes());
            file.extend_from_slice(data);
            file
        };
        let mut stored = no_gba_header(0);
        stored.extend_from_slice(&512u32.to_le_bytes());
        stored.extend_from_slice(&[0; 100]);
        let mut bad_header = coded(2, &[0x02, 1, 2, 0]);
        bad_header[0x1F] = 0;
        for (why, file) in [
            ("is not no$gba's", bad_header),
            ("holds less than its header says", stored),
            ("method 2", [no_gba_header(2), vec![0; 8]].concat()),
            ("more than a DS card keeps", coded((64 << 20) + 1, &[0])),
            ("stops short", coded(2, &[0x02, 1, 2])),
            ("stops short", coded(2, &[0x82])),
            ("stops short", coded(2, &[0x80, 1, 2])),
            ("stops short", coded(2, &[0x05, 1, 2])),
            ("past the size", coded(2, &[0x83, 7, 0])),
            ("unpacks to 1 bytes", coded(2, &[0x01, 7, 0])),
        ] {
            is_corrupt(no_gba(&file), why);
            is_corrupt(read(&file, &NdsOptions::default()), why);
        }
    }

    #[test]
    fn a_desmume_footer_is_believed_only_where_it_adds_up() {
        let file = desmume_file(&chip(512));
        let footer_at = file.len() - DSV_FOOTER;
        let words = file.len() - DSV_COOKIE.len() - 24;

        let mut wrong_pad = file.clone();
        wrong_pad[words + 5] = 0;
        let mut version_1 = file.clone();
        version_1[words + 20] = 1;
        let mut wrong_text = file.clone();
        wrong_text[footer_at] = b'!';
        for (why, file) in [
            ("a pad size that is not what precedes it", wrong_pad),
            ("a version DeSmuME does not read", version_1),
            ("another text", wrong_text),
            ("only the footer's end", file[footer_at + 10..].to_vec()),
            ("only the cookie", DSV_COOKIE.to_vec()),
        ] {
            assert!(dsv(&file).is_none(), "{why}");
        }
        // Padding after the cookie, as an Action Replay file holding one has, is passed over.
        let padded = [file.clone(), vec![ERASED; 64]].concat();
        assert_eq!(dsv(&padded).map(|(data, _)| data.len()), Some(512));
    }

    /// Every prefix of `file` and `file` with each of its first bytes flipped: a reader must answer
    /// each with a save or an error, and never panic.
    fn survives_damage(file: &[u8], step: usize) {
        let options = NdsOptions { file_name: Some("save".into()), ..NdsOptions::default() };
        for len in (0..file.len()).step_by(step) {
            let _ = read(&file[..len], &options);
            let _ = read_with_chip(&file[..len], 512);
        }
        for at in 0..file.len().min(0x200) {
            let mut damaged = file.to_vec();
            damaged[at] ^= 0xFF;
            let _ = read(&damaged, &options);
        }
    }

    fn read_with_chip(file: &[u8], chip: usize) -> Result<Read> {
        read(file, &NdsOptions { chip: Some(chip), ..NdsOptions::default() })
    }

    #[test]
    fn a_damaged_file_of_every_kind_is_answered_and_never_panics() {
        let mut v1 = ARDS_V1.to_vec();
        v1.resize(ARDS_V1_HEADER, 0);
        v1.extend(rle(3, 3, &[0xEE, 0xEE, 0x02, 0x41]));
        let mut v2 = vec![0u8; ARDS_V2_HEADER];
        v2[0xA1] = 0x02;
        v2[0xA3] = 0xC0;
        v2.extend(chip(512));
        let mut no_gba = no_gba_header(1);
        no_gba.extend_from_slice(&0u32.to_le_bytes());
        no_gba.extend_from_slice(&515u32.to_le_bytes());
        no_gba.extend_from_slice(&[0x02, 0x10, 0x20, 0x80, 0xFF, 0xFF, 0x01, 0x82, 0x33, 0x00]);
        for file in [v1, v2, no_gba, desmume_file(&chip(512))] {
            survives_damage(&file, 1);
        }
        #[cfg(feature = "nds-zlib")]
        {
            survives_damage(&dsxs_file(&chip(512)), 7);
            survives_damage(&drastic_state(*b"ASMP", &chip(512), 1), 97);
        }
    }

    // --- Compressed containers --------------------------------------------------------------------

    #[cfg(feature = "nds-zlib")]
    fn compressed<W: std::io::Write>(
        mut encoder: W,
        data: &[u8],
        finish: impl FnOnce(W) -> Vec<u8>,
    ) -> Vec<u8> {
        encoder.write_all(data).unwrap();
        finish(encoder)
    }

    #[cfg(feature = "nds-zlib")]
    fn zlib(data: &[u8]) -> Vec<u8> {
        let encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        compressed(encoder, data, |e| e.finish().unwrap())
    }

    /// A DSXS file holding `save`, with the game and the save named in its header.
    #[cfg(feature = "nds-zlib")]
    fn dsxs_file(save: &[u8]) -> Vec<u8> {
        let mut file = vec![0u8; DSXS_HEADER];
        file[..4].copy_from_slice(DSXS_ID);
        file[0x10..0x19].copy_from_slice(b"Bomberman");
        file[0x40..0x4A].copy_from_slice(b"Level 10-6");
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        file.extend(compressed(encoder, save, |e| e.finish().unwrap()));
        file
    }

    /// A DraStic state: its game code, and inflated, `save` at 0x30000 behind its record.
    #[cfg(feature = "nds-zlib")]
    fn drastic_state(code: [u8; 4], save: &[u8], width: u8) -> Vec<u8> {
        let mut state = vec![0u8; DRASTIC_SAVE];
        state.extend_from_slice(&1u32.to_le_bytes());
        state.extend_from_slice(&u32::try_from(save.len() - 1).unwrap().to_le_bytes());
        state.extend_from_slice(&0u32.to_le_bytes());
        state.push(width);
        state.extend_from_slice(save);
        state.extend_from_slice(&[0x77; 64]);
        let stream = zlib(&state);
        let mut file = DRASTIC_ID.to_vec();
        file.resize(0x34, 0);
        file.extend_from_slice(&code);
        file.resize(0x40, 0);
        file.extend_from_slice(&u32::try_from(stream.len()).unwrap().to_le_bytes());
        file.extend(stream);
        file
    }

    #[cfg(feature = "nds-zlib")]
    #[test]
    fn a_dsxs_file_s_gzip_member_is_the_save_and_its_header_the_note() {
        let bytes = chip(512);
        let file = dsxs_file(&bytes);
        assert!(is_container(&file));
        let read = read_with(&file, None, None);
        assert_eq!((read.container, read.chip, payload(&read)), (Container::Dsxs, Some(512), &bytes[..]));
        assert_eq!(read.bundle.header.description.as_deref(), Some("Bomberman / Level 10-6"));
    }

    #[cfg(feature = "nds-zlib")]
    #[test]
    fn a_dsxs_file_without_a_whole_gzip_member_is_refused() {
        let mut no_member = dsxs_file(&chip(512));
        no_member.truncate(DSXS_HEADER);
        is_corrupt(read(&no_member, &NdsOptions::default()), "no gzip data");
        let mut not_gzip = dsxs_file(&chip(512));
        not_gzip[DSXS_HEADER] = 0;
        is_corrupt(read(&not_gzip, &NdsOptions::default()), "no gzip data");
        let mut cut = dsxs_file(&chip(512));
        cut.truncate(cut.len() - 20);
        is_corrupt(read(&cut, &NdsOptions::default()), "does not inflate");
    }

    #[cfg(feature = "nds-zlib")]
    #[test]
    fn a_drastic_state_gives_its_chip_and_its_game_and_nothing_else() {
        for len in CHIPS {
            let bytes = chip(len);
            let file = drastic_state(*b"C2SE", &bytes, 3);
            assert!(is_container(&file));
            let read = read_with(&file, None, None);
            assert_eq!(read.container, Container::DrasticState);
            assert_eq!((read.chip, payload(&read)), (Some(len), &bytes[..]), "{len}");
            let game = read.bundle.header.game.as_ref().expect("its game");
            assert_eq!(game.serial.as_deref(), Some("C2SE"));
        }
    }

    #[cfg(feature = "nds-zlib")]
    #[test]
    fn a_drastic_state_laid_out_otherwise_is_refused_rather_than_misread() {
        let bytes = chip(512);
        let stream_len_at = 0x40;
        let mut past_end = drastic_state(*b"ASMP", &bytes, 1);
        past_end[stream_len_at] = past_end[stream_len_at].wrapping_add(1);
        is_corrupt(read(&past_end, &NdsOptions::default()), "past the end of the file");

        let mut garbled = drastic_state(*b"ASMP", &bytes, 1);
        let last = garbled.len() - 1;
        garbled[0x50..last].fill(0x42);
        is_corrupt(read(&garbled, &NdsOptions::default()), "does not inflate");

        let mut short = DRASTIC_ID.to_vec();
        short.resize(0x40, 0);
        let stream = zlib(&[0; 0x100]);
        short.extend_from_slice(&u32::try_from(stream.len()).unwrap().to_le_bytes());
        short.extend(stream);
        is_corrupt(read(&short, &NdsOptions::default()), "too short");

        for (why, file) in [
            ("an address width of 0", drastic_state(*b"ASMP", &bytes, 0)),
            ("an address width of 4", drastic_state(*b"ASMP", &bytes, 4)),
            ("a chip of 300 bytes", drastic_state(*b"ASMP", &chip(300), 1)),
            ("a chip of 256 bytes", drastic_state(*b"ASMP", &chip(256), 1)),
        ] {
            let result = read(&file, &NdsOptions::default());
            assert!(matches!(result, Err(Error::Corrupt { .. })), "{why}: {result:?}");
        }
    }

    #[cfg(feature = "nds-zlib")]
    #[test]
    fn a_stream_that_inflates_without_end_is_stopped_at_the_bound() {
        is_corrupt(bounded(std::io::repeat(0), "test"), "more than a DS save could be");
        let exactly = bounded(std::io::Read::take(std::io::repeat(0), MAX_INFLATED), "test");
        assert_eq!(exactly.map(|out| out.len() as u64).ok(), Some(MAX_INFLATED));
    }

    #[cfg(not(feature = "nds-zlib"))]
    #[test]
    fn without_zlib_a_compressed_file_says_which_feature_reads_it() {
        let mut file = DSXS_ID.to_vec();
        file.resize(DSXS_HEADER, 0);
        file.extend_from_slice(&[0x1F, 0x8B, 0, 0]);
        let result = read(&file, &NdsOptions::default());
        assert!(matches!(result, Err(Error::Unsupported { feature: "nds-zlib", .. })), "{result:?}");
    }
}
