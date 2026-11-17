//! Following a cue sheet to the track that carries the header.
//!
//! A `.cue` is a text index: the bytes are in the files it names. **Track one
//! is not always the data track** — a PC Engine CD commonly opens with audio
//! and puts the program on track two — so the sheet has to be read for the
//! first `MODE` track rather than the first `FILE`.
//!
//! Sheets come in two shapes and this handles both. A multi-file sheet names a
//! file per track, and each track's `INDEX` is measured from the start of its
//! own file. A single-file sheet names one image holding every track, and the
//! `INDEX` says how far into it each begins.
//!
//! A sheet is text that came with the dump, so the names in it are as
//! untrusted as the rest of the dump: `FILE "/dev/zero"` and
//! `FILE "../../.ssh/id_ed25519"` parse like any other line. Nothing here opens
//! what a sheet names, and [`resolve`] is how to find it without leaving the
//! sheet's directory.

use std::path::{Component, Path, PathBuf};

/// Where a cue sheet's data track lives.
///
/// Two places a track can be said to start, and both are here. Its content
/// starts at `INDEX 01`, which is [`offset`](Self::offset) and where a header is
/// read from. Its file starts at `INDEX 00` where the sheet stores a pregap,
/// [`pregap`](Self::pregap) bytes earlier: that is where Redump cuts a track,
/// so a hash of the track runs from there to where the next track's pregap
/// begins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataTrack {
    /// The file naming it, exactly as the sheet spells it. Untrusted: find it
    /// with [`resolve`], against the sheet's own directory.
    pub file: String,
    /// How far into that file the track's content begins: its `INDEX 01`.
    ///
    /// Not always zero for a sheet that gives the track a file of its own: Redump
    /// stores a data track's pregap at the head of its file, so a PC Engine CD's
    /// program starts three seconds into `(Track 02).bin`.
    pub offset: u64,
    /// How many bytes of pregap the file holds ahead of [`offset`](Self::offset):
    /// from the track's `INDEX 00` to its `INDEX 01`, or zero where the sheet
    /// states no `INDEX 00` in this file.
    ///
    /// Part of the track as Redump dumps it, and nothing to read a header from.
    /// A `PREGAP` line is not counted, since it describes sectors the file does
    /// not hold.
    pub pregap: u64,
    /// How many bytes of that file the track occupies from
    /// [`offset`](Self::offset), or `None` when it runs to the end of it.
    ///
    /// A sheet that gives each track its own file leaves this `None`, which is
    /// the common shape. A sheet whose tracks share one image needs it: without
    /// a length, hashing "the data track" means hashing the whole disc image,
    /// audio and all, which matches no catalogue entry and differs from the same
    /// disc dumped track-per-file. It ends where the next track's pregap begins,
    /// or its content where it has none, which is where Redump ends the track.
    pub length: Option<u64>,
    /// Bytes per sector its `TRACK` line declares: 2352 for a raw track, which
    /// keeps the sync pattern, header and error correction, 2048 for one cooked
    /// to its user data, and 2336 for Mode 2 without sync and header.
    pub sector_size: u64,
}

/// Bytes per sector when a track line does not say.
const DEFAULT_SECTOR: u64 = 2352;

/// `MM:SS:FF` to a count of sectors. A CD runs at 75 frames a second.
///
/// `None` for a time past `u64`, which no disc reaches: it is as unreadable as
/// one that does not parse.
fn msf_to_frames(text: &str) -> Option<u64> {
    let mut parts = text.split(':');
    let m: u64 = parts.next()?.trim().parse().ok()?;
    let s: u64 = parts.next()?.trim().parse().ok()?;
    let f: u64 = parts.next()?.trim().parse().ok()?;
    m.checked_mul(60)?.checked_add(s)?.checked_mul(75)?.checked_add(f)
}

/// The declared sector size of a track line: `MODE1/2352` is 2352.
fn sector_size(mode: &str) -> u64 {
    mode.split_once('/').and_then(|(_, n)| n.trim().parse().ok()).unwrap_or(DEFAULT_SECTOR)
}

