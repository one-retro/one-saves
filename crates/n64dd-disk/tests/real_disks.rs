//! Real 64DD disk images, held against the hashes No-Intro catalogs for their retail dumps.
//!
//! None of these disks is in this repository, and none may be: they are commercial dumps. So these
//! tests read them where they lie. `N64DD_DUMPS` names the folders to look in, separated as `PATH`
//! is, and a disk is found in one of them or in a folder directly inside one, under any of the
//! names it goes by. With the variable unset, every test here passes without reading anything.

use std::path::PathBuf;

use n64dd_disk::{Disk, Layout, RETAIL_LEN};
use sha1::{Digest, Sha1};

const VARIABLE: &str = "N64DD_DUMPS";

/// A disk No-Intro has a retail dump of.
struct Known {
    /// What a file of it may be called.
    names: &'static [&'static str],
    /// The SHA-1 of its retail dump, from No-Intro's 64DD DAT of 2026-05-19.
    sha1: &'static str,
}

const KNOWN: [Known; 1] = [Known {
    names: &[
        "Super Mario 64 - Disk Version (Japan) (Proto).ndd",
        "Super Mario 64 - Disk Version (Japan) (Proto).n64",
    ],
    sha1: "e8f5c3f0e151cbc3b02184a2437e39fa9d1e2686",
}];

/// Every file in the folders the variable names, and in the folders directly inside them.
fn dumps() -> Vec<PathBuf> {
    let Some(roots) = std::env::var_os(VARIABLE) else {
        eprintln!("{VARIABLE} is unset, so no real disk is read");
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

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// A disk in either layout comes out in its retail form as the dump No-Intro catalogs, which is
/// what puts MAME's image in block order right, defect table and all.
#[test]
fn a_disk_s_retail_form_is_the_dump_no_intro_catalogs() {
    let dumps = dumps();
    for known in KNOWN {
        let Some(path) = dumps.iter().find(|path| {
            path.file_name().and_then(|name| name.to_str()).is_some_and(|name| known.names.contains(&name))
        }) else {
            continue;
        };
        let image = std::fs::read(path).expect("reads");
        let disk = Disk::parse(&image).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let retail = disk.to_retail().expect("converts");
        assert_eq!(retail.len(), RETAIL_LEN);
        assert_eq!(hex(&Sha1::digest(&retail)), known.sha1, "{}", path.display());
        eprintln!("{}: {:?}, type {}, as No-Intro's dump", path.display(), disk.layout(), disk.disk_type());

        // And back: MAME's order from the retail dump holds every block where the image did.
        if disk.layout() == Layout::Mame {
            let again = Disk::parse(&retail).expect("reads the retail form");
            assert_eq!(again.ram_area(), disk.ram_area());
            assert_eq!(again.disk_id(), disk.disk_id());
        }
    }
}
