//! DuckStation's save states.
//!
//! A DuckStation state opens with a 216-byte header, every field of it a little-endian `u32` but
//! the two strings:
//!
//! | Offset | Field |
//! | ------ | ----- |
//! | 0 | `DUCC` |
//! | 4 | The version of the data, 42 the oldest DuckStation still reads |
//! | 8 | The game's title, 128 bytes, NUL-terminated |
//! | 136 | Its serial, 32 bytes, NUL-terminated |
//! | 168, 172 | The disc's path: its length and offset, the path with no NUL after it |
//! | 176 | Which disc of a playlist was in the drive |
//! | 180 | How the screenshot is compressed |
//! | 184, 188 | The screenshot's width and height |
//! | 192, 196 | Its compressed size and offset |
//! | 200 | How the data is compressed |
//! | 204, 208, 212 | The data's compressed size, its size and its offset |
//!
//! Compression is 0 for none, 1 for a zlib stream, 2 for Zstandard, which DuckStation writes by
//! default, and 3 for XZ. The screenshot is a bare RGBA framebuffer, compressed as the data is;
//! before version 69 it was never compressed, and the field at 192 was its size.
//!
//! In a bundle the version is `taken_with`'s `format_version`, since the core's data no longer
//! carries it and a state loads only at the version it was stamped with. The title, the serial and
//! the disc's file name go in `game`, the screenshot is encoded as a PNG, and the state part is the
//! data uncompressed. DuckStation always boots a BIOS and its file does not say which, so
//! `firmware` is null: unknown, which absence would not say.
//!
//! The slot is in the file's name: `SLUS-00594_3.sav` is slot 3, `SLUS-00594_resume.sav` the one
//! DuckStation takes on its own to resume from, and `SLUS-00594_3.bak` what slot 3 held before the
//! player saved over it, an `origin` of `undo`.
//!
//! A rebuilt file names no disc. The path in the file was the disc's on the machine that took the
//! state, and DuckStation loads a state naming none into the disc that is running, which on any
//! other machine is the one that will be.

use one_saves::{Bundle, Game, Origin, Recorded, Slug, TakenWith};

use crate::error::{Error, Result};
use crate::state::image::{self, le32};
use crate::state::{Contents, Found, Rebuilt, Slot, StateFile, StateOptions};

const FORMAT: &str = "DuckStation state";

/// `DUCC`, as the little-endian `u32` it is written as.
const MAGIC: u32 = 0x4343_5544;

/// The header's size, which has not changed in any version DuckStation reads.
const HEADER: usize = 216;

/// The oldest version DuckStation reads.
const OLDEST: u32 = 42;

/// The most DuckStation lets a state's data inflate to.
const MAX_DATA: usize = 32 << 20;

/// The largest screenshot this reads, on a side. DuckStation renders one into 256 by 256.
const MAX_SIDE: u32 = 4096;

