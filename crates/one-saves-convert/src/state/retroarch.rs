//! RetroArch's save states.
//!
//! RetroArch hands a core a buffer and writes back whatever the core serialized into it. Since
//! 1.9.1 it wraps that buffer in blocks of its own, and since then it has usually compressed the
//! result, so a file is up to three layers deep:
//!
//! 1. **RZIP**, when `savestate_file_compression` is on, which is the default. Twenty bytes of
//!    header — `#RZIPv`, a version byte, `#`, the chunk size as a little-endian `u32` and the total
//!    size as a `u64` — then chunks, each a `u32` length and a zlib stream of one chunk's worth.
//!    Version 2, not yet in a release, holds Zstandard frames instead.
//! 2. **`RASTATE`**, then the version byte 1, then blocks: a four-character tag, a little-endian
//!    `u32` length, and the payload padded with zeros to eight bytes. `MEM ` is the core's data at
//!    its exact size, `ACHV` the achievement runtime's, `RPLY` a replay being recorded, and `END `
//!    closes the list. RetroArch skips a tag it does not know.
//! 3. **The core's data.** A file from before 1.9.1 is this and nothing else, and RetroArch still
//!    loads one.
//!
//! The thumbnail is a PNG beside the state, named for it with `.png` after: `X.state3.png`.
//!
//! In a bundle, `MEM ` is the state part, decompressed and unwrapped, the thumbnail is the
//! screenshot, and every other block is an `aux` part whose `path` is its tag, trailing space
//! trimmed. The slot is in the file's name: `X.state` is slot 0, `X.state3` slot 3, and
//! `X.state.auto`, which RetroArch takes on its own to resume from, has no slot and an `origin` of
//! `auto`.
//!
//! What the file never says is which core wrote it. RetroArch runs whichever the player loaded,
//! and only that core loads the state, so a reader takes the core from
//! [`StateOptions::source`](crate::state::StateOptions::source) and refuses to guess.

use std::borrow::Cow;
use std::ops::Range;

use one_saves::{Bundle, Part, PartKind};

use crate::error::{Error, Result};
use crate::state::image::{self, le32};
use crate::state::{Contents, Found, Rebuilt, Slot, StateFile, StateOptions};

const FORMAT: &str = "RetroArch state";

/// The chunk size RetroArch writes, and so the one this writes.
const RZIP_CHUNK: usize = 128 * 1024;

/// The largest chunk RetroArch reads.
const MAX_CHUNK: usize = 64 << 20;

/// The most a state may inflate to, its blocks and RetroArch's wrapping around them included.
///
/// A core's state is the emulated machine's memory and the core's own bookkeeping. A Wii, which
/// Dolphin's core takes states of, has 88 MiB of RAM, and a PS2 has 32 MiB; this clears the
/// largest with room to spare. A header claiming more is a file asking to be handed memory, not a
/// state, and every byte a file inflates to is held more than once on its way into a bundle, so the
/// cap is what bounds that too.
const MAX_STATE: usize = 256 << 20;

/// The most blocks a file may hold besides the core's data.
///
/// RetroArch writes two of its own, the achievement runtime's and a replay's, and skips a tag it
/// does not know, which leaves a later version room for more. Each block becomes a part of its
/// own, which costs a hundred times the eight bytes an empty one takes in the file, and RZIP
/// compresses a run of empty ones a thousandfold: uncapped, a file of nothing else costs tens of
/// thousands of times its size.
const MAX_BLOCKS: usize = 64;

/// What pads a block's data out to eight bytes.
const PADDING: [u8; 8] = [0; 8];

