//! The registries the Universal Saves Format draws its vocabularies from.
//!
//! Five of the format's fields take a name a producer **cannot** mint: a header's `system`, a
//! card's `format`, a part's `role` and `binding`, and a source's `device_kind`. Those name
//! categories everyone shares, so minting into one would not extend it but split it — two
//! producers writing `io.mgba.emulator` and `com.libretro.emulator` have named one concept twice,
//! and a consumer grouping by that field now sees two unrelated strings. This crate is those
//! vocabularies, as data.
//!
//! The tables are **non-normative and first-come**: a system or a core is added by opening a PR
//! against the registry page, not by a new version of the format. So a name absent here is not
//! thereby invalid, and [`system`] returning `None` means "not listed", never "malformed".
//!
//! # Mapping what you have onto what to write
//!
//! The **also seen as** column of each registry records the names other tools use for the same
//! thing — directory names, core names, database labels. They exist so a producer can map its
//! input onto the right slug, and they are **never emitted**.
//!
//! ```
//! use one_saves_registry::{system_by_any_name, core_by_any_name};
//!
//! // A libretro `saves/` folder, a MiSTer core name, and a database label all land on one slug.
//! assert_eq!(system_by_any_name("megadrive").unwrap().slug, "genesis");
//! assert_eq!(system_by_any_name("ps1").unwrap().slug, "psx");
//! assert_eq!(core_by_any_name("Genesis_MiSTer").unwrap().slug, "megadrive-mister");
//! ```
//!
//! # Roles, and the one place a slug has structure
//!
//! A role ending in `-<n>` names the nth socket of the kind its prefix names, so a consumer that
//! recognises `memcard-` matches `memcard-8` without that exact role being listed. [`role`]
//! handles both arms; see [`RoleMatch`].

mod generated;

use generated::{BINDINGS, CARD_FORMATS, CORES, DEVICE_KINDS, ROLE_PREFIXES, ROLES, SYSTEMS, VENDORS};

/// A gaming system, and the names other tools use for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct System {
    /// The canonical slug, which is what a bundle's `system` field carries.
    pub slug: &'static str,
    /// The system's name in prose.
    pub name: &'static str,
    /// Other names for the same system. Never emitted.
    pub aliases: &'static [&'static str],
}

/// What hosts an emulator core.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreKind {
    /// A software core hosted by RetroArch, OnionOS and most handheld firmwares. Its canonical
    /// name is the core's `library_name`, the exact string that names a `saves/<core>/` folder.
    Libretro,
    /// A MiSTer FPGA core, named for its repository in the MiSTer-devel organization.
    MiSTer,
    /// An Analogue Pocket openFPGA core, named for the `Author.Platform` folder it installs to.
    OpenFpga,
}

/// An emulator core: the engine that runs the game, as distinct from the frontend hosting it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Core {
    /// The canonical slug, which is what a `source.app` field carries for this core.
    ///
    /// A core listed here **must** write this rather than a reverse-DNS name, even when it holds
    /// a domain, so that one core has exactly one spelling. mGBA is `mgba`, never `io.mgba`.
    pub slug: &'static str,
    /// The core's name as its authors write it.
    pub name: &'static str,
    /// What hosts it.
    pub kind: CoreKind,
    /// The systems it runs, as system slugs. This is the authoritative coverage.
    pub systems: &'static [&'static str],
    /// Former names, SD-card folders and rebuilds that count as the same core. Never emitted.
    pub aliases: &'static [&'static str],
}

/// A save role: which socket a save came out of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Role {
    /// The role, as a part's `role` field carries it.
    pub name: &'static str,
    /// What socket it names.
    pub description: &'static str,
    /// Whether it means the same thing on every system.
    pub common: bool,
    /// Systems the registry lists it under. A browsing aid, not a restriction: a `common` role
    /// applies everywhere whatever this holds.
    pub systems: &'static [&'static str],
}

/// A numbered-socket prefix, registered once and covering every number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RolePrefix {
    /// The prefix, including its trailing `-`.
    pub prefix: &'static str,
    /// What kind of socket it numbers.
    pub description: &'static str,
    /// Systems the registry lists it under.
    pub systems: &'static [&'static str],
}

