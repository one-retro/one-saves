//! mGBA's save states.
//!
//! mGBA writes a state in one of two containers, and the core's data is the same in both: a fixed
//! size, `0x61000` bytes for the GBA and `0x11800` for the Game Boy, opening with a version word
//! (`0x01000000` and `0x00400000` plus the version). After the word come the ROM's CRC-32 and the
//! title out of its header, which become `game`, and for the Game Boy the model mGBA emulated,
//! which says whether a game that runs on both ran as a Game Boy or a Game Boy Color.
//!
//! - **A PNG**, whenever it has a screenshot to write, which is the default. The image is what the
//!   screen showed. After it come mGBA's own chunks: `gbAs`, the core's data as a zlib stream, and
//!   one `gbAx` per item of extra data, holding the item's tag and length as little-endian `u32`s
//!   and the item as a zlib stream.
//! - **The core's data with its extra data after it**, when it has no screenshot. A list of 16-byte
//!   headers follows the data — the tag as a `u32`, the length as an `i32` and the offset from the
//!   start of the file as an `i64`, all little-endian — ended by one of zeros, and the items follow
//!   that.
//!
//! The items are tagged. In a bundle:
//!
//! | Tag | Item | In the bundle |
//! | --- | ---- | ------------- |
//! | 1 | The screen, as a bare framebuffer | the screenshot, encoded as a PNG |
//! | 2 | The game's save memory | a save part: the in-game save as it stood when the state was taken; a Game Boy clock footer after it goes into [`x.1sav.rtc`](one_saves::extension::Rtc) and the chip's key on the header, as on a save |
//! | 3 | The player's cheats, as mGBA's `.cheats` text | [`x.1sav.cheats.mgba`](one_saves::extension::CheatsMgba) on the state part |
//! | 5 | The framebuffer's width and height | nothing: it is how the framebuffer is read |
//! | `0x101` | When the state was taken, in microseconds | `taken_at` |
//! | `0x102` | What took it, as `mGBA 0.10.5` | `source`'s versions |
//! | any other | mGBA's own, such as the clock it is told to report | an `aux` part whose `path` is `extdata/` and the tag |
//!
//! A slot is in the file's name: `X.ss1` to `X.ss9` as the player picks them, and `X.ss0`, which
//! mGBA keeps to resume from, so it has no slot and an `origin` of `auto`.
//!
//! mGBA's libretro core keeps the same items after its data, and RetroArch wraps the lot as one
//! block; that is a RetroArch state, read by [`retroarch`](super::retroarch), whose state part is
//! the core's data with its items still in it, since that is what the core loads.

use one_saves::extension::{CheatSet, CheatsMgba};
use one_saves::{Bundle, Game, HashAlgorithm, HashValue, Part, PartKind, Slug};

use crate::error::{Error, Result};
use crate::state::image::{self, Chunk, le32};
use crate::state::{Contents, Found, PNG_SIGNATURE, Rebuilt, Slot, StateFile, StateOptions};

const FORMAT: &str = "mGBA state";

const SCREENSHOT: u32 = 1;
const SAVEDATA: u32 = 2;
const CHEATS: u32 = 3;
const SCREENSHOT_DIMENSIONS: u32 = 5;
const META_TIME: u32 = 0x101;
const META_CREATOR: u32 = 0x102;

/// The most the core's data inflates to. A GBA state is 388 KiB; this leaves room for a later one.
const MAX_STATE: usize = 16 << 20;

/// The most all of a state's items inflate to between them. The largest real one is a cartridge's
/// save memory, at 128 KiB; the cap is on the sum, since a file can hold any number of items.
const MAX_ITEMS: usize = 64 << 20;

/// The most items a state may hold. mGBA keeps one item to a tag, and has fewer than a dozen tags.
///
/// [`MAX_ITEMS`] bounds what the items hold and not how many there are, and an item of nothing
/// still becomes a part of its own, which costs far more than the sixteen bytes its header takes
/// in the file: a file of nothing but empty items would otherwise cost a hundred times its size.
const MAX_ITEM_COUNT: usize = 64;

/// The widest and tallest screen mGBA has: the Super Game Boy's, border and all. mGBA reads a
/// state's picture into the screen it is running and clips whatever is past it.
const LARGEST_SCREEN: (u32, u32) = (256, 224);

/// The `path` prefix an item mGBA keeps for itself takes as an `aux` part.
const ITEM_PATH: &str = "extdata/";

/// mGBA's items of extra data, each with its tag, in the order the file holds them.
type Items = Vec<(u32, Vec<u8>)>;

/// The two cores mGBA runs, each with a data layout of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Machine {
    Gba,
    /// The Game Boy core, which runs the Game Boy and the Game Boy Color both.
    Gb,
}

impl Machine {
    /// Which core a state's data is for, from the version word it opens with.
    fn of(data: &[u8]) -> Option<Machine> {
        let word = u32::from_le_bytes(data.get(..4)?.try_into().ok()?);
        match word & 0xFFFF_FF00 {
            0x0100_0000 => Some(Machine::Gba),
            0x0040_0000 => Some(Machine::Gb),
            _ => None,
        }
    }

    /// How many bytes the core's data runs to.
    fn size(self) -> usize {
        match self {
            Machine::Gba => 0x61000,
            Machine::Gb => 0x11800,
        }
    }
}

