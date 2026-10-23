//! Reading a disc image out of a CHD.
//!
//! CHD stores the image in compressed hunks, so unlike every other container
//! here the bytes cannot simply be looked at. That is why this is behind a
//! feature: it brings four decompressors — zlib, LZMA, zstd, and FLAC for audio
//! tracks — and without it the crate depends on nothing outside this workspace.
//!
//! Two things make a CHD not quite the image it holds. Hunks are compressed, so
//! reaching sector 16 means inflating the two or three that precede it. And a
//! **CD** CHD stores each sector as a 2352-byte frame followed by 96 bytes of
//! subcode, so the image only reappears once the subcode is dropped. A CHD of
//! anything else — a DVD, a hard disk — has no subcode and passes through.
//!
//! Bounded reading survives all of it: identification still stops after a
//! handful of hunks rather than inflating a disc.

use std::io::{self, Read, Seek, SeekFrom};
use std::ops::ControlFlow;

use chd::Chd;
use chd::metadata::MetadataTag;

use crate::DiscId;

/// Bytes of a CD frame, sector data and subcode together.
const CD_FRAME: usize = 2448;

/// Bytes of sector data in a CD frame; the remainder is subcode.
const CD_SECTOR: usize = 2352;

/// Tracks are stored padded to a multiple of this many frames.
const TRACK_PADDING: usize = 4;

/// Bytes of user data in a cooked sector, which is what a non-`_RAW` track
/// stores at the head of its frame.
const COOKED_SECTOR: usize = 2048;

/// The largest hunk `chdman` writes, and so the largest read here.
const MAX_HUNK: u64 = 1024 * 1024;

/// Refuse a CHD whose header declares a hunk larger than [`MAX_HUNK`], before
/// the `chd` crate sees it.
///
/// Opening a CHD sizes a buffer per codec by the hunk, and the crate checks
/// that size on every header version but the fifth, which is the one chdman
/// writes. Left to it, a forged header of a hundred bytes is an allocation of
/// up to 4 GiB. A header this cannot read is left for the crate to refuse.
fn refuse_oversized_hunks<F: Read + Seek>(file: &mut F) -> io::Result<()> {
    let mut header = [0u8; 124];
    file.seek(SeekFrom::Start(0))?;
    let read = crate::read_upto(file, &mut header)?;
    file.seek(SeekFrom::Start(0))?;
    let header = &header[..read];
    let u32_at =
        |at: usize| header.get(at..at + 4).map(|b| u64::from(u32::from_be_bytes([b[0], b[1], b[2], b[3]])));
    if !header.starts_with(b"MComprHD") {
        return Ok(());
    }
    // Versions 1 and 2 count a hunk in sectors, of 512 bytes or of the width
    // version 2 states; the later ones count it in bytes.
    let hunk = match u32_at(12) {
        Some(1) => u32_at(24).map(|sectors| sectors * 512),
        Some(2) => u32_at(24).zip(u32_at(76)).map(|(sectors, width)| sectors * width),
        Some(3) => u32_at(76),
        Some(4) => u32_at(44),
        Some(5) => u32_at(56),
        _ => None,
    };
    match hunk {
        Some(hunk) if hunk > MAX_HUNK => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("a CHD hunk of {hunk} bytes, where chdman writes at most {MAX_HUNK}"),
        )),
        _ => Ok(()),
    }
}