/// A memory card layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CardFormat {
    /// The format, as a card's `format` field carries it.
    pub name: &'static str,
    /// How many bytes this format's `dirent` runs to.
    ///
    /// `None` for `saturn-bup`, which keeps its entry inside the save's first block and so omits
    /// the key entirely.
    pub dirent_len: Option<usize>,
    /// What is particular about this format.
    pub notes: &'static str,
}

/// A kind of producer, as a `source.device_kind` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceKind {
    /// The value.
    pub name: &'static str,
    /// What it means.
    pub meaning: &'static str,
}

/// What a bound payload is stuck to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    /// The value, as a part's `binding` carries it.
    pub name: &'static str,
    /// What the payload is bound to.
    pub bound_to: &'static str,
}

/// An assigned name in the `x` tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vendor {
    /// The two-label name, such as `x.1sav`.
    pub name: &'static str,
    /// Whether these specifications reserved it rather than assigning it to a vendor.
    pub reserved: bool,
    /// The vendor holding it, when one does.
    pub holder: Option<&'static str>,
    /// What it covers.
    pub notes: &'static str,
}

/// Every listed system, sorted by slug.
#[must_use]
pub fn systems() -> &'static [System] {
    &SYSTEMS
}

/// The system a slug names, if it is listed.
#[must_use]
pub fn system(slug: &str) -> Option<&'static System> {
    SYSTEMS.binary_search_by_key(&slug, |s| s.slug).ok().map(|i| &SYSTEMS[i])
}

/// The system a slug or any of its recorded aliases names.
///
/// Matching is case-insensitive, because the names this maps from are directory names and
/// database labels rather than values out of a bundle. The slug it returns is always the
/// canonical lowercase one.
#[must_use]
pub fn system_by_any_name(name: &str) -> Option<&'static System> {
    system(name).or_else(|| {
        SYSTEMS.iter().find(|system| {
            system.slug.eq_ignore_ascii_case(name)
                || system.aliases.iter().any(|alias| alias.eq_ignore_ascii_case(name))
        })
    })
}

/// Every listed core, sorted by slug.
#[must_use]
pub fn cores() -> &'static [Core] {
    &CORES
}

/// The core a slug names, if it is listed.
#[must_use]
pub fn core(slug: &str) -> Option<&'static Core> {
    CORES.binary_search_by_key(&slug, |c| c.slug).ok().map(|i| &CORES[i])
}

/// The core a slug, a canonical name, or any recorded alias names.
///
/// This is what maps a `saves/<core>/` folder or a MiSTer core name onto the slug to write.
#[must_use]
pub fn core_by_any_name(name: &str) -> Option<&'static Core> {
    core(name).or_else(|| {
        CORES.iter().find(|core| {
            core.slug.eq_ignore_ascii_case(name)
                || core.name.eq_ignore_ascii_case(name)
                || core.aliases.iter().any(|alias| alias.eq_ignore_ascii_case(name))
        })
    })
}

/// Every core that runs a given system.
pub fn cores_for_system(slug: &str) -> impl Iterator<Item = &'static Core> {
    CORES.iter().filter(move |core| core.systems.contains(&slug))
}

/// Every registered role compared whole, sorted by name.
#[must_use]
pub fn roles() -> &'static [Role] {
    &ROLES
}

/// Every numbered-socket prefix.
#[must_use]
pub fn role_prefixes() -> &'static [RolePrefix] {
    &ROLE_PREFIXES
}