/// The system a state's game ran as, and what the core's data says of the game.
///
/// Both cores open their data with the ROM's CRC-32, which mGBA checks a state against before it
/// loads one, and with the title the ROM's header gives: a GBA one at `0x10` with its game code
/// after it, and the Game Boy's sixteen header bytes at `0x10`, the last of them the cartridge's
/// Color flag. The CRC is the hash that finds the ROM the state was taken against.
///
/// A Game Boy cartridge says whether it needs a Color (`0xC0`), runs on both (`0x80`) or predates
/// the Color. One that runs on both ran as whichever model mGBA emulated, which the Game Boy's data
/// keeps at `0x08`, the Color's models setting `0x80`. Its release is left unsaid, since No-Intro
/// files such games under either system.
fn identity(machine: Machine, data: &[u8]) -> (Slug, Option<Game>) {
    let slug = |text: &str| Slug::parse(text).expect("a system slug");
    let crc = |at: usize| data.get(at..at + 4).map(|word| [word[3], word[2], word[1], word[0]]);
    let mut game = Game::default();
    let system = match machine {
        Machine::Gba => {
            game.rom_hashes = crc(0x08).into_iter().map(hash).collect();
            game.title = data.get(0x10..0x1C).and_then(field);
            game.serial = data.get(0x1C..0x20).and_then(field);
            game.system = Some(slug("gba"));
            slug("gba")
        }
        Machine::Gb => {
            game.rom_hashes = crc(0x04).into_iter().map(hash).collect();
            let model = data.get(0x08).copied().unwrap_or_default();
            let header = data.get(0x10..0x20).unwrap_or_default();
            let flag = header.get(15).copied().unwrap_or_default();
            // The title gave its last byte to the flag, as rom-id reads it.
            let title = if matches!(flag, 0x80 | 0xC0) { header.get(..15) } else { Some(header) };
            game.title = title.and_then(field);
            let (release, ran_as) = match flag {
                0xC0 => (Some("gbc"), "gbc"),
                0x80 => (None, if model & 0x80 == 0 { "gb" } else { "gbc" }),
                _ => (Some("gb"), "gb"),
            };
            game.system = release.map(slug);
            slug(ran_as)
        }
    };
    (system, (!game.is_empty()).then_some(game))
}

/// A CRC-32, as the format carries one: its digest in the order it is written out.
fn hash(digest: [u8; 4]) -> HashValue {
    HashValue::new(HashAlgorithm::Crc32, digest.to_vec()).expect("a CRC-32 is four bytes")
}

/// A text field of a ROM's header, up to its first zero and trimmed, as rom-id reads one.
fn field(bytes: &[u8]) -> Option<String> {
    let end = bytes.iter().position(|&byte| byte == 0).unwrap_or(bytes.len());
    let text = String::from_utf8_lossy(&bytes[..end]).trim().to_owned();
    (!text.is_empty()).then_some(text)
}

/// Reads an mGBA state file, in either of its containers, into a state bundle.
///
/// # Errors
///
/// [`Error::NotThisFormat`] for a file whose data opens with no version word mGBA writes, and
/// [`Error::Corrupt`] for one that does not unpack.
pub fn read(file: &StateFile<'_>, options: &StateOptions) -> Result<Bundle> {
    let (data, items, picture) = if file.bytes.starts_with(PNG_SIGNATURE) {
        unpack_png(file.bytes)?
    } else {
        let (data, items) = unpack_bare(file.bytes)?;
        (data, items, None)
    };
    let machine = Machine::of(&data).ok_or_else(|| Error::NotThisFormat {
        format: FORMAT,
        why: "its data opens with no version word mGBA writes".into(),
    })?;
    let (system, game) = identity(machine, &data);
    let (origin, slot) =
        crate::state::origin_and_slot(crate::state::mgba_slot(&file.name.to_ascii_lowercase()));

    let mut found = Found {
        app: Some("mgba"),
        core: Some("mgba"),
        system: Some(system),
        game,
        origin,
        ..Found::default()
    };
    let mut state = crate::state::state_part(data, slot);
    let mut screenshot = picture;
    let mut rest = Vec::new();
    let dimensions =
        items.iter().find(|(tag, _)| *tag == SCREENSHOT_DIMENSIONS).map(|(_, item)| item.clone());

    for (tag, item) in items {
        match tag {
            SCREENSHOT if screenshot.is_none() => match framebuffer(&item, dimensions.as_deref())? {
                Some(png) => screenshot = Some(png),
                None => rest.push(aux(tag, item)),
            },
            SAVEDATA if !rest.iter().any(|part: &Part| part.kind == PartKind::Save) => {
                // The Game Boy core hands over its save as the save file holds it, with the clock
                // footer after the SRAM. The footer goes into the header's keys, as on a save.
                let system = found.system.as_ref().map(Slug::as_str);
                match (machine == Machine::Gb).then(|| crate::rtc::split(&item, system)).flatten() {
                    Some(split) => {
                        found.extensions = split.extensions();
                        rest.push(Part::new(0, split.sram));
                    }
                    None => rest.push(Part::new(0, item)),
                }
            }
            CHEATS => match std::str::from_utf8(&item).map(parse_cheats) {
                Ok(sets) if !sets.is_empty() => state = state.extension(CheatsMgba::new(sets)),
                // An empty section is the same as none to mGBA, so it is left off.
                Ok(_) => {}
                Err(_) => rest.push(aux(tag, item)),
            },
            SCREENSHOT_DIMENSIONS => {}
            META_TIME if item.len() == 8 => {
                let micros = i64::from_le_bytes(item.as_slice().try_into().expect("eight bytes"));
                found.taken_at = Some(micros.div_euclid(1_000_000));
            }
            META_CREATOR => match creator_version(&item) {
                Some(version) => {
                    found.app_version = Some(version.clone());
                    found.core_version = Some(version);
                }
                None => rest.push(aux(tag, item)),
            },
            _ => rest.push(aux(tag, item)),
        }
    }

    let mut parts = vec![state];
    if let Some(screenshot) = screenshot {
        parts.push(crate::state::screenshot_part(screenshot)?);
    }
    parts.extend(rest);
    let bundle = Bundle::new(crate::state::header(options, found), crate::state::number(parts));
    bundle.validate()?;
    Ok(bundle)
}

