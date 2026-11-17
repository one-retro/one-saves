//! The `disc-data` fingerprint: a disc's program, whatever holds it.
//!
//! 1Retro keys a disc's saves on this value. It names the program on the disc,
//! so it is the same for every copy of that program and different for anything
//! that changes it, and four rules make it so.
//!
//! **Every data track, whole.** A disc can carry more than one: a PC Engine CD
//! commonly keeps a second program near its end, and a fan translation patches
//! that one as well. Hashing the opening sectors of the first data track, as
//! 1Retro's earlier fingerprint did, gives an original and its translations one
//! value between them.
//!
//! **User data only, 2048 bytes a sector.** A raw sector's other 304 bytes are
//! its sync pattern, its address, and error correction computed from the rest;
//! on Mode 2 a subheader, and the tail of a sector that streams audio or video.
//! None of that is what the program is, and keeping it would tie the value to
//! the container: an `.iso` has thrown it away, a CHD keeps it or not. Taken
//! this way, a cue sheet with a file per track or one image of them all, an
//! `.iso`, and a CHD raw or cooked give the same value for one disc.
//!
//! **No audio.** Saves follow the program and not its music, and audio is
//! where copies of one disc part ways: a drive's read offset shifts every
//! sample, and plenty of sets keep their music as MP3. A mod that changes only
//! the music shares its original's value, which for saves is right.
//!
//! **Empty sectors at a track's edges do not count.** Whether a track ends
//! with its two-second postgap, or a tool counted the next track's silence into
//! it, differs between dumps of one disc, and between Redump's own pressings of
//! one. Sectors of nothing but zeros at either end of a track are left out;
//! inside it they count like any other.
//!
//! Measured over a library of 4960 dumps across seven systems: no two different
//! discs shared a value, copies of one disc in different containers agreed, and
//! without the last rule fifteen discs would each have had two.
//!
//! # The value
//!
//! Each data track that holds anything, in disc order, comes to a
//! [`TrackData`]: how many sectors it spans, from the first holding a non-zero
//! byte to the last, and the SHA-1 of their user data. The fingerprint is
//! SHA-256 over [`DOMAIN`] and then, a track at a time, its sector count as
//! eight bytes little-endian and its SHA-1, so it can be computed again from the
//! per-track values alone ([`DiscData::from_tracks`]). A DVD is one track, its
//! whole image.
//!
//! # Reading a disc
//!
//! - [`sheet`] for a cue sheet, opening the files it names through the caller.
//! - [`image`] for an image that came without one: an `.iso`, a DVD, a raw
//!   image of one data track.
//! - `chd::disc_data`, with the `chd` feature, for a CHD.
//!
//! Each reads forward from a plain [`Read`](io::Read), since a track may be
//! coming out of an archive, and each streams: nothing here holds more than a
//! buffer of a disc.

use std::io::{self, Read as _};

use sha1::{Digest as _, Sha1};
use sha2::Sha256;

use crate::cue;
use crate::sector::{RAW, SYNC, USER_DATA};

/// What the fingerprint's hash opens with, so that it is never mistaken for a
/// hash of anything else, and so a change to what it covers can say so.
pub const DOMAIN: &[u8] = b"disc-data v1\0";

/// A sector of zeros: what an empty sector inside a track is hashed as.
const EMPTY: [u8; USER_DATA] = [0; USER_DATA];

/// Sectors read at a time.
const BATCH: usize = 256;

/// How much of an image [`image`] looks at before reading it: enough to tell a
/// Dolphin container from the disc it holds.
const HEAD: usize = 32 * USER_DATA;

/// One data track's share of the fingerprint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TrackData {
    /// Sectors hashed: from the track's first sector holding a non-zero byte to
    /// its last.
    pub sectors: u64,
    /// SHA-1 of those sectors' user data, 2048 bytes each.
    pub sha1: [u8; 20],
}

impl TrackData {
    /// The SHA-1 as forty lower-case hex digits.
    #[must_use]
    pub fn sha1_hex(&self) -> String {
        crate::hex(&self.sha1)
    }
}