/// Reads a RetroArch state file, and its thumbnail when there is one, into a state bundle.
///
/// # Errors
///
/// [`Error::NotConvertible`] when the options name no core, since the file does not; otherwise
/// [`Error::Corrupt`] for a file that does not unwrap.
pub fn read(file: &StateFile<'_>, options: &StateOptions) -> Result<Bundle> {
    if options.source.as_ref().and_then(|source| source.core.as_ref()).is_none() {
        return Err(Error::NotConvertible(
            "a RetroArch state loads only into the core that took it, and the file does not say \
             which core that was; name it"
                .into(),
        ));
    }

    let plain = unzip(file.bytes)?;
    let (memory, blocks) = unwrap(&plain)?;
    let (origin, slot) =
        crate::state::origin_and_slot(crate::state::retroarch_slot(&file.name.to_ascii_lowercase()));

    let aux: Vec<Part> = blocks
        .into_iter()
        .map(|(tag, at)| {
            Part::new(0, plain[at].to_vec())
                .kind(PartKind::Aux)
                .path(tag_path(tag))
                .content_type("application/octet-stream")
        })
        .collect();
    // The core's data is most of the file, so where the file had to be inflated it is cut out of
    // what was inflated rather than copied out of it.
    let data = match plain {
        Cow::Owned(mut plain) => {
            plain.copy_within(memory.clone(), 0);
            plain.truncate(memory.len());
            plain
        }
        Cow::Borrowed(plain) => plain[memory].to_vec(),
    };

    let mut parts = vec![crate::state::state_part(data, slot)];
    if let Some(screenshot) = file.screenshot {
        parts.push(crate::state::screenshot_part(screenshot.to_vec())?);
    }
    parts.extend(aux);

    let found = Found { app: Some("retroarch"), origin, ..Found::default() };
    let bundle = Bundle::new(crate::state::header(options, found), crate::state::number(parts));
    bundle.validate()?;
    Ok(bundle)
}

/// Rebuilds a RetroArch state file from a state bundle: `RASTATE` blocks, RZIP-compressed as
/// RetroArch writes them by default, and the thumbnail beside it where the screenshot is a PNG.
///
/// RetroArch loads the state into whichever core the player has loaded, so the bundle has to name
/// one, and the player has to load that one. An in-game save has no block of its own, since
/// RetroArch keeps a game's save in a file of its own, so it is [left out](Rebuilt::left_out).
///
/// # Errors
///
/// [`Error::NotConvertible`] for a bundle that is not a state, names no core, holds an `aux` part
/// whose `path` is not a block's tag, or holds more than a state is read back at.
pub fn write(bundle: &Bundle, stem: &str) -> Result<Rebuilt> {
    let contents = Contents::of(bundle)?;
    if bundle.header.source.as_ref().and_then(|source| source.core.as_ref()).is_none() {
        return Err(Error::NotConvertible(
            "this state names no core, and RetroArch hands a state to the core the player loaded".into(),
        ));
    }

    let mut left_out: Vec<u64> = contents.saves.iter().map(|part| part.id).collect();
    // Every block is held to what the file is read back at before it is inflated, since a
    // compressed part's length is its own claim and inflating it is what costs the memory. The
    // wrapping is sixteen bytes, and each block eight more and its padding.
    let mut room = MAX_STATE - 16;
    let mut blocks = Vec::new();
    for part in &contents.aux {
        let tag = part.path.as_deref().and_then(path_tag).ok_or_else(|| {
            Error::NotConvertible(format!(
                "part {} is not one of RetroArch's blocks, which an `aux` part names by its four-character tag",
                part.id
            ))
        })?;
        blocks.push((tag, block_within(part, &mut room)?));
    }
    let state = block_within(contents.state, &mut room)?;

    // RetroArch writes a replay first, then the core's data, then the rest.
    let order: Vec<([u8; 4], &[u8])> = blocks
        .iter()
        .filter(|(tag, _)| tag == b"RPLY")
        .map(|(tag, data)| (*tag, data.as_ref()))
        .chain([(*b"MEM ", state.as_ref())])
        .chain(blocks.iter().filter(|(tag, _)| tag != b"RPLY").map(|(tag, data)| (*tag, data.as_ref())))
        .chain([(*b"END ", &[][..])])
        .collect();
    let headers = order.iter().map(|(tag, data)| block_header(*tag, data)).collect::<Result<Vec<_>>>()?;
    // The file is compressed from its pieces where they lie, so the core's data is never copied
    // into a file of its own first.
    let mut wrapped: Vec<&[u8]> = vec![b"RASTATE\x01"];
    for ((_, data), header) in order.iter().zip(&headers) {
        wrapped.extend([&header[..], data, &PADDING[..data.len().next_multiple_of(8) - data.len()]]);
    }

    let name = format!(
        "{}{}",
        crate::state::component(stem),
        match crate::state::slot_of(bundle, contents.state) {
            Some(Slot::Auto) => ".state.auto".to_owned(),
            Some(Slot::Numbered(0)) | None => ".state".to_owned(),
            Some(Slot::Numbered(slot)) => format!(".state{slot}"),
        }
    );
    // RetroArch's thumbnail is a PNG; a JPEG would need encoding again, which loses what the first
    // encode already dropped, so it stays in the bundle instead.
    let screenshot = match contents.screenshot {
        Some(part) if part.content_type.as_deref() == Some("image/png") => Some(part.bytes()?.into_owned()),
        Some(part) => {
            left_out.push(part.id);
            None
        }
        None => None,
    };
    left_out.sort_unstable();
    Ok(Rebuilt { name, bytes: rzip(&wrapped), screenshot, left_out })
}

