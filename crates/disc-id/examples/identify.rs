//! Identify disc images named on the command line.
//!
//! ```console
//! cargo run --example identify -- "F-Zero GX.iso" game.cue
//! ```
//!
//! A cue sheet is followed to its data track, which is a file the *sheet* names
//! rather than one named here, so it is held to what a sheet may name: a
//! regular file in the sheet's own directory. And nothing that came out of a
//! file, a name included, reaches the terminal without its control characters
//! escaped.

use std::fs::File;
use std::path::{Path, PathBuf};

/// Follow a cue sheet to its data track, so the caller reads the program
/// rather than the album that precedes it. Anything else is read as-is.
///
/// What a sheet names is the sheet's say. A sheet naming `/dev/zero` or
/// `../../.ssh/id_ed25519` is refused rather than followed, and so is one
/// naming anything but a regular file: opening a FIFO blocks until something
/// writes to it, which nothing will.
fn resolve(path: &Path) -> Result<(PathBuf, u64), String> {
    let is_cue = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("cue"));
    if !is_cue {
        return Ok((path.to_path_buf(), 0));
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok((path.to_path_buf(), 0));
    };
    let Some(track) = disc_id::cue::first_data_track(&text) else {
        return Ok((path.to_path_buf(), 0));
    };
    let directory = path.parent().unwrap_or(Path::new("."));
    let Some(target) = disc_id::cue::resolve(directory, &track.file) else {
        return Err(format!("names a file outside its directory: {}", printable(&track.file)));
    };
    if !std::fs::metadata(&target).is_ok_and(|found| found.is_file()) {
        return Err(format!("names {}, which is not a regular file", shown(&target)));
    }
    Ok((target, track.offset))
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
        let (target, offset) = match resolve(&given) {
            Ok(found) => found,
            Err(why) => {
                println!("{}: {why}", shown(&given));
                continue;
            }
        };
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
    }
}
