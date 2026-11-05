//! Every file a cue sheet names, which a scan leaves out as tracks of the
//! sheet's disc rather than ROMs of their own.

use disc_id::cue::files;

/// Audio tracks too: they are as much a part of the disc as the data track,
/// and as little a dump of their own.
#[test]
fn every_track_file_is_named_audio_included() {
    let cue = "FILE \"Game (Track 1).bin\" BINARY\n  TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\n\
               FILE \"Game (Track 2).bin\" BINARY\n  TRACK 02 AUDIO\n    INDEX 00 00:00:00\n    INDEX 01 00:02:00\n";
    assert_eq!(files(cue), vec!["Game (Track 1).bin", "Game (Track 2).bin"]);
}

/// A name with no spaces need not be quoted, and the keyword's case is the
/// writer's choice, as `first_data_track` already accepts.
#[test]
fn unquoted_names_and_a_lower_case_keyword_read_the_same() {
    let cue = "file game.img BINARY\n  TRACK 01 MODE1/2352\n    INDEX 01 00:00:00\n";
    assert_eq!(files(cue), vec!["game.img"]);
}

/// A single-file sheet names its one image once, however many tracks it holds.
#[test]
fn one_image_for_every_track_is_named_once() {
    let cue = "FILE \"Disc.bin\" BINARY\n  TRACK 01 MODE1/2352\n    INDEX 01 00:00:00\n\
               \x20 TRACK 02 AUDIO\n    INDEX 01 10:00:00\n";
    assert_eq!(files(cue), vec!["Disc.bin"]);
}

#[test]
fn a_sheet_naming_no_file_names_nothing() {
    assert_eq!(files("REM GENRE Action\nTITLE \"Game\"\n"), Vec::<String>::new());
}

/// A sheet's names are its say, and a sheet is text that came with the dump.
/// Joined to its directory unexamined, `FILE "/dev/zero"` reads a device and
/// `FILE "../../.ssh/id_ed25519"` reads a key: only a name of plain components
/// resolves, and it resolves inside the sheet's directory.
#[test]
fn a_name_resolves_only_inside_the_sheets_directory() {
    use std::path::Path;

    use disc_id::cue::{first_data_track, resolve};

    let dir = Path::new("games").join("Disc");
    assert_eq!(resolve(&dir, "Disc (Track 1).bin"), Some(dir.join("Disc (Track 1).bin")));
    assert_eq!(resolve(&dir, "tracks/Disc.bin"), Some(dir.join("tracks/Disc.bin")));
    assert_eq!(resolve(&dir, "./Disc.bin"), Some(dir.join("./Disc.bin")), "a `.` goes nowhere");

    for name in
        ["/dev/zero", "/etc/hosts", "../Other.bin", "../../.ssh/id_ed25519", "tracks/../../x.bin", "", "."]
    {
        assert_eq!(resolve(&dir, name), None, "{name:?}");
    }
    // A drive or a UNC share is a root of its own on Windows.
    #[cfg(windows)]
    for name in [r"C:\Windows\win.ini", r"C:x.bin", r"\\server\share\x.bin", r"..\x.bin"] {
        assert_eq!(resolve(&dir, name), None, "{name:?}");
    }

    // And it is the name the sheet's own track is found by.
    let cue = "FILE \"../../../../etc/hosts\" BINARY\n  TRACK 01 MODE1/2048\n    INDEX 01 00:00:00\n";
    let track = first_data_track(cue).expect("a data track");
    assert_eq!(resolve(&dir, &track.file), None);
}
