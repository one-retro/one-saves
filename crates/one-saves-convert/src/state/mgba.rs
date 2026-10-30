//! mGBA's save states.
//!
//! mGBA writes a state in one of two containers, and the core's data is the same in both: a fixed
//! size, `0x61000` bytes for the GBA and `0x11800` for the Game Boy, opening with a version word
//! (`0x01000000` and `0x00400000` plus the version).
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
//! | 2 | The game's save memory | a save part: the in-game save as it stood when the state was taken |
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
use one_saves::{Bundle, Part, PartKind, Slug};

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

/// The `path` prefix an item mGBA keeps for itself takes as an `aux` part.
const ITEM_PATH: &str = "extdata/";

/// mGBA's items of extra data, each with its tag, in the order the file holds them.
type Items = Vec<(u32, Vec<u8>)>;

/// Which machine a state is for, from the version word it opens with, and the data's size.
fn machine(state: &[u8]) -> Option<(&'static str, usize)> {
    let word = u32::from_le_bytes(state.get(..4)?.try_into().ok()?);
    match word & 0xFFFF_FF00 {
        0x0100_0000 => Some(("gba", 0x61000)),
        // A Game Boy core runs the Game Boy and the Game Boy Color both, so the data does not say
        // which game it is; the system is the caller's to give.
        0x0040_0000 => Some(("", 0x11800)),
        _ => None,
    }
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
    let (system, _) = machine(&data).ok_or_else(|| Error::NotThisFormat {
        format: FORMAT,
        why: "its data opens with no version word mGBA writes".into(),
    })?;
    let (origin, slot) =
        crate::state::origin_and_slot(crate::state::mgba_slot(&file.name.to_ascii_lowercase()));

    let mut found = Found {
        app: Some("mgba"),
        core: Some("mgba"),
        system: (!system.is_empty()).then(|| Slug::parse(system).expect("a system slug")),
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
                rest.push(Part::new(0, item));
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
/// encoded again as one, which loses nothing it could show.
///
/// # Errors
///
/// [`Error::NotConvertible`] for a bundle that is not a state, or loads into another core.
pub fn write(bundle: &Bundle, stem: &str) -> Result<Rebuilt> {
    let contents = Contents::of(bundle)?;
    crate::state::require_core(bundle, "mgba", "mGBA")?;

    let mut left_out = Vec::new();
    let mut items: Vec<(u32, Vec<u8>)> = Vec::new();
    for (index, save) in contents.saves.iter().enumerate() {
        if index == 0 {
            items.push((SAVEDATA, save.bytes()?.into_owned()));
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
            Some(tag) => items.push((tag, part.bytes()?.into_owned())),
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

    let data = contents.state.bytes()?;
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
        None => pack_bare(&data, &items),
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
    let (_, size) = machine(bytes).ok_or_else(|| Error::NotThisFormat {
        format: FORMAT,
        why: "it opens with no version word mGBA writes".into(),
    })?;
    let data = bytes
        .get(..size)
        .ok_or_else(|| corrupt(format!("it ends before the {size} bytes of the core's data")))?;

    let mut items = Vec::new();
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
        let length = i32::from_le_bytes(header[4..8].try_into().expect("four bytes"));
        let offset = i64::from_le_bytes(header[8..16].try_into().expect("eight bytes"));
        let item = usize::try_from(offset)
            .ok()
            .zip(usize::try_from(length).ok())
            .and_then(|(offset, length)| bytes.get(offset..offset.checked_add(length)?))
            .ok_or_else(|| corrupt(format!("item {tag} lies outside the file")))?;
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
    out.extend_from_slice(&image::chunk(*b"gbAs", &image::deflate_zlib(data)));
    for (tag, item) in items {
        let mut body = tag.to_le_bytes().to_vec();
        body.extend_from_slice(&u32::try_from(item.len()).expect("an item fits").to_le_bytes());
        body.extend_from_slice(&image::deflate_zlib(item));
        out.extend_from_slice(&image::chunk(*b"gbAx", &body));
    }
    out.extend_from_slice(&image::chunk(*b"IEND", &[]));
    Ok(out)
}

/// Packs the bare container: the data, the list of items and its end, then the items.
fn pack_bare(data: &[u8], items: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = data.to_vec();
    if items.is_empty() {
        return out;
    }
    let mut offset = data.len() + (items.len() + 1) * 16;
    for (tag, item) in items {
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&i32::try_from(item.len()).expect("an item fits").to_le_bytes());
        out.extend_from_slice(&i64::try_from(offset).expect("an offset fits").to_le_bytes());
        offset += item.len();
    }
    out.extend_from_slice(&[0; 16]);
    for (_, item) in items {
        out.extend_from_slice(item);
    }
    out
}

/// A PNG in the one layout mGBA reads back: eight-bit RGB, not interlaced.
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
    let (width, height, rgba) = image::decode_rgba(png)?;
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
    fn a_bare_state_reads_its_items_by_offset() {
        let file =
            pack_bare(&gba_data(), &[(SCREENSHOT, vec![0x80; 240 * 160 * 4]), (SAVEDATA, vec![1; 512])]);
        let state = StateFile { bytes: &file, name: "game.ss0", screenshot: None };
        let bundle = read(&state, &StateOptions::default()).unwrap();
        assert_eq!(bundle.header.origin, Some(Origin::Auto), "slot 0 is the one mGBA resumes from");
        assert_eq!(bundle.parts[1].content_type.as_deref(), Some("image/png"), "the framebuffer, made a PNG");
        assert_eq!(bundle.parts[2].bytes().unwrap().as_ref(), &[1; 512]);
        assert_eq!(unpack_bare(&pack_bare(&gba_data(), &[])).unwrap().1, []);
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
        let file = pack_bare(&gba_data(), &[]);
        let mut bundle =
            read(&StateFile { bytes: &file, name: "game.ss1", screenshot: None }, &options).unwrap();
        bundle.header.source = options.source.map(|source| source.app(None));
        assert!(matches!(write(&bundle, "game"), Err(Error::NotConvertible(_))));
    }
}
