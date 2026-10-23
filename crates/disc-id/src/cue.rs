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

/// Where a cue sheet's data track lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataTrack {
    /// The file naming it, exactly as the sheet spells it. Resolve against the
    /// sheet's own directory.
    pub file: String,
    /// How far into that file the track begins. Zero for a sheet that gives the
    /// data track a file of its own.
    pub offset: u64,
    /// How many bytes of that file the track occupies, or `None` when it runs
    /// to the end of it.
    ///
    /// A sheet that gives each track its own file leaves this `None`, which is
    /// the common shape and the one Redump publishes hashes for. A sheet whose
    /// tracks share one image needs it: without a length, hashing "the data
    /// track" means hashing the whole disc image, audio and all, which matches
    /// no catalogue entry and differs from the same disc dumped track-per-file.
    pub length: Option<u64>,
}

/// Bytes per sector when a track line does not say.
const DEFAULT_SECTOR: u64 = 2352;

/// `MM:SS:FF` to a count of sectors. A CD runs at 75 frames a second.
fn msf_to_frames(text: &str) -> Option<u64> {
    let mut parts = text.split(':');
    let m: u64 = parts.next()?.trim().parse().ok()?;
    let s: u64 = parts.next()?.trim().parse().ok()?;
    let f: u64 = parts.next()?.trim().parse().ok()?;
    Some((m * 60 + s) * 75 + f)
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
#[must_use]
pub fn files(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let (word, rest) = line.trim().split_once(' ')?;
            word.eq_ignore_ascii_case("FILE").then(|| file_name(rest)).flatten()
        })
        .collect()
}

/// The first data track a cue sheet names.
///
/// `None` when the sheet names no file, or names only audio — a music CD is
/// not a game disc, and saying so beats pointing at a track of songs.
#[must_use]
pub fn first_data_track(text: &str) -> Option<DataTrack> {
    // Every track's start, in file order, so the data track's length is the
    // distance to whatever comes next in the same file.
    struct Start {
        file: String,
        offset: u64,
        data: bool,
    }

    let mut starts: Vec<Start> = Vec::new();
    let mut file: Option<String> = None;
    // Sector width of the track being read, and whether it is a data track.
    // Audio is always 2352; only a MODE line states a width of its own.
    let mut pending: Option<(u64, bool)> = None;

    for line in text.lines() {
        let line = line.trim();
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));

        match word.to_ascii_uppercase().as_str() {
            "FILE" => file = file_name(rest),
            "TRACK" => {
                // `TRACK 02 MODE1/2352`
                let mode = rest.split_whitespace().nth(1).unwrap_or("");
                let upper = mode.to_ascii_uppercase();
                pending = if upper.starts_with("MODE") {
                    Some((sector_size(mode), true))
                } else {
                    Some((crate::sector::RAW as u64, false))
                };
            }
            // `INDEX 01` is where the track's content starts; `INDEX 00` is its
            // pregap, which is not what a header sits in.
            "INDEX" if pending.is_some() => {
                let mut fields = rest.split_whitespace();
                // Compare numerically: a CloneCD sheet writes `INDEX 1` where
                // others write `INDEX 01`, and they mean the same index.
                if fields.next().and_then(|n| n.parse::<u32>().ok()) != Some(1) {
                    continue;
                }
                let (sector, data) = pending.take()?;
                let Some(frames) = fields.next().and_then(msf_to_frames) else {
                    continue;
                };
                let Some(name) = file.clone() else { continue };
                starts.push(Start { file: name, offset: frames * sector, data });
            }
            _ => {}
        }
    }

    let at = starts.iter().position(|s| s.data)?;
    let track = &starts[at];
    // The next track sharing this file bounds it; one that opens a new file
    // does not, and neither does running out of tracks.
    let length = starts[at + 1..]
        .iter()
        .find(|s| s.file == track.file)
        .map(|next| next.offset.saturating_sub(track.offset));

    Some(DataTrack { file: track.file.clone(), offset: track.offset, length })
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
                // The only track in the sheet, so it runs to the end.
                length: None,
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
                // Its own file, so nothing bounds it: hash all of it, which is
                // what Redump publishes a hash for.
                length: None,
            })
        );
    }

    /// A pregap index is not where content begins.
    #[test]
    fn index_zero_is_skipped_for_index_one() {
        let cue =
            "FILE \"g.bin\" BINARY\n  TRACK 01 MODE1/2352\n    INDEX 00 00:00:00\n    INDEX 01 00:00:02\n";
        assert_eq!(first_data_track(cue).unwrap().offset, 2 * 2352);
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

    #[test]
    fn a_sheet_of_only_audio_is_not_a_game_disc() {
        let cue = "FILE \"music.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n";
        assert_eq!(first_data_track(cue), None);
    }

    #[test]
    fn a_sheet_naming_no_file_is_none() {
        assert_eq!(first_data_track("REM GENRE Action\n"), None);
    }
}