/// The file with any RZIP layer taken off.
fn unzip(bytes: &[u8]) -> Result<Cow<'_, [u8]>> {
    if !bytes.starts_with(b"#RZIPv") {
        return Ok(Cow::Borrowed(bytes));
    }
    let corrupt = |why: String| Error::Corrupt { format: FORMAT, why };
    if bytes.len() < 20 || bytes[7] != b'#' {
        return Err(corrupt("its RZIP header is cut short".into()));
    }
    let version = bytes[6];
    let chunk = le32(bytes, 8, FORMAT)? as usize;
    let total = u64::from_le_bytes(bytes[12..20].try_into().expect("eight bytes"));
    if chunk == 0 || chunk > MAX_CHUNK {
        return Err(corrupt(format!(
            "its RZIP chunks are {chunk} bytes, where RetroArch reads up to {MAX_CHUNK}"
        )));
    }
    if total == 0 || total > MAX_STATE as u64 {
        return Err(corrupt(format!("it claims to inflate to {total} bytes")));
    }
    let total =
        usize::try_from(total).map_err(|_| corrupt("it claims more than this machine addresses".into()))?;

    let mut out = Vec::new();
    let mut at = 20;
    while at < bytes.len() {
        let length = le32(bytes, at, FORMAT)? as usize;
        at += 4;
        let data = at
            .checked_add(length)
            .and_then(|end| bytes.get(at..end))
            .filter(|_| length <= chunk.saturating_mul(2))
            .ok_or_else(|| corrupt("an RZIP chunk runs past the end of the file".into()))?;
        at += length;
        let limit = chunk.min(total.saturating_sub(out.len()));
        let piece = match version {
            1 => image::inflate_zlib(data, limit, FORMAT)?,
            2 => image::inflate_zstd(data, limit, FORMAT)?,
            _ => return Err(corrupt(format!("RZIP version {version} is not one RetroArch has written"))),
        };
        out.extend_from_slice(&piece);
    }
    if out.len() != total {
        return Err(corrupt(format!("it inflates to {} bytes, not the {total} its header says", out.len())));
    }
    Ok(Cow::Owned(out))
}

/// RetroArch's blocks other than the core's data, each with its tag and where it lies, in file
/// order.
type Blocks = Vec<([u8; 4], Range<usize>)>;