/// A disc's `disc-data` fingerprint, and the tracks it was taken over.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DiscData {
    /// SHA-256 over [`DOMAIN`] and every track's sector count and SHA-1.
    pub digest: [u8; 32],
    /// Every data track that holds anything, in disc order.
    pub tracks: Vec<TrackData>,
}

impl DiscData {
    /// The fingerprint of these tracks, in this order, or `None` for none: a
    /// disc with nothing in its data tracks has no program to name.
    ///
    /// What the readers here finish with, and what a catalogue that keeps the
    /// per-track values computes the fingerprint again from.
    #[must_use]
    pub fn from_tracks(tracks: Vec<TrackData>) -> Option<Self> {
        if tracks.is_empty() {
            return None;
        }
        let mut hasher = Sha256::new();
        hasher.update(DOMAIN);
        for track in &tracks {
            hasher.update(track.sectors.to_le_bytes());
            hasher.update(track.sha1);
        }
        Some(DiscData { digest: hasher.finalize().into(), tracks })
    }

    /// The fingerprint as sixty-four lower-case hex digits.
    #[must_use]
    pub fn hex(&self) -> String {
        crate::hex(&self.digest)
    }
}

/// How a track stores its sectors, and so where in each the user data is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Layout {
    /// 2352 bytes: the sync pattern, then a header whose mode byte says where
    /// user data starts, 16 bytes in for Mode 1 and 24 for Mode 2.
    Raw,
    /// 2048 bytes of user data and nothing else.
    Cooked,
    /// 2336 bytes: Mode 2 without its sync pattern and header, so user data
    /// starts 8 bytes in, after the subheader.
    Mode2,
    /// 2324 bytes: a Mode 2 Form 2 sector's user data alone.
    Form2,
}

impl Layout {
    /// The layout of a track a cue sheet declares `width` bytes a sector, or
    /// `None` for a width no data track is stored at.
    fn of_width(width: u64) -> Option<Self> {
        match width {
            2352 => Some(Layout::Raw),
            2048 => Some(Layout::Cooked),
            2336 => Some(Layout::Mode2),
            2324 => Some(Layout::Form2),
            _ => None,
        }
    }

    /// Bytes a sector.
    pub(crate) fn width(self) -> usize {
        match self {
            Layout::Raw => RAW,
            Layout::Cooked => USER_DATA,
            Layout::Mode2 => 2336,
            Layout::Form2 => 2324,
        }
    }

    /// A sector's user data, and whether the sector was framed as data.
    ///
    /// Mode 2 puts an eight-byte subheader ahead of its user data, Form 1 and
    /// Form 2 alike; a Form 2 sector's user data runs on past 2048 bytes, and
    /// the rest of it is left out with everything else that is not the first
    /// 2048. A raw sector without the sync pattern is read where Mode 1 keeps
    /// its data: every container of a raw track holds those bytes alike, and
    /// in a data track they are silence the edges drop.
    fn user_data(self, sector: &[u8]) -> (&[u8], bool) {
        match self {
            Layout::Raw => {
                let framed = sector.starts_with(&SYNC);
                let at = if framed && sector[15] == 2 { 24 } else { 16 };
                (&sector[at..at + USER_DATA], framed)
            }
            Layout::Cooked | Layout::Form2 => (&sector[..USER_DATA], true),
            Layout::Mode2 => (&sector[8..8 + USER_DATA], true),
        }
    }
}

/// One track's user data, hashed a sector at a time, leaving out empty sectors
/// at either end.
struct Hasher {
    sha1: Sha1,
    /// Sectors hashed so far, from the first that held anything.
    sectors: u64,
    /// Empty sectors since the last that held anything: hashed when one that
    /// holds something follows, dropped when the track ends first.
    pending: u64,
}

impl Hasher {
    fn new() -> Self {
        Hasher { sha1: Sha1::new(), sectors: 0, pending: 0 }
    }