/// Where the data track starts, and how much of each frame is its data.
///
/// **Track one is not always the data track.** A PC Engine CD commonly opens
/// with audio and puts the program on track two, so starting at frame zero
/// hands the identifier music. The `CHT2` metadata names each track's type, so
/// this walks it and returns where the first `MODE1` or `MODE2` track begins.
///
/// Zero for the ordinary case — Sega CD, PlayStation, Saturn all put data on
/// track one — so this changes nothing for them.
fn data_track(entries: &[String]) -> (usize, usize) {
    let mut frames = 0usize;
    for entry in entries {
        let field =
            |name: &str| -> Option<&str> { entry.split_whitespace().find_map(|f| f.strip_prefix(name)) };
        let kind = field("TYPE:").unwrap_or("");
        if kind.starts_with("MODE") {
            // A `PGTYPE` beginning with `V` means the pregap is *stored* ahead
            // of the track rather than synthesised, so it has to be stepped
            // over too. Verified against two PC Engine discs: one `MODE1` with
            // `PGTYPE:MODE1` and one `MODE1_RAW` with `PGTYPE:VMODE1_RAW`, and
            // only the second carries its 225 frames in the file.
            if field("PGTYPE:").is_some_and(|t| t.starts_with('V')) {
                frames += field("PREGAP:").and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
            }
            // A `_RAW` track keeps the whole 2352-byte sector, sync header and
            // all; a cooked one stores only its 2048 bytes of user data at the
            // head of the frame. Taking the wrong width leaves the image with a
            // stride the sector layer cannot infer, so every lookup past the
            // first lands in the wrong place.
            let width = if kind.ends_with("_RAW") { CD_SECTOR } else { COOKED_SECTOR };
            return (frames, width);
        }
        let count: usize = field("FRAMES:").and_then(|v| v.parse().ok()).unwrap_or(0);
        // `PAD` states the padding outright where the writer recorded it, and
        // a GD-ROM's is not the rounding a CD's is: Shenmue's opening track is
        // `FRAMES:450 PAD:150`, where the rule below would guess 452 and put
        // every later lookup 148 frames out. Trust the number when it is there.
        frames += match field("PAD:").and_then(|v| v.parse::<usize>().ok()) {
            Some(pad) => count + pad,
            // chdman pads each stored track out to a whole number of groups.
            None => count.div_ceil(TRACK_PADDING) * TRACK_PADDING,
        };
    }
    // No data track named: read from the start rather than refusing outright.
    (0, CD_SECTOR)
}

fn other(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}

/// A CHD's first data track, decompressed on demand.
///
/// Hunks hold several frames each and are compressed, so nothing can be looked
/// at until the hunk holding it is inflated. This grows a buffer of the
/// **image** rather than the file: subcode dropped, frames belonging to earlier
/// tracks skipped, so what comes out is what a `.bin` of the same track would
/// hold and the sector layer can read it without knowing a CHD was involved.
struct Image<F: Read + Seek> {
    chd: Chd<F>,
    hunk_count: u32,
    next: u32,
    /// Whether frames carry subcode to drop, which a DVD or hard disk CHD
    /// does not.
    cd: bool,
    sector_width: usize,
    drop_frames: usize,
    compressed: Vec<u8>,
    hunk_buf: Vec<u8>,
    bytes: Vec<u8>,
}

impl<F: Read + Seek> Image<F> {
    /// Open a CHD and position on the first data track.
    fn open(mut file: F) -> io::Result<Self> {
        refuse_oversized_hunks(&mut file)?;
        let mut chd = Chd::open(file, None).map_err(other)?;
        let hunk_size = chd.header().hunk_size() as usize;
        let hunk_count = chd.header().hunk_count();
        let cd = chd.header().unit_bytes() as usize == CD_FRAME;

        // Where the program lives, which is not always where the disc starts.
        let tracks: Vec<String> = {
            let refs: Vec<_> = chd.metadata_refs().collect();
            let mut out = Vec::new();
            for r in refs {
                // Three tags carry the same track table in the same format,
                // and which one a file has depends on what wrote it: `CHT2` is
                // current chdman, `CHTR` is what older versions wrote, and
                // `CHGD` is a GD-ROM, which is every Dreamcast dump. Reading
                // only `CHT2` left the other two with no track table at all,
                // so the reader fell back to frame zero and handed the
                // identifier whatever track one happened to be -- music, on a
                // disc that opens with audio.
                let tag = r.metatag().to_be_bytes();
                if matches!(&tag, b"CHT2" | b"CHTR" | b"CHGD")
                    && let Ok(m) = r.read(chd.inner())
                {
                    out.push(String::from_utf8_lossy(&m.value).trim_end_matches('\0').to_string());
                }
            }
            out
        };
        let (skip_frames, sector_width) = if cd { data_track(&tracks) } else { (0, CD_SECTOR) };

        // Start at the hunk holding the data track, then drop the frames of it
        // that still belong to whatever came before.
        let frames_per_hunk = hunk_size / CD_FRAME.max(1);
        let (next, drop_frames) = if cd && frames_per_hunk > 0 {
            (
                // A hunk past `u32` is past every hunk a CHD can hold, so reading from it finds
                // nothing and the disc goes unidentified, as one that short would.
                u32::try_from(skip_frames / frames_per_hunk).unwrap_or(u32::MAX),
                skip_frames % frames_per_hunk,
            )
        } else {
            (0, 0)
        };

        Ok(Image {
            chd,
            hunk_count,
            next,
            cd,
            sector_width,
            drop_frames,
            compressed: Vec::new(),
            hunk_buf: vec![0u8; hunk_size],
            bytes: Vec::new(),
        })
    }