/// Reads a DuckStation state file into a state bundle.
///
/// # Errors
///
/// [`Error::NotThisFormat`] for a file that is not a DuckStation state, or one older than
/// DuckStation reads; [`Error::Corrupt`] for one that does not unpack; [`Error::NotConvertible`]
/// for one compressed with XZ, which this build does not inflate.
pub fn read(file: &StateFile<'_>, options: &StateOptions) -> Result<Bundle> {
    let bytes = file.bytes;
    let field = |at: usize| le32(bytes, at, FORMAT);
    if bytes.len() < HEADER || field(0)? != MAGIC {
        return Err(Error::NotThisFormat { format: FORMAT, why: "it does not open with DUCC".into() });
    }
    let version = field(4)?;
    if version < OLDEST {
        return Err(Error::NotThisFormat {
            format: FORMAT,
            why: format!("version {version} is older than DuckStation reads"),
        });
    }

    let title = text(&bytes[8..136]);
    let serial = text(&bytes[136..168]);
    let disc = span(bytes, field(172)?, field(168)?)?
        .and_then(|path| std::str::from_utf8(path).ok())
        .and_then(|path| path.rsplit(['/', '\\']).next())
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned);

    let data = span(bytes, field(212)?, field(204)?)?.ok_or_else(|| corrupt("it holds no data"))?;
    let size = field(208)? as usize;
    if size > MAX_DATA {
        return Err(corrupt(&format!("its data claims {size} bytes, past DuckStation's cap")));
    }
    let data = inflate(field(200)?, data, size)?;
    if data.len() != size {
        return Err(corrupt("its data inflates to another length than the header says"));
    }

    let (width, height) = (field(184)?, field(188)?);
    let screenshot = if width == 0 || height == 0 || field(196)? == 0 {
        None
    } else {
        if width > MAX_SIDE || height > MAX_SIDE {
            return Err(corrupt(&format!("its screenshot claims {width} by {height} pixels")));
        }
        let pixels = width * height * 4;
        let size = pixels as usize;
        // Before 69 the screenshot was never compressed, and the field at 192 was its size.
        let (compression, stored) = if version < 69 { (0, pixels) } else { (field(180)?, field(192)?) };
        let stored = span(bytes, field(196)?, stored)?
            .ok_or_else(|| corrupt("its screenshot lies outside the file"))?;
        let rgba = inflate(compression, stored, size)?;
        if rgba.len() != size {
            return Err(corrupt("its screenshot is not as many pixels as it says"));
        }
        Some(png_of(width, height, &rgba)?)
    };

    let name = file.name.to_ascii_lowercase();
    let (origin, slot) = slot_of_name(&name);
    let mut game = Game::default();
    (game.title, game.serial, game.rom_filename) = (title, serial, disc);
    let found = Found {
        app: Some("duckstation"),
        core: Some("duckstation"),
        system: Slug::parse("psx").ok(),
        game: (!game.is_empty()).then_some(game),
        taken_with: Some(TakenWith::default().firmware(Recorded::Unknown).format_version(u64::from(version))),
        origin,
        ..Found::default()
    };

    let mut parts = vec![crate::state::state_part(data, slot)];
    if let Some(png) = screenshot {
        parts.push(crate::state::screenshot_part(png)?);
    }
    let bundle = Bundle::new(crate::state::header(options, found), crate::state::number(parts));
    bundle.validate()?;
    Ok(bundle)
}

/// Rebuilds a DuckStation state file from a state bundle.
///
/// The bundle has to record the version DuckStation stamped, since the data loads at that version
/// alone. The data is compressed with Zstandard, as DuckStation compresses it by default, in a build
/// with `zstd`, and as a zlib stream otherwise; the screenshot is left uncompressed.
///
/// # Errors
///
/// [`Error::NotConvertible`] for a bundle that is not a state, loads into another core, or records
/// no format version.
pub fn write(bundle: &Bundle, stem: &str) -> Result<Rebuilt> {
    let contents = Contents::of(bundle)?;
    crate::state::require_core(bundle, "duckstation", "DuckStation")?;
    let version = bundle
        .header
        .taken_with
        .as_ref()
        .and_then(|with| with.format_version.as_ref())
        .and_then(Recorded::known)
        .and_then(|version| u32::try_from(*version).ok())
        .ok_or_else(|| {
            Error::NotConvertible(
                "DuckStation loads a state only at the version it stamped, and this one records none".into(),
            )
        })?;

    let mut left_out: Vec<u64> = contents.saves.iter().chain(&contents.aux).map(|part| part.id).collect();
    // DuckStation keeps a bare framebuffer, so the picture has to decode. One that does not, or a
    // JPEG, stays in the bundle rather than costing the state its rebuild.
    let screenshot = match contents.screenshot {
        Some(part) => {
            let decoded = (part.content_type.as_deref() == Some("image/png"))
                .then(|| image::decode_rgba(&part.bytes().ok()?).ok())
                .flatten();
            if decoded.is_none() {
                left_out.push(part.id);
            }
            decoded
        }
        None => None,
    };

    let data = contents.state.bytes()?;
    let (compression, stored) = compress(&data, version);
    let mut out = vec![0u8; HEADER];
    let mut put = |at: usize, value: u32| out[at..at + 4].copy_from_slice(&value.to_le_bytes());
    put(0, MAGIC);
    put(4, version);
    let game = bundle.header.game.as_ref();
    let title = game.and_then(|game| game.title.as_deref()).unwrap_or_default();
    let serial = game.and_then(|game| game.serial.as_deref()).unwrap_or_default();
    let (width, height, rgba) = screenshot.unwrap_or_default();
    let shot_at = HEADER;
    let data_at = shot_at + rgba.len();
    put(184, width);
    put(188, height);
    put(192, u32::try_from(rgba.len()).expect("a screenshot fits"));
    put(196, if rgba.is_empty() { 0 } else { u32::try_from(shot_at).expect("an offset fits") });
    put(200, compression);
    put(204, u32::try_from(stored.len()).expect("the data fits"));
    put(208, u32::try_from(data.len()).expect("the data fits"));
    put(212, u32::try_from(data_at).expect("an offset fits"));
    out[8..136].copy_from_slice(&fixed::<128>(title));
    out[136..168].copy_from_slice(&fixed::<32>(serial));
    out.extend_from_slice(&rgba);
    out.extend_from_slice(&stored);

    let base = crate::state::component(if serial.is_empty() { stem } else { serial });
    let name = match (crate::state::slot_of(bundle, contents.state), &bundle.header.origin) {
        (Some(Slot::Auto), _) => format!("{base}_resume.sav"),
        (Some(Slot::Numbered(slot)), Some(Origin::Undo)) => format!("{base}_{slot}.bak"),
        (Some(Slot::Numbered(slot)), _) => format!("{base}_{slot}.sav"),
        (None, _) => format!("{base}_1.sav"),
    };
    left_out.sort_unstable();
    Ok(Rebuilt { name, bytes: out, screenshot: None, left_out })
}

