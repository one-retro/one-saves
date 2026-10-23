//! Identify disc images named on the command line.
//!
//! ```console
//! cargo run --example identify -- "F-Zero GX.iso" game.cue
//! ```

use std::path::{Path, PathBuf};

/// Follow a cue sheet to its data track, so the caller reads the program
/// rather than the album that precedes it. Anything else is read as-is.
fn resolve(path: &Path) -> (PathBuf, u64) {
    let is_cue = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("cue"));
    if !is_cue {
        return (path.to_path_buf(), 0);
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return (path.to_path_buf(), 0);
    };
    match disc_id::cue::first_data_track(&text) {
        Some(track) => (path.parent().unwrap_or(Path::new(".")).join(track.file), track.offset),
        None => (path.to_path_buf(), 0),
    }
}

fn main() {
    for arg in std::env::args().skip(1) {
        let given = PathBuf::from(&arg);
        let (target, offset) = resolve(&given);
        let note = if target == given {
            String::new()
        } else {
            format!(" (via cue -> {} @{offset})", target.display())
        };

        // Hand over the file itself: identification reads forward only and
        // stops as soon as it has what it needs, so a 700 MB disc image costs
        // a few sectors rather than a read of the whole thing.
        let file = match std::fs::File::open(&target) {
            Ok(f) => f,
            Err(e) => {
                println!("{}: unreadable ({e})", given.display());
                continue;
            }
        };
        // The data track may start partway into a single-file image.
        let mut file = file;
        if offset > 0 {
            use std::io::Seek as _;
            if let Err(e) = file.seek(std::io::SeekFrom::Start(offset)) {
                println!("{}: seek failed ({e})", given.display());
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
                given.display(),
                id.system.slug,
                id.system,
                id.serial.as_deref().unwrap_or("(none stated)")
            ),
            Ok(None) => println!("{}{note}\n    unrecognised", given.display()),
            Err(e) => println!("{}{note}\n    read failed: {e}", given.display()),
        }
    }
}
