//! Convert emulator saves and memory cards to and from the Universal Saves Format.
//!
//! The format itself is [`one-saves`](https://docs.rs/one-saves); this crate is the half that
//! knows about real hardware. It reads what emulators and cartridge readers actually write, and
//! writes it back.
//!
//! | Format | Status |
//! | ------ | ------ |
//! | Flat cartridge saves (`.srm`, `.sav`, `.eep`, `.fla`) | read and write |
//! | PS1 memory cards (`.mcr`, `.mcd`, `.gme`, `.vgs`, `.vmp`) | read and write |
//! | N64 Controller Paks (`.mpk`) | read and write |
//! | GameCube memory cards (`.raw`, `.gcp`) | read and write |
//! | Dreamcast VMU (`.bin`) | read and write |
//! | PS2 memory cards (`.ps2`, with or without ECC) | read and write |
//!
//! The PS2 filesystem lives in its own crate, [`ps2_memcard`], since a PS2 card is a reusable
//! format and nothing about reading one needs this container.
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

pub mod card;
pub mod dat;
pub mod detect;
pub mod error;
pub mod profile;
pub mod raw;
pub mod rom;
pub mod rtc;

pub use card::CardOptions;
pub use detect::{Format, detect};
pub use error::{Error, Result};
pub use profile::{Profile, profile};
pub use raw::RawOptions;