/// Strip the quotes a sheet puts around a filename, or take the first word.
fn file_name(rest: &str) -> Option<String> {
    let rest = rest.trim();
    let name = match rest.strip_prefix('"') {
        Some(inner) => inner.split('"').next()?,
        None => rest.split_whitespace().next()?,
    };
    (!name.is_empty()).then(|| name.to_string())
}

/// Every file a cue sheet names, in its order and exactly as it spells them.
///
/// Audio tracks included. A disc dumped as a sheet is the sheet and all of
/// these together, so none of them is a dump of its own: a data track opens
/// like a disc and an audio track like nothing at all, and neither is the
/// thing an emulator was pointed at.
///
/// The names are the sheet's say and nothing more: an absolute path, a `..`,
/// a device. Find each with [`resolve`] rather than joining it to a directory,
/// and check it is a regular file before opening it, since opening a FIFO
/// blocks until something writes to it.
#[must_use]
pub fn files(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let (word, rest) = line.trim().split_once(' ')?;
            word.eq_ignore_ascii_case("FILE").then(|| file_name(rest)).flatten()
        })
        .collect()
}

/// Where a file a sheet names sits, against the directory the sheet is in, or
/// `None` where the name reaches outside it.
///
/// Only a relative name of plain components resolves: no root, no drive, no
/// `..`. A sheet has no business naming anything but the tracks beside it, and
/// one that names `/dev/zero` or `../../.ssh/id_ed25519` is not describing a
/// disc. A `.` is allowed, since it goes nowhere.
///
/// This is about the name alone, and opens nothing. A symbolic link inside the
/// directory still leads wherever it points, and whether what is there is a
/// regular file is the caller's to check before reading it.
///
/// ```
/// use std::path::Path;
/// use disc_id::cue::resolve;
///
/// let dir = Path::new("games");
/// assert_eq!(resolve(dir, "Disc (Track 1).bin"), Some(dir.join("Disc (Track 1).bin")));
/// assert_eq!(resolve(dir, "../../etc/passwd"), None);
/// assert_eq!(resolve(dir, "/dev/zero"), None);
/// ```
#[must_use]
pub fn resolve(dir: &Path, name: &str) -> Option<PathBuf> {
    let name = Path::new(name);
    let mut plain = false;
    for component in name.components() {
        match component {
            Component::Normal(_) => plain = true,
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) | Component::ParentDir => return None,
        }
    }
    plain.then(|| dir.join(name))
}

/// The first data track a cue sheet names.
///
/// `None` when the sheet names no file, or names only audio — a music CD is
/// not a game disc, and saying so beats pointing at a track of songs.
#[must_use]
pub fn first_data_track(text: &str) -> Option<DataTrack> {
    let sheet = Sheet::read(text);
    sheet.tracks.iter().enumerate().filter(|(_, track)| track.data).find_map(|(at, _)| sheet.cut(at))
}

/// Every data track a cue sheet names, in its order, each cut as
/// [`first_data_track`] cuts the first.
///
/// A disc can carry more than one. A PC Engine CD commonly has a second copy of
/// its program near the end, and a translation patches that one too, so a
/// fingerprint of the first alone cannot tell the translation from the
/// original. A data track the sheet gives no `INDEX 01` is left out, since
/// nothing says where its content starts.
#[must_use]
pub fn data_tracks(text: &str) -> Vec<DataTrack> {
    let sheet = Sheet::read(text);
    sheet
        .tracks
        .iter()
        .enumerate()
        .filter(|(_, track)| track.data)
        .filter_map(|(at, _)| sheet.cut(at))
        .collect()
}

/// A `TRACK` line: whether it is data, and its sector width. Audio is always
/// 2352; only a `MODE` line states a width of its own.
struct Track {
    data: bool,
    sector: u64,
}

/// An `INDEX` line, in sheet order: which track it belongs to, which index it
/// is, and where in which file it points.
struct Point {
    track: usize,
    index: u32,
    file: String,
    at: u64,
}

/// A cue sheet's tracks and index points, as its lines state them.
struct Sheet {
    tracks: Vec<Track>,
    points: Vec<Point>,
}

