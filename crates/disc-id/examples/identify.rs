//! Identify disc images named on the command line.
//!
//! ```console
//! cargo run --example identify -- "F-Zero GX.iso" game.cue
//! ```
//!
//! With `--features redump`, each is also hashed as Redump hashes its first
//! data track, which reads the track whole; with `--features disc-data`, each
//! is fingerprinted as 1Retro keys a disc's saves, which reads every data track
//! whole and prints each track's share too.
//!
//! A cue sheet is followed to its data track, which is a file the *sheet* names
//! rather than one named here, so it is held to what a sheet may name: a
//! regular file in the sheet's own directory. And nothing that came out of a
//! file, a name included, reaches the terminal without its control characters
//! escaped.

use std::fs::File;
use std::path::{Path, PathBuf};

use disc_id::cue::DataTrack;

/// Follow a cue sheet to its data track, so the caller reads the program
/// rather than the album that precedes it. Anything else is read as-is.
///
/// What a sheet names is the sheet's say. A sheet naming `/dev/zero` or
/// `../../.ssh/id_ed25519` is refused rather than followed, and so is one
/// naming anything but a regular file: opening a FIFO blocks until something
/// writes to it, which nothing will.
fn resolve(path: &Path) -> Result<(PathBuf, Option<DataTrack>), String> {
    let is_cue = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("cue"));
    if !is_cue {
        return Ok((path.to_path_buf(), None));
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok((path.to_path_buf(), None));
    };
    let Some(track) = disc_id::cue::first_data_track(&text) else {
        return Ok((path.to_path_buf(), None));
    };
    let directory = path.parent().unwrap_or(Path::new("."));
    let Some(target) = disc_id::cue::resolve(directory, &track.file) else {
        return Err(format!("names a file outside its directory: {}", printable(&track.file)));
    };
    if !std::fs::metadata(&target).is_ok_and(|found| found.is_file()) {
        return Err(format!("names {}, which is not a regular file", shown(&target)));
    }
    Ok((target, Some(track)))
}

/// Text from a file, or naming one, made safe to print.
///
/// A control character is an instruction to the terminal rather than
/// something to show, so a name or a serial carrying `ESC ]0;` retitles the
/// window and one carrying `ESC [2J` clears it. Those, and the characters that
/// reorder the text after them, are printed as their escapes instead.
fn printable(text: &str) -> String {
    text.chars()
        .map(|c| {
            let reorders = matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}');
            if c.is_control() || reorders { c.escape_unicode().to_string() } else { c.to_string() }
        })
        .collect()
}

/// A path, made safe to print.
fn shown(path: &Path) -> String {
    printable(&path.display().to_string())
}

fn main() {
    // `args_os`, because a name that is not UTF-8 is a name all the same, and
    // `args` panics on one.
    for arg in std::env::args_os().skip(1) {
        let given = PathBuf::from(&arg);
        let (target, track) = match resolve(&given) {
            Ok(found) => found,
            Err(why) => {
                println!("{}: {why}", shown(&given));
                continue;
            }
        };
        let offset = track.as_ref().map_or(0, |track| track.offset);
        let via_cue = target != given;
        let note =
            if via_cue { format!(" (via cue -> {} @{offset})", shown(&target)) } else { String::new() };

        // Hand over the file itself: identification reads forward only and
        // stops as soon as it has what it needs, so a 700 MB disc image costs
        // a few sectors rather than a read of the whole thing.
        let mut file = match File::open(&target) {
            Ok(f) => f,
            Err(e) => {
                println!("{}: unreadable ({})", shown(&given), printable(&e.to_string()));
                continue;
            }
        };
        // What a sheet names was checked by name before it was opened, and a
        // name can change what it points at in between; this is the check of
        // what was actually opened. A path given here is the caller's own
        // choice, a disc drive's device included, so it is left alone.
        if via_cue && !file.metadata().is_ok_and(|opened| opened.is_file()) {
            println!("{}{note}: not a regular file", shown(&given));
            continue;
        }
        // The data track may start partway into a single-file image.
        if offset > 0 {
            use std::io::Seek as _;
            if let Err(e) = file.seek(std::io::SeekFrom::Start(offset)) {
                println!("{}: seek failed ({})", shown(&given), printable(&e.to_string()));
                continue;
            }
        }

        // A CHD holds the image compressed, so it takes its own reader.
        #[cfg(feature = "chd")]
        let is_chd = target.extension().is_some_and(|e| e.eq_ignore_ascii_case("chd"));
        #[cfg(feature = "chd")]
        let outcome = if is_chd { disc_id::chd::identify(file) } else { disc_id::identify_seekable(file) };
        #[cfg(not(feature = "chd"))]
        let outcome = disc_id::identify_seekable(file);

        match outcome {
            Ok(Some(id)) => println!(
                "{}{note}\n    system {} ({:?})\n    serial {}",
                shown(&given),
                id.system.slug,
                id.system,
                printable(id.serial.as_deref().unwrap_or("(none stated)"))
            ),
            Ok(None) => println!("{}{note}\n    unrecognised", shown(&given)),
            Err(e) => println!("{}{note}\n    read failed: {}", shown(&given), printable(&e.to_string())),
        }
        #[cfg(feature = "redump")]
        redump(&target, track.as_ref());
        #[cfg(feature = "disc-data")]
        disc_data(&given);
    }
}

