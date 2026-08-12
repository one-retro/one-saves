//! Working out what a file is.
//!
//! Detection reads the bytes first and the extension second, because the extension is the less
//! reliable of the two: the same PS1 card ships as `.mcr`, `.mcd`, `.bin` and `.srm`, and that
//! last one is also what most libretro cores call a flat cartridge save. A file that matches no
//! signature falls back to the extension, and a file that matches neither is
//! [`Undetected`](crate::Error::Undetected) rather than guessed at.

use crate::card;
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
}

impl Format {
    /// What to call this format in a message.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Format::Bundle => "1saves bundle",
            Format::Raw => "flat cartridge save",
            Format::Ps1Card => "PS1 memory card",
            Format::N64Pak => "N64 Controller Pak",
            Format::GcCard => "GameCube memory card",
            Format::Vmu => "Dreamcast VMU",
            Format::Ps2Card => "PS2 memory card",
        }
    }

    /// The card format slug this maps to, for the formats that are cards.
    #[must_use]
    pub fn card_format(self) -> Option<&'static str> {
        match self {
            Format::Ps1Card => Some(card::ps1::CARD_FORMAT),
            Format::N64Pak => Some(card::n64::CARD_FORMAT),
            Format::GcCard => Some(card::gc::CARD_FORMAT),
            Format::Vmu => Some(card::vmu::CARD_FORMAT),
            Format::Ps2Card => Some(card::ps2::CARD_FORMAT),
            Format::Bundle | Format::Raw => None,
        }
    }

    /// The format a name denotes, for `--from` and `--to`.
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
pub fn detect(bytes: &[u8], extension: &str) -> Result<Format> {
    // A signature beats a name every time.
    if is_bundle(bytes) {
        return Ok(Format::Bundle);
    }
    if card::ps1::detect(bytes) {
        return Ok(Format::Ps1Card);
    }
    if card::vmu::detect(bytes) {
        return Ok(Format::Vmu);
    }
    if card::gc::detect(bytes) {
        return Ok(Format::GcCard);
    }
    if card::ps2::detect(bytes) {
        return Ok(Format::Ps2Card);
    }
    // The pak has no magic, so it is checked last: its test is that the index table is plausible,
    // which a file of the right length could pass by accident.
    if card::n64::detect(bytes) {
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
    fn the_bytes_beat_the_extension() {
        // `.srm` is what libretro calls both a flat save and a PS1 card dump, so the name cannot
        // settle it and the signature has to.
        let mut ps1 = vec![0u8; card::ps1::CAPACITY];
        ps1[0] = b'M';
        ps1[1] = b'C';
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
}
