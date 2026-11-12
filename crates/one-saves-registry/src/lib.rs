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
//! # Naming a core you already know
//!
//! A producer that knows which core it is does not need to search for itself. Each listed core
//! has a const, and naming one links that core alone rather than the whole table.
//!
//! ```
//! use one_saves_registry::{ClockLayout, SidecarRecord, cores, systems};
//!
//! // mGBA holds mgba.io and is listed, so `mgba` is the one spelling it writes.
//! assert_eq!(cores::MGBA.slug, "mgba");
//! // And the registry records how it lays a clock down, so a converter need not guess: as a
//! // libretro core, in a `.rtc` of its own beside the save.
//! let beside = ClockLayout::Sidecar(SidecarRecord::Registers { timestamp_width: 8 });
//! assert_eq!(cores::MGBA.clock, Some(beside));
//!
//! // Systems the same way.
//! assert_eq!(systems::GBA.name, "Game Boy Advance");
//! assert!(cores::MGBA.systems.contains(&systems::GBA.slug));
//! ```
//!
//! # Apps and platforms
//!
//! A `source` names three layers, each from a registry of its own: the platform an app ran on,
//! the app, and the core it ran. One app runs many cores and ships on many platforms, which is why
//! they are three lists rather than one.
//!
//! ```
//! use one_saves_registry::{AppCores, CoreKind, apps, cores, platform_by_any_name};
//!
//! // RetroArch loads every libretro core, so the mGBA core is one it can have run.
//! assert_eq!(apps::RETROARCH.cores, AppCores { kind: Some(CoreKind::Libretro), builds: &[] });
//! assert!(apps::RETROARCH.runs(&cores::MGBA));
//! assert!(!apps::RETROARCH.runs(&cores::GAMEBOY_MISTER));
//!
//! // A firmware's display name lands on its slug, which is what a bundle carries.
//! assert_eq!(platform_by_any_name("muOS").unwrap().slug, "muos");
//! ```
//!
//! # Roles, and the one place a slug has structure
//!
//! A role ending in `-<n>` names the nth socket of the kind its prefix names, so a consumer that
//! recognises `memcard-` matches `memcard-8` without that exact role being listed. [`role`]
//! handles both arms; see [`RoleMatch`].

mod generated;

/// A named const per listed app: [`apps::RETROARCH`], [`apps::MISTER`], and so on.
pub use generated::apps;
/// A named const per listed core: [`cores::MGBA`], [`cores::GAMBATTE`], and so on.
///
/// A producer that knows which core it is does not need a lookup, and naming a const links only
/// that core rather than the whole 200-entry table. `cores()` still returns the table for the
/// consumers that genuinely search.
pub use generated::cores;
/// A named const per listed platform: [`platforms::MUOS`], [`platforms::WINDOWS`], and so on.
pub use generated::platforms;
/// A named const per listed system: [`systems::GBA`], [`systems::PSX`], and so on.
///
/// The counterpart to [`mod@cores`], and the same bargain: a consumer that knows which system it
/// means names it, and carries that one rather than all 49.
pub use generated::systems;

use generated::{
    APP_CLOCKS, APPS, BINDINGS, CARD_FORMATS, CORES, DEVICE_KINDS, PLATFORMS, ROLE_PREFIXES, ROLES, SYSTEMS,
    VENDORS,
};

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
    /// A software core hosted by RetroArch, on its own or inside Onion and most handheld
    /// firmwares. Its canonical name is the core's `library_name`, the exact string that names a
    /// `saves/<core>/` folder.
    Libretro,
    /// A MiSTer FPGA core, named for its repository in the MiSTer-devel organization.
    MiSTer,
    /// An Analogue Pocket openFPGA core, named for the `Author.Platform` folder it installs to.
    OpenFpga,
    /// An engine that ships as an emulator of its own and has no build under the other kinds, such
    /// as Mednafen. Named as the emulator goes by.
    Standalone,
}

