//! DeSmuME's save states: `X.ds0` to `X.ds9` in its slots, and `.dst` for one saved to a file.
//!
//! A state file is `DeSmuME SState` and two NULs, four 32-bit words (the state's version, DeSmuME's
//! own version, the data's length, and the length compressed, or all ones for data stored as it
//! is), then the core's data, a zlib stream as DeSmuME writes it. The data is a run of chunks, each
//! a 32-bit type and length, ending in a type of all ones. The state part holds that data
//! uncompressed, chunks and all; DeSmuME reads it back stored as it is.
//!
//! Two chunks are read, and neither is taken out. Chunk 130 holds the ROM's header, which gives
//! the bundle the game's code and title. Chunk 61, the memory controller's, holds the card's save
//! memory inside the data, so a state carries its in-game save in the state part rather than beside
//! it; [`in_game_save`] lifts it out for a caller that wants the save on its own.

use one_saves::{Bundle, Game, Recorded, Slug, TakenWith};

use crate::error::{Error, Result};
use crate::state::{Contents, Found, Rebuilt, StateFile, StateOptions};

const FORMAT: &str = "DeSmuME state";

/// What every state file opens with.
pub(crate) const MAGIC: &[u8] = b"DeSmuME SState\0\0";

/// The header's length: the magic and four words.
const HEADER: usize = 32;

/// The compressed length that says the data is stored as it is.
const STORED: u32 = u32::MAX;

/// The most data a state is taken to hold. DeSmuME's run to about 12 MiB: the DS's 4 MiB of main
/// memory, its video memory, and the rest of the machine.
const MAX_DATA: usize = 64 << 20;

/// The memory controller's chunk, where the card's save memory is kept.
const MMU: u32 = 61;
/// The chunk holding the ROM's header.
const ROM_INFO: u32 = 130;
/// The type that ends the chunks.
const END: u32 = u32::MAX;

/// A state's data, inflated, and the two words around it that DeSmuME stamps.
struct Opened {
    version: u32,
    desmume: u32,
    data: Vec<u8>,
}

fn corrupt(why: impl Into<String>) -> Error {
    Error::Corrupt { format: FORMAT, why: why.into() }
}

fn le32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

/// The header's words and the data, inflated where DeSmuME compressed it.
fn open(bytes: &[u8]) -> Result<Opened> {
    if !bytes.starts_with(MAGIC) || bytes.len() < HEADER {
        return Err(Error::NotThisFormat {
            format: FORMAT,
            why: "it does not open with DeSmuME SState".into(),
        });
    }
    let word = |at: usize| le32(bytes, at).ok_or_else(|| corrupt("its header is cut short"));
    let (version, desmume, length, compressed) = (word(16)?, word(20)?, word(24)?, word(28)?);
    let length = usize::try_from(length).unwrap_or(usize::MAX);
    if length > MAX_DATA {
        return Err(corrupt(format!("its data claims {length} bytes, more than any DeSmuME state holds")));
    }
    let body = &bytes[HEADER..];
    let data = if compressed == STORED {
        body.get(..length).ok_or_else(|| corrupt("its data runs past the end of the file"))?.to_vec()
    } else {
        use std::io::Read as _;
        let stream = usize::try_from(compressed).ok().and_then(|len| body.get(..len));
        let stream = stream.ok_or_else(|| corrupt("its compressed data runs past the end of the file"))?;
        let mut data = Vec::with_capacity(length);
        flate2::read::ZlibDecoder::new(stream)
            .take(length as u64 + 1)
            .read_to_end(&mut data)
            .map_err(|e| corrupt(format!("its data does not inflate: {e}")))?;
        data
    };
    if data.len() != length {
        return Err(corrupt("its data inflates to another length than the header says"));
    }
    Ok(Opened { version, desmume, data })
}

