//! Emulator profiles: what `--from mgba` tells a converter.
//!
//! A profile answers three questions a bare file cannot: what kind of producer wrote these bytes,
//! what name that producer writes for itself, and which systems it could plausibly have been
//! emulating. The first two become the bundle's [`source`](one_saves::Source); the third narrows
//! the guess at `system` when nothing else settles it.
//!
//! Which arm of `app` a producer takes is not a preference. A core listed in the
//! [Emulator Cores registry](mod@one_saves_registry::cores) **must** write its slug, and one with no
//! entry **must** write a reverse-DNS name, so a producer has exactly one spelling. That is why
//! this module resolves through the registry first and falls back second.

use one_saves::{Name, Slug, Source};

/// A named producer of saves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    /// What the user typed, and what the CLI lists.
    pub key: &'static str,
    /// How the producer is described in prose.
    pub label: &'static str,
    /// The `device_kind` for bundles this producer wrote.
    pub device_kind: &'static str,
    /// The `app` name, already resolved to the arm this producer must take.
    pub app: &'static str,
    /// The systems this producer covers, as system slugs. Empty means "many, ask elsewhere".
    pub systems: &'static [&'static str],
}

impl Profile {
    /// The [`Source`] a bundle from this producer carries.
    ///
    /// `app_version` is the caller's to supply, since a profile names the software and not the
    /// build that ran.
    #[must_use]
    pub fn source(&self, app_version: Option<String>) -> Source {
        Source {
            device_kind: Slug::parse(self.device_kind).ok(),
            fingerprint: None,
            app: Name::parse(self.app).ok(),
            app_version,
            unknown: one_saves::UnknownKeys::new(),
        }
    }

    /// The one system this profile can mean, when it can only mean one.
    #[must_use]
    pub fn only_system(&self) -> Option<&'static str> {
        match self.systems {
            [only] => Some(only),
            _ => None,
        }
    }
}

/// The producers this crate knows by name.
///
/// A name absent here is not thereby unusable: `--from` also accepts a bare core slug from the
/// registry, and anything else can be spelled out with `--app` and `--device-kind`.
static PROFILES: &[Profile] = &[
    Profile {
        key: "retroarch",
        label: "RetroArch (any libretro core)",
        device_kind: "emulator",
        // RetroArch is a frontend rather than a core, and the Cores registry covers cores only,
        // so it names itself in reverse-DNS. A caller that knows which core ran should say so
        // instead, since that is the name a consumer can group by.
        app: "com.libretro.retroarch",
        systems: &[],
    },
    Profile {
        key: "mednafen",
        label: "Mednafen",
        device_kind: "emulator",
        // The Cores registry lists the Beetle forks rather than Mednafen itself — "Mednafen
        // Saturn" is an alias of `beetle-saturn`, a libretro core, and not a name upstream ever
        // writes — so standalone Mednafen is unlisted and takes a reverse-DNS name. It reverses
        // no domain of its own, the homepage having lived on someone else's twice over, so the
        // `x` tree is the arm left.
        app: "x.mednafen",
        // Everything from the Lynx to the Saturn, which leaves `only_system` nothing to settle:
        // a Mednafen save says which system ran only via `--system` or a `--rom`.
        systems: &[],
    },
    Profile {
        key: "mgba",
        label: "mGBA",
        device_kind: "emulator",
        app: "mgba",
        systems: &["gba", "gbc", "gb"],
    },
    Profile { key: "snes9x", label: "Snes9x", device_kind: "emulator", app: "snes9x", systems: &["snes"] },
    Profile { key: "bsnes", label: "bsnes", device_kind: "emulator", app: "bsnes", systems: &["snes"] },
    Profile {
        key: "gambatte",
        label: "Gambatte",
        device_kind: "emulator",
        app: "gambatte",
        systems: &["gb", "gbc"],
    },
    Profile {
        key: "sameboy",
        label: "SameBoy",
        device_kind: "emulator",
        app: "sameboy",
        systems: &["gb", "gbc"],
    },
    Profile {
        key: "duckstation",
        label: "DuckStation",
        device_kind: "emulator",
        app: "duckstation",
        systems: &["psx"],
    },
    Profile {
        key: "pcsx2",
        label: "PCSX2",
        device_kind: "emulator",
        // No Cores registry entry, and it holds pcsx2.net.
        app: "net.pcsx2",
        systems: &["ps2"],
    },
    Profile {
        key: "dolphin",
        label: "Dolphin",
        device_kind: "emulator",
        app: "org.dolphin-emu",
        systems: &["gc", "wii"],
    },
    Profile {
        key: "mupen64plus",
        label: "Mupen64Plus-Next",
        device_kind: "emulator",
        app: "mupen64plus-next",
        systems: &["n64"],
    },
    Profile {
        key: "flycast",
        label: "Flycast",
        device_kind: "emulator",
        app: "x.flycast",
        systems: &["dreamcast"],
    },
    Profile {
        key: "everdrive",
        label: "EverDrive flash cartridge",
        device_kind: "flashcart",
        app: "x.everdrive",
        systems: &[],
    },
    Profile {
        key: "gboperator",
        label: "Epilogue GB Operator",
        device_kind: "cartridge-reader",
        app: "com.epilogue.operator",
        systems: &["gb", "gbc", "gba"],
    },
    Profile {
        key: "gbxcart",
        label: "GBxCart RW",
        device_kind: "cartridge-reader",
        app: "com.insidegadgets.gbxcart",
        systems: &["gb", "gbc", "gba"],
    },
];