/// How a core stores real-time-clock state alongside a save.
///
/// A cartridge with a clock chip holds state the save memory does not, and cores disagree about
/// where to put it. Which *chip* the state came from is the save's own business — an MBC3 is a
/// Game Boy part, a Seiko S-3511A a Game Boy Advance or WonderSwan one — but the *layout* is the
/// core's own choice, and nothing in a save file says which was used. So it is recorded here,
/// against the core that writes it, rather than guessed from the bytes.
///
/// A footer also stores the host clock the core advances the chip from, and cores disagree over
/// how many bytes it takes. That width is what `x.1sav.rtc.mbc3` carries as key 3, and what
/// rebuilding a footer for a core has to match, so it is recorded beside the layout.
///
/// The layout is the core's as its [`kind`](Core::kind) hosts it, and for a libretro core that is
/// the frontend's doing as much as the core's: a libretro frontend writes the save memory a core
/// exposes to `<name>.srm` and its clock, when it exposes one, to `<name>.rtc`, and appends
/// nothing. So a libretro core that keeps a clock keeps it in a [`Sidecar`](Self::Sidecar),
/// whatever a standalone build of the same engine does. An app running a build of its own lays
/// the clock out its own way; see [`Core::clock_in`].
///
/// Unlike the rest of this crate these are not drawn from the registry pages: the specifications
/// do not describe clock layouts. They come from files real cores wrote, or from a core's own
/// source where no file has been sampled. A core whose source is not published and whose files
/// nobody has sampled is not recorded, whatever its upstream did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockLayout {
    /// Appended to the save, registers little-endian at their natural width and then the host
    /// clock. The mGBA app, and the standalone builds of SameBoy and VBA-M.
    Appended {
        /// How many bytes the host clock takes.
        timestamp_width: u8,
    },
    /// Appended to the save, registers bit-packed and padded out to a reserved length.
    ///
    /// The MiSTer Gameboy core and SGB core and their Pocket ports for an MBC3. GBA_MiSTer and
    /// WonderSwan_MiSTer for a Seiko S-3511A, which fill the length with copies of one record, and
    /// the WonderSwan core's Pocket port, which writes that record cut to twelve bytes.
    Packed {
        /// The length in bytes the footer is padded to.
        reserved: usize,
        /// How many bytes the host clock takes, wherever in the packing it sits.
        timestamp_width: u8,
    },
    /// Written to a file of its own beside the save, `<name>.rtc`, and not appended to it. Every
    /// libretro core that keeps a clock, and mincer_ray's GBA core on the Pocket.
    ///
    /// The file holds whatever the core keeps, and the record says what that is. A chip the record
    /// does not hold is one the core keeps no clock for at all: mGBA's libretro core keeps an
    /// MBC3's, and nothing for a GBA cartridge's Seiko chip, whose time a game there reads from
    /// the host.
    ///
    /// No footer, so no width: `x.1sav.rtc.mbc3` leaves key 3 out for a clock kept this way, even
    /// where the record has a timestamp of its own.
    Sidecar(SidecarRecord),
}

/// What a clock kept in a file of its own holds.
///
/// The files share a name and a place and nothing else. Some hold a reading and one holds the
/// instant a reading is derived from. Their lengths tell them apart, but only among the producers
/// sampled so far, so a consumer that knows the producer reads its record as this says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidecarRecord {
    /// The instant the clock's counters count from, as a `time_t`. The clock reads the time since
    /// then, so the file is the other operand of a reading rather than a reading. Gambatte keeps
    /// one for an MBC3 and for a HuC-3 alike, eight little-endian bytes under libretro and four
    /// big-endian ones in OpenEmu's build, and VBA-M's libretro core one for a HuC-3.
    Origin {
        /// How many bytes the instant takes.
        timestamp_width: u8,
        /// Whether it is written most significant byte first.
        big_endian: bool,
    },
    /// An MBC3's registers as an appending core puts them after the save, ten little-endian words
    /// and then the host clock, alone in the file. mGBA's and VBA-M's libretro cores.
    Registers {
        /// How many bytes the host clock takes.
        timestamp_width: u8,
    },
    /// SameBoy's own record of an MBC3, the clock section of its machine state: the live and
    /// latched registers a byte each, the host clock as eight little-endian bytes from offset 16,
    /// and the cycles counted into the current second, 32 bytes in all. SameBoy's libretro core.
    SameBoy,
    /// A Seiko S-3511A's calendar and the host clock it was taken at, bit-packed. mincer_ray's GBA
    /// core.
    PackedSeiko,
}

/// How a core keeps a Hudson HuC-3's clock, which every core that keeps one keeps its own way.
///
/// A [`ClockLayout`] covers the chips whose state the producers that keep them agree about, an
/// MBC3's registers and a Seiko chip's calendar, where the layout is the core's and the chip the
/// save's. A HuC-3 is a controller with a 256-nybble window, and no two producers keep the same
/// part of it in the same shape, not even where their MBC3s agree: VBA-M keeps an MBC3's registers
/// beside the save and a HuC-3's origin. So it is recorded beside the layout, for the cores whose
/// files or source say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Huc3Layout {
    /// Packed into the footer the core keeps an MBC3 in, padded to this length: the minute and day
    /// counters, the seconds into the minute and the host clock. The MiSTer Game Boy cores and
    /// their Pocket ports.
    Packed {
        /// The length in bytes the footer is padded to.
        reserved: usize,
    },
    /// In a file of its own beside the save, `<name>.rtc`: the instant the counters count from.
    /// Gambatte and VBA-M under libretro.
    Origin {
        /// How many bytes the instant takes.
        timestamp_width: u8,
        /// Whether it is written most significant byte first.
        big_endian: bool,
    },
    /// Appended to the save: the controller's whole window, two nybbles to a byte, the lower
    /// address in the lower half, and then the host clock it was latched at, eight bytes
    /// little-endian. The mGBA app.
    Memory,
    /// Appended to the save as SameBoy's own seventeen packed bytes: the host clock the counters
    /// were last moved at, eight bytes little-endian, then the minute and day counters and the
    /// alarm's minute and day, two bytes each, and whether the alarm is on. The SameBoy app.
    SameBoy,
    /// Appended to the save as VBA-M's own 24 bytes: the instant the counters count from, eight
    /// bytes little-endian, then four little-endian words of the controller's state as a game left
    /// it, the last time it read and the shift it was reading at. The VisualBoyAdvance-M app.
    Vbam,
    /// Not kept at all: the core saves no HuC-3 clock, and a game's clock starts over each time it
    /// is loaded. mGBA's and SameBoy's libretro cores, and OpenEmu's Gambatte, which will not load
    /// a HuC-3 cartridge at all.
    NotKept,
}