    fn push(&mut self, user: &[u8]) {
        if user.iter().all(|&byte| byte == 0) {
            // Ahead of the first sector holding anything, an empty one is the
            // track's leading edge, and goes.
            if self.sectors > 0 {
                self.pending += 1;
            }
            return;
        }
        for _ in 0..self.pending {
            self.sha1.update(EMPTY);
        }
        self.sectors += self.pending + 1;
        self.pending = 0;
        self.sha1.update(user);
    }

    fn finish(self) -> Option<TrackData> {
        (self.sectors > 0).then(|| TrackData { sectors: self.sectors, sha1: self.sha1.finalize().into() })
    }
}

/// What reading a track found.
pub(crate) struct Reading {
    /// The track's data, or `None` where it held nothing.
    pub(crate) track: Option<TrackData>,
    /// Bytes read, a partial sector at the end included.
    pub(crate) bytes: u64,
    /// Whether a sector of a raw track had no sync pattern and was not silent
    /// either: music, where a track of data was expected.
    pub(crate) music: bool,
}

/// Hash the sectors `reader` holds, `layout` wide, to its end. A partial
/// sector at the end is read and not hashed.
pub(crate) fn read_track<R: io::Read>(mut reader: R, layout: Layout) -> io::Result<Reading> {
    let width = layout.width();
    let mut buf = vec![0u8; BATCH * width];
    let mut hasher = Hasher::new();
    let mut bytes = 0u64;
    let mut music = false;
    loop {
        let read = crate::read_upto(&mut reader, &mut buf)?;
        bytes += read as u64;
        for sector in buf[..read].chunks_exact(width) {
            let (user, framed) = layout.user_data(sector);
            music |= !framed && sector.iter().any(|&byte| byte != 0);
            hasher.push(user);
        }
        if read < buf.len() {
            return Ok(Reading { track: hasher.finish(), bytes, music });
        }
    }
}

/// The fingerprint of the disc a cue sheet describes.
///
/// `open` is handed each file the sheet names that holds a data track, spelled
/// exactly as the sheet spells it, and returns a reader from that file's first
/// byte. A file of nothing but audio is never asked for. The names are the
/// sheet's say and as untrusted as the rest of a dump: find each with
/// [`cue::resolve`] against the sheet's own directory, or look it up in the
/// archive the sheet came out of.
///
/// Reads forward only. Where data tracks share one image, it is opened once
/// and read through from the first to the end of the last, music between them
/// included; where each has its own file, only those files are read.
///
/// `Ok(None)` for a sheet naming no data, or only data tracks of nothing but
/// zeros; for a data track stored at a width no data track has; and for one
/// that starts past the end of its file, which is a dump with a track missing.
/// A track its file ends partway through is hashed as far as it goes. An error
/// is a file that could not be opened or read.
///
/// ```no_run
/// use std::fs::File;
/// use std::path::Path;
///
/// use disc_id::{cue, disc_data};
///
/// let sheet = Path::new("Game.cue");
/// let dir = sheet.parent().unwrap();
/// let text = std::fs::read_to_string(sheet)?;
/// let found = disc_data::sheet(&text, |name| match cue::resolve(dir, name) {
///     Some(path) => File::open(path),
///     None => Err(std::io::Error::other(format!("{name} is not beside the sheet"))),
/// })?;
/// if let Some(found) = found {
///     println!("{}", found.hex());
/// }
/// # Ok::<(), std::io::Error>(())
/// ```
pub fn sheet<F, R>(text: &str, mut open: F) -> io::Result<Option<DiscData>>
where
    F: FnMut(&str) -> io::Result<R>,
    R: io::Read,
{
    let mut found = Vec::new();
    // The file being read, and how far into it the reading is.
    let mut current: Option<(String, R, u64)> = None;
    for track in cue::data_tracks(text) {
        let Some(layout) = Layout::of_width(track.sector_size) else { return Ok(None) };
        let further_on =
            matches!(&current, Some((file, _, at)) if *file == track.file && *at <= track.offset);
        if !further_on {
            current = Some((track.file.clone(), open(&track.file)?, 0));
        }
        let Some((_, file, at)) = current.as_mut() else { continue };
        // Read up to it rather than seek: an archive member cannot.
        let skip = track.offset - *at;
        let skipped = io::copy(&mut file.by_ref().take(skip), &mut io::sink())?;
        *at += skipped;
        if skipped < skip {
            return Ok(None);
        }
        let read = read_track(file.by_ref().take(track.length.unwrap_or(u64::MAX)), layout)?;
        *at += read.bytes;
        found.extend(read.track);
    }
    Ok(DiscData::from_tracks(found))
}

