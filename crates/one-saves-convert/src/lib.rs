//! Convert emulator saves and memory cards to and from the Universal Saves Format.
//!
//! The format itself is [`one-saves`](https://docs.rs/one-saves); this crate is the half that
//! knows about real hardware. It reads what emulators and cartridge readers actually write, and
//! writes it back.
//!
//! | Format | Filesystem crate | Feature |
//! | ------ | ---------------- | ------- |
//! | Flat cartridge saves (`.srm`, `.sav`, `.eep`, `.fla`) | — | always |
//! | PS1 memory cards (`.mcr`, `.mcd`, `.gme`, `.vgs`, `.vmp`) | [`ps1_memcard`] | `ps1` |
//! | N64 Controller Paks (`.mpk`) | [`n64_cpak`] | `n64` |
//! | GameCube memory cards (`.raw`, `.gcp`) | [`gc_memcard`] | `gc` |
//! | Dreamcast VMU (`.bin`) | [`dreamcast_vmu`] | `vmu` |
//! | PS2 memory cards (`.ps2`, with or without ECC) | [`ps2_memcard`] | `ps2` |
//! | Neo Geo memory cards (`.neo`, bare or in a MiSTer save) | [`neogeo_memcard`] | `neogeo` |
//!
//! Every card format reads and writes. No filesystem is implemented here: each lives in a crate of
//! its own, because a memory card is a reusable format and nothing about reading one needs this
//! container. What [`card`] holds is the adapter per format — the mapping between a card's saves
//! and a bundle's nested parts — and nothing else.
//!
//! # Clock state
//!
//! Emulators append real-time-clock state to the saves of cartridges that have a clock — Pokémon
//! Crystal, the Zelda Oracle pair, Pokémon Ruby and friends — and nothing in the file says where
//! the save memory ends. That footer carries a live timestamp, so left inline it gives the save a
//! content hash that moves every time the clock does.
//!
//! [`rtc`] splits it out: the save part becomes the bare save memory, and the clock moves into
//! the header's extension keys — `x.1sav.rtc` for the portable instant and `x.1sav.rtc.mbc3` for
//! the chip's own state, so extraction hands the emulator back exactly the file it wrote.
//!
//! Every card format here round-trips: a card read into a bundle and written back out is the
//! same card. What that does **not** promise is byte-exact fidelity through a *rebuild* on PS2,
//! where a writer reallocates the filesystem; keep a `card-image` part if you need that.
//!
//! # Features
//!
//! Everything optional is a feature, and all of them are on by default.
//!
//! | Feature | What it is | What it brings |
//! | ------- | ---------- | -------------- |
//! | `cards` | All six card formats at once | the six below |
//! | `ps1` `n64` `gc` `vmu` `ps2` `neogeo` | One card format each | that format's crate, and nothing else |
//! | `dat` | The `dat` module: resolving a digest through a DAT catalog | `datary`, an XML reader and a derive macro |
//! | `dat-cmpro` | The ClrMamePro syntax as well as Logiqx XML | `winnow` |
//! | `rom` | The `rom` module: ROM headers and digests | `crc32fast`, `md-5`, `sha1`, `sha2` |
//! | `zstd` | Forwarded to [`one_saves`] | `zstd`, and the C library it compiles |
//!
//! Flat saves, detection, profiles and [`rtc`] are always present: they are what this crate is,
//! and they depend on nothing but [`one_saves`].
//!
//! [`Format`] keeps every variant whichever card features are on, so a downstream `match` compiles
//! the same way in every build. What a format's feature takes away is the reader and the writer:
//! [`card::read`] and [`card::write`] then return [`Error::Unsupported`], and
//! [`detect`](detect()) can no longer recognise that format by its **signature** — its extensions
//! still resolve, so a `.mcr` is named as a PS1 card and refused rather than misread.

// docs.rs builds with `--cfg docsrs` on nightly, which is what puts the feature badge on the two
// modules below. Nothing else sets it, so a stable build never sees the nightly attribute.
#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod card;
#[cfg(feature = "dat")]
#[cfg_attr(docsrs, doc(cfg(feature = "dat")))]
pub mod dat;
pub mod detect;
pub mod error;
#[cfg(feature = "icon")]
mod icon;
pub mod profile;
pub mod raw;
#[cfg(feature = "rom")]
#[cfg_attr(docsrs, doc(cfg(feature = "rom")))]
pub mod rom;
pub mod rtc;

pub use card::CardOptions;
pub use detect::{Format, detect};
pub use error::{Error, Result};
pub use profile::{Profile, profile};
pub use raw::RawOptions;
