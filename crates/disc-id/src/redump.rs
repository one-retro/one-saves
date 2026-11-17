//! The `redump-track` fingerprint: a disc's first data track, hashed as
//! Redump hashes it.
//!
//! Redump publishes a SHA-1 and an MD5 for every track of every disc it has
//! verified, each over the track's own file: its sectors raw, 2352 bytes apiece
//! with the sync pattern, header and error correction they were read with, from
//! where the track's pregap starts to where the next track's does. So there is
//! one digest a catalogue keys a disc on after all, and it is the first data
//! track's. 1Retro's `redump-track` fingerprint is that SHA-1 and MD5.
//!
//! It is worth computing here because it survives a change of container. A cue
//! sheet with a file per track, one image holding every track, a CloneCD `.img`
//! and a CHD of one disc all hold the same raw sectors, and each entry point
//! strips its own container back down to them:
//!
//! - [`track`] reads the track out of the file a cue sheet's [`DataTrack`]
//!   names, whether that is the track's own file or an image of every track.
//! - [`image`] reads a disc image that came with no sheet: a DVD, which Redump
//!   hashes whole, or a raw image of one data track.
//! - `chd::redump`, with the `chd` feature, decompresses a CHD a hunk at a
//!   time.
//!
//! Each reads forward from a plain [`Read`], since a track may be coming out of
//! an archive, and each streams, since a track is hundreds of megabytes: nothing
//! here holds more than a buffer of it.
//!
//! Two things are never hashed. An audio track is not the program, and a disc
//! of nothing but audio has no fingerprint. A cooked track, an `.iso` of
//! 2048-byte sectors or Mode 2 at 2336, has thrown away the bytes Redump hashed,
//! and no hash of what is left matches anything, so it is `None` rather than a
//! value that looks like an answer.

use std::io::{self, Read};

use md5::{Digest as _, Md5};
use sha1::Sha1;

use crate::cue::DataTrack;
use crate::sector::{Geometry, RAW, SYNC, USER_DATA};

/// SHA-1 and MD5 of a disc's first data track, and how many bytes they cover.
///
/// The three values a Redump DAT lists for a track, so a lookup can take any
/// of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RedumpHash {
    /// SHA-1 of the track's raw sectors.
    pub sha1: [u8; 20],
    /// MD5 of the same bytes.
    pub md5: [u8; 16],
    /// How many bytes were hashed: a whole number of raw sectors for a CD
    /// track, the whole image for a DVD.
    pub size: u64,
}

impl RedumpHash {
    /// The SHA-1 as a DAT writes it: forty lower-case hex digits.
    #[must_use]
    pub fn sha1_hex(&self) -> String {
        crate::hex(&self.sha1)
    }

    /// The MD5 as a DAT writes it: thirty-two lower-case hex digits.
    #[must_use]
    pub fn md5_hex(&self) -> String {
        crate::hex(&self.md5)
    }
}

/// Both digests at once, over one pass of the bytes.
pub(crate) struct Hasher {
    sha1: Sha1,
    md5: Md5,
    size: u64,
}

impl Hasher {
    pub(crate) fn new() -> Self {
        Hasher { sha1: Sha1::new(), md5: Md5::new(), size: 0 }
    }

    pub(crate) fn update(&mut self, bytes: &[u8]) {
        self.sha1.update(bytes);
        self.md5.update(bytes);
        self.size += bytes.len() as u64;
    }

    pub(crate) fn finish(self) -> RedumpHash {
        RedumpHash { sha1: self.sha1.finalize().into(), md5: self.md5.finalize().into(), size: self.size }
    }
}

/// Sectors read at a time: 600 KB, and always whole sectors, so a sector's
/// sync pattern never straddles two reads.
const BATCH: usize = 256;

/// Which sectors of a raw stream must open with the sync pattern for it to be
/// a data track.
pub(crate) enum Framed {
    /// The one this many bytes in, where the track's content starts. A pregap
    /// ahead of it need not be framed: a data track following audio commonly
    /// opens with a second of silence, written as audio.
    At(u64),
    /// Every one: an image with no sheet is one data track only if none of it
    /// is audio, since nothing else says where the data would end.
    Every,
}