/// Each chunk of a state's data: its type and its bytes, up to the chunk that ends them.
fn chunks(data: &[u8]) -> impl Iterator<Item = (u32, &[u8])> {
    let mut at = 0usize;
    std::iter::from_fn(move || {
        let kind = le32(data, at)?;
        if kind == END {
            return None;
        }
        let len = usize::try_from(le32(data, at + 4)?).ok()?;
        let chunk = data.get(at + 8..at.checked_add(8)?.checked_add(len)?)?;
        at += 8 + len;
        Some((kind, chunk))
    })
}

/// The game code and title the ROM's header carries, from chunk 130, where the state recorded it:
/// a run of records, each four bytes naming it, an element size and a count, the `GINF` one the
/// header itself.
fn rom_of(data: &[u8]) -> Option<(Option<String>, Option<String>)> {
    let (_, chunk) = chunks(data).find(|(kind, _)| *kind == ROM_INFO)?;
    let mut at = 0usize;
    while let (Some(tag), Some(size), Some(count)) =
        (chunk.get(at..at + 4), le32(chunk, at + 4), le32(chunk, at + 8))
    {
        let len = usize::try_from(u64::from(size) * u64::from(count)).ok()?;
        let value = chunk.get(at + 12..at.checked_add(12)?.checked_add(len)?)?;
        if tag == b"GINF" && value.len() >= 0x10 {
            let field = |bytes: &[u8]| {
                let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
                let text: String = bytes[..end].iter().map(|&b| char::from(b)).collect();
                let text = text.trim().to_owned();
                (!text.is_empty() && text.chars().all(|c| c.is_ascii_graphic() || c == ' ')).then_some(text)
            };
            return Some((field(&value[0x0C..0x10]), field(&value[..0x0C])));
        }
        at += 12 + len;
    }
    None
}

/// The card's save memory a state's data holds, from chunk 61, and the game code chunk 130 gives.
///
/// The chunk's own version decides where the memory sits: versions 0 and 1 keep its length and
/// then the bytes, and from version 2 the backup device's six words come first. What comes out is
/// the memory as DeSmuME held it, padded to its size or, in old states, only as far as the game
/// wrote; [`nds::read_opened`](crate::nds::read_opened) fits it to the chip.
///
/// # Errors
///
/// [`Error::NotThisFormat`] for a file that is not a DeSmuME state, and [`Error::Corrupt`] for one
/// with no save memory in it.
pub fn in_game_save(bytes: &[u8]) -> Result<crate::nds::Opened> {
    let opened = open(bytes)?;
    let (_, mmu) = chunks(&opened.data)
        .find(|(kind, _)| *kind == MMU)
        .ok_or_else(|| corrupt("it has no memory controller"))?;
    let at = match le32(mmu, 0) {
        Some(0) => 4,
        Some(1) => 8,
        Some(_) => 28,
        None => return Err(corrupt("its memory controller is empty")),
    };
    let len = le32(mmu, at).and_then(|n| usize::try_from(n).ok());
    let save = len.and_then(|len| mmu.get(at + 4..at.checked_add(4)?.checked_add(len)?));
    let save = save.ok_or_else(|| corrupt("its save memory runs past its memory controller"))?;
    if save.is_empty() {
        return Err(corrupt("it holds no save memory"));
    }
    let game_code = rom_of(&opened.data).and_then(|(code, _)| code);
    Ok(crate::nds::Opened {
        data: save.to_vec(),
        container: crate::nds::Container::DesmumeState,
        chip: None,
        game_code,
        note: None,
    })
}

/// DeSmuME's version, as it stamps it: major times 100000, minor times 10000, build times 100.
fn version_of(stamp: u32) -> String {
    format!("{}.{}.{}", stamp / 100_000, stamp / 10_000 % 10, stamp / 100 % 100)
}

/// The slot a state's name says: `X.ds0` to `X.ds9`. A `.dst` was saved to a file, and has none.
fn slot_of(name: &str) -> Option<u64> {
    let (_, tail) = name.rsplit_once(".ds")?;
    (tail.len() == 1).then(|| tail.parse().ok()).flatten()
}

