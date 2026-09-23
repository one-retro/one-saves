//! The registry data: read from the specification pages, written as JSON and as Rust.
//!
//! Two steps, deliberately separable. Syncing needs the specifications repository checked out;
//! generating needs only the JSON this repository already carries, which is what lets CI check
//! that the committed tables still match their data without fetching anything.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::markdown::{codes, one_code, prose, tables};

/// A gaming system, and the names other tools use for it.
#[derive(Serialize, Deserialize)]
pub struct System {
    pub slug: String,
    pub name: String,
    pub aliases: Vec<String>,
}

/// An emulator core.
#[derive(Serialize, Deserialize)]
pub struct Core {
    pub slug: String,
    pub name: String,
    pub kind: String,
    pub systems: Vec<String>,
    pub aliases: Vec<String>,
    /// The section it is browsed under, which is not its coverage.
    pub family: String,
}

/// A save role compared whole.
#[derive(Serialize, Deserialize)]
pub struct RoleEntry {
    pub role: String,
    pub description: String,
    pub common: bool,
    pub systems: Vec<String>,
}

/// A numbered-socket prefix.
#[derive(Serialize, Deserialize)]
pub struct RolePrefix {
    pub prefix: String,
    pub description: String,
    pub common: bool,
    pub systems: Vec<String>,
}

/// Both halves of the roles page.
#[derive(Serialize, Deserialize)]
pub struct Roles {
    pub roles: Vec<RoleEntry>,
    pub prefixes: Vec<RolePrefix>,
}

/// An assigned name in the `x` tree.
#[derive(Serialize, Deserialize)]
pub struct VendorEntry {
    pub name: String,
    pub kind: String,
    pub vendor: String,
    pub notes: String,
}

/// A memory card layout.
#[derive(Serialize, Deserialize)]
pub struct CardFormat {
    pub format: String,
    pub dirent_len: usize,
    /// The format's block size in bytes, which 0.2 added and every format has.
    pub block_size: usize,
    pub notes: String,
}

/// A spec-owned `device_kind`.
#[derive(Serialize, Deserialize)]
pub struct DeviceKind {
    pub value: String,
    pub meaning: String,
}

/// A spec-owned `binding`.
#[derive(Serialize, Deserialize)]
pub struct Binding {
    pub value: String,
    pub bound_to: String,
}

/// How one core lays out clock state, as `clocks.json` records it.
#[derive(Serialize, Deserialize)]
pub struct Clock {
    pub layout: String,
    #[serde(default)]
    pub reserved: Option<usize>,
}

/// `clocks.json`: clock layouts, keyed by core slug.
///
/// Deliberately not part of [`Registries`], and deliberately not written by [`write_json`]. The
/// specification pages do not describe clock layouts, so this file is this repository's own and a
/// `--docs` sync must leave it alone.
#[derive(Serialize, Deserialize)]
pub struct Clocks {
    #[serde(rename = "_note", default)]
    pub note: Vec<String>,
    pub cores: BTreeMap<String, Clock>,
}

/// Everything the registries hold.
pub struct Registries {
    pub systems: Vec<System>,
    pub cores: Vec<Core>,
    pub roles: Roles,
    pub vendors: Vec<VendorEntry>,
    pub card_formats: Vec<CardFormat>,
    pub device_kinds: Vec<DeviceKind>,
    pub bindings: Vec<Binding>,
}

type Fallible<T> = Result<T, String>;

