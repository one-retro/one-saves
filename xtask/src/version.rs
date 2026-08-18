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

    let members = members(&manifest)?;
    let (current, rewritten) = retarget(&manifest, &members, how)?;
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
fn members(manifest: &str) -> Result<Vec<String>, String> {
    let start = manifest.find("members = [").ok_or("the root manifest has no `members` list")?;
    let list = &manifest[start..];
    let end = list.find(']').ok_or("the `members` list is not closed")?;

    let mut members = Vec::new();
    for quoted in list[..end].split('"').skip(1).step_by(2) {
        let name = quoted.rsplit('/').next().unwrap_or(quoted);
        members.push(name.to_owned());
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
fn retarget(manifest: &str, members: &[String], how: Bump) -> Result<(Version, String), String> {
    let mut section = String::new();
    let mut declared: Option<Version> = None;
    let mut pinned: Vec<(String, Version)> = Vec::new();
    let mut lines: Vec<String> = Vec::new();

    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            section = trimmed.to_owned();
            lines.push(line.to_owned());
            continue;
        }

        // The `[workspace.package]` version, which is what every member inherits. `rust-version`
        // sits in the same table and is not it, so the key is matched whole.
        if section == "[workspace.package]" && key_of(trimmed) == Some("version") {
            let (found, rewritten) = swap_version(line, how)?;
            declared = Some(found);
            lines.push(rewritten);
            continue;
        }

        // A member's pin. A third-party dependency in the same table keeps whatever it says.
        if section == "[workspace.dependencies]"
            && let Some(key) = key_of(trimmed)
            && members.iter().any(|member| member == key)
        {
            let (found, rewritten) = swap_version(line, how)?;
            pinned.push((key.to_owned(), found));
            lines.push(rewritten);
            continue;
        }

        lines.push(line.to_owned());
    }

    let declared = declared.ok_or("`[workspace.package]` has no `version`")?;
    for (member, pin) in &pinned {
        if *pin != declared {
            return Err(format!(
                "{member} is pinned at {pin} but the workspace is at {declared}; the manifest is \
                 half-edited, so fix it by hand before releasing"
            ));
        }
    }

    let mut rewritten = lines.join("\n");
    if manifest.ends_with('\n') {
        rewritten.push('\n');
    }
    Ok((declared, rewritten))
}

/// The key a manifest line assigns to, or `None` if it does not look like an assignment.
fn key_of(trimmed: &str) -> Option<&str> {
    let key = trimmed.split('=').next()?.trim();
    (!key.is_empty() && !key.starts_with('#')).then_some(key)
}

/// Replaces the quoted value of the `version` key on one line, whichever table it sits in.
///
/// The key is found rather than the value, because a line carries other quoted strings — a path,
/// a feature list — and a search for the old number would sooner or later hit one of them.
fn swap_version(line: &str, how: Bump) -> Result<(Version, String), String> {
    let mut at = 0;
    let found = loop {
        let offset = line[at..].find("version").ok_or_else(|| format!("no `version` in {line:?}"))?;
        let start = at + offset;
        // `rust-version` ends in `version` too, so the character before has to be one that cannot
        // continue a key.
        let joined =
            line[..start].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '-' || c == '_');
        at = start + "version".len();
        if !joined {
            break start;
        }
    };

    let rest = &line[found..];
    let open = rest.find('"').ok_or_else(|| format!("no quoted version in {line:?}"))?;
    let close = rest[open + 1..].find('"').ok_or_else(|| format!("unterminated version in {line:?}"))?;
    let text = &rest[open + 1..open + 1 + close];

    let current = Version::parse(text)?;
    let new = current.bumped(how);
    let value = found + open + 1;
    Ok((current, format!("{}{new}{}", &line[..value], &line[value + text.len()..])))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"[workspace]
members = [
    "crates/one-saves",
    "xtask",
]

[workspace.package]
version = "0.2.0"
rust-version = "1.88"

[workspace.dependencies]
one-saves = { version = "0.2.0", path = "crates/one-saves", default-features = false }
dcbor = "0.25"
datary = { version = "0.3", default-features = false }
"#;

    fn bumped(how: Bump) -> String {
        let members = members(MANIFEST).unwrap();
        retarget(MANIFEST, &members, how).unwrap().1
    }

    #[test]
    fn a_member_is_named_by_its_last_path_segment() {
        assert_eq!(members(MANIFEST).unwrap(), ["one-saves", "xtask"]);
    }

    #[test]
    fn every_copy_of_the_version_moves_together() {
        let out = bumped(Bump::Patch);
        assert!(out.contains("version = \"0.2.1\"\n"), "{out}");
        assert!(out.contains("one-saves = { version = \"0.2.1\", path"), "{out}");
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
        // It ends in `version` and sits in the same table, which is the one way this could go
        // quietly wrong: a bumped MSRV would be caught by nothing until CI.
        for how in [Bump::Major, Bump::Minor, Bump::Patch] {
            assert!(bumped(how).contains("rust-version = \"1.88\""), "{how:?}");
        }
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
        let members = members(&drifted).unwrap();
        let error = retarget(&drifted, &members, Bump::Patch).unwrap_err();
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