/// Rebuilds an mGBA state file from a state bundle.
///
/// A PNG screenshot makes it the PNG container mGBA writes by default; without one it is the bare
/// container. mGBA reads only an eight-bit RGB PNG, so a screenshot in any other PNG layout is
/// encoded again as one, which loses nothing it could show. One larger than any screen mGBA has is
/// not, since mGBA would clip it, and is left out.
///
/// # Errors
///
/// [`Error::NotConvertible`] for a bundle that is not a state, loads into another core, or holds a
/// part larger than mGBA's file has room for.
pub fn write(bundle: &Bundle, stem: &str) -> Result<Rebuilt> {
    let contents = Contents::of(bundle)?;
    crate::state::require_core(bundle, "mgba", "mGBA")?;

    // Each part is held to what mGBA's file has room for before it is inflated, since a
    // compressed part's length is its own claim and inflating it is what costs the memory.
    let data = crate::state::bytes_within(contents.state, MAX_STATE, "an mGBA state")?;
    let mut room = MAX_ITEMS;
    let mut item = |part: &Part| -> Result<Vec<u8>> {
        let bytes = crate::state::bytes_within(part, room, "an mGBA state")?.into_owned();
        room = room.saturating_sub(bytes.len());
        Ok(bytes)
    };
    let mut left_out = Vec::new();
    let mut items: Vec<(u32, Vec<u8>)> = Vec::new();
    for (index, save) in contents.saves.iter().enumerate() {
        if index == 0 {
            let mut save = item(save)?;
            // The Game Boy core takes its save back with the clock footer on it, as its file has
            // it; the GBA core keeps its clock elsewhere. The footer is the one mGBA's save memory
            // holds whichever app ran it: under libretro the frontend writes the same bytes to a
            // file of their own, but in a state they are where the core keeps them.
            let form =
                crate::rtc::ClockForm::Mbc3Appended(one_saves::extension::RtcMbc3::DEFAULT_TIMESTAMP_WIDTH);
            if Machine::of(&data) == Some(Machine::Gb)
                && let Some(footer) = crate::rtc::footer_of_form(bundle, form)
            {
                save.extend_from_slice(&footer);
            }
            items.push((SAVEDATA, save));
        } else {
            left_out.push(save.id);
        }
    }
    if let Some(cheats) = contents.state.extensions.get::<CheatsMgba>() {
        items.push((CHEATS, cheats_text(cheats).into_bytes()));
    }
    for part in &contents.aux {
        match part
            .path
            .as_deref()
            .and_then(|path| path.strip_prefix(ITEM_PATH))
            .and_then(|tag| tag.parse().ok())
        {
            Some(tag) => items.push((tag, item(part)?)),
            None => left_out.push(part.id),
        }
    }
    if let Some(seconds) = bundle.header.taken_at {
        items.push((META_TIME, seconds.saturating_mul(1_000_000).to_le_bytes().to_vec()));
    }
    let source = bundle.header.source.as_ref();
    let version =
        source.and_then(|source| source.app_version.clone().or_else(|| source.core_version.clone()));
    if let Some(version) = version {
        items.push((META_CREATOR, format!("mGBA {version}\0").into_bytes()));
    }
    items.sort_by_key(|(tag, _)| *tag);

    // A picture that is not a PNG mGBA can read back, and cannot be made one, stays in the bundle
    // rather than costing the state its rebuild; the state is then written without one.
    let png = match contents.screenshot {
        Some(part) => {
            let png = (part.content_type.as_deref() == Some("image/png"))
                .then(|| rgb_png(&part.bytes().ok()?).ok())
                .flatten();
            if png.is_none() {
                left_out.push(part.id);
            }
            png
        }
        None => None,
    };
    let bytes = match png {
        Some(png) => pack_png(&png, &data, &items)?,
        None => pack_bare(&data, &items)?,
    };

    let stem = crate::state::component(stem);
    let name = match crate::state::slot_of(bundle, contents.state) {
        Some(Slot::Auto) => format!("{stem}.ss0"),
        Some(Slot::Numbered(slot)) => format!("{stem}.ss{slot}"),
        None => format!("{stem}.ss1"),
    };
    left_out.sort_unstable();
    Ok(Rebuilt { name, bytes, screenshot: None, left_out })
}

/// An item mGBA keeps for itself, as an `aux` part named for its tag.
fn aux(tag: u32, item: Vec<u8>) -> Part {
    Part::new(0, item)
        .kind(PartKind::Aux)
        .path(format!("{ITEM_PATH}{tag}"))
        .content_type("application/octet-stream")
}

/// The version an `mGBA 0.10.5` creator names, or `None` for something that is not mGBA's.
fn creator_version(item: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(item.strip_suffix(&[0]).unwrap_or(item)).ok()?;
    let version = text.strip_prefix("mGBA ")?;
    (!version.is_empty()).then(|| version.to_owned())
}

/// Unpacks the PNG container: the core's data, the items, and the image without mGBA's chunks.
fn unpack_png(bytes: &[u8]) -> Result<(Vec<u8>, Items, Option<Vec<u8>>)> {
    let corrupt = |why: &str| Error::Corrupt { format: FORMAT, why: why.into() };
    let chunks = image::chunks(bytes, FORMAT)?;
    let mut data = None;
    let mut items = Vec::new();
    let mut inflated = 0;
    let mut picture = PNG_SIGNATURE.to_vec();
    for Chunk { kind, data: body, raw } in chunks {
        match &kind {
            b"gbAs" => data = Some(image::inflate_zlib(body, MAX_STATE, FORMAT)?),
            b"gbAx" => {
                if items.len() == MAX_ITEM_COUNT {
                    return Err(corrupt("it holds more items than mGBA writes"));
                }
                let tag = le32(body, 0, FORMAT)?;
                let size = le32(body, 4, FORMAT)? as usize;
                inflated = size.saturating_add(inflated);
                if inflated > MAX_ITEMS {
                    return Err(corrupt("its items claim more than any mGBA writes"));
                }
                let item = image::inflate_zlib(&body[8..], size, FORMAT)?;
                if item.len() != size {
                    return Err(corrupt("an item inflates to another length than it says"));
                }
                items.push((tag, item));
            }
            _ => picture.extend_from_slice(raw),
        }
    }
    let data = data.ok_or_else(|| corrupt("its PNG carries no gbAs chunk, which is the core's data"))?;
    Ok((data, items, Some(picture)))
}

/// Unpacks the bare container: the core's data at its fixed size, then the items.
fn unpack_bare(bytes: &[u8]) -> Result<(Vec<u8>, Items)> {
    let corrupt = |why: String| Error::Corrupt { format: FORMAT, why };
    let size = Machine::of(bytes)
        .ok_or_else(|| Error::NotThisFormat {
            format: FORMAT,
            why: "it opens with no version word mGBA writes".into(),
        })?
        .size();
    let data = bytes
        .get(..size)
        .ok_or_else(|| corrupt(format!("it ends before the {size} bytes of the core's data")))?;

    let mut items = Vec::new();
    let mut copied: usize = 0;
    let mut at = size;
    // A state with no items has nothing after its data, not even the list's end.
    while at < bytes.len() {
        let header = at
            .checked_add(16)
            .and_then(|end| bytes.get(at..end))
            .ok_or_else(|| corrupt("its list of items is cut short".into()))?;
        let tag = u32::from_le_bytes(header[0..4].try_into().expect("four bytes"));
        if tag == 0 {
            break;
        }
        if items.len() == MAX_ITEM_COUNT {
            return Err(corrupt("it holds more items than mGBA writes".into()));
        }
        let length = i32::from_le_bytes(header[4..8].try_into().expect("four bytes"));
        let offset = i64::from_le_bytes(header[8..16].try_into().expect("eight bytes"));
        let item = usize::try_from(offset)
            .ok()
            .zip(usize::try_from(length).ok())
            .and_then(|(offset, length)| bytes.get(offset..offset.checked_add(length)?))
            .ok_or_else(|| corrupt(format!("item {tag} lies outside the file")))?;
        // Each header names its own span, and nothing stops a thousand of them naming the same
        // one, so the file's own length bounds no item's copy but not the sum of them.
        copied = copied.saturating_add(item.len());
        if copied > MAX_ITEMS {
            return Err(corrupt("its items claim more than any mGBA writes".into()));
        }
        items.push((tag, item.to_vec()));
        at += 16;
    }
    Ok((data.to_vec(), items))
}