/// Reads a DeSmuME state file into a state bundle.
///
/// # Errors
///
/// [`Error::NotThisFormat`] for a file that is not one, and [`Error::Corrupt`] for one whose data
/// does not come out at the length its header gives.
pub fn read(file: &StateFile<'_>, options: &StateOptions) -> Result<Bundle> {
    let opened = open(file.bytes)?;
    let game = rom_of(&opened.data).map(|(serial, title)| {
        let mut game = Game::default();
        (game.serial, game.title) = (serial, title);
        game
    });
    let found = Found {
        app: Some("desmume"),
        core: Some("desmume"),
        core_version: Some(version_of(opened.desmume)),
        system: Slug::parse(crate::nds::SYSTEM).ok(),
        game: game.filter(|game| !game.is_empty()),
        taken_with: Some(TakenWith::default().format_version(u64::from(opened.version))),
        ..Found::default()
    };
    let slot = slot_of(&file.name.to_ascii_lowercase());
    let parts = vec![crate::state::state_part(opened.data, slot)];
    let bundle = Bundle::new(crate::state::header(options, found), crate::state::number(parts));
    bundle.validate()?;
    Ok(bundle)
}

/// Rebuilds a DeSmuME state file from a state bundle, its data stored as it is.
///
/// # Errors
///
/// [`Error::NotConvertible`] for a bundle that is not a state, loads into another core, records no
/// format version, or holds more data than a DeSmuME state does.
pub fn write(bundle: &Bundle, stem: &str) -> Result<Rebuilt> {
    let contents = Contents::of(bundle)?;
    crate::state::require_core(bundle, "desmume", "DeSmuME")?;
    let version = bundle
        .header
        .taken_with
        .as_ref()
        .and_then(|with| with.format_version.as_ref())
        .and_then(Recorded::known)
        .and_then(|version| u32::try_from(*version).ok())
        .ok_or_else(|| {
            Error::NotConvertible(
                "DeSmuME loads a state at the version it stamped, and this one records none".into(),
            )
        })?;
    let desmume = bundle
        .header
        .source
        .as_ref()
        .and_then(|source| source.core_version.as_deref())
        .and_then(stamp_of)
        .unwrap_or(0);
    let data = crate::state::bytes_within(contents.state, MAX_DATA, "a DeSmuME state")?;
    let mut bytes = Vec::with_capacity(HEADER + data.len());
    bytes.extend_from_slice(MAGIC);
    let length = u32::try_from(data.len()).expect("held to MAX_DATA");
    for word in [version, desmume, length, STORED] {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    bytes.extend_from_slice(data);
    let extension = contents
        .state
        .slot
        .filter(|&slot| slot <= 9)
        .map_or_else(|| "dst".to_owned(), |slot| format!("ds{slot}"));
    let left_out = contents
        .saves
        .iter()
        .chain(&contents.aux)
        .chain(contents.screenshot.as_slice())
        .map(|part| part.id)
        .collect();
    Ok(Rebuilt {
        name: format!("{}.{extension}", crate::state::component(stem)),
        bytes,
        screenshot: None,
        left_out,
    })
}

/// The word DeSmuME stamps for a version written `0.9.13`.
fn stamp_of(version: &str) -> Option<u32> {
    let mut parts = version.split('.').map(str::parse::<u32>);
    let (major, minor, build) = (parts.next()?.ok()?, parts.next()?.ok()?, parts.next()?.ok()?);
    (minor < 10 && build < 100).then(|| major * 100_000 + minor * 10_000 + build * 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A state's data: a memory controller of version 8 holding `save`, and a ROM's header.
    fn data(save: &[u8], code: [u8; 4]) -> Vec<u8> {
        let mut mmu = 8u32.to_le_bytes().to_vec();
        mmu.extend_from_slice(&[0; 24]);
        mmu.extend_from_slice(&u32::try_from(save.len()).unwrap().to_le_bytes());
        mmu.extend_from_slice(save);
        let mut header = vec![0u8; 0x160];
        header[..9].copy_from_slice(b"POKEMON D");
        header[0x0C..0x10].copy_from_slice(&code);
        let mut rom = b"GINF".to_vec();
        rom.extend_from_slice(&1u32.to_le_bytes());
        rom.extend_from_slice(&0x160u32.to_le_bytes());
        rom.extend_from_slice(&header);
        let mut data = Vec::new();
        for (kind, chunk) in [(1u32, &b"CPU!"[..]), (MMU, &mmu), (ROM_INFO, &rom)] {
            data.extend_from_slice(&kind.to_le_bytes());
            data.extend_from_slice(&u32::try_from(chunk.len()).unwrap().to_le_bytes());
            data.extend_from_slice(chunk);
        }
        data.extend_from_slice(&END.to_le_bytes());
        data
    }

    fn file(data: &[u8]) -> Vec<u8> {
        use std::io::Write as _;
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(data).unwrap();
        let stream = encoder.finish().unwrap();
        let mut bytes = MAGIC.to_vec();
        for word in [12, 0x164A4, u32::try_from(data.len()).unwrap(), u32::try_from(stream.len()).unwrap()] {
            bytes.extend_from_slice(&u32::to_le_bytes(word));
        }
        bytes.extend_from_slice(&stream);
        bytes
    }

    #[test]
    fn a_state_reads_its_game_and_version_and_rebuilds_stored() {
        let save = vec![0x5A; 512 * 1024];
        let data = data(&save, *b"ADAD");
        let bytes = file(&data);
        let state = StateFile { bytes: &bytes, name: "Pokemon Diamond.ds3", screenshot: None };
        let bundle = read(&state, &StateOptions::default()).expect("reads");
        assert_eq!(bundle.parts[0].payload, data, "the data, inflated");
        assert_eq!(bundle.parts[0].slot, Some(3));
        let game = bundle.header.game.as_ref().unwrap();
        assert_eq!((game.serial.as_deref(), game.title.as_deref()), (Some("ADAD"), Some("POKEMON D")));
        let source = bundle.header.source.as_ref().unwrap();
        assert_eq!(source.core_version.as_deref(), Some("0.9.13"));

        let rebuilt = write(&bundle, "Pokemon Diamond").expect("rebuilds");
        assert_eq!(rebuilt.name, "Pokemon Diamond.ds3");
        let again = StateFile { bytes: &rebuilt.bytes, name: &rebuilt.name, screenshot: None };
        let round = read(&again, &StateOptions::default()).expect("reads its own file");
        assert_eq!((round.parts, round.header.game), (bundle.parts, bundle.header.game));
        assert_eq!(&rebuilt.bytes[16..24], &[12, 0, 0, 0, 0xA4, 0x64, 0x01, 0]);
    }

    #[test]
    fn its_in_game_save_lifts_out_with_its_game() {
        let save = vec![0x5A; 512 * 1024];
        let opened = in_game_save(&file(&data(&save, *b"IPKE"))).expect("lifts");
        assert_eq!((opened.data, opened.game_code.as_deref()), (save, Some("IPKE")));
    }

    /// A state's file holding `data` as it is, with its header's length as `length`.
    fn stored(data: &[u8], length: u32) -> Vec<u8> {
        let mut bytes = MAGIC.to_vec();
        for word in [12, 0x164A4, length, STORED] {
            bytes.extend_from_slice(&u32::to_le_bytes(word));
        }
        bytes.extend_from_slice(data);
        bytes
    }

    /// Data of one chunk, `kind`, holding `chunk`.
    fn one_chunk(kind: u32, chunk: &[u8]) -> Vec<u8> {
        let mut data = kind.to_le_bytes().to_vec();
        data.extend_from_slice(&u32::try_from(chunk.len()).unwrap().to_le_bytes());
        data.extend_from_slice(chunk);
        data.extend_from_slice(&END.to_le_bytes());
        data
    }

    fn lifted(data: &[u8]) -> Result<crate::nds::Opened> {
        in_game_save(&stored(data, u32::try_from(data.len()).unwrap()))
    }

    fn is_corrupt(result: Result<crate::nds::Opened>, why: &str) {
        match result {
            Err(Error::Corrupt { why: found, .. }) => assert!(found.contains(why), "{found:?}, not {why:?}"),
            other => panic!("expected it refused as corrupt ({why}), got {other:?}"),
        }
    }

    #[test]
    fn the_oldest_memory_controllers_keep_the_save_nearer_their_start() {
        let save = [0x5A; 512];
        for (version, before) in [(0u32, 0usize), (1, 4)] {
            let mut mmu = version.to_le_bytes().to_vec();
            mmu.resize(4 + before, 0);
            mmu.extend_from_slice(&512u32.to_le_bytes());
            mmu.extend_from_slice(&save);
            let opened = lifted(&one_chunk(MMU, &mmu)).expect("lifts");
            assert_eq!((opened.data, opened.game_code), (save.to_vec(), None), "version {version}");
        }
        // A stored state reads as a compressed one does.
        let data = data(&save, *b"ASMP");
        let opened = lifted(&data).expect("lifts");
        assert_eq!(opened.game_code.as_deref(), Some("ASMP"));
    }

    #[test]
    fn a_state_that_does_not_hold_together_is_refused() {
        let good = data(&[0x5A; 512], *b"ASMP");
        assert!(matches!(in_game_save(b"DeSmuME"), Err(Error::NotThisFormat { .. })));
        assert!(matches!(in_game_save(&[0; 64]), Err(Error::NotThisFormat { .. })));
        is_corrupt(in_game_save(&stored(&good, u32::MAX - 1)), "more than any DeSmuME state holds");
        is_corrupt(in_game_save(&stored(&good, u32::try_from(good.len() + 1).unwrap())), "past the end");

        let mut cut = file(&good);
        cut.truncate(cut.len() - 4);
        is_corrupt(in_game_save(&cut), "runs past the end of the file");
        let mut short = file(&good);
        short[24..28].copy_from_slice(&u32::try_from(good.len() - 1).unwrap().to_le_bytes());
        is_corrupt(in_game_save(&short), "another length");
        let mut garbled = file(&good);
        let last = garbled.len();
        garbled[HEADER + 2..last].fill(0x42);
        assert!(matches!(in_game_save(&garbled), Err(Error::Corrupt { .. })));

        is_corrupt(lifted(&one_chunk(1, b"CPU!")), "no memory controller");
        is_corrupt(lifted(&END.to_le_bytes()), "no memory controller");
        // A chunk claiming more than the data holds ends the chunks.
        let mut overlong = one_chunk(MMU, &[8, 0, 0, 0]);
        overlong[4] = 0xFF;
        is_corrupt(lifted(&overlong), "no memory controller");
        is_corrupt(lifted(&one_chunk(MMU, &[])), "empty");
        let mut past = 8u32.to_le_bytes().to_vec();
        past.resize(28, 0);
        past.extend_from_slice(&512u32.to_le_bytes());
        past.extend_from_slice(&[0; 100]);
        is_corrupt(lifted(&one_chunk(MMU, &past)), "runs past its memory controller");
        let mut nothing = 8u32.to_le_bytes().to_vec();
        nothing.resize(32, 0);
        is_corrupt(lifted(&one_chunk(MMU, &nothing)), "no save memory");
    }

    #[test]
    fn a_damaged_state_is_answered_and_never_panics() {
        let data = data(&[0x5A; 512], *b"ASMP");
        let good = stored(&data, u32::try_from(data.len()).unwrap());
        for len in 0..good.len() {
            let _ = in_game_save(&good[..len]);
        }
        for at in HEADER..good.len().min(HEADER + 0x200) {
            let mut damaged = good.clone();
            damaged[at] ^= 0xFF;
            let _ = in_game_save(&damaged);
        }
    }

    #[test]
    fn a_version_goes_to_its_stamp_and_back() {
        for (stamp, text) in [(0x164A4, "0.9.13"), (0x16378, "0.9.10"), (0x1624C, "0.9.7")] {
            assert_eq!(version_of(stamp), text);
            assert_eq!(stamp_of(text), Some(stamp));
        }
    }
}