/// Where the core's data lies in an unwrapped file, and every other block with its tag.
///
/// A file that is not `RASTATE` is the core's data alone, which is what RetroArch before 1.9.1
/// wrote.
fn unwrap(plain: &[u8]) -> Result<(Range<usize>, Blocks)> {
    let Some(rest) = plain.strip_prefix(b"RASTATE") else {
        return Ok((0..plain.len(), Vec::new()));
    };
    let corrupt = |why: String| Error::Corrupt { format: FORMAT, why };
    match rest.first() {
        Some(1) => {}
        Some(version) => {
            return Err(corrupt(format!("RASTATE version {version} is not one RetroArch reads")));
        }
        None => return Err(corrupt("it ends after its RASTATE header".into())),
    }

    let mut memory = None;
    let mut blocks = Vec::new();
    let mut at = 8;
    while at + 8 <= plain.len() {
        let tag: [u8; 4] = plain[at..at + 4].try_into().expect("four bytes");
        let length = le32(plain, at + 4, FORMAT)? as usize;
        at += 8;
        if &tag == b"END " {
            break;
        }
        let data =
            at.checked_add(length).filter(|&end| end <= plain.len()).map(|end| at..end).ok_or_else(|| {
                corrupt(format!("its {} block runs past the end", String::from_utf8_lossy(&tag)))
            })?;
        if &tag == b"MEM " {
            memory = Some(data);
        } else if blocks.len() == MAX_BLOCKS {
            return Err(corrupt(format!(
                "it holds more than {MAX_BLOCKS} blocks besides the core's data, where RetroArch writes two"
            )));
        } else {
            blocks.push((tag, data));
        }
        at = at.saturating_add(length.next_multiple_of(8));
    }
    let memory = memory.ok_or_else(|| corrupt("it holds no MEM block, which is the core's data".into()))?;
    Ok((memory, blocks))
}

/// A part's bytes as a block, held to what is left of `room` for it, and taken out of `room`.
fn block_within<'a>(part: &'a Part, room: &mut usize) -> Result<Cow<'a, [u8]>> {
    let data = crate::state::bytes_within(part, room.saturating_sub(8), "a RetroArch state")?;
    *room = room.saturating_sub(8 + data.len().next_multiple_of(8));
    Ok(data)
}

/// A block's header: its tag, and its length as the `u32` RetroArch gives it.
fn block_header(tag: [u8; 4], data: &[u8]) -> Result<[u8; 8]> {
    let length = u32::try_from(data.len()).map_err(|_| {
        Error::NotConvertible(format!(
            "a block of {} bytes is more than RetroArch's file has room for",
            data.len()
        ))
    })?;
    let mut header = [0; 8];
    header[..4].copy_from_slice(&tag);
    header[4..].copy_from_slice(&length.to_le_bytes());
    Ok(header)
}

/// Compresses the file `plain` is the pieces of as RZIP version 1, in RetroArch's chunk size.
fn rzip(plain: &[&[u8]]) -> Vec<u8> {
    let total: usize = plain.iter().map(|piece| piece.len()).sum();
    let mut out = b"#RZIPv\x01#".to_vec();
    out.extend_from_slice(&u32::try_from(RZIP_CHUNK).expect("the chunk size fits").to_le_bytes());
    out.extend_from_slice(&(total as u64).to_le_bytes());
    let mut push = |chunk: &[u8]| {
        let compressed = image::deflate_zlib(chunk);
        out.extend_from_slice(&u32::try_from(compressed.len()).expect("a chunk fits").to_le_bytes());
        out.extend_from_slice(&compressed);
    };
    // A chunk can span pieces, and a piece many chunks, so each is filled from the pieces in turn.
    let mut chunk = Vec::with_capacity(RZIP_CHUNK.min(total));
    for mut piece in plain.iter().copied() {
        while !piece.is_empty() {
            let take = (RZIP_CHUNK - chunk.len()).min(piece.len());
            chunk.extend_from_slice(&piece[..take]);
            piece = &piece[take..];
            if chunk.len() == RZIP_CHUNK {
                push(&chunk);
                chunk.clear();
            }
        }
    }
    if !chunk.is_empty() {
        push(&chunk);
    }
    out
}

/// The `path` an `aux` part takes for a block: its tag with the trailing space trimmed, or the tag
/// in hex where it is not printable.
fn tag_path(tag: [u8; 4]) -> String {
    let text = std::str::from_utf8(&tag).ok().map(|text| text.trim_end_matches(' '));
    match text {
        Some(text) if !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_graphic()) => text.to_owned(),
        _ => format!("0x{:08x}", u32::from_be_bytes(tag)),
    }
}