/// Packs the PNG container around an eight-bit RGB PNG.
fn pack_png(png: &[u8], data: &[u8], items: &[(u32, Vec<u8>)]) -> Result<Vec<u8>> {
    let mut out = PNG_SIGNATURE.to_vec();
    for chunk in image::chunks(png, "screenshot")? {
        if &chunk.kind != b"IEND" {
            out.extend_from_slice(chunk.raw);
        }
    }
    out.extend_from_slice(&image::chunk(*b"gbAs", &image::deflate_zlib(data))?);
    for (tag, item) in items {
        let mut body = tag.to_le_bytes().to_vec();
        body.extend_from_slice(&u32::try_from(item.len()).map_err(|_| too_long(*tag, item))?.to_le_bytes());
        body.extend_from_slice(&image::deflate_zlib(item));
        out.extend_from_slice(&image::chunk(*b"gbAx", &body)?);
    }
    out.extend_from_slice(&image::chunk(*b"IEND", &[])?);
    Ok(out)
}

/// Packs the bare container: the data, the list of items and its end, then the items.
fn pack_bare(data: &[u8], items: &[(u32, Vec<u8>)]) -> Result<Vec<u8>> {
    let mut out = data.to_vec();
    if items.is_empty() {
        return Ok(out);
    }
    let mut offset = data.len() + (items.len() + 1) * 16;
    for (tag, item) in items {
        let length = i32::try_from(item.len()).map_err(|_| too_long(*tag, item))?;
        let at = i64::try_from(offset).map_err(|_| too_long(*tag, item))?;
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&at.to_le_bytes());
        offset += item.len();
    }
    out.extend_from_slice(&[0; 16]);
    for (_, item) in items {
        out.extend_from_slice(item);
    }
    Ok(out)
}

/// What an item too long for the length mGBA gives it is refused as.
fn too_long(tag: u32, item: &[u8]) -> Error {
    Error::NotConvertible(format!("item {tag} is {} bytes, more than an mGBA state has room for", item.len()))
}

/// A PNG in the one layout mGBA reads back: eight-bit RGB, not interlaced.
///
/// One in any other layout is decoded and encoded again, and refused first where it claims to be
/// larger than [`LARGEST_SCREEN`].
fn rgb_png(png: &[u8]) -> Result<Vec<u8>> {
    let header = image::chunks(png, "screenshot")?
        .into_iter()
        .find(|chunk| &chunk.kind == b"IHDR")
        .map(|chunk| chunk.data.to_vec());
    if let Some(header) = header
        && header.len() == 13
        && header[8] == 8
        && header[9] == 2
        && header[12] == 0
    {
        return Ok(png.to_vec());
    }
    let (width, height, rgba) = image::decode_rgba(png, LARGEST_SCREEN)?;
    image::encode(width, height, 3, &image::rgb_of(&rgba))
}

/// A framebuffer item as a PNG, or `None` where its size says nothing about its shape.
///
/// Four bytes a pixel, red first and the fourth unused. Its width and height come from the item
/// mGBA keeps for them where there is one, and otherwise from the screens mGBA has: the GBA's, the
/// Game Boy's, and the Super Game Boy's with its border.
fn framebuffer(item: &[u8], dimensions: Option<&[u8]>) -> Result<Option<Vec<u8>>> {
    let given = dimensions
        .filter(|d| d.len() == 4)
        .map(|d| (u32::from(u16::from_le_bytes([d[0], d[1]])), u32::from(u16::from_le_bytes([d[2], d[3]]))));
    let pixels = item.len() / 4;
    let shape = given.or_else(|| {
        [(240, 160), (160, 144), (256, 224)].into_iter().find(|(w, h)| (w * h) as usize == pixels)
    });
    let Some((width, height)) = shape.filter(|(w, h)| (w * h) as usize * 4 == item.len()) else {
        return Ok(None);
    };
    image::encode(width, height, 3, &image::rgb_of(item)).map(Some)
}

/// Reads mGBA's cheats text into its sets.
///
/// A set opens at a `#` line, whose rest is its name. The `!` lines before it are its directives,
/// and `!disabled` among them switches it off. Every other line is a code, in the set opened last,
/// or in a set of its own with no name when none has been.
#[must_use]
pub fn parse_cheats(text: &str) -> Vec<CheatSet> {
    let mut sets = Vec::new();
    let mut current: Option<CheatSet> = None;
    let mut disabled = false;
    let mut directives = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix('#') {
            sets.extend(current.take());
            let name = name.trim_start();
            current = Some(CheatSet {
                name: (!name.is_empty()).then(|| name.to_owned()),
                disabled: std::mem::take(&mut disabled),
                directives: std::mem::take(&mut directives),
                codes: Vec::new(),
            });
        } else if let Some(directive) = line.strip_prefix('!') {
            let directive = directive.trim_start();
            if directive.eq_ignore_ascii_case("disabled") {
                disabled = true;
            } else {
                directives.push(directive.to_owned());
            }
        } else {
            current
                .get_or_insert_with(|| CheatSet {
                    disabled: std::mem::take(&mut disabled),
                    directives: std::mem::take(&mut directives),
                    ..CheatSet::default()
                })
                .codes
                .push(line.to_owned());
        }
    }
    sets.extend(current);
    sets
}