fn corrupt(why: &str) -> Error {
    Error::Corrupt { format: FORMAT, why: why.into() }
}

/// The text in a fixed, NUL-terminated field, where there is any and it is UTF-8.
fn text(field: &[u8]) -> Option<String> {
    let end = field.iter().position(|&byte| byte == 0).unwrap_or(field.len());
    std::str::from_utf8(&field[..end]).ok().filter(|text| !text.is_empty()).map(ToOwned::to_owned)
}

/// A fixed field holding text, cut at a character so DuckStation's NUL still fits.
fn fixed<const N: usize>(text: &str) -> [u8; N] {
    let mut end = text.len().min(N - 1);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut field = [0; N];
    field[..end].copy_from_slice(&text.as_bytes()[..end]);
    field
}

/// The bytes a header's offset and length point at, `None` for an empty span.
fn span(bytes: &[u8], offset: u32, length: u32) -> Result<Option<&[u8]>> {
    if length == 0 {
        return Ok(None);
    }
    let (offset, length) = (offset as usize, length as usize);
    offset
        .checked_add(length)
        .and_then(|end| bytes.get(offset..end))
        .map(Some)
        .ok_or_else(|| corrupt("a field points past the end of the file"))
}

/// Inflates a block by DuckStation's compression type, to at most `limit` bytes.
fn inflate(compression: u32, stored: &[u8], limit: usize) -> Result<Vec<u8>> {
    match compression {
        0 => Ok(stored.to_vec()),
        1 => image::inflate_zlib(stored, limit, FORMAT),
        2 => image::inflate_zstd(stored, limit, FORMAT),
        3 => Err(Error::NotConvertible(
            "this DuckStation state is compressed with XZ, which this build does not inflate; save it \
             again with Zstandard or Deflate"
                .into(),
        )),
        other => Err(corrupt(&format!("compression type {other} is not one DuckStation writes"))),
    }
}

/// Compresses the data the way DuckStation would at this version, and says which way that was.
///
/// Compression types were numbered at version 56; an older state is written uncompressed, which
/// every version reads.
fn compress(data: &[u8], version: u32) -> (u32, Vec<u8>) {
    if version < 56 {
        return (0, data.to_vec());
    }
    #[cfg(feature = "zstd")]
    if let Ok(compressed) = zstd::stream::encode_all(data, 0) {
        return (2, compressed);
    }
    (1, image::deflate_zlib(data))
}

/// A framebuffer as a PNG: RGB where every pixel is opaque, which DuckStation's are, and RGBA
/// otherwise.
fn png_of(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    if rgba.as_chunks::<4>().0.iter().all(|&[_, _, _, alpha]| alpha == 0xFF) {
        image::encode(width, height, 3, &image::rgb_of(rgba))
    } else {
        image::encode(width, height, 4, rgba)
    }
}