/// How a core keeps one system's save memory in its file, where that is not the memory's own bytes
/// one after another.
///
/// A bundle carries the memory alone, as the console's hardware addresses it, and a medium addressed
/// only in whole words, such as a serial EEPROM, with each word most significant byte first: a Mega
/// Drive's SRAM as its own bytes, a GBA's chip without the padding a file adds, an MBC7's words in
/// the order the chip shifts them out. Cores disagree about the file,
/// and MiSTer's cores will not read every one of them, so writing a file for a core means laying
/// the memory out as that core keeps it. A core with no layout for a system keeps the memory as it
/// is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveLayout {
    /// The cartridge's 16-bit address window, as Genesis Plus GX keeps a Mega Drive's: SRAM on one
    /// byte lane sits at every other byte of the file, and the other lane holds `fill`. A serial
    /// EEPROM is kept as its own bytes, never spread out.
    Expanded {
        /// What the lane the SRAM is not on holds.
        fill: u8,
        /// How long the file is.
        length: ExpandedLength,
        /// The length an EEPROM's file is padded to with `fill`, for a core that writes every
        /// EEPROM at one size: PicoDrive's 8 KiB.
        eeprom_length: Option<u32>,
    },
    /// The SRAM's own bytes, one after another, as BlastEm and MiSTer keep a Mega Drive's, padded
    /// with 0xFF to `length` for a core that writes every save at one size.
    Packed {
        /// The size the core writes every save at.
        length: Option<u32>,
    },
    /// The chip at the start of a file of `length` bytes, the rest 0xFF, as gpSP and SkyEmu keep a
    /// GBA's.
    Padded {
        /// The file's length.
        length: u32,
        /// Whether each eight-byte block of an EEPROM is kept back to front, as SkyEmu keeps them.
        reversed_eeprom: bool,
    },
    /// VBA Next's GBA file: 0x22000 bytes, an EEPROM at 0x20000 behind 0xFF and any other chip at
    /// the start.
    VbaNext,
    /// The chip itself, an EEPROM padded with 0xFF to at least `eeprom_min` bytes: MiSTer's GBA
    /// core reads a 4 Kbit EEPROM only from a file of 8 KiB, the size it writes one at.
    Chip {
        /// The shortest file an EEPROM is read from.
        eeprom_min: u32,
    },
    /// A Game Boy MBC7 cartridge's EEPROM, kept otherwise than as the chip.
    ///
    /// The chip holds 16-bit words and shifts each out most significant bit first, so a payload
    /// holds each word most significant byte first. A core can keep each word the other way round,
    /// as SameBoy and Gearboy do, and can pad the chip out to the size of a cartridge's RAM, as TGB
    /// Dual and DoubleCherryGB do. Every other Game Boy save it keeps as the memory.
    Mbc7 {
        /// Whether each word's two bytes are the other way round from the chip's.
        swapped: bool,
        /// The length the file is padded to, and what pads it, where the core pads it.
        padding: Option<(u32, u8)>,
    },
}

/// How long an [`Expanded`](SaveLayout::Expanded) file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpandedLength {
    /// Up to the last byte that is not 0xFF, as Genesis Plus GX's libretro port reports it: the
    /// file is as long as its data, and the core reads anything shorter as 0xFF.
    Trimmed,
    /// Twice the SRAM, which is the range the cartridge's header declares, as PicoDrive writes it.
    Doubled,
    /// Always this long, as clownmdemu writes every save at 64 KiB.
    Fixed(u32),
}

/// A core's [`SaveLayout`] for one of the systems it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemLayout {
    /// The system's slug.
    pub system: &'static str,
    /// How the core keeps that system's save memory.
    pub layout: SaveLayout,
}

/// One app's own build of a core, laying a clock out differently from the core's kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AppClock {
    /// The core's slug.
    pub(crate) core: &'static str,
    /// The app's slug.
    pub(crate) app: &'static str,
    /// How that build lays an MBC3's or a Seiko chip's clock out, where anyone has seen.
    pub(crate) layout: Option<ClockLayout>,
    /// How that build keeps a HuC-3's clock, where anyone has seen.
    pub(crate) huc3: Option<Huc3Layout>,
}

/// An emulator core: the engine that runs the game, as distinct from the [`App`] hosting it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Core {
    /// The canonical slug, which is what a `source.core` field carries for this core.
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
    /// How it lays out clock state, for the cores that write any, as its [`kind`](Self::kind)
    /// hosts it: a libretro core in a libretro frontend, a MiSTer core on a MiSTer.
    ///
    /// `None` means this registry records no clock convention for the core — either because it
    /// runs nothing with a clock chip, or because nobody has checked. A consumer meeting `None`
    /// should fall back to whatever it does for an unlisted producer rather than assume there is
    /// no clock.
    ///
    /// An app that ships a build of its own may lay the clock out differently, as the mGBA app
    /// does; [`clock_in`](Self::clock_in) answers for a known app.
    pub clock: Option<ClockLayout>,
    /// How it keeps a HuC-3's clock, for the Game Boy cores whose files or source say, as its
    /// [`kind`](Self::kind) hosts it; [`huc3_in`](Self::huc3_in) answers for a known app.
    ///
    /// `None` means nobody has checked, which is not the same as
    /// [`NotKept`](Huc3Layout::NotKept).
    pub huc3: Option<Huc3Layout>,
    /// How it keeps save memory in its file, for the systems where that is not the memory's own
    /// bytes, as its [`kind`](Self::kind) hosts it; [`layout_in`](Self::layout_in) answers for one
    /// system and a known app.
    pub layouts: &'static [SystemLayout],
}