/// The tag an `aux` part's `path` names, the other way.
fn path_tag(path: &str) -> Option<[u8; 4]> {
    if let Some(hex) = path.strip_prefix("0x") {
        let value = u32::from_str_radix(hex, 16).ok().filter(|_| hex.len() == 8)?;
        return Some(value.to_be_bytes());
    }
    if path.is_empty() || path.len() > 4 || !path.bytes().all(|byte| byte.is_ascii_graphic()) {
        return None;
    }
    let mut tag = *b"    ";
    tag[..path.len()].copy_from_slice(path.as_bytes());
    Some(tag)
}

#[cfg(test)]
mod tests {
    use one_saves::{Name, Origin, Source};

    use super::*;

    fn options() -> StateOptions {
        StateOptions::default().source(Source::default().core(Name::parse("genesis-plus-gx").unwrap()))
    }

    fn file<'a>(bytes: &'a [u8], name: &'static str) -> StateFile<'a> {
        StateFile { bytes, name, screenshot: None }
    }

    #[test]
    fn a_wrapped_compressed_state_comes_out_as_the_cores_data() {
        let core: Vec<u8> = (0..300_000u32).map(|n| (n % 251) as u8).collect();
        let mut wrapped = b"RASTATE\x01".to_vec();
        push_block(&mut wrapped, *b"MEM ", &core);
        push_block(&mut wrapped, *b"ACHV", b"cheevos");
        push_block(&mut wrapped, *b"END ", &[]);

        let bundle = read(&file(&rzip(&[&wrapped]), "Sonic CD (USA).state3"), &options()).unwrap();
        assert_eq!(bundle.parts[0].kind, PartKind::State);
        assert_eq!(bundle.parts[0].bytes().unwrap(), core.as_slice());
        assert_eq!(bundle.parts[0].slot, Some(3));
        assert_eq!(
            (bundle.parts[1].kind.clone(), bundle.parts[1].path.as_deref()),
            (PartKind::Aux, Some("ACHV"))
        );
        assert_eq!(
            bundle.header.source.as_ref().and_then(|s| s.app.as_ref()).map(Name::as_str),
            Some("retroarch")
        );

        // And back: the same blocks, which RetroArch reads the same way.
        let rebuilt = write(&bundle, "Sonic CD (USA)").unwrap();
        assert_eq!(rebuilt.name, "Sonic CD (USA).state3");
        assert_eq!(unzip(&rebuilt.bytes).unwrap().as_ref(), wrapped.as_slice());
    }

    #[test]
    fn a_bare_core_state_is_the_cores_data_whole() {
        let bundle = read(&file(b"GENPLUS-GX 1.7.7", "game.state.auto"), &options()).unwrap();
        assert_eq!(bundle.parts[0].bytes().unwrap().as_ref(), b"GENPLUS-GX 1.7.7");
        assert_eq!((bundle.header.origin.clone(), bundle.parts[0].slot), (Some(Origin::Auto), None));
        assert_eq!(write(&bundle, "game").unwrap().name, "game.state.auto");
    }

    #[test]
    fn the_core_is_never_guessed() {
        let error =
            read(&file(b"RASTATE\x01END \0\0\0\0", "game.state"), &StateOptions::default()).unwrap_err();
        assert!(matches!(error, Error::NotConvertible(_)), "{error}");
    }