impl Sheet {
    fn read(text: &str) -> Self {
        let mut tracks: Vec<Track> = Vec::new();
        let mut points: Vec<Point> = Vec::new();
        let mut file: Option<String> = None;

        for line in text.lines() {
            let line = line.trim();
            let (word, rest) = line.split_once(' ').unwrap_or((line, ""));

            match word.to_ascii_uppercase().as_str() {
                "FILE" => file = file_name(rest),
                "TRACK" => {
                    // `TRACK 02 MODE1/2352`
                    let mode = rest.split_whitespace().nth(1).unwrap_or("");
                    let data = mode.to_ascii_uppercase().starts_with("MODE");
                    let sector = if data { sector_size(mode) } else { crate::sector::RAW as u64 };
                    tracks.push(Track { data, sector });
                }
                "INDEX" => {
                    let Some(track) = tracks.len().checked_sub(1) else { continue };
                    let mut fields = rest.split_whitespace();
                    // Numerically: a CloneCD sheet writes `INDEX 1` where others
                    // write `INDEX 01`, and they mean the same index.
                    let Some(index) = fields.next().and_then(|n| n.parse::<u32>().ok()) else { continue };
                    // A sheet's sector width is its own say, so the product can
                    // overflow too, and is skipped like a time that does not parse.
                    let Some(at) = fields
                        .next()
                        .and_then(msf_to_frames)
                        .and_then(|frames| frames.checked_mul(tracks[track].sector))
                    else {
                        continue;
                    };
                    let Some(name) = file.clone() else { continue };
                    points.push(Point { track, index, file: name, at });
                }
                _ => {}
            }
        }
        Sheet { tracks, points }
    }

