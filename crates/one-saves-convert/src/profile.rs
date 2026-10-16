//! Emulator profiles: what `--from mgba` tells a converter.
//!
//! A profile answers three questions a bare file cannot: what kind of producer wrote these bytes,
//! which app or core that producer was, and which systems it could plausibly have been emulating.
//! The first two become the bundle's [`source`](one_saves::Source); the third narrows the guess at
//! `system` when nothing else settles it.
//!
//! A converter is not the producer it names. It is a tool reporting what it *observed* — saves a
//! user found in a RetroArch install, a card PCSX2 wrote — so it writes only what a registry
//! lists: an [app](mod@one_saves_registry::apps) as its slug in `app`, a
//! [core](mod@one_saves_registry::cores) as its slug in `core`. A producer neither registry
//! lists is left out of both rather than given a reverse-DNS name, since minting one on its behalf
//! would claim a name in a namespace the converter does not hold. Its profile still settles
//! `device_kind` and the systems.

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
    /// The producer's slug in the Apps registry, when it is a listed app.
    pub app: Option<&'static str>,
    /// The producer's slug in the Emulator Cores registry, when it is a listed core.
    ///
    /// A standalone emulator is the core it is built around — mGBA run on its own is the `mgba`
    /// core — so its profile names that, and leaves the app it ran in to the caller.
    pub core: Option<&'static str>,
    /// The systems this producer covers, as system slugs. Empty means "many, ask elsewhere".
    pub systems: &'static [&'static str],
}

impl Profile {
    /// The [`Source`] a bundle from this producer carries.
    ///
    /// Versions are the caller's to add, since a profile names the software and not the build
    /// that ran.
    #[must_use]
    pub fn source(&self) -> Source {
        Source::default()
            .device_kind(Slug::parse(self.device_kind).ok())
            .app(self.app.and_then(|app| Name::parse(app).ok()))
            .core(self.core.and_then(|core| Name::parse(core).ok()))
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

/// A profile for a listed core, which is most of them.
const fn core(key: &'static str, label: &'static str, systems: &'static [&'static str]) -> Profile {
    Profile { key, label, device_kind: "emulator", app: None, core: Some(key), systems }
}

/// A profile for a producer neither registry lists, which names nobody and says what it can.
const fn unlisted(
    key: &'static str,
    label: &'static str,
    device_kind: &'static str,
    systems: &'static [&'static str],
) -> Profile {
    Profile { key, label, device_kind, app: None, core: None, systems }
}

/// The producers this crate knows by name.
///
/// A name absent here is not thereby unusable: `--from` also accepts any app or core the
/// registries list, by slug or by name.
static PROFILES: &[Profile] = &[
    Profile {
        key: "retroarch",
        label: "RetroArch (any libretro core)",
        device_kind: "emulator",
        // The app alone. Which core ran is the caller's to add, and the one a consumer can group
        // by, so `--core` is worth passing alongside.
        app: Some("retroarch"),
        core: None,
        systems: &[],
    },
    // The Cores registry lists the Beetle forks rather than Mednafen itself — "Mednafen Saturn" is
    // an alias of `beetle-saturn`, a libretro core, and not a name upstream ever writes — so
    // standalone Mednafen is on neither list. It covers everything from the Lynx to the Saturn,
    // which leaves `only_system` nothing to settle: a Mednafen save says which system ran only via
    // `--system` or a `--rom`.
    unlisted("mednafen", "Mednafen", "emulator", &[]),
    core("mgba", "mGBA", &["gba", "gbc", "gb"]),
    core("snes9x", "Snes9x", &["snes"]),
    core("bsnes", "bsnes", &["snes"]),
    core("gambatte", "Gambatte", &["gb", "gbc"]),
    core("sameboy", "SameBoy", &["gb", "gbc"]),
    core("duckstation", "DuckStation", &["psx"]),
    // Neither has an entry in the Cores registry, for all that each holds a domain.
    unlisted("pcsx2", "PCSX2", "emulator", &["ps2"]),
    unlisted("dolphin", "Dolphin", "emulator", &["gc", "wii"]),
    Profile { key: "mupen64plus", ..core("mupen64plus-next", "Mupen64Plus-Next", &["n64"]) },
    unlisted("flycast", "Flycast", "emulator", &["dreamcast"]),
    // Producers that run no core, so neither registry has anything to say about them.
    unlisted("everdrive", "EverDrive flash cartridge", "flashcart", &[]),
    unlisted("gboperator", "Epilogue GB Operator", "cartridge-reader", &["gb", "gbc", "gba"]),
    unlisted("gbxcart", "GBxCart RW", "cartridge-reader", &["gb", "gbc", "gba"]),
];