/// The origin and slot a DuckStation file's name says.
fn slot_of_name(name: &str) -> (Option<Origin>, Option<u64>) {
    if name.ends_with("resume.sav") {
        return (Some(Origin::Auto), None);
    }
    let (stem, undo) = match (name.strip_suffix(".sav"), name.strip_suffix(".bak")) {
        (Some(stem), _) => (stem, false),
        (None, Some(stem)) => (stem, true),
        (None, None) => return (None, None),
    };
    let slot = stem.rsplit_once('_').and_then(|(_, slot)| slot.parse().ok());
    (if undo && slot.is_some() { Some(Origin::Undo) } else { None }, slot)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle_at(version: u32) -> Bundle {
        let mut file = vec![0u8; HEADER];
        let put = |file: &mut Vec<u8>, at: usize, value: u32| {
            file[at..at + 4].copy_from_slice(&value.to_le_bytes());
        };
        put(&mut file, 0, MAGIC);
        put(&mut file, 4, version);
        file[8..8 + 12].copy_from_slice(b"Spider-Man\0\0");
        file[136..146].copy_from_slice(b"SLUS-00875");
        let path = b"/Users/someone/PSX/Spider-Man (USA).chd";
        let screen: Vec<u8> = (0..16 * 8u8).flat_map(|n| [n, 0x40, 0x80, 0xFF]).collect();
        let data: Vec<u8> = (0..70_000u32).map(|n| (n % 253) as u8).collect();
        let deflated = image::deflate_zlib(&data);
        let (path_at, shot_at) = (HEADER, HEADER + path.len());
        let data_at = shot_at + screen.len();
        for (at, value) in [
            (168, path.len()),
            (172, path_at),
            (184, 16),
            (188, 8),
            (192, screen.len()),
            (196, shot_at),
            (200, 1),
            (204, deflated.len()),
            (208, data.len()),
            (212, data_at),
        ] {
            put(&mut file, at, u32::try_from(value).unwrap());
        }
        file.extend_from_slice(path);
        file.extend_from_slice(&screen);
        file.extend_from_slice(&deflated);
        read(
            &StateFile { bytes: &file, name: "SLUS-00875_3.sav", screenshot: None },
            &StateOptions::default(),
        )
        .unwrap()
    }

    #[test]
    fn a_state_lifts_off_its_header() {
        let bundle = bundle_at(72);
        let game = bundle.header.game.as_ref().unwrap();
        assert_eq!(game.title.as_deref(), Some("Spider-Man"));
        assert_eq!(game.serial.as_deref(), Some("SLUS-00875"));
        assert_eq!(game.rom_filename.as_deref(), Some("Spider-Man (USA).chd"), "the disc, not where it sat");
        let with = bundle.header.taken_with.as_ref().unwrap();
        assert_eq!(
            (&with.format_version, &with.firmware),
            (&Some(Recorded::Known(72)), &Some(Recorded::Unknown))
        );
        assert_eq!(bundle.parts[0].slot, Some(3));
        assert_eq!(bundle.parts[0].bytes().unwrap().len(), 70_000, "the data, inflated");
        assert_eq!(bundle.parts[1].content_type.as_deref(), Some("image/png"));
    }

    #[test]
    fn a_rebuilt_state_reads_back_as_the_same_bundle() {
        let bundle = bundle_at(72);
        let rebuilt = write(&bundle, "ignored").unwrap();
        assert_eq!(rebuilt.name, "SLUS-00875_3.sav", "named for the serial, as DuckStation names it");
        let again = read(
            &StateFile { bytes: &rebuilt.bytes, name: &rebuilt.name, screenshot: None },
            &StateOptions::default(),
        )
        .unwrap();
        assert_eq!(again.parts, bundle.parts);
        // The disc's path is not written back, so its name is all that is gone.
        assert_eq!(again.header.game.as_ref().unwrap().rom_filename, None);
    }

    #[test]
    fn a_serial_never_names_a_directory() {
        let mut bundle = bundle_at(72);
        bundle.header.game.as_mut().unwrap().serial = Some("../../SLUS-00875".into());
        assert_eq!(write(&bundle, "ignored").unwrap().name, ".._.._SLUS-00875_3.sav");
    }

    #[test]
    fn a_name_says_the_slot_and_whether_the_emulator_kept_it() {
        assert_eq!(slot_of_name("slus-00875_3.sav"), (None, Some(3)));
        assert_eq!(slot_of_name("slus-00875_resume.sav"), (Some(Origin::Auto), None));
        assert_eq!(slot_of_name("resume.sav"), (Some(Origin::Auto), None));
        assert_eq!(slot_of_name("slus-00875_3.bak"), (Some(Origin::Undo), Some(3)));
        assert_eq!(slot_of_name("savestate_10.sav"), (None, Some(10)));
    }

    #[test]
    fn a_version_older_than_duckstation_reads_is_refused() {
        let mut file = vec![0u8; HEADER];
        file[..4].copy_from_slice(&MAGIC.to_le_bytes());
        file[4..8].copy_from_slice(&41u32.to_le_bytes());
        let state = StateFile { bytes: &file, name: "x_1.sav", screenshot: None };
        assert!(matches!(read(&state, &StateOptions::default()), Err(Error::NotThisFormat { .. })));
    }
}