impl Core {
    /// How this core lays out clock state when `app` runs it.
    ///
    /// An app that loads the core as one of its kind, as RetroArch loads a libretro core, writes
    /// what [`clock`](Self::clock) says. An app that ships a build of its own writes what that
    /// build does: the mGBA app appends the clock its libretro core keeps beside the save, and
    /// OpenEmu's Gambatte writes its instant in four big-endian bytes rather than eight
    /// little-endian ones. Those are recorded where a build's files have been sampled, and a build
    /// nobody has sampled answers `None`, since the core's kind says nothing about it.
    ///
    /// With no app, [`clock`](Self::clock).
    ///
    /// ```
    /// use one_saves_registry::{ClockLayout, SidecarRecord, apps, cores};
    ///
    /// let beside = Some(ClockLayout::Sidecar(SidecarRecord::Registers { timestamp_width: 8 }));
    /// assert_eq!(cores::MGBA.clock_in(Some(&apps::RETROARCH)), beside);
    /// let appended = Some(ClockLayout::Appended { timestamp_width: 8 });
    /// assert_eq!(cores::MGBA.clock_in(Some(&apps::MGBA)), appended);
    /// ```
    #[must_use]
    pub fn clock_in(&self, app: Option<&App>) -> Option<ClockLayout> {
        match self.build_in(app) {
            Build::Kind => self.clock,
            Build::Own(found) => found.and_then(|build| build.layout),
        }
    }

    /// How this core keeps a HuC-3's clock when `app` runs it, by the same rule as
    /// [`clock_in`](Self::clock_in).
    ///
    /// ```
    /// use one_saves_registry::{Huc3Layout, apps, cores};
    ///
    /// assert_eq!(cores::MGBA.huc3_in(Some(&apps::RETROARCH)), Some(Huc3Layout::NotKept));
    /// assert_eq!(cores::MGBA.huc3_in(Some(&apps::MGBA)), Some(Huc3Layout::Memory));
    /// ```
    #[must_use]
    pub fn huc3_in(&self, app: Option<&App>) -> Option<Huc3Layout> {
        match self.build_in(app) {
            Build::Kind => self.huc3,
            Build::Own(found) => found.and_then(|build| build.huc3),
        }
    }

    /// How this core keeps `system`'s save memory in its file when `app` runs it, where that is not
    /// the memory's own bytes.
    ///
    /// `None` means the file is the memory as it is, or that an app's own build has not been
    /// checked: the registry records layouts for the cores as their kinds host them.
    ///
    /// ```
    /// use one_saves_registry::{ExpandedLength, SaveLayout, apps, cores};
    ///
    /// let gpgx = cores::GENESIS_PLUS_GX.layout_in(Some(&apps::RETROARCH), "genesis");
    /// assert!(matches!(gpgx, Some(SaveLayout::Expanded { fill: 0xFF, length: ExpandedLength::Trimmed, .. })));
    /// assert_eq!(cores::MEGADRIVE_MISTER.layout_in(None, "genesis"), Some(SaveLayout::Packed { length: Some(65_536) }));
    /// assert_eq!(cores::MGBA.layout_in(None, "gba"), None, "mGBA keeps the chip as it is");
    /// ```
    #[must_use]
    pub fn layout_in(&self, app: Option<&App>, system: &str) -> Option<SaveLayout> {
        match self.build_in(app) {
            Build::Kind => self.layouts.iter().find(|l| l.system == system).map(|l| l.layout),
            Build::Own(_) => None,
        }
    }

    /// Which build of this core `app` runs: the core as its kind hosts it, or a build of the app's
    /// own, with what the registry records of that build.
    fn build_in(&self, app: Option<&App>) -> Build {
        let Some(app) = app else { return Build::Kind };
        if let Ok(index) = APP_CLOCKS.binary_search_by_key(&(self.slug, app.slug), |c| (c.core, c.app)) {
            return Build::Own(Some(&APP_CLOCKS[index]));
        }
        // A standalone engine's entry is its own build already, as a standalone emulator ships it.
        if app.cores.builds.contains(&self.slug) && self.kind != CoreKind::Standalone {
            Build::Own(None)
        } else {
            Build::Kind
        }
    }
}

/// Which build of a core an app runs.
enum Build {
    /// The core as its kind hosts it, which its own entry describes.
    Kind,
    /// A build of the app's own, and the registry's record of it, when it has one.
    Own(Option<&'static AppClock>),
}

/// What an app runs, as the Apps registry's **Cores** column says it.
///
/// An app loads every core of a kind, ships builds of its own, or both: Provenance loads libretro
/// cores beside the engines it builds itself. The column always names at least one of the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppCores {
    /// The kind whose every core the app loads: RetroArch loads every libretro core, a MiSTer
    /// every MiSTer core.
    pub kind: Option<CoreKind>,
    /// The cores it runs builds of its own of, by slug, as a standalone emulator runs its one
    /// engine. Empty for an app that only loads a kind.
    pub builds: &'static [&'static str],
}

