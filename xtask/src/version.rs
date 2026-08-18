//! Bumping the one version every crate in this workspace shares.
//!
//! There is no per-crate version to edit: each member carries `version.workspace = true`, so the
//! number lives in the root manifest twice over — once under `[workspace.package]`, and once in
//! each member's entry under `[workspace.dependencies]`, where the pin is what makes a published
//! crate depend on the version it was released beside rather than on whatever is newest.
//!
//! Those copies have to move together. Editing them by hand is ten lines in one file and works
//! right up until it does not, and a pin left behind is not caught until `cargo publish` resolves
//! it against the registry, which is the worst moment to find out.

use std::fmt;
use std::path::Path;

use toml_edit::{DocumentMut, Item, value};

/// A version as this workspace states it: three numbers and nothing else.
///
/// No pre-release and no build metadata, because nothing here has ever wanted one and refusing
/// them keeps the arithmetic below total.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Version {
    major: u64,
    minor: u64,
    patch: u64,
}

impl Version {
    fn parse(text: &str) -> Result<Self, String> {
        let mut parts = text.split('.');
        let mut number = |what: &str| -> Result<u64, String> {
            parts
                .next()
                .ok_or_else(|| format!("{text:?} has no {what}"))?
                .parse()
                .map_err(|_| format!("{text:?} has a {what} that is not a number"))
        };
        let version = Version { major: number("major")?, minor: number("minor")?, patch: number("patch")? };
        match parts.next() {
            None => Ok(version),
            Some(_) => Err(format!("{text:?} has more than three components")),
        }
    }

