//! Working out what a file is.
//!
//! Detection reads the bytes first and the extension second, because the extension is the less
//! reliable of the two: the same PS1 card ships as `.mcr`, `.mcd`, `.bin` and `.srm`, and that
//! last one is also what most libretro cores call a flat cartridge save. A file that matches no
//! signature falls back to the extension, and a file that matches neither is
//! [`Undetected`](crate::Error::Undetected) rather than guessed at.
//!
//! [`Format`] lists every card format this crate knows of, whether or not this build was compiled
//! to read it. That is deliberate: a variant that came and went with a feature would make a
//! downstream `match` compile or not depending on how the crate was built, and a build that cannot
//! read a format still has to be able to name it.

use crate::error::{Error, Result};
use crate::raw;

/// What a file turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// A bundle already.
    Bundle,
    /// A flat cartridge save, with nothing in it to say what it belongs to.
    Raw,
    /// A PlayStation memory card, in any of its containers.
    Ps1Card,
    /// A Nintendo 64 Controller Pak.
    N64Pak,
    /// A GameCube memory card image.
    GcCard,
    /// A Dreamcast VMU image.
    Vmu,
    /// A PlayStation 2 memory card image.
    Ps2Card,
    /// A Neo Geo memory card image.
    NeoGeoCard,
}

impl Format {
    /// What to call this format in a message.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Format::Bundle => "1saves bundle",
            Format::Raw => "flat cartridge save",
            Format::Ps1Card => "PS1 memory card",
            Format::N64Pak => "N64 Controller Pak",
            Format::GcCard => "GameCube memory card",
            Format::Vmu => "Dreamcast VMU",
            Format::Ps2Card => "PS2 memory card",
            Format::NeoGeoCard => "Neo Geo memory card",
        }
    }

    /// The card format slug this maps to, for the formats that are cards.
    ///
    /// This is the one place each slug is written. The card modules read it from here rather than
    /// declaring their own, so a build without a format still spells it the same way.
    #[must_use]
    pub const fn card_format(self) -> Option<&'static str> {
        match self {
            Format::Ps1Card => Some("ps1-mc"),
            Format::N64Pak => Some("n64-cpak"),
            Format::GcCard => Some("gc-mc"),
            Format::Vmu => Some("vmu"),
            Format::Ps2Card => Some("ps2-mc"),
            Format::NeoGeoCard => Some("neogeo-mc"),
            Format::Bundle | Format::Raw => None,
        }
    }

    /// The registry slug for the system whose saves this format holds.
    #[must_use]
    pub const fn system(self) -> Option<&'static str> {
        match self {
            Format::Ps1Card => Some("psx"),
            Format::N64Pak => Some("n64"),
            Format::GcCard => Some("gc"),
            Format::Vmu => Some("dreamcast"),
            Format::Ps2Card => Some("ps2"),
            // One system, not two: the MVS cabinet and the AES console take the same card and read
            // each other's saves, which is the whole point of it.
            Format::NeoGeoCard => Some("neogeo"),
            Format::Bundle | Format::Raw => None,
        }
    }

    /// The socket this format is normally read from, when a caller does not say.
    ///
    /// Every other card format goes in a numbered memory card slot. A Neo Geo card has a socket of
    /// its own in the registry, because a cabinet keeps its own backup RAM in `internal` beside it.
    #[must_use]
    pub const fn default_role(self) -> &'static str {
        match self {
            Format::NeoGeoCard => "neogeo-card",
            _ => "memcard-1",
        }
    }

    /// The extension a file of this format conventionally takes, without the dot.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Format::Bundle => one_saves::EXTENSION,
            Format::Raw => "srm",
            Format::Ps1Card => "mcr",
            Format::N64Pak => "mpk",
            Format::GcCard => "raw",
            Format::Vmu => "bin",
            Format::Ps2Card => "ps2",
            Format::NeoGeoCard => "neo",
        }
    }

    /// Whether **this build** can read and write the format.
    ///
    /// Every card format is behind a feature. One that is off leaves the variant in place — so
    /// detection by name still resolves and errors still say what the file is — and takes the
    /// reader and writer with it.
    #[must_use]
    pub const fn is_supported(self) -> bool {
        match self {
            Format::Bundle | Format::Raw => true,
            Format::Ps1Card => cfg!(feature = "ps1"),
            Format::N64Pak => cfg!(feature = "n64"),
            Format::GcCard => cfg!(feature = "gc"),
            Format::Vmu => cfg!(feature = "vmu"),
            Format::Ps2Card => cfg!(feature = "ps2"),
            Format::NeoGeoCard => cfg!(feature = "neogeo"),
        }
    }

    /// The feature that supplies this format, for an error that has to name one.
    pub(crate) const fn feature(self) -> &'static str {
        match self {
            Format::Bundle | Format::Raw => "",
            Format::Ps1Card => "ps1",
            Format::N64Pak => "n64",
            Format::GcCard => "gc",
            Format::Vmu => "vmu",
            Format::Ps2Card => "ps2",
            Format::NeoGeoCard => "neogeo",
        }
    }

    /// [`Error::Unsupported`] naming this format, for a build that cannot handle it.
    pub(crate) const fn unsupported(self) -> Error {
        Error::Unsupported { format: self.label(), feature: self.feature() }
    }

    /// The format a bundle's `card.format` slug denotes.
    ///
    /// `None` for a slug this crate does not know at all, which is a different thing from one it
    /// knows and was not compiled to write — see [`is_supported`](Self::is_supported).
    #[must_use]
    pub fn from_card_format(slug: &str) -> Option<Self> {
        [Format::Ps1Card, Format::N64Pak, Format::GcCard, Format::Vmu, Format::Ps2Card, Format::NeoGeoCard]
            .into_iter()
            .find(|format| format.card_format() == Some(slug))
    }

    /// The format a name denotes, for `--from` and `--to`.
    ///
    /// Names resolve whether or not this build can read the format, so a request for one it
    /// cannot handle is refused in those words rather than as an unknown name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "1saves" | "bundle" => Some(Format::Bundle),
            "raw" | "srm" | "sav" => Some(Format::Raw),
            "ps1" | "ps1-mc" | "psx" | "mcr" => Some(Format::Ps1Card),
            "n64" | "n64-cpak" | "cpak" | "mpk" => Some(Format::N64Pak),
            "gc" | "gc-mc" | "gamecube" => Some(Format::GcCard),
            "vmu" | "dreamcast" => Some(Format::Vmu),
            "ps2" | "ps2-mc" | "psu" => Some(Format::Ps2Card),
            "neogeo" | "neogeo-mc" | "neo" | "mvs" | "aes" => Some(Format::NeoGeoCard),
            _ => None,
        }
    }
}