/// An app: the software or device that runs an emulator core.
///
/// A frontend that loads cores, an FPGA device that loads them as bitstreams, or a standalone
/// emulator. A firmware that ships an app is a [`Platform`] rather than an app, so a save a muOS
/// handheld wrote names `retroarch` here; a device whose firmware loads cores itself, such as the
/// Analogue Pocket, is the app. A producer that runs no core has no entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct App {
    /// The canonical slug, which is what a `source.app` field carries for this app.
    ///
    /// An app listed here **must** write this rather than a reverse-DNS name, even when it holds
    /// a domain. RetroArch is `retroarch`, never `com.libretro.retroarch`.
    pub slug: &'static str,
    /// The app's name as its authors write it.
    pub name: &'static str,
    /// What it runs.
    pub cores: AppCores,
}

impl App {
    /// Whether this app runs a core, as far as the registry says.
    #[must_use]
    pub fn runs(&self, core: &Core) -> bool {
        self.cores.kind == Some(core.kind) || self.cores.builds.contains(&core.slug)
    }
}

/// An operating system or firmware an [`App`] runs on.
///
/// A producer names the most specific one it knows: a handheld running KNULLI is `knulli`, not
/// `linux`, though KNULLI is a Linux distribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Platform {
    /// The canonical slug, which is what a `source.platform` field carries for this platform.
    pub slug: &'static str,
    /// The platform's name as its authors write it.
    pub name: &'static str,
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
    /// Every listed format has one. `saturn-bup` was the exception until 0.3: its entry sits at
    /// the head of the save's own first block rather than in a directory region, which changes
    /// where a reader finds those thirty bytes and not whether they can be carried.
    pub dirent_len: usize,
    /// The format's block size in bytes: the unit a card allocates a save in.
    ///
    /// For every format but one this is a property of the format, and reading it is enough.
    /// `saturn-bup` is the exception: its 64 is the Saturn's **internal** memory, and a backup
    /// cartridge carries its own, so a block count derived from this one is wrong for a cartridge.
    ///
    /// The medium is sometimes recoverable and sometimes not. A Saturn read whole is a device
    /// whose parts carry a [`role`] — `internal` against `ram-cart` — but a cartridge dumped
    /// by itself is a card bundle with no role anywhere in it, and nothing then says which it was.
    /// The specifications leave this as written for 0.5; settling it means keying the block size
    /// on the card map's `capacity`, which every card carries, rather than on the format alone.
    pub block_size: usize,
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

/// Every listed app, sorted by slug.
#[must_use]
pub fn apps() -> &'static [App] {
    &APPS
}

/// The app a slug names, if it is listed.
#[must_use]
pub fn app(slug: &str) -> Option<&'static App> {
    APPS.binary_search_by_key(&slug, |a| a.slug).ok().map(|i| &APPS[i])
}

/// The app a slug or its name names, compared case-insensitively.
#[must_use]
pub fn app_by_any_name(name: &str) -> Option<&'static App> {
    app(name).or_else(|| {
        APPS.iter().find(|app| app.slug.eq_ignore_ascii_case(name) || app.name.eq_ignore_ascii_case(name))
    })
}

/// Every listed platform, sorted by slug.
#[must_use]
pub fn platforms() -> &'static [Platform] {
    &PLATFORMS
}

/// The platform a slug names, if it is listed.
#[must_use]
pub fn platform(slug: &str) -> Option<&'static Platform> {
    PLATFORMS.binary_search_by_key(&slug, |p| p.slug).ok().map(|i| &PLATFORMS[i])
}