    /// Whether every hunk has been read.
    fn exhausted(&self) -> bool {
        self.next >= self.hunk_count
    }

    /// The image decompressed so far.
    fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Inflate until the image holds `want` bytes, or the CHD runs out.
    fn grow_to(&mut self, want: usize) -> io::Result<()> {
        while self.bytes.len() < want && !self.exhausted() {
            self.chd
                .hunk(self.next)
                .map_err(other)?
                .read_hunk_in(&mut self.compressed, &mut self.hunk_buf)
                .map_err(other)?;
            self.next += 1;
            if self.cd {
                // Keep the sector, drop the subcode that follows it.
                for frame in self.hunk_buf.chunks(CD_FRAME) {
                    if self.drop_frames > 0 {
                        self.drop_frames -= 1;
                        continue;
                    }
                    let take = self.sector_width.min(frame.len());
                    self.bytes.extend_from_slice(&frame[..take]);
                }
            } else {
                let hunk = std::mem::take(&mut self.hunk_buf);
                self.bytes.extend_from_slice(&hunk);
                self.hunk_buf = hunk;
            }
        }
        Ok(())
    }
}

/// Identify the disc inside a CHD.
///
/// Inflates only as far as identification asks, which for every system here is
/// a few hunks. `Ok(None)` means the image matches nothing recognised; an error
/// means the CHD itself could not be read.
pub fn identify<F: Read + Seek>(file: F) -> io::Result<Option<DiscId>> {
    let mut image = Image::open(file)?;
    let mut want = 64 * 1024;
    loop {
        image.grow_to(want)?;
        match crate::identify_prefix(image.bytes()) {
            ControlFlow::Break(found) => return Ok(found),
            // More was asked for than the hunks can supply: this is the disc.
            ControlFlow::Continue(more) if more > image.bytes().len() && !image.exhausted() => {
                want = more;
            }
            ControlFlow::Continue(_) => return Ok(None),
        }
    }
}

/// The user data of however many of the first `max_sectors` the disc holds.
///
/// The CHD counterpart of [`crate::sector::user_data_upto`], for the same
/// reason: a data track shorter than the window is a real disc and should be
/// read, not refused.
pub fn user_data_upto<F: Read + Seek>(file: F, max_sectors: usize) -> io::Result<Vec<u8>> {
    let mut image = Image::open(file)?;
    // An upper bound: raw sectors are the widest a track can be.
    let want = max_sectors.saturating_mul(CD_SECTOR);
    // Grow once to the bound; a short disc simply exhausts before reaching it.
    image.grow_to(want)?;
    while !image.exhausted() && image.bytes().len() < want {
        image.grow_to(want)?;
    }
    Ok(crate::sector::user_data_upto(image.bytes(), max_sectors))
}