/// Writes sets as mGBA's cheats text, the way mGBA writes its own `.cheats` files.
#[must_use]
pub fn cheats_text(cheats: &CheatsMgba) -> String {
    let mut text = String::new();
    for set in &cheats.sets {
        if set.disabled {
            text.push_str("!disabled\n");
        }
        for directive in &set.directives {
            text.push('!');
            text.push_str(directive);
            text.push('\n');
        }
        text.push_str("# ");
        text.push_str(set.name.as_deref().unwrap_or_default());
        text.push('\n');
        for code in &set.codes {
            text.push_str(code);
            text.push('\n');
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use one_saves::{Name, Origin};

    use super::*;

    /// A GBA state's data: the version word for 0.10, then filler.
    fn gba_data() -> Vec<u8> {
        let mut data = vec![0x5A; 0x61000];
        data[..4].copy_from_slice(&0x0100_0007u32.to_le_bytes());
        data
    }

    fn screen() -> Vec<u8> {
        image::encode(240, 160, 3, &vec![0x40; 240 * 160 * 3]).unwrap()
    }

    #[test]
    fn a_png_state_splits_into_its_parts_and_packs_back() {
        let save = vec![0xA5; 0x20000];
        let cheats = "!GSAv1\n# Infinite money\nD8BAE4D9 4864DCE5\n";
        let items = vec![
            (SAVEDATA, save.clone()),
            (CHEATS, cheats.as_bytes().to_vec()),
            (4, vec![0; 16]),
            (META_TIME, 1_793_000_000_123_456i64.to_le_bytes().to_vec()),
            (META_CREATOR, b"mGBA 0.10.5\0".to_vec()),
        ];
        let file = pack_png(&screen(), &gba_data(), &items).unwrap();

        let state = StateFile { bytes: &file, name: "Pokemon Emerald.ss1", screenshot: None };
        let bundle = read(&state, &StateOptions::default()).unwrap();
        assert_eq!(bundle.header.system.as_ref().map(Slug::as_str), Some("gba"));
        assert_eq!(bundle.header.taken_at, Some(1_793_000_000));
        let source = bundle.header.source.as_ref().unwrap();
        assert_eq!(source.app_version.as_deref(), Some("0.10.5"));
        let kinds: Vec<_> = bundle.parts.iter().map(|part| part.kind.clone()).collect();
        assert_eq!(kinds, [PartKind::State, PartKind::Screenshot, PartKind::Save, PartKind::Aux]);
        assert_eq!(bundle.parts[0].bytes().unwrap(), gba_data().as_slice());
        assert_eq!(bundle.parts[0].slot, Some(1));
        assert_eq!(bundle.parts[1].bytes().unwrap(), screen().as_slice(), "the image, without mGBA's chunks");
        assert_eq!(bundle.parts[2].bytes().unwrap(), save.as_slice());
        assert_eq!(bundle.parts[3].path.as_deref(), Some("extdata/4"));
        let sets = &bundle.parts[0].extensions.get::<CheatsMgba>().unwrap().sets;
        assert_eq!(sets[0].directives, ["GSAv1"]);

        // Rebuilt, it unpacks to the same data and items; only the time loses its microseconds.
        let rebuilt = write(&bundle, "Pokemon Emerald").unwrap();
        assert_eq!(rebuilt.name, "Pokemon Emerald.ss1");
        let (data, items, _) = unpack_png(&rebuilt.bytes).unwrap();
        assert_eq!(data, gba_data());
        let tags: Vec<u32> = items.iter().map(|(tag, _)| *tag).collect();
        assert_eq!(tags, [SAVEDATA, CHEATS, 4, META_TIME, META_CREATOR]);
        assert_eq!(items[1].1, cheats.as_bytes());
    }

    #[test]
    fn the_data_names_the_rom_it_was_taken_against() {
        let mut data = gba_data();
        data[0x08..0x0C].copy_from_slice(&0x1F1C_08FBu32.to_le_bytes());
        data[0x10..0x20].copy_from_slice(b"POKEMON EMERBPEE");
        let file = pack_bare(&data, &[]).unwrap();
        let bundle =
            read(&StateFile { bytes: &file, name: "game.ss1", screenshot: None }, &StateOptions::default())
                .unwrap();
        let game = bundle.header.game.as_ref().unwrap();
        assert_eq!(game.title.as_deref(), Some("POKEMON EMER"));
        assert_eq!(game.serial.as_deref(), Some("BPEE"));
        assert_eq!(game.rom_hashes, [hash([0x1F, 0x1C, 0x08, 0xFB])], "the CRC as a DAT writes it");
        assert_eq!(game.system.as_ref().map(Slug::as_str), Some("gba"));
    }

    #[test]
    fn a_game_boy_state_ran_as_the_model_mgba_emulated() {
        // The cartridge's Color flag, the model mGBA ran, then the system the state is for and the
        // release the game's hints name.
        let cases = [
            (0x80, 0x80, "gbc", None, "POKEMON_GLDAAUE"),
            (0x80, 0x00, "gb", None, "POKEMON_GLDAAUE"),
            (0xC0, 0xC0, "gbc", Some("gbc"), "POKEMON_GLDAAUE"),
            (0x00, 0x80, "gb", Some("gb"), "POKEMON_GLDAAUEX"),
        ];
        for (flag, model, system, release, title) in cases {
            let mut data = vec![0; 0x11800];
            data[..4].copy_from_slice(&0x0040_0003u32.to_le_bytes());
            data[0x04..0x08].copy_from_slice(&0x6BDE_3C3Eu32.to_le_bytes());
            data[0x08] = model;
            data[0x10..0x1F].copy_from_slice(b"POKEMON_GLDAAUE");
            data[0x1F] = if flag == 0 { b'X' } else { flag };
            let file = pack_bare(&data, &[]).unwrap();
            let state = StateFile { bytes: &file, name: "game.ss1", screenshot: None };
            let bundle = read(&state, &StateOptions::default()).unwrap();
            let game = bundle.header.game.as_ref().unwrap();
            assert_eq!(
                bundle.header.system.as_ref().map(Slug::as_str),
                Some(system),
                "{flag:#x} on {model:#x}"
            );
            assert_eq!(game.system.as_ref().map(Slug::as_str), release, "{flag:#x} on {model:#x}");
            assert_eq!(game.title.as_deref(), Some(title), "the flag is no part of the title");
            assert_eq!(game.rom_hashes, [hash([0x6B, 0xDE, 0x3C, 0x3E])]);
        }
    }

    #[test]
    fn a_game_boy_saves_clock_footer_moves_to_the_header_and_back() {
        use one_saves::extension::{Rtc, RtcMbc3};

        let mut data = vec![0; 0x11800];
        data[..4].copy_from_slice(&0x0040_0003u32.to_le_bytes());
        data[0x1F] = 0x80;
        let registers = [3, 2, 0, 0, 0];
        // Written fifty seconds before the state, as mGBA writes the footer when it flushes the save.
        let anchor = 1_793_379_451;
        let mut save = vec![0x42; 0x8000];
        save.extend_from_slice(&crate::rtc::build_mbc3(&registers, &registers, anchor, 8));
        let items =
            [(SAVEDATA, save.clone()), (META_TIME, ((anchor + 50) * 1_000_000).to_le_bytes().to_vec())];
        let file = pack_bare(&data, &items).unwrap();

        let state = StateFile { bytes: &file, name: "Pokemon Gold.ss1", screenshot: None };
        let bundle = read(&state, &StateOptions::default()).unwrap();
        assert_eq!(bundle.parts[1].bytes().unwrap().as_ref(), &save[..0x8000], "the SRAM alone");
        let clock = bundle.header.extensions.get::<RtcMbc3>().expect("the footer's clock");
        assert_eq!((clock.live, clock.taken_at), (registers, anchor), "anchored when the footer was written");
        assert_eq!(bundle.header.taken_at, Some(anchor + 50));
        assert!(bundle.header.extensions.get::<Rtc>().is_some());

        let rebuilt = write(&bundle, "Pokemon Gold").unwrap();
        assert_eq!(unpack_bare(&rebuilt.bytes).unwrap().1[0], (SAVEDATA, save), "the footer goes back on");
        assert_eq!(rebuilt.bytes, file);
    }

    #[test]
    fn a_gba_save_keeps_its_bytes_whatever_its_length() {
        // mGBA keeps a GBA cartridge's clock out of the save it puts in a state, so nothing after the
        // save memory is a footer to take off.
        let mut save = vec![0x42; 0x20000];
        save.extend_from_slice(&[0; 16]);
        let file = pack_bare(&gba_data(), &[(SAVEDATA, save.clone())]).unwrap();
        let state = StateFile { bytes: &file, name: "game.ss1", screenshot: None };
        let bundle = read(&state, &StateOptions::default()).unwrap();
        assert_eq!(bundle.parts[1].bytes().unwrap().as_ref(), save.as_slice());
        assert!(bundle.header.extensions.is_empty());
    }

    #[test]
    fn a_game_the_caller_names_keeps_its_hashes_and_takes_the_files_where_it_has_none() {
        let mut data = gba_data();
        data[0x08..0x0C].copy_from_slice(&0x1F1C_08FBu32.to_le_bytes());
        let file = pack_bare(&data, &[]).unwrap();
        let state = StateFile { bytes: &file, name: "game.ss1", screenshot: None };

        let mut named = Game::default();
        named.title = Some("Pokémon Emerald".into());
        let bundle = read(&state, &StateOptions::default().game(named.clone())).unwrap();
        let game = bundle.header.game.as_ref().unwrap();
        assert_eq!(game.title.as_deref(), Some("Pokémon Emerald"), "the caller's title wins");
        assert_eq!(game.rom_hashes, [hash([0x1F, 0x1C, 0x08, 0xFB])], "the file's hash fills the gap");

        named.rom_hashes = vec![HashValue::new(HashAlgorithm::Sha1, vec![7; 20]).unwrap()];
        let bundle = read(&state, &StateOptions::default().game(named.clone())).unwrap();
        assert_eq!(bundle.header.game.as_ref().unwrap().rom_hashes, named.rom_hashes);
    }

    #[test]
    fn a_bare_state_reads_its_items_by_offset() {
        let file =
            pack_bare(&gba_data(), &[(SCREENSHOT, vec![0x80; 240 * 160 * 4]), (SAVEDATA, vec![1; 512])])
                .unwrap();
        let state = StateFile { bytes: &file, name: "game.ss0", screenshot: None };
        let bundle = read(&state, &StateOptions::default()).unwrap();
        assert_eq!(bundle.header.origin, Some(Origin::Auto), "slot 0 is the one mGBA resumes from");
        assert_eq!(bundle.parts[1].content_type.as_deref(), Some("image/png"), "the framebuffer, made a PNG");
        assert_eq!(bundle.parts[2].bytes().unwrap().as_ref(), &[1; 512]);
        assert_eq!(unpack_bare(&pack_bare(&gba_data(), &[]).unwrap()).unwrap().1, []);
    }

    #[test]
    fn items_that_all_name_one_span_are_refused_before_they_are_copied() {
        // One large item, then headers that all point back at it: each copy is in bounds, and
        // together they would be gigabytes.
        let mut file = gba_data();
        let span = 1 << 20;
        let headers = 128;
        let item_at = file.len() + (headers + 1) * 16;
        for _ in 0..headers {
            file.extend_from_slice(&4u32.to_le_bytes());
            file.extend_from_slice(&i32::try_from(span).unwrap().to_le_bytes());
            file.extend_from_slice(&i64::try_from(item_at).unwrap().to_le_bytes());
        }
        file.extend_from_slice(&[0; 16]);
        file.resize(item_at + span, 0);
        assert!(matches!(unpack_bare(&file), Err(Error::Corrupt { .. })));
    }

    #[test]
    fn more_items_than_mgba_writes_are_refused_before_they_become_parts() {
        // An item of nothing is sixteen bytes in the bare container and a part in the bundle, so a
        // file of a million of them once cost a hundred times its size.
        let empty: Vec<(u32, Vec<u8>)> = (0x200..).zip(vec![Vec::new(); MAX_ITEM_COUNT + 1]).collect();
        let bare = pack_bare(&gba_data(), &empty).unwrap();
        let png = pack_png(&screen(), &gba_data(), &empty).unwrap();
        for (file, container) in [(bare, "bare"), (png, "PNG")] {
            match read_as(&file, "game.ss1") {
                Err(Error::Corrupt { why, .. }) => assert!(why.contains("items"), "{container}: {why}"),
                other => panic!("{container}: {other:?}"),
            }
        }
        let most = read_as(&pack_bare(&gba_data(), &empty[1..]).unwrap(), "game.ss1").unwrap();
        assert_eq!(most.parts.len(), 1 + MAX_ITEM_COUNT, "as many as are read");
    }

    #[test]
    fn a_part_claiming_more_than_mgbas_file_holds_is_refused_before_it_is_inflated() {
        // Each claims more than its length in the file has room for, or more than mGBA's reader
        // takes back. An item past 2 GiB once panicked here, after it had been inflated.
        let bundle = read_as(&pack_bare(&gba_data(), &[]).unwrap(), "game.ss1").unwrap();
        let claims = [
            crate::state::part_claiming(1, (1 << 31) + 16).kind(PartKind::Aux).path("extdata/4"),
            crate::state::part_claiming(1, MAX_ITEMS as u64 + 1),
        ];
        for part in claims {
            let mut odd = bundle.clone();
            odd.parts.push(part);
            let error = write(&odd, "game").unwrap_err();
            assert!(matches!(error, Error::NotConvertible(_)), "{error}");
        }
        let mut odd = bundle;
        odd.parts[0] = crate::state::part_claiming(0, MAX_STATE as u64 + 1).kind(PartKind::State);
        let error = write(&odd, "game").unwrap_err();
        assert!(matches!(error, Error::NotConvertible(_)), "the core's data: {error}");
    }

    #[test]
    fn a_picture_larger_than_any_screen_mgba_has_is_left_out_without_being_decoded() {
        // A few dozen bytes claiming a petabyte of pixels, which decoding would have allocated
        // for, and one a pixel wider than the Super Game Boy's screen, which mGBA would clip.
        let (most_wide, most_high) = LARGEST_SCREEN;
        let wide = image::encode(most_wide + 1, most_high, 4, &vec![0; 4 * (257 * 224)]).unwrap();
        for picture in [image::claiming(1 << 24, 1 << 24), wide] {
            let mut bundle = read_as(&pack_bare(&gba_data(), &[]).unwrap(), "game.ss1").unwrap();
            bundle.parts.push(crate::state::screenshot_part(picture).unwrap());
            bundle.parts = crate::state::number(bundle.parts);
            let rebuilt = write(&bundle, "game").unwrap();
            assert_eq!((rebuilt.left_out, rebuilt.bytes), (vec![1], gba_data()), "written bare");
        }

        // The largest screen mGBA has is drawn again as the RGB it reads.
        let largest = image::encode(most_wide, most_high, 4, &vec![0; 4 * (256 * 224)]).unwrap();
        let mut bundle = read_as(&pack_bare(&gba_data(), &[]).unwrap(), "game.ss1").unwrap();
        bundle.parts.push(crate::state::screenshot_part(largest).unwrap());
        bundle.parts = crate::state::number(bundle.parts);
        assert_eq!(write(&bundle, "game").unwrap().left_out, []);
    }

    #[test]
    fn cheats_round_trip_through_mgbas_text() {
        let text = "!disabled\n!PARv3\n# Walk through walls\n1234 5678\nABCD EF01\n# \n0000 0000\n";
        let sets = parse_cheats(text);
        assert_eq!(sets.len(), 2);
        assert!(sets[0].disabled && sets[0].directives == ["PARv3"] && sets[0].codes.len() == 2);
        assert_eq!(sets[1].name, None, "a `#` with nothing after it names nothing");
        assert_eq!(cheats_text(&CheatsMgba::new(sets)), text);
    }

    #[test]
    fn a_state_for_another_core_is_not_rebuilt_as_mgbas() {
        let source = one_saves::Source::default().core(Name::parse("vba-next").unwrap());
        let options = StateOptions::default().source(source);
        let file = pack_bare(&gba_data(), &[]).unwrap();
        let mut bundle =
            read(&StateFile { bytes: &file, name: "game.ss1", screenshot: None }, &options).unwrap();
        bundle.header.source = options.source.map(|source| source.app(None));
        assert!(matches!(write(&bundle, "game"), Err(Error::NotConvertible(_))));
    }

    fn read_as(file: &[u8], name: &str) -> Result<Bundle> {
        read(&StateFile { bytes: file, name, screenshot: None }, &StateOptions::default())
    }

    /// The PNG's colour type: the tenth byte of IHDR's data, which opens at 16.
    fn color_type(png: &[u8]) -> u8 {
        png[25]
    }

    #[test]
    fn an_rgb_picture_goes_back_as_it_came_and_any_other_as_the_rgb_mgba_reads() {
        let bundle = read_as(&pack_png(&screen(), &gba_data(), &[]).unwrap(), "game.ss1").unwrap();
        let rebuilt = write(&bundle, "game").unwrap();
        let (_, _, picture) = unpack_png(&rebuilt.bytes).unwrap();
        assert_eq!(picture.unwrap(), screen(), "eight-bit RGB is what mGBA reads, so it is kept");

        // mGBA reads only eight-bit RGB, so a picture with an alpha channel is encoded again.
        let pixels: Vec<u8> =
            (0..240 * 160u32).flat_map(|n| [n.to_le_bytes()[0], 0x20, 0x40, 0x80]).collect();
        let rgba = image::encode(240, 160, 4, &pixels).unwrap();
        let mut bundle = bundle;
        let mut part = crate::state::screenshot_part(rgba).unwrap();
        part.id = 1;
        bundle.parts[1] = part;
        let rebuilt = write(&bundle, "game").unwrap();
        assert_eq!(rebuilt.left_out, []);
        let picture = unpack_png(&rebuilt.bytes).unwrap().2.unwrap();
        assert_eq!(color_type(&picture), 2, "RGB");
        let (width, height, again) = image::decode_rgba(&picture, LARGEST_SCREEN).unwrap();
        assert_eq!(
            (width, height, image::rgb_of(&again)),
            (240, 160, image::rgb_of(&pixels)),
            "the colours kept"
        );
    }

    #[test]
    fn a_picture_mgba_cannot_read_back_is_left_out_and_the_state_written_bare() {
        let jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, b'J', b'F', b'I', b'F'];
        let broken = [PNG_SIGNATURE, b"not a picture"].concat();
        for picture in [jpeg, broken] {
            let mut bundle = read_as(&pack_bare(&gba_data(), &[]).unwrap(), "game.ss1").unwrap();
            bundle.parts.push(crate::state::screenshot_part(picture).unwrap());
            bundle.parts = crate::state::number(bundle.parts);
            let rebuilt = write(&bundle, "game").unwrap();
            assert_eq!(rebuilt.left_out, [1]);
            assert_eq!(rebuilt.bytes, gba_data(), "the bare container, with nothing after the data");
        }
    }

    #[test]
    fn a_framebuffer_takes_its_shape_from_the_item_mgba_keeps_for_it() {
        let screen = vec![0x33; 8 * 4 * 4];
        let dimensions = [8, 0, 4, 0].to_vec();
        let file =
            pack_bare(&gba_data(), &[(SCREENSHOT, screen.clone()), (SCREENSHOT_DIMENSIONS, dimensions)])
                .unwrap();
        let bundle = read_as(&file, "game.ss1").unwrap();
        let png = bundle.parts[1].bytes().unwrap();
        assert_eq!(image::decode_rgba(&png, LARGEST_SCREEN).unwrap().0, 8, "eight wide, as the item says");
        assert_eq!(bundle.parts.len(), 2, "the dimensions are how it was read, and nothing of their own");

        // With no item, or one the framebuffer's length belies, a size no screen of mGBA's has is
        // kept as mGBA wrote it.
        for items in [
            vec![(SCREENSHOT, screen.clone())],
            vec![(SCREENSHOT, screen), (SCREENSHOT_DIMENSIONS, vec![9, 0, 4, 0])],
        ] {
            let bundle = read_as(&pack_bare(&gba_data(), &items).unwrap(), "game.ss1").unwrap();
            assert_eq!(bundle.parts[1].kind, PartKind::Aux);
            assert_eq!(bundle.parts[1].path.as_deref(), Some("extdata/1"));
        }
    }

    #[test]
    fn items_the_reader_cannot_take_apart_are_kept_as_mgba_wrote_them() {
        let items = vec![
            (SAVEDATA, vec![1; 512]),
            (SAVEDATA, vec![2; 512]),
            (CHEATS, vec![0xFF, 0xFE]),
            (META_TIME, vec![0; 4]),
            (META_CREATOR, b"medusa 0.1\0".to_vec()),
        ];
        let file = pack_bare(&gba_data(), &items).unwrap();
        let bundle = read_as(&file, "game.ss1").unwrap();
        let paths: Vec<_> = bundle.parts[2..].iter().map(|part| part.path.as_deref().unwrap()).collect();
        assert_eq!(
            paths,
            ["extdata/2", "extdata/3", "extdata/257", "extdata/258"],
            "a second save, cheats that are not text, a time of another length, another creator"
        );
        assert_eq!(bundle.parts[1].kind, PartKind::Save, "the first save is the save");
        assert_eq!(bundle.header.taken_at, None);
        assert_eq!(bundle.header.source.as_ref().unwrap().app_version, None);

        // And they go back where they were, so the file is mGBA's own again.
        let rebuilt = write(&bundle, "game").unwrap();
        assert_eq!((rebuilt.bytes, rebuilt.left_out), (file, vec![]));
    }

    #[test]
    fn an_empty_cheats_section_is_no_cheats() {
        let bundle =
            read_as(&pack_bare(&gba_data(), &[(CHEATS, b"\n\n".to_vec())]).unwrap(), "game.ss1").unwrap();
        assert_eq!(bundle.parts.len(), 1, "nothing on the side");
        assert!(bundle.parts[0].extensions.is_empty(), "and no key on the state part");
    }

    #[test]
    fn what_mgba_has_no_item_for_is_left_out() {
        let mut bundle =
            read_as(&pack_bare(&gba_data(), &[(SAVEDATA, vec![1; 512])]).unwrap(), "game.ss1").unwrap();
        bundle.parts.push(Part::new(2, *b"SAVE").path("other.sav"));
        bundle.parts.push(Part::new(3, *b"MORE").kind(PartKind::Aux).path("notes"));
        bundle.validate().unwrap();
        let rebuilt = write(&bundle, "game").unwrap();
        assert_eq!(rebuilt.left_out, [2, 3], "a second save, and a part that names no item");
        let (_, items) = unpack_bare(&rebuilt.bytes).unwrap();
        assert_eq!(items, [(SAVEDATA, vec![1; 512])]);
    }

    #[test]
    fn a_file_that_does_not_unpack_is_corrupt() {
        let corrupt = |file: &[u8], what: &str| {
            let error = read_as(file, "game.ss1").unwrap_err();
            assert!(matches!(error, Error::Corrupt { .. }), "{what}: {error}");
        };
        let png = pack_png(&screen(), &gba_data(), &[(SAVEDATA, vec![1; 512])]).unwrap();
        corrupt(&png[..png.len() - 20], "a PNG cut short");
        corrupt(&screen(), "a picture with no core's data");

        let item = |tag: u32, size: u32, data: &[u8]| {
            let mut body = tag.to_le_bytes().to_vec();
            body.extend_from_slice(&size.to_le_bytes());
            body.extend_from_slice(&image::deflate_zlib(data));
            image::chunk(*b"gbAx", &body).unwrap()
        };
        let with = |chunks: &[Vec<u8>]| {
            let mut out = pack_png(&screen(), &gba_data(), &[]).unwrap();
            let end = out.len() - 12;
            out.splice(end..end, chunks.concat());
            out
        };
        corrupt(&with(&[item(2, 513, &[1; 512])]), "an item shorter than it says");
        // Refused on its claim, before anything is inflated; the bare container's test above has
        // the claims add up.
        let claims = u32::try_from(MAX_ITEMS + 1).unwrap();
        match read_as(&with(&[item(4, claims, &[])]), "game.ss1") {
            Err(Error::Corrupt { why, .. }) => assert!(why.contains("claim"), "{why}"),
            other => panic!("an item past the cap: {other:?}"),
        }

        let bare = pack_bare(&gba_data(), &[(SAVEDATA, vec![1; 512])]).unwrap();
        corrupt(&bare[..0x60000], "data cut short");
        corrupt(&bare[..0x61000 + 8], "a list of items cut short");
        let mut outside = bare.clone();
        outside[0x61000 + 8..0x61000 + 16].copy_from_slice(&i64::MAX.to_le_bytes());
        corrupt(&outside, "an item outside the file");
    }

    #[test]
    fn data_with_no_version_word_mgba_writes_is_not_mgbas() {
        let data = vec![0; 0x61000];
        let error = read_as(&pack_png(&screen(), &data, &[]).unwrap(), "game.ss1").unwrap_err();
        assert!(matches!(error, Error::NotThisFormat { .. }), "{error}");
        assert!(matches!(read_as(&data, "game.ss1"), Err(Error::NotThisFormat { .. })));
    }
}