/// The platform a slug or its name names, compared case-insensitively.
///
/// A firmware writes its own name with capitals a slug cannot carry — `muOS`, `KNULLI` — and this
/// is what maps that onto the slug to write.
#[must_use]
pub fn platform_by_any_name(name: &str) -> Option<&'static Platform> {
    platform(name).or_else(|| {
        PLATFORMS.iter().find(|platform| {
            platform.slug.eq_ignore_ascii_case(name) || platform.name.eq_ignore_ascii_case(name)
        })
    })
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
        assert!(APPS.windows(2).all(|w| w[0].slug < w[1].slug), "apps must be sorted by slug");
        assert!(PLATFORMS.windows(2).all(|w| w[0].slug < w[1].slug), "platforms must be sorted by slug");
    }

    #[test]
    fn the_registries_are_the_size_the_documents_are() {
        assert_eq!(SYSTEMS.len(), 50);
        assert_eq!(CORES.len(), 202);
        assert_eq!(APPS.len(), 24);
        assert_eq!(PLATFORMS.len(), 13);
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
    fn an_app_runs_the_cores_its_column_names() {
        // Each of these three loads one whole kind, and nothing of the others.
        for (app, kind) in [
            (apps::RETROARCH, CoreKind::Libretro),
            (apps::MISTER, CoreKind::MiSTer),
            (apps::ANALOGUE_POCKET, CoreKind::OpenFpga),
        ] {
            assert_eq!(app.cores, AppCores { kind: Some(kind), builds: &[] }, "{}", app.slug);
            assert!(cores().iter().filter(|c| app.runs(c)).all(|c| c.kind == kind), "{}", app.slug);
        }
        assert!(apps::MISTER.runs(&cores::GAMEBOY_MISTER));
        assert!(!apps::MISTER.runs(&cores::GAMBATTE));

        // Provenance loads libretro cores and builds standalone engines of its own beside them.
        assert_eq!(apps::PROVENANCE.cores.kind, Some(CoreKind::Libretro));
        assert!(apps::PROVENANCE.runs(&cores::SAMEBOY));
        assert!(apps::PROVENANCE.runs(&cores::MUPEN64PLUS));
        assert!(!apps::PROVENANCE.runs(&cores::PCSX2));
        // And a standalone emulator runs its one engine, under whatever name that engine has.
        assert_eq!(apps::DOLPHIN.cores, AppCores { kind: None, builds: &["dolphin-emu"] });
    }

    #[test]
    fn no_name_is_both_an_app_and_a_platform() {
        // The Pocket's firmware loads cores itself, so it is an app with nothing beneath it, and
        // the specification keeps the two lists disjoint so one name has one reading.
        for app in apps() {
            assert_eq!(platform(app.slug), None, "{} is on both lists", app.slug);
        }
        assert_eq!(platform("analogue-pocket"), None);
    }

    #[test]
    fn a_display_name_lands_on_its_slug() {
        assert_eq!(app_by_any_name("RetroArch").map(|a| a.slug), Some("retroarch"));
        assert_eq!(app_by_any_name("Analogue Pocket").map(|a| a.slug), Some("analogue-pocket"));
        assert_eq!(platform_by_any_name("muOS").map(|p| p.slug), Some("muos"));
        assert_eq!(platform_by_any_name("KNULLI").map(|p| p.slug), Some("knulli"));
        // The engine and the frontend are different registries, so neither answers for the other.
        assert_eq!(app_by_any_name("gambatte"), None);
        assert_eq!(core_by_any_name("retroarch"), None);
    }

    #[test]
    fn the_app_and_platform_consts_and_the_tables_are_the_same_entries() {
        assert_eq!(app("retroarch"), Some(&apps::RETROARCH));
        assert_eq!(platform("muos"), Some(&platforms::MUOS));
        for entry in apps() {
            assert_eq!(app(entry.slug), Some(entry));
        }
        for entry in platforms() {
            assert_eq!(platform(entry.slug), Some(entry));
        }
    }

    #[test]
    fn a_renamed_core_still_resolves_through_its_old_name() {
        assert_eq!(core_by_any_name("Genesis_MiSTer").map(|c| c.slug), Some("megadrive-mister"));
        assert_eq!(core_by_any_name("Mednafen Saturn").map(|c| c.slug), Some("beetle-saturn"));
    }

    #[test]
    #[allow(
        clippy::used_underscore_items,
        reason = "a slug that starts with a digit takes a const that starts with an underscore"
    )]
    fn the_consts_and_the_table_are_the_same_entries() {
        // The table is generated *from* the consts, so this is a check that the generator still
        // emits both from one source rather than a check on the data.
        assert_eq!(core("mgba"), Some(&cores::MGBA));
        assert_eq!(core("gambatte"), Some(&cores::GAMBATTE));
        assert_eq!(core("3do-mister"), Some(&cores::_3DO_MISTER));
        for entry in cores() {
            assert_eq!(core(entry.slug), Some(entry));
        }
    }

    #[test]
    #[allow(
        clippy::used_underscore_items,
        reason = "a slug that starts with a digit takes a const that starts with an underscore"
    )]
    fn the_system_consts_and_the_table_are_the_same_entries() {
        assert_eq!(system("gba"), Some(&systems::GBA));
        assert_eq!(system("psx"), Some(&systems::PSX));
        assert_eq!(system("3do"), Some(&systems::_3DO));
        for entry in systems() {
            assert_eq!(system(entry.slug), Some(entry));
        }
    }

    #[test]
    fn a_core_runs_the_systems_its_consts_name() {
        // The two const modules are generated from one pair of tables, so a core's coverage can
        // be checked against the system consts rather than against loose strings.
        assert!(cores::MGBA.systems.contains(&systems::GBA.slug));
        assert!(!cores::GAMBATTE.systems.contains(&systems::GBA.slug));
    }

    #[test]
    fn a_clock_layout_is_recorded_only_where_there_is_a_clock_to_lay_out() {
        // Every layout in `clocks.json` belongs to a core that runs a system with a clock chip.
        // A slug typo'd into that file would otherwise sit there doing nothing in silence.
        for entry in cores().iter().filter(|c| c.clock.is_some()) {
            assert!(
                entry
                    .systems
                    .iter()
                    .any(|s| matches!(*s, "gb" | "gbc" | "gba" | "wonderswan" | "wonderswan-color")),
                "{} records a clock layout but runs no system with a clock chip",
                entry.slug
            );
        }
    }

    #[test]
    fn the_layouts_are_the_ones_real_cores_write() {
        // Under a libretro frontend every core that keeps a clock keeps it in a `.rtc` beside the
        // save, and what each one puts in it is its own.
        let beside = |record| Some(ClockLayout::Sidecar(record));
        assert_eq!(
            cores::GAMBATTE.clock,
            beside(SidecarRecord::Origin { timestamp_width: 8, big_endian: false })
        );
        assert_eq!(cores::MGBA.clock, beside(SidecarRecord::Registers { timestamp_width: 8 }));
        assert_eq!(cores::VBA_M.clock, beside(SidecarRecord::Registers { timestamp_width: 8 }));
        assert_eq!(cores::SAMEBOY.clock, beside(SidecarRecord::SameBoy));
        // A GBA core keeps its clock in a file too, a record of another chip entirely.
        assert_eq!(cores::MINCER_RAY_GBA.clock, beside(SidecarRecord::PackedSeiko));
        // The MiSTer core writes a whole SD block; its openFPGA ports write sixteen bytes. Both
        // give the host clock four bytes, where the appending emulators give it eight.
        let block = Some(ClockLayout::Packed { reserved: 512, timestamp_width: 4 });
        assert_eq!(cores::GAMEBOY_MISTER.clock, block);
        assert_eq!(cores::SGB_MISTER.clock, block);
        // Named without the `-gb` ending the old suffix rule looked for, and packed all the same.
        assert_eq!(cores::BUDUDE2_GBC.clock, Some(ClockLayout::Packed { reserved: 16, timestamp_width: 4 }));
        // The GBA and WonderSwan cores append a whole block too, of a different chip's record.
        assert_eq!(cores::GBA_MISTER.clock, block);
        assert_eq!(cores::WONDERSWAN_MISTER.clock, block);
        assert_eq!(
            cores::AGG23_WONDERSWAN.clock,
            Some(ClockLayout::Packed { reserved: 12, timestamp_width: 4 })
        );
        // Spiritualized publishes no source and its releases predate the Pocket's clock, so
        // nothing says how they would lay one down.
        assert_eq!(cores::SPIRITUALIZED_GBC.clock, None);
        // A MiSTer core for a system with no MBC3 in it records nothing, rather than inheriting
        // the Game Boy core's layout because the two slugs end alike.
        assert_eq!(cores::PSX_MISTER.clock, None);
    }

    #[test]
    fn a_huc3_is_kept_the_way_each_core_keeps_one() {
        // The same layout for both chips on MiSTer and in Gambatte, and not in VBA-M, which keeps
        // an MBC3's registers beside the save and a HuC-3's origin.
        assert_eq!(cores::GAMEBOY_MISTER.huc3, Some(Huc3Layout::Packed { reserved: 512 }));
        assert_eq!(cores::SGB_MISTER.huc3, Some(Huc3Layout::Packed { reserved: 512 }));
        assert_eq!(cores::BUDUDE2_GBC.huc3, Some(Huc3Layout::Packed { reserved: 16 }));
        let origin = Some(Huc3Layout::Origin { timestamp_width: 8, big_endian: false });
        assert_eq!(cores::GAMBATTE.huc3, origin);
        assert_eq!(cores::VBA_M.huc3, origin);
        // Two libretro cores keep none at all, which is an answer rather than an absence of one.
        assert_eq!(cores::MGBA.huc3, Some(Huc3Layout::NotKept));
        assert_eq!(cores::SAMEBOY.huc3, Some(Huc3Layout::NotKept));
        // A core nobody has looked at says nothing.
        assert_eq!(cores::SPIRITUALIZED_GBC.huc3, None);
        // And only a core that runs a Game Boy has one to keep.
        for core in cores().iter().filter(|core| core.huc3.is_some()) {
            assert!(core.systems.iter().any(|s| matches!(*s, "gb" | "gbc")), "{}", core.slug);
        }
    }

    #[test]
    fn an_app_running_a_build_of_its_own_lays_the_clock_out_its_own_way() {
        // The mGBA app appends the record its libretro core keeps beside the save.
        assert_eq!(
            cores::MGBA.clock_in(Some(&apps::MGBA)),
            Some(ClockLayout::Appended { timestamp_width: 8 })
        );
        // OpenEmu's Gambatte writes the same instant narrower and the other way round.
        assert_eq!(
            cores::GAMBATTE.clock_in(Some(&apps::OPENEMU)),
            Some(ClockLayout::Sidecar(SidecarRecord::Origin { timestamp_width: 4, big_endian: true }))
        );
        // RetroArch loads the core as a libretro core, so it writes what the core's kind does.
        for core in [&cores::MGBA, &cores::GAMBATTE, &cores::SAMEBOY, &cores::GAMEBOY_MISTER] {
            if apps::RETROARCH.runs(core) {
                assert_eq!(core.clock_in(Some(&apps::RETROARCH)), core.clock, "{}", core.slug);
            }
            // And naming no app says nothing more than the core does.
            assert_eq!(core.clock_in(None), core.clock, "{}", core.slug);
        }
        // A MiSTer runs MiSTer cores as their kind.
        assert_eq!(cores::GAMEBOY_MISTER.clock_in(Some(&apps::MISTER)), cores::GAMEBOY_MISTER.clock);
        // A build of its own nobody has sampled is not assumed to be the libretro core's: OpenEmu
        // builds mGBA itself, and Provenance builds Gambatte though it also loads libretro cores.
        assert_eq!(cores::MGBA.clock_in(Some(&apps::OPENEMU)), None);
        assert_eq!(cores::GAMBATTE.clock_in(Some(&apps::PROVENANCE)), None);

        // The HuC-3 the same way: the mGBA app keeps the whole controller where its libretro core
        // keeps nothing, and OpenEmu's Gambatte will not load a HuC-3 cartridge at all.
        assert_eq!(cores::MGBA.huc3_in(Some(&apps::MGBA)), Some(Huc3Layout::Memory));
        assert_eq!(cores::MGBA.huc3_in(Some(&apps::RETROARCH)), Some(Huc3Layout::NotKept));
        assert_eq!(cores::MGBA.huc3_in(None), Some(Huc3Layout::NotKept));
        assert_eq!(cores::GAMBATTE.huc3_in(Some(&apps::OPENEMU)), Some(Huc3Layout::NotKept));
        assert_eq!(cores::GAMEBOY_MISTER.huc3_in(Some(&apps::MISTER)), cores::GAMEBOY_MISTER.huc3);

        // The standalone SameBoy and VisualBoyAdvance-M append both chips' clocks to the save, where
        // their libretro cores keep an MBC3's beside it.
        for (core, app, huc3) in [
            (&cores::SAMEBOY, &apps::SAMEBOY, Huc3Layout::SameBoy),
            (&cores::VBA_M, &apps::VISUALBOYADVANCE_M, Huc3Layout::Vbam),
        ] {
            assert_eq!(core.clock_in(Some(app)), Some(ClockLayout::Appended { timestamp_width: 8 }));
            assert_eq!(core.huc3_in(Some(app)), Some(huc3));
            assert!(matches!(core.clock_in(Some(&apps::RETROARCH)), Some(ClockLayout::Sidecar(_))));
        }
        // BGB has no other build, so its entry is the app's.
        assert_eq!(cores::BGB.clock_in(Some(&apps::BGB)), Some(ClockLayout::Appended { timestamp_width: 8 }));
    }

    #[test]
    fn a_save_layout_is_the_core_s_as_its_kind_hosts_it() {
        // A Mega Drive's SRAM spread over its bus by some cores and packed by others.
        assert!(matches!(
            cores::PICODRIVE.layout_in(Some(&apps::RETROARCH), "genesis"),
            Some(SaveLayout::Expanded {
                fill: 0x00,
                length: ExpandedLength::Doubled,
                eeprom_length: Some(8192)
            })
        ));
        assert_eq!(cores::BLASTEM.layout_in(None, "genesis"), Some(SaveLayout::Packed { length: None }));
        // Only for the system the layout is for: Genesis Plus GX keeps a Master System's as it is.
        assert_eq!(cores::GENESIS_PLUS_GX.layout_in(None, "mastersystem"), None);
        assert_eq!(
            cores::GBA_MISTER.layout_in(Some(&apps::MISTER), "gba"),
            Some(SaveLayout::Chip { eeprom_min: 8192 })
        );
        // A Game Boy MBC7's EEPROM: SameBoy turns each word round, mGBA keeps the chip's order.
        assert_eq!(
            cores::SAMEBOY.layout_in(Some(&apps::RETROARCH), "gbc"),
            Some(SaveLayout::Mbc7 { swapped: true, padding: None })
        );
        assert_eq!(
            cores::TGB_DUAL.layout_in(None, "gb"),
            Some(SaveLayout::Mbc7 { swapped: false, padding: Some((8192, 0xFF)) })
        );
        assert_eq!(cores::MGBA.layout_in(None, "gbc"), None);
        // An app's own build nobody has checked keeps nothing assumed: OpenEmu builds Genesis Plus GX.
        assert_eq!(cores::GENESIS_PLUS_GX.layout_in(Some(&apps::OPENEMU), "genesis"), None);
        for core in &CORES {
            for layout in core.layouts {
                assert!(core.systems.contains(&layout.system), "{} lays out {}", core.slug, layout.system);
            }
        }
    }

    #[test]
    fn every_recorded_build_is_one_its_app_ships() {
        // An override for an app that loads the core as its kind would contradict the core's own
        // entry, and one for an app that does not run the core at all is a typo.
        for entry in &APP_CLOCKS {
            let app = app(entry.app).unwrap_or_else(|| panic!("{} is not a listed app", entry.app));
            assert!(app.cores.builds.contains(&entry.core), "{} ships no build of {}", entry.app, entry.core);
        }
        assert!(APP_CLOCKS.is_sorted_by_key(|c| (c.core, c.app)), "sorted, for the binary search");
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
            ("ps1-mc", 128),
            ("ps2-mc", 512),
            ("n64-cpak", 32),
            ("gc-mc", 64),
            ("vmu", 32),
            ("neogeo-mc", 4),
            // Saturn's entry sits at the head of the save's own first block; the block list
            // after it is the writer's, and the thirty bytes ahead of it are not.
            ("saturn-bup", 30),
        ] {
            assert_eq!(card_format(name).unwrap().dirent_len, len, "{name}");
        }
    }

    #[test]
    fn block_sizes_are_what_the_memory_cards_spec_fixes() {
        // The unit a card allocates a save in, added in 0.2. `saturn-bup` has one like the rest:
        // the 64 is the console's internal memory.
        for (name, block) in [
            ("ps1-mc", 8192),
            ("ps2-mc", 1024),
            ("n64-cpak", 256),
            ("gc-mc", 8192),
            ("vmu", 512),
            ("neogeo-mc", 64),
            ("saturn-bup", 64),
        ] {
            assert_eq!(card_format(name).unwrap().block_size, block, "{name}");
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
