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
/// [`Error::NotConvertible`] for a bundle that is not a state, loads into another core, records no
/// format version, or holds more data than DuckStation loads.
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
    // DuckStation keeps a bare framebuffer, so the picture has to decode, and no larger than the
    // reader takes. One that does not, or a JPEG, stays in the bundle rather than costing the state
    // its rebuild.
    let screenshot = match contents.screenshot {
        Some(part) => {
            let decoded = (part.content_type.as_deref() == Some("image/png"))
                .then(|| image::decode_rgba(&part.bytes().ok()?, (MAX_SIDE, MAX_SIDE)).ok())
                .flatten();
            if decoded.is_none() {
                left_out.push(part.id);
            }
            decoded
        }
        None => None,
    };

    // Held to DuckStation's cap before it is inflated, which also keeps every length and offset
    // below in the `u32` DuckStation gives it.
    let data = crate::state::bytes_within(contents.state, MAX_DATA, "a DuckStation state")?;
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
    let fits = |value: usize| {
        u32::try_from(value)
            .map_err(|_| Error::NotConvertible(format!("{value} does not fit a DuckStation header")))
    };
    put(184, width);
    put(188, height);
    put(192, fits(rgba.len())?);
    put(196, if rgba.is_empty() { 0 } else { fits(shot_at)? });
    put(200, compression);
    put(204, fits(stored.len())?);
    put(208, fits(data.len())?);
    put(212, fits(data_at)?);
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
    use one_saves::{Name, Part, PartKind, Source};

    use super::*;

    /// The core's data every test state holds, long enough that compressing it shows.
    fn data() -> Vec<u8> {
        (0..70_000u32).map(|n| (n % 253) as u8).collect()
    }

    /// A 16 by 8 screen, every pixel opaque.
    fn screen() -> Vec<u8> {
        (0..16 * 8u8).flat_map(|n| [n, 0x40, 0x80, 0xFF]).collect()
    }

    /// Bytes as DuckStation stores them under a compression type. Anything but zlib and Zstandard
    /// is stored as it is, which is all a test of a type the reader refuses needs.
    fn stored(compression: u32, bytes: &[u8]) -> Vec<u8> {
        match compression {
            1 => image::deflate_zlib(bytes),
            #[cfg(feature = "zstd")]
            2 => zstd::stream::encode_all(bytes, 0).unwrap(),
            _ => bytes.to_vec(),
        }
    }

    /// Sets a header field.
    fn put(file: &mut [u8], at: usize, value: usize) {
        file[at..at + 4].copy_from_slice(&u32::try_from(value).unwrap().to_le_bytes());
    }

    /// A header field.
    fn field(file: &[u8], at: usize) -> u32 {
        le32(file, at, FORMAT).unwrap()
    }

    /// A file laid out as DuckStation writes one at `version`: the disc's path, the 16 by 8
    /// `screen` stored by `shot`, then the data stored by `compression`.
    fn file_of(version: u32, compression: u32, shot: u32, screen: &[u8]) -> Vec<u8> {
        let mut file = vec![0u8; HEADER];
        file[..4].copy_from_slice(&MAGIC.to_le_bytes());
        file[4..8].copy_from_slice(&version.to_le_bytes());
        file[8..8 + 12].copy_from_slice(b"Spider-Man\0\0");
        file[136..146].copy_from_slice(b"SLUS-00875");
        let path = b"/Users/someone/PSX/Spider-Man (USA).chd";
        let (screen, kept) = (stored(shot, screen), stored(compression, &data()));
        let (path_at, shot_at) = (HEADER, HEADER + path.len());
        let data_at = shot_at + screen.len();
        let pictured = !screen.is_empty();
        for (at, value) in [
            (168, path.len()),
            (172, path_at),
            (180, shot as usize),
            (184, if pictured { 16 } else { 0 }),
            (188, if pictured { 8 } else { 0 }),
            (192, screen.len()),
            (196, if pictured { shot_at } else { 0 }),
            (200, compression as usize),
            (204, kept.len()),
            (208, data().len()),
            (212, data_at),
        ] {
            put(&mut file, at, value);
        }
        file.extend_from_slice(path);
        file.extend_from_slice(&screen);
        file.extend_from_slice(&kept);
        file
    }

    fn read_as(file: &[u8], name: &str) -> Result<Bundle> {
        read(&StateFile { bytes: file, name, screenshot: None }, &StateOptions::default())
    }

    /// A state at `version`, its data a zlib stream and its screen stored as it is.
    fn bundle_at(version: u32) -> Bundle {
        read_as(&file_of(version, 1, 0, &screen()), "SLUS-00875_3.sav").unwrap()
    }

    /// The pixels of a bundle's screenshot, and its PNG's colour type.
    fn picture(bundle: &Bundle) -> (u32, u32, Vec<u8>, u8) {
        let part = bundle.parts.iter().find(|part| part.kind == PartKind::Screenshot).expect("a screenshot");
        let png = part.bytes().unwrap();
        let (width, height, rgba) = image::decode_rgba(&png, (MAX_SIDE, MAX_SIDE)).unwrap();
        // The colour type is the tenth byte of IHDR's data, which opens at 16.
        (width, height, rgba, png[25])
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
    fn the_data_and_the_screen_come_out_whole_however_they_were_stored() {
        // None, a zlib stream, and Zstandard, which DuckStation writes by default. Since 69 it
        // compresses the screenshot the same way as the data.
        let mut types = vec![0, 1];
        if cfg!(feature = "zstd") {
            types.push(2);
        }
        for compression in types {
            let bundle = read_as(&file_of(93, compression, compression, &screen()), "SLUS-00875_1.sav")
                .unwrap_or_else(|e| panic!("compression {compression}: {e}"));
            assert_eq!(
                bundle.parts[0].bytes().unwrap().as_ref(),
                data().as_slice(),
                "compression {compression}"
            );
            assert_eq!(picture(&bundle), (16, 8, screen(), 2), "compression {compression}: opaque, so RGB");
        }
    }

    #[cfg(not(feature = "zstd"))]
    #[test]
    fn zstandard_is_named_and_refused_in_a_build_without_it() {
        let error = read_as(&file_of(93, 2, 0, &screen()), "SLUS-00875_1.sav").unwrap_err();
        assert!(matches!(error, Error::Unsupported { feature: "zstd", .. }), "{error}");
    }

    #[test]
    fn xz_is_refused_in_words_and_a_type_duckstation_never_wrote_as_corrupt() {
        let error = read_as(&file_of(93, 3, 0, &screen()), "SLUS-00875_1.sav").unwrap_err();
        assert!(matches!(error, Error::NotConvertible(_)), "XZ: {error}");
        let error = read_as(&file_of(93, 4, 0, &screen()), "SLUS-00875_1.sav").unwrap_err();
        assert!(matches!(error, Error::Corrupt { .. }), "type 4: {error}");
        let error = read_as(&file_of(93, 1, 4, &screen()), "SLUS-00875_1.sav").unwrap_err();
        assert!(matches!(error, Error::Corrupt { .. }), "a screen of type 4: {error}");
    }

    #[test]
    fn before_69_the_screen_is_raw_whatever_the_field_beside_it_holds() {
        // Up to 50 the field at 180 is where a playlist's file name sits, and from 51 to 68 it is
        // unused; at 69 it became the screenshot's compression type. A state that old has its
        // data uncompressed, since compression types were only numbered at 56.
        let mut file = file_of(50, 0, 0, &screen());
        put(&mut file, 176, 12);
        put(&mut file, 180, 0x4000);
        let bundle = read_as(&file, "SLUS-00875_1.sav").unwrap();
        assert_eq!(picture(&bundle), (16, 8, screen(), 2));

        // The same bytes stamped 69 read the offset as a compression type, and fail on it.
        file[4..8].copy_from_slice(&69u32.to_le_bytes());
        assert!(matches!(read_as(&file, "SLUS-00875_1.sav"), Err(Error::Corrupt { .. })));
    }

    #[test]
    fn a_screen_with_transparency_stays_rgba() {
        let mut screen = screen();
        screen[3] = 0x80;
        let bundle = read_as(&file_of(93, 1, 1, &screen), "SLUS-00875_1.sav").unwrap();
        assert_eq!(picture(&bundle), (16, 8, screen.clone(), 6), "RGBA, the alpha kept");

        // And it goes back as it came, alpha and all.
        let rebuilt = write(&bundle, "ignored").unwrap();
        let again = read_as(&rebuilt.bytes, &rebuilt.name).unwrap();
        assert_eq!(picture(&again), (16, 8, screen, 6));
    }

    #[test]
    fn a_state_with_no_screen_has_no_screenshot_and_writes_none() {
        let bundle = read_as(&file_of(93, 1, 0, &[]), "SLUS-00875_1.sav").unwrap();
        assert_eq!(bundle.parts.len(), 1, "the state part alone");
        let rebuilt = write(&bundle, "ignored").unwrap();
        assert_eq!([184, 188, 192, 196].map(|at| field(&rebuilt.bytes, at)), [0; 4]);
    }

    #[test]
    fn a_rebuilt_state_reads_back_as_the_same_bundle() {
        let bundle = bundle_at(72);
        let rebuilt = write(&bundle, "ignored").unwrap();
        assert_eq!(rebuilt.name, "SLUS-00875_3.sav", "named for the serial, as DuckStation names it");
        assert_eq!(field(&rebuilt.bytes, 4), 72, "at the version it was stamped with");
        assert_eq!(rebuilt.left_out, [], "DuckStation has a place for all of it");
        let again = read_as(&rebuilt.bytes, &rebuilt.name).unwrap();
        assert_eq!(again.parts, bundle.parts);
        // The disc's path is not written back, so its name is all that is gone.
        assert_eq!(again.header.game.as_ref().unwrap().rom_filename, None);
        assert_eq!([168, 172].map(|at| field(&rebuilt.bytes, at)), [0, 0]);
    }

    #[test]
    fn the_data_is_compressed_as_duckstation_would_at_its_version() {
        // Zstandard where the build has it, as DuckStation writes by default, and zlib where not.
        let rebuilt = write(&bundle_at(72), "ignored").unwrap();
        assert_eq!(field(&rebuilt.bytes, 200), if cfg!(feature = "zstd") { 2 } else { 1 });

        // Before 56 the types had no numbers, so the data goes uncompressed, which every version
        // reads.
        let old = read_as(&file_of(50, 0, 0, &screen()), "SLUS-00875_1.sav").unwrap();
        let rebuilt = write(&old, "ignored").unwrap();
        assert_eq!(field(&rebuilt.bytes, 200), 0);
        assert_eq!(field(&rebuilt.bytes, 204), field(&rebuilt.bytes, 208), "stored at its size");
        assert_eq!(read_as(&rebuilt.bytes, &rebuilt.name).unwrap().parts, old.parts);
    }

    #[test]
    fn a_state_that_records_no_version_is_not_rebuilt() {
        // DuckStation loads a state only at the version it was stamped with, and nothing else in
        // the file says which that was.
        for version in [None, Some(Recorded::Unknown), Some(Recorded::Known(1 << 32))] {
            let mut bundle = bundle_at(72);
            bundle.header.taken_with.as_mut().unwrap().format_version = version.clone();
            assert!(matches!(write(&bundle, "ignored"), Err(Error::NotConvertible(_))), "{version:?}");
        }
    }

    #[test]
    fn a_state_for_another_core_is_not_rebuilt_as_duckstations() {
        let mut bundle = bundle_at(72);
        bundle.header.source = Some(Source::default().core(Name::parse("pcsx-rearmed").unwrap()));
        assert!(matches!(write(&bundle, "ignored"), Err(Error::NotConvertible(_))));
    }

    #[test]
    fn a_rebuilt_file_is_named_for_its_slot_and_who_took_it() {
        let named = |name: &str| {
            write(&read_as(&file_of(72, 1, 0, &screen()), name).unwrap(), "ignored").unwrap().name
        };
        assert_eq!(named("SLUS-00875_resume.sav"), "SLUS-00875_resume.sav", "the one it resumes from");
        assert_eq!(named("SLUS-00875_3.bak"), "SLUS-00875_3.bak", "what slot 3 held before");
        // A state DuckStation saved for no game in particular comes back as the game's own.
        assert_eq!(named("resume.sav"), "SLUS-00875_resume.sav");
        assert_eq!(named("savestate_10.sav"), "SLUS-00875_10.sav");

        // With no slot, the first; with no serial, the stem.
        let mut bundle = bundle_at(72);
        bundle.parts[0].slot = None;
        assert_eq!(write(&bundle, "ignored").unwrap().name, "SLUS-00875_1.sav");
        bundle.header.game.as_mut().unwrap().serial = None;
        assert_eq!(write(&bundle, "Spider-Man (USA)").unwrap().name, "Spider-Man (USA)_1.sav");
    }

    #[test]
    fn a_serial_never_names_a_directory() {
        let mut bundle = bundle_at(72);
        bundle.header.game.as_mut().unwrap().serial = Some("../../SLUS-00875".into());
        assert_eq!(write(&bundle, "ignored").unwrap().name, ".._.._SLUS-00875_3.sav");
    }

    #[test]
    fn a_title_too_long_for_its_field_is_cut_at_a_character() {
        let mut bundle = bundle_at(72);
        // 126 bytes, then a character of two, which would leave no room for the NUL.
        bundle.header.game.as_mut().unwrap().title = Some(format!("{}é", "a".repeat(126)));
        let rebuilt = write(&bundle, "ignored").unwrap();
        let title = read_as(&rebuilt.bytes, &rebuilt.name).unwrap().header.game.unwrap().title.unwrap();
        assert_eq!(title, "a".repeat(126));
    }

    #[test]
    fn what_duckstation_has_no_place_for_is_left_out() {
        let jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, b'J', b'F', b'I', b'F'];
        let broken = [crate::state::PNG_SIGNATURE, b"not a picture"].concat();
        for screenshot in [jpeg, broken] {
            let mut bundle = bundle_at(72);
            let mut picture = crate::state::screenshot_part(screenshot).unwrap();
            picture.id = 1;
            bundle.parts[1] = picture;
            bundle.parts.push(Part::new(2, *b"SAVE"));
            bundle.parts.push(Part::new(3, *b"MORE").kind(PartKind::Aux).path("notes"));
            bundle.validate().unwrap();

            let rebuilt = write(&bundle, "ignored").unwrap();
            assert_eq!(rebuilt.left_out, [1, 2, 3], "a picture it cannot decode, a save and the rest");
            let again = read_as(&rebuilt.bytes, &rebuilt.name).unwrap();
            assert_eq!(again.parts.len(), 1, "the state loads without them");
        }
    }

    #[test]
    fn a_picture_past_what_the_reader_takes_is_left_out_without_being_decoded() {
        // A few dozen bytes claiming a petabyte of pixels, which decoding would have allocated
        // for, and one a pixel wider than the reader takes back.
        let wide = image::encode(MAX_SIDE + 1, 1, 4, &vec![0xFF; 4 * (MAX_SIDE as usize + 1)]).unwrap();
        for picture in [image::claiming(1 << 24, 1 << 24), wide] {
            let mut bundle = bundle_at(72);
            let mut part = crate::state::screenshot_part(picture).unwrap();
            part.id = 1;
            bundle.parts[1] = part;
            let rebuilt = write(&bundle, "ignored").unwrap();
            assert_eq!(rebuilt.left_out, [1]);
            assert_eq!(read_as(&rebuilt.bytes, &rebuilt.name).unwrap().parts.len(), 1, "no screen");
        }
    }

    #[test]
    fn data_claiming_more_than_duckstation_loads_is_refused_before_it_is_inflated() {
        // Past 4 GiB this once panicked on the header's `u32`, after it had been inflated.
        for size in [MAX_DATA as u64 + 1, (1 << 32) + 16] {
            let mut bundle = bundle_at(72);
            bundle.parts[0] = crate::state::part_claiming(0, size).kind(PartKind::State);
            let error = write(&bundle, "ignored").unwrap_err();
            assert!(matches!(error, Error::NotConvertible(_)), "{size}: {error}");
        }
    }

    #[test]
    fn a_header_that_points_anywhere_but_its_own_bytes_is_corrupt() {
        let corrupt = |file: &[u8]| matches!(read_as(file, "SLUS-00875_1.sav"), Err(Error::Corrupt { .. }));
        let file = file_of(72, 1, 0, &screen());
        let altered = |at: usize, value: usize| {
            let mut file = file.clone();
            put(&mut file, at, value);
            file
        };
        assert!(corrupt(&altered(204, 0)), "no data");
        assert!(corrupt(&altered(212, file.len())), "data past the end");
        assert!(corrupt(&altered(208, data().len() + 1)), "data shorter than it says");
        assert!(corrupt(&altered(208, MAX_DATA + 1)), "data past DuckStation's cap");
        assert!(corrupt(&altered(184, 5000)), "a screen wider than any");
        assert!(corrupt(&altered(188, 9)), "a screen of fewer pixels than it says");
        assert!(corrupt(&altered(196, file.len())), "a screen past the end");
        assert!(corrupt(&altered(172, usize::try_from(u32::MAX).unwrap())), "a path past the end");
        assert!(corrupt(&file[..HEADER + 10]), "a file cut short");
    }

    #[test]
    fn a_name_says_the_slot_and_whether_the_emulator_kept_it() {
        assert_eq!(slot_of_name("slus-00875_3.sav"), (None, Some(3)));
        assert_eq!(slot_of_name("slus-00875_resume.sav"), (Some(Origin::Auto), None));
        assert_eq!(slot_of_name("resume.sav"), (Some(Origin::Auto), None));
        assert_eq!(slot_of_name("slus-00875_3.bak"), (Some(Origin::Undo), Some(3)));
        assert_eq!(slot_of_name("savestate_10.sav"), (None, Some(10)));
        assert_eq!(slot_of_name("slus-00875.bak"), (None, None), "a backup of no slot is no undo");
        assert_eq!(slot_of_name("slus-00875_3.png"), (None, None));
    }

    #[test]
    fn a_version_older_than_duckstation_reads_is_refused() {
        let mut file = vec![0u8; HEADER];
        file[..4].copy_from_slice(&MAGIC.to_le_bytes());
        file[4..8].copy_from_slice(&41u32.to_le_bytes());
        let state = StateFile { bytes: &file, name: "x_1.sav", screenshot: None };
        assert!(matches!(read(&state, &StateOptions::default()), Err(Error::NotThisFormat { .. })));
        assert!(
            matches!(read_as(b"DUCC", "x_1.sav"), Err(Error::NotThisFormat { .. })),
            "nor a header cut short"
        );
    }
}
