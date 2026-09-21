//! A PlayStation card DuckStation wrote: what its saves are called, and what they look like.
//!
//! Two saves, which between them cover what a PS1 icon can be: Gran Turismo keeps a still one, and
//! Crash Bandicoot 2 animates over three frames.

// Every test here reads either a title or a picture off a card, so a build with neither has
// nothing to run and nothing to compile the helpers for.
#![cfg(all(feature = "ps1", any(feature = "shift-jis", feature = "icon")))]

use one_saves::dcbor::CBOR;
use one_saves::{Bundle, ReverseDnsName};
use one_saves_convert::{CardOptions, Format};

const CARD: &str = "data/saves/PS1/Gran Turismo/DuckStation/shared_card_1.mcd";

fn card() -> Bundle {
    let bytes =
        std::fs::read(format!("{}/../../{CARD}", env!("CARGO_MANIFEST_DIR"))).expect("the vendored card");
    one_saves_convert::card::read(Format::Ps1Card, &bytes, &CardOptions::default()).expect("reads the card")
}

/// The saves, in the order the directory lists them: Gran Turismo, then Crash 2.
fn saves() -> Vec<Bundle> {
    card()
        .parts
        .iter()
        .map(|part| Bundle::from_slice(&part.bytes().expect("nested")).expect("a save"))
        .collect()
}

#[cfg(feature = "shift-jis")]
fn label_of(save: &Bundle) -> String {
    let key = ReverseDnsName::parse("x.1sav.label").expect("well-formed");
    let map = save.header.extensions.get(&key).expect("a label").as_map().expect("a map");
    map.get::<u64, CBOR>(0).expect("a title").as_text().expect("text").to_owned()
}

/// The frames of a save's icon, each as the PNG and whatever hold it states.
#[cfg(feature = "icon")]
fn frames(save: &Bundle) -> Vec<(Vec<u8>, Option<i64>)> {
    let key = ReverseDnsName::parse("x.1sav.icon").expect("well-formed");
    let map = save.header.extensions.get(&key).expect("an icon").as_map().expect("a map");
    let frames = map.get::<u64, CBOR>(0).expect("frames");
    frames
        .as_array()
        .expect("an array")
        .iter()
        .map(|frame| {
            let frame = frame.as_array().expect("a frame");
            let png = frame[0].as_byte_string().expect("a PNG").to_vec();
            (png, frame.get(1).and_then(|hold| i64::try_from(hold.clone()).ok()))
        })
        .collect()
}

/// A short digest of what a PNG decodes to, so a change of encoder is not a change of picture.
#[cfg(feature = "icon")]
fn digest(png: &[u8]) -> String {
    use std::fmt::Write as _;

    use sha2::{Digest, Sha256};

    let decoder = png::Decoder::new(std::io::Cursor::new(png));
    let mut reader = decoder.read_info().expect("a PNG");
    let mut pixels = vec![0; reader.output_buffer_size().expect("bounded")];
    let info = reader.next_frame(&mut pixels).expect("one frame");
    pixels.truncate(info.buffer_size());
    assert_eq!((info.width, info.height), (16, 16), "a PS1 icon is 16x16");

    Sha256::digest(&pixels).iter().take(8).fold(String::new(), |mut out, byte| {
        write!(out, "{byte:02x}").expect("writing to a String");
        out
    })
}

#[cfg(feature = "shift-jis")]
#[test]
fn a_playstation_save_is_labelled_by_the_title_in_its_first_block() {
    // Full width, as the card holds it. The format asks for NFC, which does not fold these to
    // ASCII, and folding them would be writing a title the save does not say.
    assert_eq!(label_of(&saves()[0]), "ＧＴ　ｇａｍｅ　ｄａｔａ");
    assert_eq!(label_of(&saves()[1]), "Ｃｒａｓｈ　Ｂａｎｄｉｃｏｏｔ　２");
}