/// Hash raw sectors from `reader` to its end, if they are a data track: framed
/// as `framed` says, whole sectors, and `expected` bytes where that is known.
///
/// `None` for anything else. A track declared raw whose content opens without
/// the sync pattern is not raw, however its sheet labels it; and one that ends
/// before its sheet says it does is not all there. Either would hash to a value
/// that names nothing.
pub(crate) fn raw_track<R: Read>(
    mut reader: R,
    framed: &Framed,
    expected: Option<u64>,
) -> io::Result<Option<RedumpHash>> {
    if let Framed::At(content) = framed
        && !content.is_multiple_of(RAW as u64)
    {
        return Ok(None);
    }
    let mut buf = vec![0u8; BATCH * RAW];
    let mut hasher = Hasher::new();
    // Whether a sector that has to be framed was seen, and was.
    let mut seen = false;
    loop {
        let read = crate::read_upto(&mut reader, &mut buf)?;
        if read == 0 {
            break;
        }
        let chunk = &buf[..read];
        match framed {
            Framed::At(content) => {
                let within = content.checked_sub(hasher.size).and_then(|d| usize::try_from(d).ok());
                if let Some(within) = within.filter(|&within| within < read) {
                    if chunk.get(within..within + SYNC.len()) != Some(&SYNC[..]) {
                        return Ok(None);
                    }
                    seen = true;
                }
            }
            Framed::Every => {
                if !chunk.chunks(RAW).all(|sector| sector.starts_with(&SYNC)) {
                    return Ok(None);
                }
                seen = true;
            }
        }
        hasher.update(chunk);
        if read < buf.len() {
            break;
        }
    }
    let size = hasher.size;
    let whole = size.is_multiple_of(RAW as u64) && expected.is_none_or(|expected| expected == size);
    Ok((seen && whole).then(|| hasher.finish()))
}