/// The fingerprint of a disc image that came without a cue sheet.
///
/// The image is one track: an `.iso` of 2048-byte sectors, a DVD's image
/// whole, which is every DVD dump, or a raw image of one data track, each
/// sector read by its own mode byte. Whether the bytes are a disc at all is
/// the caller's to have settled, as [`is_disc_image`](crate::is_disc_image)
/// does: hashed as sectors, anything gives a value.
///
/// `Ok(None)` for an image of nothing but zeros; for a raw image with music in
/// it, since without a sheet nothing says where its data would end; and for a
/// Dolphin WIA or RVZ container, which holds a disc compressed rather than its
/// bytes. A partial sector at the end is not hashed.
pub fn image<R: io::Read>(mut file: R) -> io::Result<Option<DiscData>> {
    let mut head = vec![0u8; HEAD];
    let read = crate::read_upto(&mut file, &mut head)?;
    head.truncate(read);
    if crate::wia_disc_head(&head).is_some() {
        return Ok(None);
    }
    let layout = if head.starts_with(&SYNC) { Layout::Raw } else { Layout::Cooked };
    let read = read_track(io::Cursor::new(head).chain(file), layout)?;
    if read.music {
        return Ok(None);
    }
    Ok(read.track.and_then(|track| DiscData::from_tracks(vec![track])))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A raw sector: sync, an address, `mode`, then `user` where that mode
    /// keeps it, and filler where error correction would be.
    fn raw(mode: u8, user: &[u8]) -> Vec<u8> {
        let mut sector = SYNC.to_vec();
        sector.extend_from_slice(&[0, 2, 0, mode]);
        if mode == 2 {
            // A subheader, twice over as the format writes it.
            sector.extend_from_slice(&[1, 0, 8, 0, 1, 0, 8, 0]);
        }
        sector.extend_from_slice(user);
        sector.resize(RAW, 0xEC);
        sector
    }

    fn user(n: u8) -> Vec<u8> {
        (0..USER_DATA).map(|i| u8::try_from(i % 256).unwrap().wrapping_mul(n).wrapping_add(n)).collect()
    }

    fn cooked(sectors: &[Vec<u8>]) -> Vec<u8> {
        sectors.concat()
    }

    fn of(bytes: &[u8]) -> DiscData {
        image(bytes).unwrap().expect("a fingerprint")
    }

    /// The definition, against Python's `hashlib` over the same bytes:
    /// `sha1(b"".join(user(n) for n in (1, 2, 3)))`, then
    /// `sha256(b"disc-data v1\0" + (3).to_bytes(8, "little") + that sha1)`.
    #[test]
    fn the_value_is_sha256_over_each_tracks_count_and_sha1() {
        let found = of(&cooked(&[user(1), user(2), user(3)]));
        assert_eq!(found.tracks.len(), 1);
        assert_eq!(found.tracks[0].sectors, 3);
        assert_eq!(found.tracks[0].sha1_hex(), "fe1855c293678bf482873381423dfd2ab4fd764b");
        assert_eq!(found.hex(), "5faf56a1f71cc76d81f6adb4bbbc24ce9381af84ef00721563657fbd5fd339ba");
        assert_eq!(DiscData::from_tracks(found.tracks.clone()), Some(found));
    }

    /// Mode 1 and Mode 2 keep the same 2048 bytes in different places, and a
    /// cooked image keeps them alone: all three are one disc.
    #[test]
    fn raw_mode_1_mode_2_and_cooked_agree() {
        let sectors = [user(1), user(2), user(3)];
        let cooked = of(&cooked(&sectors));
        let mode1: Vec<u8> = sectors.iter().flat_map(|s| raw(1, s)).collect();
        let mode2: Vec<u8> = sectors.iter().flat_map(|s| raw(2, s)).collect();
        assert_eq!(of(&mode1), cooked);
        assert_eq!(of(&mode2), cooked);
    }

    /// Mode 2 without its sync and header, 2336 bytes a sector, and Form 2's
    /// user data alone, 2324: the same 2048 bytes again.
    #[test]
    fn the_narrower_mode_2_widths_agree_too() {
        let sectors = [user(4), user(5)];
        let expected = of(&cooked(&sectors));
        let mode2: Vec<u8> = sectors.iter().flat_map(|s| raw(2, s)[16..16 + 2336].to_vec()).collect();
        let form2: Vec<u8> = sectors
            .iter()
            .flat_map(|s| {
                let mut sector = s.clone();
                sector.resize(2324, 0x77);
                sector
            })
            .collect();
        assert_eq!(read_track(&mode2[..], Layout::Mode2).unwrap().track, Some(expected.tracks[0]));
        assert_eq!(read_track(&form2[..], Layout::Form2).unwrap().track, Some(expected.tracks[0]));
    }

    /// Empty sectors at either end of a track do not count, and inside it they
    /// do: a postgap is not the program, a run of zeros within it is.
    #[test]
    fn only_the_edges_are_trimmed() {
        let core = of(&cooked(&[user(1), user(2)]));
        let padded = cooked(&[vec![0; USER_DATA], user(1), user(2), vec![0; USER_DATA], vec![0; USER_DATA]]);
        assert_eq!(of(&padded), core);

        let gapped = of(&cooked(&[user(1), vec![0; USER_DATA], user(2)]));
        assert_ne!(gapped, core);
        assert_eq!(gapped.tracks[0].sectors, 3);
    }

    /// A raw track that ends in silence written as audio, as a track measured
    /// up to the next one's `INDEX 01` does, is the same track.
    #[test]
    fn unframed_silence_is_an_empty_sector() {
        let mut bytes: Vec<u8> = [user(1), user(2)].iter().flat_map(|s| raw(1, s)).collect();
        let core = of(&bytes);
        bytes.extend(vec![0u8; 3 * RAW]);
        assert_eq!(of(&bytes), core);
    }

    /// Without a sheet, nothing says where the data in an image with music in
    /// it ends.
    #[test]
    fn a_raw_image_with_music_has_no_fingerprint() {
        let mut bytes = raw(1, &user(1));
        bytes.extend((0..RAW).map(|i| u8::try_from(i % 251).unwrap()));
        assert_eq!(image(&bytes[..]).unwrap(), None);
    }

    #[test]
    fn nothing_but_zeros_is_no_program() {
        assert_eq!(image(&vec![0u8; 4 * USER_DATA][..]).unwrap(), None);
        assert_eq!(image(&[][..]).unwrap(), None);
        assert_eq!(DiscData::from_tracks(Vec::new()), None);
    }

    /// The part of a sector past the last whole one is not a sector.
    #[test]
    fn a_partial_sector_at_the_end_is_not_hashed() {
        let whole = cooked(&[user(1), user(2)]);
        let mut ragged = whole.clone();
        ragged.extend_from_slice(&[9; 100]);
        assert_eq!(of(&ragged), of(&whole));
    }

    #[test]
    fn a_dolphin_container_is_not_its_disc() {
        let mut rvz = b"RVZ\x01".to_vec();
        rvz.resize(8 * USER_DATA, 0x11);
        assert_eq!(image(&rvz[..]).unwrap(), None);
    }
}