    /// Where the track at `track` (counted from zero, in sheet order) lies, or
    /// `None` where the sheet gives it no `INDEX 01`.
    fn cut(&self, track: usize) -> Option<DataTrack> {
        // `INDEX 01` is where a track's content starts, and the header with it.
        let start = self.points.iter().find(|p| p.track == track && p.index == 1)?;
        // `INDEX 00` is its pregap, which counts only where this file holds it.
        let pregap = self
            .points
            .iter()
            .find(|p| p.track == track && p.index == 0 && p.file == start.file && p.at <= start.at)
            .map_or(0, |p| start.at - p.at);
        // Whatever a later track stores next in the same file bounds it: its
        // pregap, or its content where it has none. One that opens a new file
        // does not, and neither does running out of tracks.
        let length = self
            .points
            .iter()
            .find(|p| p.track > track && p.file == start.file)
            .map(|next| next.at.saturating_sub(start.at));

        Some(DataTrack {
            file: start.file.clone(),
            offset: start.at,
            pregap,
            length,
            sector_size: self.tracks[track].sector,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sheet whose data track is first: the common shape, offset zero.
    #[test]
    fn a_data_track_at_the_start_has_no_offset() {
        let cue =
            "FILE \"Hawaiian Island Girls (US).img\" BINARY\n  TRACK 01 MODE1/2352\n    INDEX 01 00:00:00\n";
        assert_eq!(
            first_data_track(cue),
            Some(DataTrack {
                file: "Hawaiian Island Girls (US).img".into(),
                offset: 0,
                pregap: 0,
                // The only track in the sheet, so it runs to the end.
                length: None,
                sector_size: 2352,
            })
        );
    }

    /// From a real disc: audio first, program on track two at 00:47:65, all in
    /// one image. Taking the first `FILE` alone would hand back music.
    #[test]
    fn audio_before_the_program_is_measured_past() {
        let cue = "FILE \"Cosmic Fantasy 2 (US).img\" BINARY\n\
                   \x20 TRACK 01 AUDIO\n    INDEX 01 00:00:00\n\
                   \x20 TRACK 02 MODE1/2352\n    INDEX 01 00:47:65\n\
                   \x20 TRACK 03 AUDIO\n    INDEX 01 01:33:26\n";
        let found = first_data_track(cue).expect("track two is data");
        assert_eq!(found.file, "Cosmic Fantasy 2 (US).img");
        // (47 * 75 + 65) sectors of 2352 bytes.
        assert_eq!(found.offset, 3590 * 2352);
        // Track three shares the image, so it bounds the program:
        // ((60 + 33) * 75 + 26) = 7001 sectors in.
        assert_eq!(found.length, Some((7001 - 3590) * 2352));
    }

    /// A sheet naming a file per track: the data track's own file, from its
    /// start.
    #[test]
    fn a_file_per_track_points_at_the_right_one() {
        let cue = "FILE \"game (Track 01).bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n\
                   FILE \"game (Track 02).bin\" BINARY\n  TRACK 02 MODE1/2352\n    INDEX 01 00:00:00\n";
        assert_eq!(
            first_data_track(cue),
            Some(DataTrack {
                file: "game (Track 02).bin".into(),
                offset: 0,
                pregap: 0,
                // Its own file, so nothing bounds it: hash all of it, which is
                // what Redump publishes a hash for.
                length: None,
                sector_size: 2352,
            })
        );
    }

    /// A pregap index is not where content begins.
    #[test]
    fn index_zero_is_skipped_for_index_one() {
        let cue =
            "FILE \"g.bin\" BINARY\n  TRACK 01 MODE1/2352\n    INDEX 00 00:00:00\n    INDEX 01 00:00:02\n";
        let found = first_data_track(cue).unwrap();
        assert_eq!(found.offset, 2 * 2352);
        assert_eq!(found.pregap, 2 * 2352, "but it is part of the track");
    }

    /// Redump's sheet for Akumajou Dracula X (Japan): the program's file opens
    /// with its three-second pregap, and the hash Redump publishes for
    /// `(Track 02).bin` covers all of it. Reading from `INDEX 01` finds the
    /// header; hashing from there misses 225 sectors of the track.
    #[test]
    fn a_track_file_holds_its_pregap_ahead_of_the_program() {
        let cue = "FILE \"Dracula X (Track 01).bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n\
                   FILE \"Dracula X (Track 02).bin\" BINARY\n  TRACK 02 MODE1/2352\n\
                   \x20   INDEX 00 00:00:00\n    INDEX 01 00:03:00\n\
                   FILE \"Dracula X (Track 03).bin\" BINARY\n  TRACK 03 AUDIO\n\
                   \x20   INDEX 00 00:00:00\n    INDEX 01 00:02:00\n";
        assert_eq!(
            first_data_track(cue),
            Some(DataTrack {
                file: "Dracula X (Track 02).bin".into(),
                offset: 225 * 2352,
                pregap: 225 * 2352,
                length: None,
                sector_size: 2352,
            })
        );
    }

    /// The same disc's tracks in one image, as a merge of Redump's files writes
    /// them: the program starts at its pregap and stops where track three's
    /// begins, not where track three's music does. Measured to the next
    /// `INDEX 01` instead, it would lose its own pregap and take the next one.
    #[test]
    fn one_image_cuts_the_program_at_the_pregaps_either_side() {
        let cue = "FILE \"Dracula X.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n\
                   \x20 TRACK 02 MODE1/2352\n    INDEX 00 00:48:65\n    INDEX 01 00:51:65\n\
                   \x20 TRACK 03 AUDIO\n    INDEX 00 03:07:14\n    INDEX 01 03:09:14\n";
        let found = first_data_track(cue).expect("track two is data");
        // Redump's track one is 3665 sectors, its track two 10374.
        assert_eq!(found.offset - found.pregap, 3665 * 2352);
        assert_eq!(found.pregap, 225 * 2352);
        assert_eq!(found.length.map(|length| found.pregap + length), Some(10374 * 2352));
    }

    /// A cooked track says so, and a hash of raw sectors has nothing to read.
    #[test]
    fn the_sector_width_is_the_track_lines() {
        for (mode, width) in
            [("MODE1/2352", 2352), ("MODE1/2048", 2048), ("MODE2/2336", 2336), ("MODE2/2352", 2352)]
        {
            let cue = format!("FILE \"g.bin\" BINARY\n  TRACK 01 {mode}\n    INDEX 01 00:00:00\n");
            assert_eq!(first_data_track(&cue).unwrap().sector_size, width, "{mode}");
        }
    }

    /// A `PREGAP` line states silence the file does not hold, so it is not
    /// part of what the file's track is.
    #[test]
    fn a_pregap_the_file_does_not_hold_is_not_counted() {
        let cue = "FILE \"g.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n\
                   \x20 TRACK 02 MODE1/2352\n    PREGAP 00:02:00\n    INDEX 01 00:10:00\n";
        let found = first_data_track(cue).unwrap();
        assert_eq!((found.offset, found.pregap), (750 * 2352, 0));
    }

    /// A CloneCD sheet writes its numbers unpadded. Comparing the text alone
    /// misses the index entirely and the disc goes unrecognised.
    #[test]
    fn unpadded_track_and_index_numbers_read_the_same() {
        let cue = "FILE \"Akumajou Dracula X Chi no Rondo (JP) (US).img\" BINARY\n\
                   \x20  TRACK 1 AUDIO\n   INDEX 1 00:00:00\n\
                   \x20  TRACK 2 MODE1/2352\n   INDEX 1 00:51:65\n";
        let found = first_data_track(cue).expect("track two is data");
        assert_eq!(found.offset, (51 * 75 + 65) * 2352);
    }

    /// A PC Engine CD keeps a second copy of its program near the end, between
    /// tracks of music. Each is cut where the first is: from its `INDEX 01` to
    /// where the next track's pregap begins.
    #[test]
    fn every_data_track_is_cut_as_the_first_is() {
        let cue = "FILE \"g.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n\
                   \x20 TRACK 02 MODE1/2352\n    INDEX 00 00:10:00\n    INDEX 01 00:13:00\n\
                   \x20 TRACK 03 AUDIO\n    INDEX 00 00:40:00\n    INDEX 01 00:42:00\n\
                   \x20 TRACK 04 MODE1/2352\n    INDEX 00 01:00:00\n    INDEX 01 01:03:00\n\
                   \x20 TRACK 05 AUDIO\n    INDEX 00 01:30:00\n    INDEX 01 01:32:00\n";
        let found = data_tracks(cue);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0], first_data_track(cue).unwrap());
        assert_eq!(found[1].offset, 63 * 75 * 2352);
        assert_eq!(found[1].pregap, 3 * 75 * 2352);
        assert_eq!(found[1].length, Some(27 * 75 * 2352), "up to track five's pregap");
    }

    /// A file per track: each data track is its own file, from its `INDEX 01`
    /// to its end, and audio between them is no track of these.
    #[test]
    fn every_data_track_names_its_own_file() {
        let cue = "FILE \"g (Track 1).bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n\
                   FILE \"g (Track 2).bin\" BINARY\n  TRACK 02 MODE1/2352\n    INDEX 00 00:00:00\n    INDEX 01 00:03:00\n\
                   FILE \"g (Track 3).bin\" BINARY\n  TRACK 03 AUDIO\n    INDEX 01 00:00:00\n\
                   FILE \"g (Track 4).iso\" BINARY\n  TRACK 04 MODE1/2048\n    INDEX 01 00:00:00\n";
        let found = data_tracks(cue);
        let files: Vec<&str> = found.iter().map(|track| track.file.as_str()).collect();
        assert_eq!(files, ["g (Track 2).bin", "g (Track 4).iso"]);
        assert_eq!((found[0].offset, found[0].length), (225 * 2352, None));
        assert_eq!((found[1].sector_size, found[1].length), (2048, None));
    }

    #[test]
    fn a_sheet_of_only_audio_is_not_a_game_disc() {
        let cue = "FILE \"music.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n";
        assert_eq!(first_data_track(cue), None);
        assert_eq!(data_tracks(cue), []);
    }

    #[test]
    fn a_sheet_naming_no_file_is_none() {
        assert_eq!(first_data_track("REM GENRE Action\n"), None);
    }

    #[test]
    fn a_position_past_u64_is_skipped_rather_than_wrapped() {
        let time = "FILE \"a.bin\" BINARY\n  TRACK 01 MODE1/2352\n  INDEX 01 307445734561825861:00:00\n";
        assert_eq!(first_data_track(time), None);
        let width = "FILE \"a.bin\" BINARY\n  TRACK 01 MODE1/18446744073709551615\n  INDEX 01 00:00:02\n";
        assert_eq!(first_data_track(width), None);
    }
}
