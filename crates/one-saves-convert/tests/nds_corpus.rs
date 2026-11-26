//! Runs the DS reader against every save the corpus downloaded: Action Replay DS files of both
//! versions, compressed and not, DeSmuME's `.dsv` and save states, a no$gba file, a DSXS file, two
//! DraStic states and raw dumps.
//!
//! None of them is in this repository, and none may be: they were shared on GameFAQs under their
//! uploaders' terms, not the corpus's CC0. So this reads them where they lie. `ONE_SAVES_NDS_CORPUS`
//! names the corpus's `nds` folder; with it unset, the test passes without reading anything.
//!
//! The corpus converted each to its game's chip with converters of its own, ran the result in six
//! cores, and recorded the chip's SHA-1 in the entry's `provenance.json` as `verified.converted`.
//! Each file here is read with that chip's length, standing in for the game code the release has,
//! and has to come out as those bytes.

#![cfg(all(feature = "rom", feature = "states", feature = "nds-zlib"))]

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use one_saves::HashAlgorithm;
use one_saves_convert::nds::{self, NdsOptions};

const VARIABLE: &str = "ONE_SAVES_NDS_CORPUS";

/// The one save no chip can hold: an Explorers of Sky file whose upper 128 KiB is a second,
/// different save, which the 1 Mbit chip cannot address. The corpus cut it by hand; a producer
/// carries it as the file it came in.
const TWO_SAVES: &str = "35489";

/// `"key": value` out of a JSON object's text, from where `key` first appears after `from`.
fn field<'a>(text: &'a str, from: usize, key: &str) -> Option<(&'a str, usize)> {
    let at = text[from..].find(&format!("\"{key}\":"))? + from + key.len() + 3;
    let rest = text[at..].trim_start();
    let value = if let Some(quoted) = rest.strip_prefix('"') {
        &quoted[..quoted.find('"')?]
    } else {
        &rest[..rest.find([',', '}', '\n'])?]
    };
    Some((value.trim(), at))
}

/// The chip the corpus converted an entry to: its length and SHA-1.
fn converted(provenance: &str) -> Option<(usize, String)> {
    let (_, at) = field(provenance, 0, "converted")?;
    let (size, _) = field(provenance, at, "size")?;
    let (sha1, _) = field(provenance, at, "sha1")?;
    Some((size.parse().ok()?, sha1.to_owned()))
}

fn sha1_of(bytes: &[u8]) -> String {
    let hashes = one_saves_convert::rom::hashes(bytes);
    let sha1 = hashes.iter().find(|hash| hash.algorithm() == HashAlgorithm::Sha1).expect("a SHA-1");
    sha1.digest().iter().fold(String::new(), |mut hex, byte| {
        let _ = write!(hex, "{byte:02x}");
        hex
    })
}

/// Every entry a downloaded save sits in, under the corpus's `nds` folder.
fn entries(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for writer in std::fs::read_dir(root).expect("the corpus's nds folder").flatten() {
        if !writer.path().is_dir() {
            continue;
        }
        for entry in std::fs::read_dir(writer.path()).into_iter().flatten().flatten() {
            if entry.file_name().to_string_lossy().contains("[GameFAQs ") {
                found.push(entry.path());
            }
        }
    }
    found.sort();
    found
}

#[test]
fn every_downloaded_ds_save_comes_out_as_the_chip_the_corpus_ran() {
    let Some(root) = std::env::var_os(VARIABLE) else {
        eprintln!("{VARIABLE} is unset, so no downloaded DS save is read");
        return;
    };
    let mut read = 0;
    let mut told_alone = 0;
    for entry in entries(Path::new(&root)) {
        let provenance = std::fs::read_to_string(entry.join("provenance.json")).expect("provenance");
        let Some((chip, sha1)) = converted(&provenance) else { continue };
        let name = entry.file_name().unwrap().to_string_lossy().into_owned();
        for path in std::fs::read_dir(&entry).unwrap().flatten().map(|file| file.path()) {
            let file_name = path.file_name().unwrap().to_string_lossy().into_owned();
            if file_name == "provenance.json" || file_name == "proof.png" {
                continue;
            }
            let bytes = std::fs::read(&path).unwrap();
            let options =
                NdsOptions { chip: Some(chip), file_name: Some(file_name), ..NdsOptions::default() };
            let result = if bytes.starts_with(b"DeSmuME SState\0\0") {
                let opened = one_saves_convert::state::desmume::in_game_save(&bytes).expect("a save inside");
                nds::read_opened(&bytes, &opened, &options)
            } else {
                nds::read(&bytes, &options)
            };
            let result = result.unwrap_or_else(|e| panic!("{name}: {e}"));
            result.bundle.validate().unwrap_or_else(|e| panic!("{name}: {e}"));
            if name.contains(TWO_SAVES) {
                assert_eq!(result.chip, None, "{name}: two saves, and one chip");
                continue;
            }
            // Without the game, the file says the chip it was made from or goes whole: never another.
            let options = NdsOptions { file_name: Some("save".into()), ..NdsOptions::default() };
            let unknown = if bytes.starts_with(b"DeSmuME SState\0\0") {
                let opened = one_saves_convert::state::desmume::in_game_save(&bytes).unwrap();
                nds::read_opened(&bytes, &opened, &options)
            } else {
                nds::read(&bytes, &options)
            };
            let unknown = unknown.unwrap_or_else(|e| panic!("{name}, without the game: {e}"));
            if let Some(told) = unknown.chip {
                assert_eq!(told, chip, "{name}: without the game, read as the wrong chip");
                told_alone += 1;
            }
            let payload = &result.bundle.parts[0].payload;
            assert_eq!(
                (result.chip, payload.len(), sha1_of(payload)),
                (Some(chip), chip, sha1.clone()),
                "{name}, read as {}",
                result.container.label()
            );
            read += 1;
        }
    }
    println!(
        "{read} downloaded DS saves read as the chips the corpus ran, {told_alone} of them without the game"
    );
    assert!(read > 0, "{VARIABLE} names a folder holding no downloaded DS save");
}