/// What a role resolves to.
///
/// A consumer meeting [`Unknown`](RoleMatch::Unknown) **must not** guess which socket is meant.
/// Round-trip it and show it, but never match on it: restoring a controller pak's save into a
/// cartridge slot is worse than declining to restore it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleMatch {
    /// A role listed in the registry, compared whole.
    Registered(&'static Role),
    /// The nth socket of a registered kind, as `memcard-8` is the eighth `memcard-`.
    Numbered(&'static RolePrefix, u32),
    /// Not a role this registry lists.
    Unknown,
}

/// Resolves a role, handling both a whole name and a numbered socket.
///
/// ```
/// use one_saves_registry::{role, RoleMatch};
///
/// assert!(matches!(role("cartridge"), RoleMatch::Registered(_)));
///
/// // A prefix is registered once and covers every number, listed or not.
/// let RoleMatch::Numbered(prefix, n) = role("memcard-8") else { panic!() };
/// assert_eq!((prefix.prefix, n), ("memcard-", 8));
///
/// // An unrecognised prefix stays unrecognised however it ends.
/// assert_eq!(role("atari-2600"), RoleMatch::Unknown);
/// ```
#[must_use]
pub fn role(name: &str) -> RoleMatch {
    if let Ok(index) = ROLES.binary_search_by_key(&name, |r| r.name) {
        return RoleMatch::Registered(&ROLES[index]);
    }
    // The number is decimal with no leading zero, so `memcard-01` is not socket 1 spelled oddly
    // but an ordinary slug that happens to end in digits.
    if let Some(split) = name.rfind('-') {
        let (prefix, digits) = name.split_at(split + 1);
        let plausible = !digits.is_empty()
            && digits.bytes().all(|b| b.is_ascii_digit())
            && (digits.len() == 1 || !digits.starts_with('0'));
        if plausible
            && let Some(entry) = ROLE_PREFIXES.iter().find(|entry| entry.prefix == prefix)
            && let Ok(number) = digits.parse()
        {
            return RoleMatch::Numbered(entry, number);
        }
    }
    RoleMatch::Unknown
}

/// Every card format the Memory Cards specification has rebuild rules for.
#[must_use]
pub fn card_formats() -> &'static [CardFormat] {
    &CARD_FORMATS
}

/// The card format a slug names.
#[must_use]
pub fn card_format(name: &str) -> Option<&'static CardFormat> {
    CARD_FORMATS.iter().find(|format| format.name == name)
}

/// Every value the spec-owned `device_kind` vocabulary holds.
#[must_use]
pub fn device_kinds() -> &'static [DeviceKind] {
    &DEVICE_KINDS
}

/// Whether a value is a `device_kind` this specification defines.
#[must_use]
pub fn is_device_kind(name: &str) -> bool {
    DEVICE_KINDS.iter().any(|kind| kind.name == name)
}

/// Every value the spec-owned `binding` vocabulary holds.
#[must_use]
pub fn bindings() -> &'static [Binding] {
    &BINDINGS
}

/// Whether a value is a `binding` this specification defines.
#[must_use]
pub fn is_binding(name: &str) -> bool {
    BINDINGS.iter().any(|binding| binding.name == name)
}

/// Every assigned name in the `x` tree.
#[must_use]
pub fn vendors() -> &'static [Vendor] {
    &VENDORS
}

