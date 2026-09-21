//! What a console calls a save, for [`x.1sav.label`].
//!
//! Only when the producer read the text. A producer that would be guessing omits the key, and a
//! save with no text anywhere has nothing to put here — whether it has any is the game's doing
//! rather than the format's. Nothing here fills a gap from `game`, which names the release rather
//! than the save.
//!
//! The latitude is normalization: transcoding out of the encoding a console stored, and trimming
//! the padding a fixed-width field pads with. Anything further would be writing a line the save
//! does not say.
//!
//! [`x.1sav.label`]: https://docs.1retro.com/specifications/extensions/x.1sav.label/

// Which of these is live depends on which card formats are compiled in — a GameCube reads a plain
// field, a PlayStation a Shift-JIS one, and the fallback below reaches for the first when the
// decoder is off. Enumerating that per function would be a second copy of the call graph, kept by
// hand, and wrong the first time a format starts reading a title. The module as a whole is gated
// on there being a format that reads one at all, which is the part worth stating.
#![allow(dead_code)]

use one_saves::ReverseDnsName;
use one_saves::dcbor::{CBOR, Map};

/// The `x.1sav.label` key.
pub(crate) fn label_key() -> ReverseDnsName {
    ReverseDnsName::parse("x.1sav.label").expect("a spec name is well-formed")
}

/// The `x.1sav.label` value, where there is a title to carry.
///
/// `title` is required, so a save with only a detail line has nothing to write. The schema caps
/// each at 256 bytes, which is headroom rather than a target: the longest of these on real
/// hardware is a PS1 title at 64.
pub(crate) fn value(title: Option<String>, detail: Option<String>) -> Option<CBOR> {
    let title = title.filter(|line| fits(line))?;
    let mut map = Map::new();
    map.insert(0u64, title);
    if let Some(detail) = detail.filter(|line| fits(line)) {
        map.insert(1u64, detail);
    }
    Some(map.into())
}

/// Whether a line is one the schema admits: not empty, and not past its ceiling.
fn fits(line: &str) -> bool {
    (1..=256).contains(&line.len())
}

/// Reads a fixed-width field of plain text, trimming the padding it is padded with.
///
/// Returns `None` where the field holds a byte this cannot transcode. A console that stored a
/// title in an encoding nothing here decodes has a title, and omitting the key says so honestly,
/// where writing the ASCII that happened to survive would say something the save does not.
pub(crate) fn ascii_field(bytes: &[u8]) -> Option<String> {
    let text = until_nul(bytes);
    if text.iter().any(|&b| !(0x20..0x7f).contains(&b)) {
        return None;
    }
    let trimmed = std::str::from_utf8(text).ok()?.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// Reads a title a console stored in Shift-JIS.
///
/// Full width stays full width — the format asks for NFC, which does not fold `Ｔ` to `T`, and
/// folding it would be normalization the spec does not grant. A field this cannot read returns
/// `None` rather than a partial reading, which is the same rule as everywhere else here: a
/// producer that would be guessing omits the key.
#[cfg(feature = "shift-jis")]
pub(crate) fn shift_jis_field(bytes: &[u8]) -> Option<String> {
    let text = until_nul(bytes);
    let (decoded, _, malformed) = encoding_rs::SHIFT_JIS.decode(text);
    // Replacement characters would be a title the save does not say, spelled with question marks.
    if malformed {
        return None;
    }
    let trimmed = decoded.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// Without a decoder, a title in an encoding this cannot read is one it does not carry.
#[cfg(not(feature = "shift-jis"))]
pub(crate) fn shift_jis_field(bytes: &[u8]) -> Option<String> {
    ascii_field(bytes)
}

/// A fixed-width field runs to its first NUL, or to its end.
fn until_nul(bytes: &[u8]) -> &[u8] {
    match bytes.iter().position(|&b| b == 0) {
        Some(end) => &bytes[..end],
        None => bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fixed_width_field_loses_its_padding_and_nothing_else() {
        assert_eq!(ascii_field(b"F-ZERO GX\0\0\0"), Some("F-ZERO GX".into()));
        assert_eq!(ascii_field(b"Super Smash Bros. Melee         "), Some("Super Smash Bros. Melee".into()));
        // Inner spacing is the save's, not padding.
        assert_eq!(ascii_field(b"Game Data 2026/09/17"), Some("Game Data 2026/09/17".into()));
        // A field that is only padding holds no title.
        assert_eq!(ascii_field(b"          "), None);
        assert_eq!(ascii_field(b"\0\0\0\0"), None);
        // And one this cannot read is omitted rather than half-read.
        assert_eq!(ascii_field(b"M\xe9tal"), None);
    }

    #[cfg(feature = "shift-jis")]
    #[test]
    fn a_title_in_shift_jis_is_read_rather_than_approximated() {
        // "ＴＥＫＫＥＮ　４", which is what a PS2 card in `data/saves` actually holds.
        let tekken = b"\x82\x73\x82\x64\x82\x6a\x82\x6a\x82\x64\x82\x6d\x81\x40\x82\x53";
        assert_eq!(shift_jis_field(tekken), Some("ＴＥＫＫＥＮ　４".into()));
        // Full width stays full width: the format asks for NFC, which does not fold these, and
        // folding them would be writing a title the save does not say.
        assert_ne!(shift_jis_field(tekken), Some("TEKKEN 4".into()));
        // Kana reads too, which is the half a table of Latin letters would have missed.
        assert_eq!(shift_jis_field(b"\x82\xa0"), Some("あ".into()));
        // ASCII passes through, and padding goes.
        assert_eq!(shift_jis_field(b"DQ8 n01   "), Some("DQ8 n01".into()));
        // A lead byte with nothing after it is a truncated field, not a character.
        assert_eq!(shift_jis_field(b"AB\x82"), None);
    }

    #[test]
    fn a_label_needs_a_title_and_a_detail_is_optional() {
        assert!(value(Some("F-ZERO GX".into()), None).is_some());
        assert!(value(Some("F-ZERO GX".into()), Some("26/08/20 HANS".into())).is_some());
        // A detail with no title is nothing the schema can carry.
        assert!(value(None, Some("orphan".into())).is_none());
        assert!(value(Some(String::new()), None).is_none());
    }
}