/// Hash everything `reader` holds.
pub(crate) fn whole<R: Read>(mut reader: R) -> io::Result<RedumpHash> {
    let mut buf = vec![0u8; BATCH * RAW];
    let mut hasher = Hasher::new();
    loop {
        match reader.read(&mut buf) {
            Ok(0) => return Ok(hasher.finish()),
            Ok(n) => hasher.update(&buf[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
}

/// The Redump hash of a cue sheet's data track, read out of `file`.
///
/// `file` reads the file [`DataTrack::file`] names, from its first byte. That
/// is the track's own file where the sheet gives it one, and the image every
/// track shares where it does not; the track is cut out of it here, from its
/// pregap through [`DataTrack::length`], so a caller hands over the file and
/// never slices it. A CloneCD `.img` is the second shape, with the sheet that
/// came beside it.
///
/// Reads forward only, so `file` can be an archive member as well as a file:
/// whatever precedes the track is read and dropped, which for a data track that
/// is the disc's first or second is at most a few megabytes of audio.
///
/// `Ok(None)` for a track the sheet declares cooked, and for one whose bytes
/// turn out not to be raw sectors, or not all there. An error is a read that
/// failed.
///
/// One thing the sheet has to get right for this to match Redump: where the
/// track's pregap starts. A sheet that states no `INDEX 00` for a pregap the
/// image does hold, as CloneCD's commonly do, leaves those sectors to the track
/// before, and the value then differs from Redump's by them.
///
/// ```no_run
/// use std::fs::File;
/// use std::path::Path;
///
/// use disc_id::{cue, redump};
///
/// let sheet = Path::new("Game.cue");
/// let text = std::fs::read_to_string(sheet)?;
/// if let Some(track) = cue::first_data_track(&text)
///     && let Some(path) = cue::resolve(sheet.parent().unwrap(), &track.file)
///     && let Some(found) = redump::track(&track, File::open(path)?)?
/// {
///     println!("{} bytes", found.size);
/// }
/// # Ok::<(), std::io::Error>(())
/// ```
pub fn track<R: Read>(track: &DataTrack, mut file: R) -> io::Result<Option<RedumpHash>> {
    if track.sector_size != RAW as u64 {
        return Ok(None);
    }
    let Some(start) = track.offset.checked_sub(track.pregap) else {
        return Ok(None);
    };
    // Read up to it rather than seek: an archive member cannot.
    if io::copy(&mut (&mut file).take(start), &mut io::sink())? < start {
        return Ok(None);
    }
    let span = match track.length.map(|length| track.pregap.checked_add(length)) {
        Some(None) => return Ok(None),
        Some(Some(span)) => Some(span),
        None => None,
    };
    raw_track(file.take(span.unwrap_or(u64::MAX)), &Framed::At(track.pregap), span)
}

/// How much of an image [`image`] looks at before deciding what it is: through
/// sector 31 of a cooked image, past where a DVD's UDF descriptors follow its
/// ISO 9660 ones.
const HEAD: usize = 32 * USER_DATA;

/// The Redump hash of a disc image that came with no cue sheet.
///
/// Two kinds of image answer, and they are the two whose digest Redump takes
/// over the whole file.
///
/// - **A DVD**, cooked, as every DVD dump is: Redump hashes the image whole.
///   GameCube and Wii discs say what they are in their header. A PlayStation 2
///   DVD carries a UDF bridge beside its ISO 9660 volume, and a PlayStation 2
///   CD does not, which was checked on three of each. Size alone would not do:
///   124 of the PlayStation 2 DVDs Redump lists are smaller than a CD can be.
/// - **A raw image of one data track**, every sector of it framed. Without a
///   sheet nothing says where a data track ends, so an image with audio in it,
///   anywhere, is `None` rather than a hash of the data and the music together.
///
/// Anything else is `None`: a cooked image of a CD, which no longer holds the
/// bytes Redump hashed; a compressed Dolphin container, which holds the disc
/// but not as its bytes; and anything that is not a disc at all.
///
/// Reads `file` to its end for an image that answers, and for a raw one that
/// turns out to hold audio. A cooked CD is refused from its first 64 KiB.
pub fn image<R: Read>(mut file: R) -> io::Result<Option<RedumpHash>> {
    let mut head = vec![0u8; HEAD];
    let read = crate::read_upto(&mut file, &mut head)?;
    head.truncate(read);
    if crate::wia_disc_head(&head).is_some() {
        return Ok(None);
    }
    match Geometry::detect(&head) {
        Some(Geometry::RawMode1 | Geometry::RawMode2) => {
            raw_track(io::Cursor::new(head).chain(file), &Framed::Every, None)
        }
        Some(Geometry::Cooked) if is_dvd(&head) => whole(io::Cursor::new(head).chain(file)).map(Some),
        _ => Ok(None),
    }
}

/// Whether a cooked image's opening sectors are a DVD's.
fn is_dvd(head: &[u8]) -> bool {
    if crate::nintendo(head).is_some() {
        return true;
    }
    // UDF's volume recognition sequence starts at the same byte as ISO 9660's
    // descriptors and runs on after them, one descriptor a sector; an `NSR`
    // entry in it is what says a UDF volume is there.
    head.chunks(USER_DATA).skip(16).any(|sector| matches!(sector.get(1..6), Some(b"NSR02" | b"NSR03")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A raw sector: sync, an address, mode 1, and `fill` for the rest.
    fn data(fill: u8) -> Vec<u8> {
        let mut sector = SYNC.to_vec();
        sector.extend_from_slice(&[0, 2, 0, 1]);
        sector.resize(RAW, fill);
        sector
    }

    /// The digests themselves, against Python's `hashlib` over the same bytes.
    #[test]
    fn both_digests_are_of_every_byte() {
        let bytes: Vec<u8> = (0..3u8).flat_map(data).collect();
        let found = raw_track(&bytes[..], &Framed::Every, None).unwrap().unwrap();
        assert_eq!(found.size, 3 * RAW as u64);
        assert_eq!(found.sha1_hex(), "21374b1dcba1b1d134809c4acb8d301e07b6a440");
        assert_eq!(found.md5_hex(), "4215139923d7ee8abf772ca4b2c58658");
    }

    /// A pregap of silence ahead of the content is the track's all the same;
    /// silence where the content should start means the track is not raw.
    #[test]
    fn only_the_content_has_to_be_framed() {
        let mut bytes = vec![0u8; 2 * RAW];
        bytes.extend(data(7));
        assert!(raw_track(&bytes[..], &Framed::At(2 * RAW as u64), None).unwrap().is_some());
        assert!(raw_track(&bytes[..], &Framed::At(0), None).unwrap().is_none());
        assert!(raw_track(&bytes[..], &Framed::Every, None).unwrap().is_none());
    }

    /// A track cut short is not the track, and neither is a ragged end.
    #[test]
    fn a_track_that_is_not_all_there_has_no_fingerprint() {
        let bytes: Vec<u8> = (0..2).flat_map(|_| data(1)).collect();
        assert!(raw_track(&bytes[..], &Framed::At(0), Some(3 * RAW as u64)).unwrap().is_none());
        assert!(raw_track(&bytes[..RAW + 10], &Framed::At(0), None).unwrap().is_none());
        assert!(raw_track(&bytes[..], &Framed::At(0), Some(2 * RAW as u64)).unwrap().is_some());
    }

    /// Cooked bytes are none of these, whatever a sheet calls them.
    #[test]
    fn a_cooked_track_is_none() {
        let cooked = vec![0x5Au8; 4 * USER_DATA];
        let track = DataTrack { file: "g.iso".into(), offset: 0, pregap: 0, length: None, sector_size: 2048 };
        assert_eq!(super::track(&track, &cooked[..]).unwrap(), None, "declared cooked");
        let track = DataTrack { sector_size: 2352, ..track };
        assert_eq!(super::track(&track, &cooked[..]).unwrap(), None, "declared raw, and is not");
        assert_eq!(image(&cooked[..]).unwrap(), None, "a cooked image that is no DVD");
    }

    /// A PlayStation 2 DVD's volume recognition sequence, as the three checked
    /// open: ISO 9660's descriptors, then UDF's.
    #[test]
    fn a_udf_bridge_makes_a_dvd_and_it_is_hashed_whole() {
        let mut dvd = vec![0u8; 40 * USER_DATA];
        for (sector, id) in [(16, b"CD001"), (17, b"CD001"), (18, b"BEA01"), (19, b"NSR02"), (20, b"TEA01")] {
            dvd[sector * USER_DATA + 1..sector * USER_DATA + 6].copy_from_slice(id);
        }
        let found = image(&dvd[..]).unwrap().expect("a DVD");
        assert_eq!(found, whole(&dvd[..]).unwrap());
        assert_eq!(found.size, dvd.len() as u64);

        dvd[19 * USER_DATA + 1..19 * USER_DATA + 6].copy_from_slice(b"CD001");
        assert_eq!(image(&dvd[..]).unwrap(), None, "the same image without UDF is a CD's");
    }

    #[test]
    fn a_gamecube_disc_is_a_dvd() {
        let mut gc = b"GAFE01".to_vec();
        gc.resize(0x1C, 0);
        gc.extend_from_slice(&0xC233_9F3Du32.to_be_bytes());
        gc.resize(8 * USER_DATA, 0x11);
        assert_eq!(image(&gc[..]).unwrap().map(|found| found.size), Some(gc.len() as u64));

        let mut rvz = b"RVZ\x01".to_vec();
        rvz.resize(0x58, 0);
        rvz.extend_from_slice(&gc[..0x80]);
        rvz.resize(8 * USER_DATA, 0);
        assert_eq!(image(&rvz[..]).unwrap(), None, "the container is not the disc's bytes");
    }
}