/// The `x`-tree assignment a name falls under, if any.
///
/// A name in the tree is exactly two labels, and below its own name a vendor is on its own, so
/// `x.fceumm.anything` resolves to the `x.fceumm` assignment.
#[must_use]
pub fn vendor(name: &str) -> Option<&'static Vendor> {
    let two_labels = {
        let mut labels = name.split('.');
        match (labels.next(), labels.next()) {
            (Some(first), Some(second)) => format!("{first}.{second}"),
            _ => return None,
        }
    };
    VENDORS.iter().find(|vendor| vendor.name == two_labels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tables_are_sorted_so_binary_search_is_sound() {
        assert!(SYSTEMS.windows(2).all(|w| w[0].slug < w[1].slug), "systems must be sorted by slug");
        assert!(CORES.windows(2).all(|w| w[0].slug < w[1].slug), "cores must be sorted by slug");
        assert!(ROLES.windows(2).all(|w| w[0].name < w[1].name), "roles must be sorted by name");
    }

    #[test]
    fn the_registries_are_the_size_the_documents_are() {
        assert_eq!(SYSTEMS.len(), 49);
        assert_eq!(CORES.len(), 122);
        assert_eq!(CARD_FORMATS.len(), 7);
        assert_eq!(DEVICE_KINDS.len(), 5);
        assert_eq!(BINDINGS.len(), 4);
    }

    #[test]
    fn a_slug_names_one_system_and_not_a_family() {
        // Successive generations get their own slug even where their media are compatible, so a
        // tool declines a PS2 card for a PS1 emulator by ordinary matching rather than a special
        // case.
        assert_ne!(system("psx"), system("ps2"));
        assert_eq!(system_by_any_name("playstation").unwrap().slug, "psx");
    }

    #[test]
    fn aliases_map_the_names_other_tools_use() {
        for (alias, slug) in [
            ("megadrive", "genesis"),
            ("md", "genesis"),
            ("ps1", "psx"),
            ("gameboy", "gb"),
            ("ngc", "gc"),
            ("2600", "atari-2600"),
        ] {
            assert_eq!(system_by_any_name(alias).map(|s| s.slug), Some(slug), "{alias}");
        }
    }

    #[test]
    fn a_core_listed_here_has_exactly_one_spelling() {
        // mGBA holds mgba.io and is listed, so it writes `mgba` and never `io.mgba`.
        let mgba = core("mgba").expect("mgba is listed");
        assert_eq!(mgba.kind, CoreKind::Libretro);
        assert!(mgba.systems.contains(&"gba"));
        assert_eq!(core_by_any_name("mGBA").map(|c| c.slug), Some("mgba"));
    }

    #[test]
    fn a_renamed_core_still_resolves_through_its_old_name() {
        assert_eq!(core_by_any_name("Genesis_MiSTer").map(|c| c.slug), Some("megadrive-mister"));
        assert_eq!(core_by_any_name("Mednafen Saturn").map(|c| c.slug), Some("beetle-saturn"));
    }

    #[test]
    fn every_core_runs_a_listed_system() {
        for core in cores() {
            for slug in core.systems {
                assert!(system(slug).is_some(), "{} runs unlisted system {slug}", core.slug);
            }
        }
    }

    #[test]
    fn numbered_sockets_resolve_past_what_is_listed() {
        // A device with eight card slots writes `memcard-8` and is understood.
        for n in [1u32, 2, 8, 64] {
            let RoleMatch::Numbered(prefix, got) = role(&format!("memcard-{n}")) else {
                panic!("memcard-{n} should resolve as a numbered socket");
            };
            assert_eq!((prefix.prefix, got), ("memcard-", n));
        }
    }

    #[test]
    fn an_unrecognised_prefix_stays_unrecognised_however_it_ends() {
        // This is the only place the specifications read structure out of a slug, and it applies
        // to registered prefixes only. A system slug that happens to end in digits is not a socket.
        assert_eq!(role("atari-2600"), RoleMatch::Unknown);
        assert_eq!(role("nosuchthing-1"), RoleMatch::Unknown);
        // No leading zero, so this is not socket 1 under a second spelling.
        assert_eq!(role("memcard-01"), RoleMatch::Unknown);
    }

    #[test]
    fn dirent_lengths_are_what_the_memory_cards_spec_fixes() {
        for (name, len) in [
            ("ps1-mc", Some(128)),
            ("ps2-mc", Some(512)),
            ("n64-cpak", Some(32)),
            ("gc-mc", Some(64)),
            ("vmu", Some(32)),
            ("neogeo-mc", Some(4)),
            // Saturn keeps its entry inside the save's first block, so the key is absent.
            ("saturn-bup", None),
        ] {
            assert_eq!(card_format(name).unwrap().dirent_len, len, "{name}");
        }
    }

    #[test]
    fn the_x_tree_resolves_below_an_assignment() {
        assert_eq!(vendor("x.1sav").map(|v| v.name), Some("x.1sav"));
        // Below its own name a vendor is on its own, with nothing further to register.
        assert_eq!(vendor("x.1sav.rtc").map(|v| v.name), Some("x.1sav"));
        assert!(vendor("x.1sav").unwrap().reserved);
        assert_eq!(vendor("io.mgba"), None);
    }
}
