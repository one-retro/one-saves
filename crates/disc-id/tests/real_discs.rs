//! Real discs: the Redump hash against the hashes Redump publishes, and the `disc-data`
//! fingerprint against copies of one disc and discs that differ.
//!
//! None of these discs is in this repository, and none may be: they are commercial dumps. So these
//! tests read them where they lie. `DISC_ID_DUMPS` names the folders to look in, separated as
//! `PATH` is, and a disc is found in one of them or in a folder directly inside one, under any of
//! the names it goes by. With the variable unset, every test here passes without reading anything.
//!
//! A CHD is read as one, and a cue sheet is followed to the tracks beside it.

#![cfg(all(feature = "chd", any(feature = "redump", feature = "disc-data")))]

#[cfg(feature = "redump")]
use std::io;
#[cfg(feature = "redump")]
use std::path::Path;
use std::path::PathBuf;

#[cfg(feature = "redump")]
use disc_id::redump::RedumpHash;

const VARIABLE: &str = "DISC_ID_DUMPS";

/// A disc whose data track Redump has a hash for.
#[cfg(feature = "redump")]
struct Known {
    /// What a file of it may be called: Redump's name first, then names it was found under.
    names: &'static [&'static str],
    /// Which track that is, which is the first data track.
    track: u8,
    sha1: &'static str,
    md5: &'static str,
    size: u64,
}

/// Puzzle Bobble's is from its page on redump.org, disc 59078; the PC Engine discs' are from the
/// Redump DAT of 2016-11-06. Each of the PC Engine discs stores a pregap of its own length in its
/// track file, from 224 to 231 sectors, so each tests where the track starts.
#[cfg(feature = "redump")]
const KNOWN: [Known; 6] = [
    Known {
        names: &["Puzzle Bobble ~ Bust-A-Move (Japan) (En,Ja).chd"],
        track: 1,
        sha1: "f1764842973d087963a69c80160619002b5366ca",
        md5: "ae9eeff58f32e86375af017ce984af04",
        size: 2_740_080,
    },
    Known {
        names: &["Golden Axe (Japan).chd", "Golden Axe (Japan).cue", "Golden Axe (JP).chd"],
        track: 2,
        sha1: "48e1cd1b9225989718b6f16d09682adf4c65a64c",
        md5: "492e288fb2d4d489ccdfec5369ac9279",
        size: 15_245_664,
    },
    Known {
        names: &[
            "Gradius II - Gofer no Yabou (Japan).chd",
            "Gradius II - Gofer no Yabou (Japan).cue",
            "Gradius II - Gofer no Yabou (JP).chd",
        ],
        track: 2,
        sha1: "673bfdcab4e0aadb31ea7cc680223fb1d26aa30a",
        md5: "751e3e4cd68daa06901d3087bc3d68f8",
        size: 8_523_648,
    },
    Known {
        names: &[
            "Ys Book I & II (USA) (Rev 2).chd",
            "Ys Book I & II (USA) (Rev 2).cue",
            "Ys Book I & II (NA).chd",
        ],
        track: 2,
        sha1: "d0498146b90725fee8fc2b00b2aa1ce47973549e",
        md5: "e8db8109220893ad3aa9506cf7f13c23",
        size: 20_307_168,
    },
    Known {
        names: &[
            "Ys III - Wanderers from Ys (USA) (Rev 1).chd",
            "Ys III - Wanderers from Ys (USA) (Rev 1).cue",
            "Ys III - Wanderers from Ys (NA).chd",
        ],
        track: 2,
        sha1: "fda4ecab121db0ab443956dcdf236a0f38868f73",
        md5: "10ddbd55a98baf64df1c64064ef17242",
        size: 58_701_216,
    },
    Known {
        names: &[
            "Dungeon Master - Theron's Quest (USA).chd",
            "Dungeon Master - Theron's Quest (USA).cue",
            "Dungeon Master - Theron's Quest (NA).chd",
        ],
        track: 2,
        sha1: "66afcd22fa26995e65e3bce2f9b1bc1eb405f6ee",
        md5: "f23601102138f87c33025877767ebf76",
        size: 8_104_992,
    },
];

/// Every file in the folders the variable names, and in the folders directly inside them.
fn files() -> Vec<PathBuf> {
    let Some(roots) = std::env::var_os(VARIABLE) else {
        eprintln!("{VARIABLE} is unset, so no real disc is read");
        return Vec::new();
    };
    let mut found = Vec::new();
    for root in std::env::split_paths(&roots) {
        let Ok(entries) = std::fs::read_dir(&root) else {
            panic!("{VARIABLE} names {}, which is not a folder", root.display());
        };
        for path in entries.flatten().map(|entry| entry.path()) {
            if path.is_dir() {
                let inner = std::fs::read_dir(&path).into_iter().flatten().flatten();
                found.extend(inner.map(|entry| entry.path()).filter(|path| path.is_file()));
            } else {
                found.push(path);
            }
        }
    }
    found
}