/// The user data of the first `sectors` sectors of the disc inside a CHD.
///
/// The CHD counterpart of [`crate::sector::user_data_prefix`], and it returns
/// the identical bytes: a game stored as a CHD and the same game stored as a
/// cue and bin both come out as plain 2048-byte sectors. That is what makes a
/// fingerprint taken over these bytes survive a change of container.
///
/// `Ok(None)` when the disc is shorter than the sectors asked for.
pub fn user_data_prefix<F: Read + Seek>(file: F, sectors: usize) -> io::Result<Option<Vec<u8>>> {
    let mut image = Image::open(file)?;
    // An upper bound: raw sectors are the widest a track can be.
    let mut want = sectors.saturating_mul(CD_SECTOR);
    loop {
        image.grow_to(want)?;
        match crate::sector::user_data_prefix(image.bytes(), sectors) {
            ControlFlow::Break(bytes) => return Ok(Some(bytes)),
            ControlFlow::Continue(more) if more > image.bytes().len() && !image.exhausted() => {
                want = more;
            }
            ControlFlow::Continue(_) => return Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CD_SECTOR, COOKED_SECTOR, MAX_HUNK, data_track, identify};

    fn entries(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|s| (*s).to_string()).collect()
    }

    /// The ordinary case: data on track one, nothing skipped.
    #[test]
    fn a_disc_whose_first_track_is_data_starts_at_zero() {
        let tracks = entries(&[
            "TRACK:1 TYPE:MODE1 SUBTYPE:NONE FRAMES:10149 PREGAP:0",
            "TRACK:2 TYPE:AUDIO SUBTYPE:NONE FRAMES:7984 PREGAP:150",
        ]);
        assert_eq!(data_track(&tracks), (0, COOKED_SECTOR));
    }

    /// Read from a real PC Engine CD: audio first, program on track two. Its
    /// 3665 frames pad up to a multiple of four.
    #[test]
    fn audio_before_the_program_is_skipped() {
        let tracks = entries(&[
            "TRACK:1 TYPE:AUDIO SUBTYPE:NONE FRAMES:3665 PREGAP:0",
            "TRACK:2 TYPE:MODE1 SUBTYPE:NONE FRAMES:10149 PREGAP:225 PGTYPE:MODE1",
        ]);
        assert_eq!(data_track(&tracks), (3668, COOKED_SECTOR), "3665 padded to 3668");
    }

    /// A GD-ROM states its padding rather than leaving it to be inferred, and
    /// its padding is not the CD rounding rule: Shenmue's opening track is
    /// `FRAMES:450 PAD:150`, where rounding to a multiple of four would guess
    /// 452 and leave every later lookup 148 frames short.
    #[test]
    fn stated_padding_is_used_rather_than_inferred() {
        let tracks = entries(&[
            "TRACK:1 TYPE:AUDIO SUBTYPE:NONE FRAMES:450 PAD:150 PREGAP:0",
            "TRACK:2 TYPE:MODE1_RAW SUBTYPE:NONE FRAMES:1000 PAD:0 PREGAP:0",
        ]);
        assert_eq!(data_track(&tracks), (600, CD_SECTOR), "450 + 150 stated");
    }

    /// Without a `PAD` field the rounding rule still applies, so a CD track
    /// table written by an older chdman reads the same as it always did.
    #[test]
    fn padding_falls_back_to_the_rounding_rule() {
        let tracks = entries(&[
            "TRACK:1 TYPE:AUDIO SUBTYPE:NONE FRAMES:450 PREGAP:0",
            "TRACK:2 TYPE:MODE1 SUBTYPE:NONE FRAMES:1000 PREGAP:0",
        ]);
        assert_eq!(data_track(&tracks), (452, COOKED_SECTOR), "450 rounded to 452");
    }

    /// The other real disc: its pregap *is* in the file, so the program starts
    /// 225 frames later than the track table alone would suggest. Getting this
    /// wrong reads 225 frames of silence as the boot header.
    #[test]
    fn a_stored_pregap_is_stepped_over_too() {
        let tracks = entries(&[
            "TRACK:1 TYPE:AUDIO SUBTYPE:NONE FRAMES:3009 PREGAP:0 PGTYPE:MODE1",
            "TRACK:2 TYPE:MODE1_RAW SUBTYPE:NONE FRAMES:3446 PREGAP:225 PGTYPE:VMODE1_RAW",
        ]);
        assert_eq!(data_track(&tracks), (3012 + 225, CD_SECTOR), "a _RAW track keeps its sync header");
    }

    /// An all-audio disc has no program to find; reading from the start is a
    /// better answer than refusing, and identification will simply say no.
    #[test]
    fn a_disc_with_no_data_track_reads_from_the_start() {
        let tracks = entries(&["TRACK:1 TYPE:AUDIO SUBTYPE:NONE FRAMES:100 PREGAP:0"]);
        assert_eq!(data_track(&tracks).0, 0);
    }

    /// A version 5 header, the one the `chd` crate does not check, declaring
    /// a hunk of `hunk` bytes.
    fn v5_header(hunk: u32) -> Vec<u8> {
        let mut header = vec![0u8; 124];
        header[..8].copy_from_slice(b"MComprHD");
        header[8..12].copy_from_slice(&124u32.to_be_bytes());
        header[12..16].copy_from_slice(&5u32.to_be_bytes());
        header[32..40].copy_from_slice(&u64::from(hunk).to_be_bytes());
        header[40..48].copy_from_slice(&124u64.to_be_bytes());
        header[56..60].copy_from_slice(&hunk.to_be_bytes());
        header[60..64].copy_from_slice(&2448u32.to_be_bytes());
        header
    }

    #[test]
    fn a_hunk_larger_than_chdman_writes_is_refused_before_opening() {
        for hunk in [u32::try_from(MAX_HUNK).unwrap() + 1, u32::MAX] {
            let error = identify(std::io::Cursor::new(v5_header(hunk))).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
            assert!(error.to_string().contains("chdman writes at most"), "{error}");
        }
        // At the limit the header is the crate's to judge, and this one is not a whole CHD.
        let error = identify(std::io::Cursor::new(v5_header(19_584))).unwrap_err();
        assert!(!error.to_string().contains("chdman writes at most"), "{error}");
    }
}