/// Every profile this crate knows.
#[must_use]
pub fn profiles() -> &'static [Profile] {
    PROFILES
}

/// The profile a name denotes.
///
/// A name that is not a profile but *is* a listed app or emulator core resolves through the
/// registry instead, so `--from mister` and `--from fceumm` work without this module having to
/// list all of them.
#[must_use]
pub fn profile(name: &str) -> Option<Profile> {
    if let Some(found) = PROFILES.iter().find(|p| p.key.eq_ignore_ascii_case(name)) {
        return Some(found.clone());
    }
    // A listed name writes its slug, which is exactly what the registry hands back.
    if let Some(app) = one_saves_registry::app_by_any_name(name) {
        return Some(Profile {
            key: app.slug,
            label: app.name,
            device_kind: "emulator",
            app: Some(app.slug),
            core: None,
            systems: &[],
        });
    }
    let core = one_saves_registry::core_by_any_name(name)?;
    Some(Profile {
        key: core.slug,
        label: core.name,
        device_kind: "emulator",
        app: None,
        core: Some(core.slug),
        systems: core.systems,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listed_core_is_written_as_the_core_and_not_the_app() {
        // Under 0.3 this was `app: "mgba"`. That now names an app, and mGBA is an engine.
        let source = profile("mgba").expect("known").source();
        assert!(matches!(&source.core, Some(Name::Slug(slug)) if slug.as_str() == "mgba"));
        assert_eq!(source.app, None, "whether it ran standalone or in RetroArch is not known");
    }

    #[test]
    fn retroarch_is_an_app_with_its_core_left_to_the_caller() {
        let source = profile("retroarch").expect("known").source();
        assert!(matches!(&source.app, Some(Name::Slug(slug)) if slug.as_str() == "retroarch"));
        assert_eq!(source.core, None);
    }

    #[test]
    fn an_unlisted_producer_is_left_out_rather_than_minted_for() {
        // PCSX2 holds pcsx2.net, but `net.pcsx2` would be this crate claiming a name in PCSX2's
        // namespace. The profile still knows what kind of producer it is and what it runs.
        let pcsx2 = profile("pcsx2").expect("known");
        let source = pcsx2.source();
        assert_eq!((&source.app, &source.core), (&None, &None));
        assert_eq!(source.device_kind.as_ref().map(Slug::as_str), Some("emulator"));
        assert_eq!(pcsx2.only_system(), Some("ps2"));
    }

    #[test]
    fn any_listed_app_or_core_works_without_being_named_here() {
        // The registry carries 122 cores; this module lists a dozen and defers for the rest.
        let fceumm = profile("fceumm").expect("resolves through the registry");
        assert_eq!(fceumm.core, Some("fceumm"));
        assert_eq!(fceumm.only_system(), Some("nes"));
        assert_eq!(profile("Genesis_MiSTer").and_then(|p| p.core), Some("megadrive-mister"));
        // And the apps, by slug or by the name they go by.
        assert_eq!(profile("Analogue Pocket").and_then(|p| p.app), Some("analogue-pocket"));
        assert_eq!(profile("mister").map(|p| (p.app, p.core)), Some((Some("mister"), None)));
    }

    #[test]
    fn standalone_mednafen_is_not_one_of_the_beetle_forks_it_was_forked_into() {
        // The registry carries "Mednafen Saturn" and six siblings as aliases of the libretro
        // forks, so the bare name has to reach the emulator rather than one of its children.
        assert_eq!(profile("mednafen").map(|p| p.core), Some(None));
        assert_eq!(profile("Mednafen Saturn").and_then(|p| p.core), Some("beetle-saturn"));
        // Many systems, so the profile settles none of them on its own.
        assert_eq!(profile("mednafen").and_then(|p| p.only_system()), None);
    }

    #[test]
    fn every_profile_names_only_what_a_registry_lists() {
        // An observer's whole rule: a slug from the registry or nothing at all.
        for profile in profiles() {
            if let Some(app) = profile.app {
                assert!(one_saves_registry::app(app).is_some(), "{}: {app} is not a listed app", profile.key);
            }
            if let Some(core) = profile.core {
                assert!(
                    one_saves_registry::core(core).is_some(),
                    "{}: {core} is not a listed core",
                    profile.key
                );
            }
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