#[cfg(feature = "icon")]
#[test]
fn a_still_icon_is_one_frame() {
    let frames = frames(&saves()[0]);
    assert_eq!(frames.len(), 1);
    // Four bits a pixel with the *high* nibble on the left. Reading it the other way round is the
    // mistake that still produces a picture: every pair of pixels swaps, which combs each solid
    // shape into stripes a pixel wide.
    assert_eq!(digest(&frames[0].0), "64a85b8fc2f84831");
    // A PlayStation keeps no timing of its own; the console runs the frames at its own rate.
    assert_eq!(frames[0].1, None);

    // No banner either, which is a GameCube thing.
    let saves = saves();
    let key = ReverseDnsName::parse("x.1sav.icon").expect("well-formed");
    let map = saves[0].header.extensions.get(&key).unwrap().as_map().unwrap();
    assert!(map.get::<u64, CBOR>(1).is_none());
}

#[cfg(feature = "icon")]
#[test]
fn an_animated_icon_keeps_every_frame_in_display_order() {
    let frames = frames(&saves()[1]);
    assert_eq!(frames.len(), 3, "the header asks for three");

    let digests: Vec<String> = frames.iter().map(|(png, _)| digest(png)).collect();
    // Two of them are the same picture, which is the save's doing rather than the decoder's: those
    // frames are byte-identical in the payload. A stride of zero would make all three alike, so
    // the third differing is what says the frames are being walked rather than re-read.
    assert_eq!(digests[0], "018cac3b7d7df4f5");
    assert_eq!(digests[1], digests[0], "the first two frames really are one picture");
    assert_eq!(digests[2], "ed0574e23eb443f9", "and the third is another");

    assert!(frames.iter().all(|(_, hold)| hold.is_none()), "a PS1 states no hold");
}

#[cfg(feature = "icon")]
#[test]
fn an_icon_belongs_to_the_save_rather_than_to_the_card() {
    // It has to survive the save being sliced out, so it is in the save's own header.
    let key = ReverseDnsName::parse("x.1sav.icon").expect("well-formed");
    for part in &card().parts {
        assert!(!part.extensions.contains_key(&key));
    }
}

/// A save exported on its own, filed onto a card so the card reader can be pointed at it.
#[cfg(feature = "shift-jis")]
fn from_psv(rest: &str) -> Bundle {
    let path = format!("{}/../../data/saves/PS1/{rest}", env!("CARGO_MANIFEST_DIR"));
    let save = ps1_memcard::read_single(&std::fs::read(path).expect("the vendored save")).expect("a save");
    let image = ps1_memcard::CardBuilder::new().add(save).build().expect("one save fits");
    let card =
        one_saves_convert::card::read(Format::Ps1Card, &image, &CardOptions::default()).expect("reads");
    Bundle::from_slice(&card.parts[0].bytes().expect("nested")).expect("a save")
}

/// A Japanese release's title, which is the case the Shift-JIS table exists for.
///
/// The Latin letters here are full-width ones — the card stores `Ｃ` at 0x8262, not `C` at 0x43 —
/// and they stay full-width. Narrowing them is what NFKC would do and the format asks for NFC, so
/// a reader that normalises harder than it was told writes a title the save does not carry.
#[cfg(feature = "shift-jis")]
#[test]
fn a_japanese_title_keeps_its_full_width_letters() {
    let save = from_psv("Capcom vs SNK Millennium Fight 2000 Pro/PS3/BISLPM-87053.PSV");
    assert_eq!(label_of(&save), "ＣＡＰＣＯＭ　ＶＳ．　ＳＮＫ　ＰＲＯ");
}

/// A save that spans two blocks, labelled off the first one.
///
/// Final Fantasy IV writes the party's HP and the playtime into the title, so what the console
/// lists is the state of the game rather than a name for it. Nothing here has to understand that;
/// it is worth a fixture because a title full of digits and punctuation is the one a Shift-JIS
/// table gets wrong quietly, where a title full of letters comes out looking fine either way.
#[cfg(feature = "shift-jis")]
#[test]
fn a_two_block_save_is_labelled_off_its_first_block() {
    let save = from_psv("Final Fantasy Chronicles/PS3/BASLUS-01360464634.PSV");
    assert_eq!(label_of(&save), "ＦＦ４　３１０７／３１０７　　　４５：２０");
}