/// Whether these bytes open with the bundle tag.
///
/// The tag's own encoding is the file magic, so this is a five-byte check that needs no decoder.
#[must_use]
pub fn is_bundle(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\xda1SAV")
}

/// Works out what a file is, from its bytes and its name.
///
/// `extension` is whatever followed the last `.`, without it, or empty.
///
/// A card format this build was not compiled with cannot be recognised by its **signature**, since
/// the reader that knows the signature is what came off. Its extensions still resolve, so a
/// `.mcr` handed to a build without `ps1` is named as a PS1 card and refused rather than misread.
/// What such a build cannot do is tell a card dumped under an ambiguous name — `.bin`, `.srm` —
/// from the flat save those usually mean.
pub fn detect(bytes: &[u8], extension: &str) -> Result<Format> {
    // A signature beats a name every time.
    if is_bundle(bytes) {
        return Ok(Format::Bundle);
    }
    #[cfg(feature = "ps1")]
    if ps1_memcard::detect(bytes) {
        return Ok(Format::Ps1Card);
    }
    #[cfg(feature = "vmu")]
    if dreamcast_vmu::detect(bytes) {
        return Ok(Format::Vmu);
    }
    #[cfg(feature = "gc")]
    if gc_memcard::detect(bytes) {
        return Ok(Format::GcCard);
    }
    #[cfg(feature = "ps2")]
    if crate::card::ps2::detect(bytes) {
        return Ok(Format::Ps2Card);
    }
    // The pak has no magic, so it is checked last: its test is that the index table is plausible,
    // which a file of the right length could pass by accident.
    #[cfg(feature = "neogeo")]
    if neogeo_memcard::detect(bytes) {
        return Ok(Format::NeoGeoCard);
    }
    #[cfg(feature = "n64")]
    if n64_cpak::detect(bytes) {
        return Ok(Format::N64Pak);
    }

    // Nothing matched a signature, so fall back to the name.
    let lowered = extension.to_ascii_lowercase();
    match lowered.as_str() {
        "1saves" => Ok(Format::Bundle),
        "mcr" | "mcd" | "gme" | "vgs" | "vmp" => Ok(Format::Ps1Card),
        "mpk" | "pak" => Ok(Format::N64Pak),
        "raw" | "gcp" => Ok(Format::GcCard),
        "ps2" => Ok(Format::Ps2Card),
        "neo" => Ok(Format::NeoGeoCard),
        _ if raw::is_raw_extension(&lowered) => Ok(Format::Raw),
        _ => Err(Error::Undetected),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bundle_is_recognised_by_its_magic_alone() {
        // The tag's encoding reads "1SAV" from byte 1, which is the whole of the check.
        assert!(is_bundle(b"\xda1SAV\x82\xa0\x81"));
        assert!(!is_bundle(b"1SAV"));
        assert_eq!(detect(b"\xda1SAV\x82\xa0\x81", "").unwrap(), Format::Bundle);
    }

    #[test]
    #[cfg(feature = "ps1")]
    fn the_bytes_beat_the_extension() {
        // `.srm` is what libretro calls both a flat save and a PS1 card dump, so the name cannot
        // settle it and the signature has to.
        let mut ps1 = vec![0u8; ps1_memcard::CAPACITY];
        ps1[..2].copy_from_slice(ps1_memcard::MAGIC);
        assert_eq!(detect(&ps1, "srm").unwrap(), Format::Ps1Card);
        assert_eq!(detect(&[1, 2, 3, 4], "srm").unwrap(), Format::Raw);
    }

    #[test]
    fn an_unknown_file_is_not_guessed_at() {
        assert!(matches!(detect(&[0u8; 7], "wat"), Err(Error::Undetected)));
    }

    #[test]
    fn names_resolve_for_the_from_and_to_flags() {
        assert_eq!(Format::from_name("ps1"), Some(Format::Ps1Card));
        assert_eq!(Format::from_name("PS2"), Some(Format::Ps2Card));
        assert_eq!(Format::from_name("nonsense"), None);
        assert_eq!(Format::Ps1Card.card_format(), Some("ps1-mc"));
        assert_eq!(Format::Raw.card_format(), None);
    }

    #[test]
    fn every_card_format_names_a_slug_a_system_and_an_extension() {
        // The four card properties are parallel, and a variant added with one of them missing is
        // the mistake this catches.
        for format in [
            Format::Ps1Card,
            Format::N64Pak,
            Format::GcCard,
            Format::Vmu,
            Format::Ps2Card,
            Format::NeoGeoCard,
        ] {
            assert!(format.card_format().is_some(), "{format:?} has no card format slug");
            assert!(!format.default_role().is_empty(), "{format:?} has no default role");
            assert!(format.system().is_some(), "{format:?} has no system slug");
            assert!(!format.extension().is_empty(), "{format:?} has no extension");
            assert!(!format.feature().is_empty(), "{format:?} has no feature");
        }
        for format in [Format::Bundle, Format::Raw] {
            assert_eq!(format.card_format(), None);
            assert!(format.is_supported(), "{format:?} is not behind a feature");
        }
    }

    #[test]
    fn a_name_resolves_even_for_a_format_this_build_cannot_read() {
        // The point of keeping every variant: a build without `ps1` still turns "mcr" into a PS1
        // card, so the refusal can say what the file is.
        assert_eq!(Format::from_name("mcr"), Some(Format::Ps1Card));
        assert_eq!(detect(&[0u8; 4], "mcr").unwrap(), Format::Ps1Card);
        assert_eq!(Format::Ps1Card.is_supported(), cfg!(feature = "ps1"));
    }
}