/// Fingerprint what `given` holds as 1Retro does, and print it with each data
/// track's share: every data track a sheet names, those of a CHD, or an image
/// whole.
///
/// A sheet's files are held to what [`resolve`] holds its data track to: a
/// regular file in the sheet's own directory.
#[cfg(feature = "disc-data")]
fn disc_data(given: &Path) {
    use std::io::{self, BufReader};

    let is = |extension: &str| given.extension().is_some_and(|e| e.eq_ignore_ascii_case(extension));
    let found = if is("cue") {
        let text = match std::fs::read_to_string(given) {
            Ok(text) => text,
            Err(e) => return println!("    disc-data unreadable ({})", printable(&e.to_string())),
        };
        let directory = given.parent().unwrap_or(Path::new("."));
        disc_id::disc_data::sheet(&text, |name| {
            let refused = |why: String| io::Error::other(why);
            let Some(target) = disc_id::cue::resolve(directory, name) else {
                return Err(refused(format!("names a file outside its directory: {}", printable(name))));
            };
            let regular =
                |metadata: io::Result<std::fs::Metadata>| metadata.is_ok_and(|found| found.is_file());
            if !regular(std::fs::metadata(&target)) {
                return Err(refused(format!("names {}, which is not a regular file", shown(&target))));
            }
            let file = File::open(&target)?;
            if !regular(file.metadata()) {
                return Err(refused(format!("names {}, which is not a regular file", shown(&target))));
            }
            Ok(BufReader::new(file))
        })
    } else {
        let file = match File::open(given) {
            Ok(file) => BufReader::new(file),
            Err(e) => return println!("    disc-data unreadable ({})", printable(&e.to_string())),
        };
        #[cfg(feature = "chd")]
        let found = if is("chd") { disc_id::chd::disc_data(file) } else { disc_id::disc_data::image(file) };
        #[cfg(not(feature = "chd"))]
        let found = disc_id::disc_data::image(file);
        found
    };
    match found {
        Ok(Some(found)) => {
            println!("    disc-data {}", found.hex());
            for track in &found.tracks {
                println!("      {} sectors, sha1 {}", track.sectors, track.sha1_hex());
            }
        }
        Ok(None) => println!("    disc-data none: no data"),
        Err(e) => println!("    disc-data read failed: {}", printable(&e.to_string())),
    }
}

/// Hash the first data track of what `target` holds, as Redump does, and print
/// it: the track a sheet named, the data track of a CHD, or an image whole.
#[cfg(feature = "redump")]
fn redump(target: &Path, track: Option<&DataTrack>) {
    let file = match File::open(target) {
        Ok(file) => std::io::BufReader::new(file),
        Err(e) => return println!("    redump unreadable ({})", printable(&e.to_string())),
    };
    #[cfg(feature = "chd")]
    let is_chd = target.extension().is_some_and(|e| e.eq_ignore_ascii_case("chd"));
    #[cfg(not(feature = "chd"))]
    let is_chd = false;
    let found = match track {
        Some(track) => disc_id::redump::track(track, file),
        #[cfg(feature = "chd")]
        None if is_chd => disc_id::chd::redump(file),
        None => disc_id::redump::image(file),
    };
    match found {
        Ok(Some(found)) => {
            println!("    redump sha1 {} md5 {} size {}", found.sha1_hex(), found.md5_hex(), found.size);
        }
        Ok(None) => {
            println!("    redump none: no raw data track{}", if is_chd { " in this CHD" } else { "" });
        }
        Err(e) => println!("    redump read failed: {}", printable(&e.to_string())),
    }
}