/// Every profile this crate knows.
#[must_use]
pub fn profiles() -> &'static [Profile] {
    PROFILES
}

/// The profile a name denotes.
///
/// A name that is not a profile but *is* a listed emulator core resolves through the registry
/// instead, so `--from fceumm` works without this module having to list all 122 cores.
#[must_use]
pub fn profile(name: &str) -> Option<Profile> {
    if let Some(found) = PROFILES.iter().find(|p| p.key.eq_ignore_ascii_case(name)) {
        return Some(found.clone());
    }
    // A listed core writes its slug, which is exactly what the registry hands back.
    let core = one_saves_registry::core_by_any_name(name)?;
    Some(Profile {
        key: core.slug,
        label: core.name,
        device_kind: "emulator",
        app: core.slug,
        systems: core.systems,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listed_core_resolves_to_its_slug_and_not_a_reverse_dns_name() {
        // mGBA holds mgba.io but is listed, so `mgba` is its one spelling.
        let mgba = profile("mgba").expect("known");
        assert_eq!(mgba.app, "mgba");
        assert!(matches!(mgba.source(None).app, Some(Name::Slug(_))));
    }

    #[test]
    fn an_unlisted_producer_takes_a_reverse_dns_name() {
        // PCSX2 is not an entry in the Cores registry, so it names itself by its domain.
        let pcsx2 = profile("pcsx2").expect("known");
        assert!(matches!(pcsx2.source(None).app, Some(Name::ReverseDns(_))));
    }

    #[test]
    fn any_listed_core_works_without_being_named_here() {
        // The registry carries 122 cores; this module lists a dozen and defers for the rest.
        let fceumm = profile("fceumm").expect("resolves through the registry");
        assert_eq!(fceumm.app, "fceumm");
        assert_eq!(fceumm.only_system(), Some("nes"));
        assert_eq!(profile("Genesis_MiSTer").map(|p| p.app), Some("megadrive-mister"));
    }

    #[test]
    fn standalone_mednafen_is_not_one_of_the_beetle_forks_it_was_forked_into() {
        // The registry carries "Mednafen Saturn" and six siblings as aliases of the libretro
        // forks, so the bare name has to reach the emulator rather than one of its children.
        assert_eq!(profile("mednafen").map(|p| p.app), Some("x.mednafen"));
        assert_eq!(profile("Mednafen Saturn").map(|p| p.app), Some("beetle-saturn"));
        // Many systems, so the profile settles none of them on its own.
        assert_eq!(profile("mednafen").and_then(|p| p.only_system()), None);
    }

    #[test]
    fn every_profiles_names_are_well_formed_and_its_systems_are_listed() {
        for profile in profiles() {
            let source = profile.source(None);
            assert!(source.app.is_some(), "{}: app is not a well-formed name", profile.key);
            assert!(
                one_saves_registry::is_device_kind(profile.device_kind),
                "{}: {} is not a spec-owned device kind",
                profile.key,
                profile.device_kind
            );
            for system in profile.systems {
                assert!(one_saves_registry::system(system).is_some(), "{}: {system}", profile.key);
            }
        }
    }
}