/// The Redump hash of a CHD, or of the data track a cue sheet names beside it.
#[cfg(feature = "redump")]
fn redump(path: &Path) -> io::Result<Option<RedumpHash>> {
    let file = io::BufReader::new(std::fs::File::open(path)?);
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("chd")) {
        return disc_id::chd::redump(file);
    }
    let text = std::fs::read_to_string(path)?;
    let track = disc_id::cue::first_data_track(&text).expect("a data track");
    let folder = path.parent().expect("a folder");
    let bin = disc_id::cue::resolve(folder, &track.file).expect("a track beside its sheet");
    disc_id::redump::track(&track, io::BufReader::new(std::fs::File::open(bin)?))
}

/// Check every disc of track `track` that is found, and say how many were.
#[cfg(feature = "redump")]
fn check(track: u8) {
    let files = files();
    let mut checked = 0;
    for known in KNOWN.iter().filter(|known| known.track == track) {
        let found = files.iter().filter(|path| {
            path.file_name().and_then(|name| name.to_str()).is_some_and(|name| known.names.contains(&name))
        });
        for path in found {
            let got =
                redump(path).unwrap().unwrap_or_else(|| panic!("{} has no fingerprint", path.display()));
            assert_eq!(
                (got.sha1_hex().as_str(), got.md5_hex().as_str(), got.size),
                (known.sha1, known.md5, known.size),
                "{}",
                path.display(),
            );
            checked += 1;
        }
    }
    eprintln!("{checked} disc(s) checked against Redump's track {track}");
}

/// A CHD of a disc whose program is on track one, as most are, gives Redump's hash of that track.
#[cfg(feature = "redump")]
#[test]
fn a_chd_matches_redumps_track_1() {
    check(1);
}

/// A PC Engine CD, whose program is on track two behind a pregap Redump keeps in the track's file.
#[cfg(feature = "redump")]
#[test]
fn a_pc_engine_cd_matches_redumps_track_2() {
    check(2);
}

/// The four dumps of Akumajou Dracula X that 1Retro's earlier fingerprint could not tell apart:
/// three fan translations and the release they patch. Each patches the disc's second data track,
/// and the first is the same in three of them.
#[cfg(feature = "disc-data")]
const RONDO: [&str; 4] = [
    "Castlevania - Rondo of Blood (USA).chd",
    "Castlevania - Rondo of Blood (T-En).chd",
    "Castlevania - Rondo of Blood (T-En + Arranged Audio).chd",
    "Castlevania - Rondo of Blood (T-En + English PSP Audio v1.01).chd",
];

/// The `disc-data` fingerprint of a CHD, a cue sheet's tracks, or an image.
#[cfg(feature = "disc-data")]
fn disc_data(path: &std::path::Path) -> disc_id::disc_data::DiscData {
    use std::fs::File;
    use std::io::BufReader;

    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let found = match extension.as_str() {
        "chd" => disc_id::chd::disc_data(BufReader::new(File::open(path).unwrap())),
        "cue" => {
            let text = std::fs::read_to_string(path).unwrap();
            let folder = path.parent().expect("a folder");
            disc_id::disc_data::sheet(&text, |name| {
                let file = disc_id::cue::resolve(folder, name).expect("a track beside its sheet");
                File::open(file).map(BufReader::new)
            })
        }
        _ => disc_id::disc_data::image(BufReader::new(File::open(path).unwrap())),
    };
    found.unwrap().unwrap_or_else(|| panic!("{} has no fingerprint", path.display()))
}

/// Copies of one disc under one name, a cue sheet and a CHD and an image of it wherever they are
/// found, give one value; the four Rondo dumps give four.
#[cfg(feature = "disc-data")]
#[test]
fn copies_of_a_disc_agree_and_the_rondo_dumps_do_not() {
    let mut by_name: std::collections::BTreeMap<String, Vec<PathBuf>> = std::collections::BTreeMap::new();
    for path in files() {
        let disc =
            ["chd", "cue", "iso"].iter().any(|e| path.extension().is_some_and(|x| x.eq_ignore_ascii_case(e)));
        if let (true, Some(stem)) = (disc, path.file_stem().and_then(|s| s.to_str())) {
            by_name.entry(stem.to_string()).or_default().push(path);
        }
    }
    let mut compared = 0;
    for (name, copies) in by_name.iter().filter(|(_, copies)| copies.len() > 1) {
        let first = disc_data(&copies[0]);
        for copy in &copies[1..] {
            assert_eq!(disc_data(copy), first, "{name}: {} and {}", copies[0].display(), copy.display());
            compared += 1;
        }
    }
    let rondo: Vec<PathBuf> = files()
        .into_iter()
        .filter(|path| path.file_name().and_then(|n| n.to_str()).is_some_and(|n| RONDO.contains(&n)))
        .collect();
    let values: std::collections::BTreeSet<[u8; 32]> =
        rondo.iter().map(|path| disc_data(path).digest).collect();
    assert_eq!(values.len(), rondo.len(), "every Rondo dump its own value");
    eprintln!("{compared} copies compared, {} Rondo dumps told apart", rondo.len());
}