fn read(path: &Path) -> Fallible<String> {
    std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Reads every registry out of the specification pages.
pub fn read_from_docs(docs: &Path) -> Fallible<Registries> {
    let content = docs.join("src/content/docs");
    Ok(Registries {
        systems: systems(&read(&content.join("registries/systems.md"))?)?,
        cores: cores(&read(&content.join("registries/cores.md"))?)?,
        roles: roles(&read(&content.join("registries/roles.md"))?)?,
        vendors: vendors(&read(&content.join("registries/vendors.md"))?)?,
        card_formats: card_formats(&read(&content.join("specifications/memory-cards.md"))?)?,
        // 0.2 split the format page: the per-field tables moved to `bundle.md`, and what stayed
        // behind is the prose about shapes and extensions.
        device_kinds: device_kinds(&read(&content.join("specifications/bundle.md"))?)?,
        bindings: bindings(&read(&content.join("specifications/bundle.md"))?)?,
    })
}

fn systems(page: &str) -> Fallible<Vec<System>> {
    let mut out = Vec::new();
    for table in tables(page).iter().filter(|t| t.headed_by("Slug")) {
        for row in &table.rows {
            out.push(System { slug: one_code(&row[0])?, name: row[1].clone(), aliases: codes(&row[2]) });
        }
    }
    out.sort_by(|a, b| a.slug.cmp(&b.slug));
    Ok(out)
}

fn cores(page: &str) -> Fallible<Vec<Core>> {
    let mut out = Vec::new();
    for table in tables(page).iter().filter(|t| t.headed_by("Slug")) {
        for row in &table.rows {
            out.push(Core {
                slug: one_code(&row[0])?,
                name: row[1].clone(),
                kind: one_code(&row[2])?,
                systems: codes(&row[3]),
                aliases: row.get(4).map(|c| codes(c)).unwrap_or_default(),
                family: table.heading.clone(),
            });
        }
    }
    out.sort_by(|a, b| a.slug.cmp(&b.slug));
    Ok(out)
}

/// One name's listing, before the per-system rows are folded together.
struct Listing {
    name: String,
    description: String,
    section: String,
    systems: Vec<String>,
}

/// Folds the per-system listings of one name into a single entry.
///
/// A role is registered once and may then be listed again under each system that has that socket,
/// so the sections are a browsing aid rather than a scope. A role in **Common** means the same
/// thing everywhere, which is not the same as being listed for no system at all, so that
/// distinction is kept rather than flattened into an empty list.
fn fold(listings: Vec<Listing>) -> Vec<(String, String, bool, Vec<String>)> {
    let mut merged: BTreeMap<String, (String, bool, Vec<String>)> = BTreeMap::new();
    for listing in listings {
        let entry =
            merged.entry(listing.name).or_insert_with(|| (listing.description.clone(), false, Vec::new()));
        if listing.section == "Common" || listing.section == "Numbered sockets" {
            entry.1 = true;
            entry.0 = listing.description;
        }
        for system in listing.systems {
            if !entry.2.contains(&system) {
                entry.2.push(system);
            }
        }
    }
    merged
        .into_iter()
        .map(|(name, (description, common, systems))| (name, description, common, systems))
        .collect()
}

fn roles(page: &str) -> Fallible<Roles> {
    let (mut plain, mut prefixed) = (Vec::new(), Vec::new());

    for table in tables(page) {
        if table.headed_by("Role") {
            for row in &table.rows {
                let name = one_code(&row[0])?;
                let listing = Listing {
                    description: prose(row.last().expect("a row has cells")),
                    section: table.heading.clone(),
                    // A three-column row names the systems it applies to in the middle cell.
                    systems: if row.len() == 3 { codes(&row[1]) } else { Vec::new() },
                    name: name.clone(),
                };
                if name.ends_with('-') { prefixed.push(listing) } else { plain.push(listing) }
            }
        } else if table.headed_by("Prefix") {
            for row in &table.rows {
                prefixed.push(Listing {
                    name: one_code(&row[0])?,
                    description: prose(&row[1]),
                    section: table.heading.clone(),
                    systems: Vec::new(),
                });
            }
        }
    }

    Ok(Roles {
        roles: fold(plain)
            .into_iter()
            .map(|(role, description, common, systems)| RoleEntry { role, description, common, systems })
            .collect(),
        prefixes: fold(prefixed)
            .into_iter()
            .map(|(prefix, description, common, systems)| RolePrefix { prefix, description, common, systems })
            .collect(),
    })
}

fn vendors(page: &str) -> Fallible<Vec<VendorEntry>> {
    let mut out = Vec::new();
    for table in tables(page).iter().filter(|t| t.headed_by("Name")) {
        for row in &table.rows {
            out.push(VendorEntry {
                name: one_code(&row[0])?,
                kind: row[1].clone(),
                vendor: row[2].clone(),
                notes: prose(&row[3]),
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn card_formats(page: &str) -> Fallible<Vec<CardFormat>> {
    let mut out = Vec::new();
    for table in tables(page).iter().filter(|t| t.headed_by("Format")) {
        for row in &table.rows {
            let dirent = row[1].trim();
            out.push(CardFormat {
                format: one_code(&row[0])?,
                dirent_len: dirent.parse().map_err(|_| format!("{dirent:?} is not a dirent length"))?,
                block_size: {
                    let block = row[2].trim();
                    block.parse().map_err(|_| format!("{block:?} is not a block size"))?
                },
                notes: prose(&row[3]),
            });
        }
    }
    Ok(out)
}

fn device_kinds(page: &str) -> Fallible<Vec<DeviceKind>> {
    let mut out = Vec::new();
    for table in tables(page).iter().filter(|t| t.header == ["Value", "Meaning"]) {
        for row in &table.rows {
            out.push(DeviceKind { value: one_code(&row[0])?, meaning: prose(&row[1]) });
        }
    }
    Ok(out)
}

fn bindings(page: &str) -> Fallible<Vec<Binding>> {
    let mut out = Vec::new();
    for table in tables(page).iter().filter(|t| t.header == ["Value", "Bound to"]) {
        for row in &table.rows {
            out.push(Binding { value: one_code(&row[0])?, bound_to: prose(&row[1]) });
        }
    }
    Ok(out)
}

/// Writes one registry file, pretty-printed and newline-terminated so it reviews as text.
fn store<T: Serialize>(dir: &Path, name: &str, value: &T) -> Fallible<()> {
    let path = dir.join(format!("{name}.json"));
    let json = serde_json::to_string_pretty(value).map_err(|e| format!("{name}.json: {e}"))?;
    std::fs::write(&path, json + "\n").map_err(|e| format!("{}: {e}", path.display()))
}

/// Writes the JSON this repository carries, which is what the Rust tables are generated from.
pub fn write_json(data: &Registries, dir: &Path) -> Fallible<()> {
    store(dir, "systems", &data.systems)?;
    store(dir, "cores", &data.cores)?;
    store(dir, "roles", &data.roles)?;
    store(dir, "vendors", &data.vendors)?;
    store(dir, "card-formats", &data.card_formats)?;
    store(dir, "device-kinds", &data.device_kinds)?;
    store(dir, "bindings", &data.bindings)?;
    Ok(())
}

/// Reads one registry file.
fn load<T: for<'de> Deserialize<'de>>(dir: &Path, name: &str) -> Fallible<T> {
    let text = read(&dir.join(format!("{name}.json")))?;
    serde_json::from_str(&text).map_err(|e| format!("{name}.json: {e}"))
}

/// Reads the clock layouts, which are this repository's rather than the specification's.
pub fn read_clocks(dir: &Path) -> Fallible<Clocks> {
    load(dir, "clocks")
}

/// Reads the JSON back, which is all generating the Rust tables needs.
pub fn read_json(dir: &Path) -> Fallible<Registries> {
    Ok(Registries {
        systems: load(dir, "systems")?,
        cores: load(dir, "cores")?,
        roles: load(dir, "roles")?,
        vendors: load(dir, "vendors")?,
        card_formats: load(dir, "card-formats")?,
        device_kinds: load(dir, "device-kinds")?,
        bindings: load(dir, "bindings")?,
    })
}