    /// This version, moved as `how` says.
    ///
    /// The zero-major rules are Cargo's, not semver's: while the major is 0 the *minor* is the
    /// breaking position, so `patch` is what an additive change takes and `minor` is what a
    /// breaking one takes. `major` is the move to 1.0, which for this workspace means the
    /// specification stabilized.
    fn bumped(self, how: Bump) -> Self {
        match how {
            Bump::Major => Version { major: self.major + 1, minor: 0, patch: 0 },
            Bump::Minor => Version { minor: self.minor + 1, patch: 0, ..self },
            Bump::Patch => Version { patch: self.patch + 1, ..self },
            Bump::Same => self,
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Which number moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bump {
    Major,
    Minor,
    Patch,
    /// Leave the version alone and report what it already is.
    ///
    /// For a release whose bump was made by hand, and for a second run after one failed between
    /// the bump and the publish.
    Same,
}

impl Bump {
    fn parse(text: &str) -> Result<Self, String> {
        match text {
            "major" => Ok(Bump::Major),
            "minor" => Ok(Bump::Minor),
            "patch" => Ok(Bump::Patch),
            "same" => Ok(Bump::Same),
            other => Err(format!("{other:?} is not major, minor, patch or same")),
        }
    }
}

/// Sets the workspace version, and prints it.
///
/// The new version goes to stdout on its own, and everything else to stderr, so a caller can take
/// the number without parsing anything: `version=$(cargo xtask version patch)`.
pub(crate) fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let [how] = args else {
        return Err("expected `version <major|minor|patch|same>`".to_owned());
    };
    let how = Bump::parse(how)?;

    let path = root.join("Cargo.toml");
    let manifest = std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;

    let (current, rewritten) = retarget(&manifest, how)?;
    let new = current.bumped(how);

    if how == Bump::Same {
        eprintln!("version is {current}, unchanged");
    } else {
        std::fs::write(&path, rewritten).map_err(|e| format!("write {}: {e}", path.display()))?;
        eprintln!("{current} -> {new} in {}", path.display());
    }
    println!("{new}");
    Ok(())
}

/// The workspace members, by the crate name each directory carries.
///
/// Read from the manifest rather than assumed, so a member added later is covered without
/// touching this file. The name is the last path segment, which is what every member here uses.
fn members(doc: &DocumentMut) -> Result<Vec<String>, String> {
    let list = doc["workspace"]["members"].as_array().ok_or("the root manifest has no `members` list")?;

    let mut members = Vec::new();
    for entry in list {
        let path = entry.as_str().ok_or("a `members` entry is not a string")?;
        members.push(path.rsplit('/').next().unwrap_or(path).to_owned());
    }
    if members.is_empty() {
        return Err("the `members` list is empty".to_owned());
    }
    Ok(members)
}

/// Rewrites every copy of the version, and checks they all held the same one to begin with.
///
/// Returns the version that was there and the manifest with the new one in it. A pin that
/// disagreed with `[workspace.package]` is an error rather than something to quietly fix: the two
/// only drift when an edit went half-done, and which half was right is not this task's to guess.
fn retarget(manifest: &str, how: Bump) -> Result<(Version, String), String> {
    let mut doc: DocumentMut =
        manifest.parse().map_err(|e| format!("the root manifest is not valid TOML: {e}"))?;
    let members = members(&doc)?;

    let declared = Version::parse(
        doc["workspace"]["package"]["version"].as_str().ok_or("`[workspace.package]` has no `version`")?,
    )?;
    let new = declared.bumped(how);

    set_version(&mut doc["workspace"]["package"], new)?;

    let deps = doc["workspace"]["dependencies"]
        .as_table_mut()
        .ok_or("the root manifest has no `[workspace.dependencies]`")?;
    for member in &members {
        // A member with no entry here is one nothing else depends on, which is fine: `xtask` is
        // one, and so is the binary. A third-party dependency in the same table is not a member
        // and keeps whatever it says.
        let Some(dep) = deps.get_mut(member) else { continue };
        let Some(pin) = set_version(dep, new)? else { continue };
        if pin != declared {
            return Err(format!(
                "{member} is pinned at {pin} but the workspace is at {declared}; the manifest is \
                 half-edited, so fix it by hand before releasing"
            ));
        }
    }

    Ok((declared, doc.to_string()))
}

/// Replaces the `version` of one table, returning what was there.
///
/// `None` means the table has no `version` at all, which for a dependency is a path-only entry.
/// The value is swapped in under the key's existing decor, so the spacing and any trailing comment
/// on that line survive; addressing the key rather than searching the text for the old number is
/// the whole reason this task uses a TOML parser.
fn set_version(item: &mut Item, new: Version) -> Result<Option<Version>, String> {
    let Some(table) = item.as_table_like_mut() else { return Ok(None) };
    let Some(slot) = table.get_mut("version") else { return Ok(None) };
    let Some(current) = slot.as_str().map(Version::parse).transpose()? else { return Ok(None) };

    let decor = slot.as_value().map(|v| v.decor().clone());
    *slot = value(new.to_string());
    if let (Some(rewritten), Some(decor)) = (slot.as_value_mut(), decor) {
        *rewritten.decor_mut() = decor;
    }
    Ok(Some(current))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"[workspace]
members = [
    "crates/one-saves",
    "xtask",
]

# Why this crate pays for what it pays for.
[workspace.package]
version = "0.2.0"
rust-version = "1.88"

[workspace.dependencies]
one-saves = { version = "0.2.0", path = "crates/one-saves", default-features = false }
dcbor = "0.25"
datary = { version = "0.3", default-features = false }
"#;

    fn bumped(how: Bump) -> String {
        retarget(MANIFEST, how).unwrap().1
    }

    #[test]
    fn a_member_is_named_by_its_last_path_segment() {
        let doc: DocumentMut = MANIFEST.parse().unwrap();
        assert_eq!(members(&doc).unwrap(), ["one-saves", "xtask"]);
    }

    #[test]
    fn every_copy_of_the_version_moves_together() {
        let out = bumped(Bump::Patch);
        assert!(out.contains("version = \"0.2.1\"\n"), "{out}");
        assert!(out.contains("one-saves = { version = \"0.2.1\", path"), "{out}");
    }

    #[test]
    fn nothing_but_the_version_is_touched() {
        // The point of parsing rather than scanning: comments, key order and spacing all survive,
        // and the only bytes that differ are the ones that had to.
        let out = bumped(Bump::Patch);
        assert_eq!(out, MANIFEST.replace("\"0.2.0\"", "\"0.2.1\""), "{out}");
    }

    #[test]
    fn a_third_party_pin_is_left_alone() {
        // `datary` is at 0.3 in the same table and has nothing to do with this workspace.
        let out = bumped(Bump::Major);
        assert!(out.contains("dcbor = \"0.25\""), "{out}");
        assert!(out.contains("datary = { version = \"0.3\""), "{out}");
    }

    #[test]
    fn rust_version_is_not_the_version() {
        // It ends in `version` and sits in the same table, which a search over the text has to be
        // told about and a parser simply cannot confuse.
        for how in [Bump::Major, Bump::Minor, Bump::Patch] {
            assert!(bumped(how).contains("rust-version = \"1.88\""), "{how:?}");
        }
    }

    #[test]
    fn a_path_may_contain_the_word_version() {
        // What the hand-rolled scanner this replaced got wrong: it found `version` inside the path
        // string and failed on a manifest that is perfectly legal.
        let m = MANIFEST.replace(
            "one-saves = { version = \"0.2.0\", path = \"crates/one-saves\"",
            "one-saves = { path = \"crates/version-x\", version = \"0.2.0\"",
        );
        let out = retarget(&m, Bump::Patch).unwrap().1;
        assert!(out.contains("path = \"crates/version-x\", version = \"0.2.1\""), "{out}");
    }

    #[test]
    fn same_rewrites_nothing() {
        assert_eq!(bumped(Bump::Same), MANIFEST);
    }

    #[test]
    fn the_zero_major_rules_are_cargos() {
        let v = Version::parse("0.2.3").unwrap();
        assert_eq!(v.bumped(Bump::Patch).to_string(), "0.2.4");
        assert_eq!(v.bumped(Bump::Minor).to_string(), "0.3.0");
        assert_eq!(v.bumped(Bump::Major).to_string(), "1.0.0");
    }

    #[test]
    fn a_half_edited_manifest_is_an_error_rather_than_a_guess() {
        let drifted =
            MANIFEST.replace("one-saves = { version = \"0.2.0\"", "one-saves = { version = \"0.1.0\"");
        let error = retarget(&drifted, Bump::Patch).unwrap_err();
        assert!(error.contains("pinned at 0.1.0"), "{error}");
    }

    #[test]
    fn a_version_is_three_numbers_and_nothing_else() {
        assert!(Version::parse("0.2").is_err());
        assert!(Version::parse("0.2.0.1").is_err());
        assert!(Version::parse("0.2.0-rc.1").is_err());
        assert!(Version::parse("").is_err());
    }
}
