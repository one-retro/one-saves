//! Identify every file under a directory and tally what was found, folder by folder.
//!
//! ```text
//! cargo run -p rom-id --example survey -- /path/to/gba /path/to/snes ...
//! ```
//!
//! Each line is a folder: how many files, how many a header identified, how many only their name,
//! and how many neither. Below it, every file whose header names a system its name does not
//! allow, and every cartridge clock found, since those are the answers worth checking by hand.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Default)]
struct Tally {
    files: usize,
    header: usize,
    name: usize,
    none: usize,
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            walk(&path, out);
        } else if kind.is_file()
            && !path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.'))
        {
            out.push(path);
        }
    }
}

fn main() {
    let roots: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    assert!(!roots.is_empty(), "at least one directory to survey");
    let mut files = Vec::new();
    for root in &roots {
        let mut found = Vec::new();
        walk(root, &mut found);
        found.sort();
        // A folder is shown from its root's own name, so two roots' folders stay apart.
        let base = root.parent().unwrap_or(root);
        files.extend(found.into_iter().map(|path| (base.to_path_buf(), path)));
    }
    eprintln!("{} files", files.len());

    // A share answers slowly but in parallel, so many readers at once go much faster than one.
    let next = AtomicUsize::new(0);
    let mut outcomes: Vec<(usize, Outcome)> = std::thread::scope(|scope| {
        let readers: Vec<_> = (0..READERS)
            .map(|_| {
                scope.spawn(|| {
                    let mut mine = Vec::new();
                    loop {
                        let at = next.fetch_add(1, Ordering::Relaxed);
                        let Some((_, path)) = files.get(at) else { break mine };
                        if at.is_multiple_of(500) {
                            eprintln!("{at}/{} {}", files.len(), path.display());
                        }
                        mine.push((at, survey(path)));
                    }
                })
            })
            .collect();
        readers.into_iter().flat_map(|reader| reader.join().expect("a reader panicked")).collect()
    });
    outcomes.sort_by_key(|(at, _)| *at);

    let mut tallies: BTreeMap<String, Tally> = BTreeMap::new();
    let mut disagreements = Vec::new();
    let mut clocks = Vec::new();
    let mut by_name = Vec::new();
    for (at, outcome) in outcomes {
        let (base, path) = &files[at];
        let folder = path
            .parent()
            .map(|p| p.strip_prefix(base).unwrap_or(p).display().to_string())
            .unwrap_or_default();
        let tally = tallies.entry(folder).or_default();
        tally.files += 1;
        match outcome {
            Outcome::None => tally.none += 1,
            Outcome::Name(system) => {
                tally.name += 1;
                // The first few of each folder: a header that should have been found and was not.
                if tally.name <= 3 {
                    by_name.push(format!("{} ({system})", path.display()));
                }
            }
            Outcome::Header { disagrees, clock } => {
                tally.header += 1;
                disagreements.extend(disagrees.map(|system| format!("{} -> {system}", path.display())));
                clocks.extend(clock.map(|clock| format!("{clock} {}", path.display())));
            }
        }
    }

    println!("{:>6} {:>6} {:>6} {:>6}  folder", "files", "header", "name", "none");
    for (folder, tally) in &tallies {
        println!("{:>6} {:>6} {:>6} {:>6}  {folder}", tally.files, tally.header, tally.name, tally.none);
    }
    for (title, lines) in [
        ("header disagrees with the name", &disagreements),
        ("clocks", &clocks),
        ("named but no header (first 3 a folder)", &by_name),
    ] {
        println!("\n{title} ({}):", lines.len());
        for line in lines {
            println!("  {line}");
        }
    }
}

const READERS: usize = 16;

/// What one file came to.
enum Outcome {
    None,
    Name(&'static str),
    /// The system its header names when its name allows another, and its clock.
    Header {
        disagrees: Option<&'static str>,
        clock: Option<String>,
    },
}

fn survey(path: &Path) -> Outcome {
    let name = path.to_string_lossy();
    let Ok(mut file) = std::fs::File::open(path) else { return Outcome::None };
    let Ok(Some(id)) = rom_id::identify_file(Some(&name), &mut file) else { return Outcome::None };
    if id.evidence == rom_id::Evidence::Name {
        return Outcome::Name(id.system.slug);
    }
    let allowed = rom_id::candidates(&name);
    Outcome::Header {
        disagrees: (!allowed.is_empty() && !allowed.contains(&id.system)).then_some(id.system.slug),
        clock: id.cartridge.clock.map(|clock| format!("{clock:?} ({})", id.system.slug)),
    }
}
