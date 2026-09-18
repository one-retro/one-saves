//! A PlayStation card DuckStation wrote: what its save is called, and what it looks like.

#![cfg(feature = "ps1")]

use one_saves::dcbor::CBOR;
use one_saves::{Bundle, ReverseDnsName};
use one_saves_convert::{CardOptions, Format};

const CARD: &str = "data/saves/PS1/Gran Turismo/DuckStation/shared_card_1.mcd";

fn save() -> Bundle {
    let bytes =
        std::fs::read(format!("{}/../../{CARD}", env!("CARGO_MANIFEST_DIR"))).expect("the vendored card");
    let card = one_saves_convert::card::read(Format::Ps1Card, &bytes, &CardOptions::default())
        .expect("reads the card");
    assert_eq!(card.parts.len(), 1, "the card holds one save");
    Bundle::from_slice(&card.parts[0].bytes().expect("nested")).expect("a save")
}

#[cfg(feature = "shift-jis")]
#[test]
fn a_playstation_save_is_labelled_by_the_title_in_its_first_block() {
    let save = save();
    let key = ReverseDnsName::parse("x.1sav.label").expect("well-formed");
    let map = save.header.extensions.get(&key).expect("a label").as_map().expect("a map");

    // Full width, as the card holds it. The format asks for NFC, which does not fold these to
    // ASCII, and folding them would be writing a title the save does not say.
    assert_eq!(map.get::<u64, CBOR>(0).unwrap().as_text().unwrap(), "ＧＴ　ｇａｍｅ　ｄａｔａ");
    // A PS1 title is one line; there is no second for a detail.
    assert!(map.get::<u64, CBOR>(1).is_none());
}

#[cfg(feature = "icon")]
#[test]
fn a_playstation_save_carries_its_icon_as_png() {
    use std::fmt::Write as _;

    use sha2::{Digest, Sha256};

    let save = save();
    let key = ReverseDnsName::parse("x.1sav.icon").expect("well-formed");
    let map = save.header.extensions.get(&key).expect("an icon").as_map().expect("a map");

    let frames = map.get::<u64, CBOR>(0).expect("frames");
    let frames = frames.as_array().expect("an array");
    assert_eq!(frames.len(), 1, "this save animates over one frame");
    // A PS1 keeps no timing of its own: the console runs the frames at its own rate.
    assert_eq!(frames[0].as_array().expect("a frame").len(), 1, "a reading and no hold");
    // Nor a banner, which is a GameCube thing.
    assert!(map.get::<u64, CBOR>(1).is_none());

    let png = frames[0].as_array().unwrap()[0].as_byte_string().expect("a PNG");
    let decoder = png::Decoder::new(std::io::Cursor::new(png));
    let mut reader = decoder.read_info().expect("a PNG");
    let mut buf = vec![0; reader.output_buffer_size().expect("bounded")];
    let info = reader.next_frame(&mut buf).expect("one frame");
    buf.truncate(info.buffer_size());

    assert_eq!((info.width, info.height), (16, 16));
    // Four bits a pixel with the *high* nibble on the left. Reading it the other way round is the
    // mistake that still produces a picture: every pair of pixels swaps, which combs each solid
    // shape into stripes a pixel wide. The digest is over pixels, so a change of PNG encoder is
    // not a change of picture.
    let digest = Sha256::digest(&buf).iter().take(8).fold(String::new(), |mut out, byte| {
        write!(out, "{byte:02x}").expect("writing to a String");
        out
    });
    assert_eq!(digest, "64a85b8fc2f84831");

    // And it belongs to the save, so it is not on the card's part.
    let bytes = std::fs::read(format!("{}/../../{CARD}", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let card =
        one_saves_convert::card::read(Format::Ps1Card, &bytes, &CardOptions::default()).expect("reads");
    assert!(!card.parts[0].extensions.contains_key(&key));
}