    #[test]
    fn a_forged_header_is_refused_before_it_is_believed() {
        let mut forged = b"#RZIPv\x01#".to_vec();
        forged.extend_from_slice(&131_072u32.to_le_bytes());
        forged.extend_from_slice(&u64::MAX.to_le_bytes());
        assert!(matches!(unzip(&forged), Err(Error::Corrupt { .. })));

        let mut overlong = b"RASTATE\x01MEM ".to_vec();
        overlong.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(unwrap(&overlong), Err(Error::Corrupt { .. })));
    }

    #[test]
    fn a_tag_round_trips_through_its_path() {
        for tag in [*b"ACHV", *b"RPLY", *b"AB  ", [0, 1, 2, 3]] {
            assert_eq!(path_tag(&tag_path(tag)), Some(tag));
        }
        assert_eq!(tag_path(*b"MEM "), "MEM");
        assert_eq!((path_tag("MEMORY"), path_tag(""), path_tag("0x123")), (None, None, None));
    }

    /// Appends a block: its tag, its length, and its data padded with zeros to eight bytes.
    fn push_block(out: &mut Vec<u8>, tag: [u8; 4], data: &[u8]) {
        out.extend_from_slice(&tag);
        out.extend_from_slice(&u32::try_from(data.len()).unwrap().to_le_bytes());
        out.extend_from_slice(data);
        out.resize(out.len().next_multiple_of(8), 0);
    }

    /// A `RASTATE` file holding these blocks, in this order, then `END `.
    fn wrapped(blocks: &[([u8; 4], &[u8])]) -> Vec<u8> {
        let mut out = b"RASTATE\x01".to_vec();
        for (tag, data) in blocks {
            push_block(&mut out, *tag, data);
        }
        push_block(&mut out, *b"END ", &[]);
        out
    }

    /// RZIP of a version and a chunk size, each chunk compressed by `compress`.
    fn rzip_of(version: u8, chunk: usize, plain: &[u8], compress: impl Fn(&[u8]) -> Vec<u8>) -> Vec<u8> {
        let mut out = vec![b'#', b'R', b'Z', b'I', b'P', b'v', version, b'#'];
        out.extend_from_slice(&u32::try_from(chunk).unwrap().to_le_bytes());
        out.extend_from_slice(&(plain.len() as u64).to_le_bytes());
        for piece in plain.chunks(chunk) {
            let compressed = compress(piece);
            out.extend_from_slice(&u32::try_from(compressed.len()).unwrap().to_le_bytes());
            out.extend_from_slice(&compressed);
        }
        out
    }

    fn thumbnail() -> Vec<u8> {
        image::encode(4, 2, 3, &[0x40; 4 * 2 * 3]).unwrap()
    }

    #[test]
    fn the_thumbnail_beside_a_state_is_its_screenshot_and_goes_back_beside_it() {
        let file = rzip(&[&wrapped(&[(*b"MEM ", b"CORE")])]);
        let png = thumbnail();
        let state = StateFile { bytes: &file, name: "game.state1", screenshot: Some(&png) };
        let bundle = read(&state, &options()).unwrap();
        assert_eq!(bundle.parts[1].kind, PartKind::Screenshot);
        assert_eq!(bundle.parts[1].bytes().unwrap().as_ref(), png.as_slice(), "kept as RetroArch wrote it");

        let rebuilt = write(&bundle, "game").unwrap();
        assert_eq!(rebuilt.screenshot, Some(png), "written beside the state, as RetroArch keeps it");
        assert_eq!(rebuilt.left_out, []);
    }

    #[test]
    fn a_replay_goes_before_the_cores_data_as_retroarch_writes_it() {
        // Read in any order, written in RetroArch's: the replay, the core's data, then the rest.
        let file = wrapped(&[(*b"MEM ", b"CORE"), (*b"ACHV", b"cheevos"), (*b"RPLY", b"input log")]);
        let bundle = read(&self::file(&file, "game.state"), &options()).unwrap();
        let paths: Vec<_> = bundle.parts[1..].iter().map(|part| part.path.as_deref()).collect();
        assert_eq!(paths, [Some("ACHV"), Some("RPLY")], "in file order");

        let rebuilt = write(&bundle, "game").unwrap();
        let expected = wrapped(&[(*b"RPLY", b"input log"), (*b"MEM ", b"CORE"), (*b"ACHV", b"cheevos")]);
        assert_eq!(unzip(&rebuilt.bytes).unwrap().as_ref(), expected.as_slice());
    }

    #[test]
    fn an_uncompressed_file_and_a_small_chunk_size_both_read() {
        // With `savestate_file_compression` off the file is `RASTATE` bare; a chunk size other
        // than RetroArch's own is still the file's to choose.
        let core: Vec<u8> = (0..10_000u32).map(|n| (n % 241) as u8).collect();
        let plain = wrapped(&[(*b"MEM ", &core)]);
        for file in [plain.clone(), rzip_of(1, 4096, &plain, image::deflate_zlib)] {
            let bundle = read(&self::file(&file, "game.state"), &options()).unwrap();
            assert_eq!(bundle.parts[0].bytes().unwrap().as_ref(), core.as_slice());
        }
    }

    #[cfg(feature = "zstd")]
    #[test]
    fn rzip_version_2_holds_zstandard_frames() {
        let core: Vec<u8> = (0..300_000u32).map(|n| (n % 251) as u8).collect();
        let plain = wrapped(&[(*b"MEM ", &core)]);
        let file = rzip_of(2, RZIP_CHUNK, &plain, |piece| zstd::stream::encode_all(piece, 0).unwrap());
        let bundle = read(&self::file(&file, "game.state"), &options()).unwrap();
        assert_eq!(bundle.parts[0].bytes().unwrap().as_ref(), core.as_slice());
    }

    #[cfg(not(feature = "zstd"))]
    #[test]
    fn rzip_version_2_is_named_and_refused_in_a_build_without_zstandard() {
        let file = rzip_of(2, RZIP_CHUNK, &wrapped(&[(*b"MEM ", b"CORE")]), <[u8]>::to_vec);
        let error = read(&self::file(&file, "game.state"), &options()).unwrap_err();
        assert!(matches!(error, Error::Unsupported { feature: "zstd", .. }), "{error}");
    }

    #[test]
    fn a_layer_retroarch_never_wrote_is_corrupt() {
        let plain = wrapped(&[(*b"MEM ", b"CORE")]);
        let corrupt = |file: &[u8], what: &str| {
            let error = read(&self::file(file, "game.state"), &options()).unwrap_err();
            assert!(matches!(error, Error::Corrupt { .. }), "{what}: {error}");
        };
        corrupt(&rzip_of(3, RZIP_CHUNK, &plain, image::deflate_zlib), "RZIP version 3");
        let mut empty = rzip(&[&plain]);
        empty[8..12].copy_from_slice(&0u32.to_le_bytes());
        corrupt(&empty, "RZIP chunks of nothing");
        corrupt(&rzip(&[&plain])[..19], "an RZIP header cut short");
        let mut short = rzip(&[&plain]);
        short[12..20].copy_from_slice(&(plain.len() as u64 + 1).to_le_bytes());
        corrupt(&short, "RZIP that inflates to less than it says");

        corrupt(b"RASTATE\x02MEM \x04\0\0\0CORE\0\0\0\0END \0\0\0\0", "RASTATE version 2");
        corrupt(b"RASTATE", "RASTATE and nothing after");
        corrupt(&wrapped(&[(*b"ACHV", b"cheevos")]), "no core's data");
    }

    #[test]
    fn more_blocks_than_retroarch_writes_are_refused_before_they_become_parts() {
        // An empty block is eight bytes, RZIP compresses a run of them a thousandfold, and each
        // was a part of its own: a 73 KB file once cost gigabytes.
        let with_empty = |tags: &[[u8; 4]]| {
            let mut blocks = vec![(*b"MEM ", &b"CORE"[..])];
            blocks.extend(tags.iter().map(|tag| (*tag, &[][..])));
            wrapped(&blocks)
        };
        let tags: Vec<[u8; 4]> = (0..MAX_BLOCKS).map(|n| u32::try_from(n).unwrap().to_be_bytes()).collect();
        let most = read(&file(&with_empty(&tags), "game.state"), &options()).unwrap();
        assert_eq!(most.parts.len(), 1 + MAX_BLOCKS, "as many as are read");

        let many = rzip(&[&with_empty(&vec![*b"ACHV"; 100_000])]);
        assert!(many.len() < 8192, "{} bytes", many.len());
        match read(&file(&many, "game.state"), &options()) {
            Err(Error::Corrupt { why, .. }) => assert!(why.contains("blocks"), "{why}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_state_claiming_past_the_largest_any_core_takes_is_refused_before_it_is_inflated() {
        // A gibibyte was under the cap before, and a megabyte of zeros claiming it was inflated
        // whole, then written out whole again.
        for claim in [MAX_STATE as u64 + 1, 1 << 30] {
            let mut forged = b"#RZIPv\x01#".to_vec();
            forged.extend_from_slice(&u32::try_from(MAX_CHUNK).unwrap().to_le_bytes());
            forged.extend_from_slice(&claim.to_le_bytes());
            let zeros = image::deflate_zlib(&[0; 4096]);
            forged.extend_from_slice(&u32::try_from(zeros.len()).unwrap().to_le_bytes());
            forged.extend_from_slice(&zeros);
            match unzip(&forged) {
                Err(Error::Corrupt { why, .. }) => assert!(why.contains("claims"), "{claim}: {why}"),
                other => panic!("{claim}: {other:?}"),
            }
        }
    }

    #[test]
    fn a_part_claiming_more_than_the_file_is_read_back_at_is_refused_before_it_is_inflated() {
        // Past 4 GiB a block once panicked on its `u32` length, after it had been inflated.
        let bundle = read(&file(&wrapped(&[(*b"MEM ", b"CORE")]), "game.state"), &options()).unwrap();
        let mut odd = bundle.clone();
        odd.parts.push(crate::state::part_claiming(1, (1 << 32) + 16).kind(PartKind::Aux).path("ACHV"));
        let error = write(&odd, "game").unwrap_err();
        assert!(matches!(error, Error::NotConvertible(_)), "{error}");

        // The core's data alone at the cap leaves no room for RetroArch's wrapping around it.
        let mut odd = bundle;
        odd.parts[0] = crate::state::part_claiming(0, MAX_STATE as u64).kind(PartKind::State);
        let error = write(&odd, "game").unwrap_err();
        assert!(matches!(error, Error::NotConvertible(_)), "the core's data: {error}");
    }

    #[test]
    fn what_retroarch_keeps_elsewhere_is_left_out() {
        let file = rzip(&[&wrapped(&[(*b"MEM ", b"CORE")])]);
        let mut bundle = read(&self::file(&file, "game.state"), &options()).unwrap();
        let jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, b'J', b'F', b'I', b'F'];
        bundle.parts.push(crate::state::screenshot_part(jpeg).unwrap());
        bundle.parts.push(Part::new(0, *b"SRAM"));
        bundle.parts = crate::state::number(bundle.parts);
        bundle.validate().unwrap();

        // RetroArch keeps a game's save in a file of its own, and its thumbnail is a PNG.
        let rebuilt = write(&bundle, "game").unwrap();
        assert_eq!((rebuilt.left_out, rebuilt.screenshot), (vec![1, 2], None));
    }

    #[test]
    fn a_part_that_is_no_block_or_a_state_with_no_core_is_not_rebuilt() {
        let file = wrapped(&[(*b"MEM ", b"CORE")]);
        let bundle = read(&self::file(&file, "game.state"), &options()).unwrap();
        for path in [Some("extdata/4"), None] {
            let mut odd = bundle.clone();
            let mut part = Part::new(1, *b"MORE").kind(PartKind::Aux);
            part.path = path.map(ToOwned::to_owned);
            odd.parts.push(part);
            assert!(matches!(write(&odd, "game"), Err(Error::NotConvertible(_))), "{path:?}");
        }

        // The app alone is not enough: RetroArch hands the state to whichever core is loaded.
        let mut coreless = bundle;
        coreless.header.source.as_mut().unwrap().core = None;
        coreless.validate().expect("RetroArch is something it loads into");
        assert!(matches!(write(&coreless, "game"), Err(Error::NotConvertible(_))));
    }
}
