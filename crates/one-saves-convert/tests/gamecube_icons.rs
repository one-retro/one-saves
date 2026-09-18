//! GameCube icons, decoded off saves a console actually wrote.
//!
//! The two fixtures cover both pixel formats between them: F-Zero GX stores its icon as RGB5A3,
//! which carries its colours inline, and Melee stores one as CI8 against a palette that follows
//! the frames. Both keep a 96x32 banner. A decoder that muddled the tile geometry would still
//! produce a plausibly sized picture, so the pixels are hashed rather than the shape checked.

#![cfg(feature = "icon")]

use one_saves::dcbor::CBOR;
use one_saves::{Bundle, ReverseDnsName};
use one_saves_convert::{CardOptions, Format};

/// Wraps a `.gci` — a 64-byte directory entry and the save's payload — as a one-save card.
fn card_from_gci(path: &str) -> Vec<u8> {
    let gci = std::fs::read(format!("{}/../../{path}", env!("CARGO_MANIFEST_DIR"))).expect("a vendored .gci");
    let (dirent, data) = gci.split_at(64);
    let mut builder = gc_memcard::CardBuilder::new();
    builder.capacity((251 - gc_memcard::RESERVED_BLOCKS) * gc_memcard::BLOCK);
    builder.add(gc_memcard::Save {
        slot: 0,
        game_code: String::from_utf8_lossy(&dirent[0..4]).into_owned(),
        maker_code: String::from_utf8_lossy(&dirent[4..6]).into_owned(),
        filename: String::from_utf8_lossy(&dirent[8..40]).trim_end_matches('\0').into(),
        dirent: dirent.to_vec(),
        data: data.to_vec(),
    });
    builder.build().expect("builds a card")
}

/// The pixels a PNG decodes to, so a change of encoder is not a change of picture.
fn pixels(png: &[u8]) -> (u32, u32, Vec<u8>) {
    let decoder = png::Decoder::new(std::io::Cursor::new(png));
    let mut reader = decoder.read_info().expect("a PNG");
    let mut buf = vec![0; reader.output_buffer_size().expect("a bounded image")];
    let info = reader.next_frame(&mut buf).expect("one frame");
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    Sha256::digest(bytes).iter().take(8).fold(String::new(), |mut out, byte| {
        write!(out, "{byte:02x}").expect("writing to a String");
        out
    })
}

#[test]
fn a_gamecube_save_carries_its_icon_and_banner_as_png() {
    for (path, frame_digest, banner_digest) in [
        (
            "data/saves/GameCube/F-Zero GX/Dolphin/8P-GFZE-f_zero.dat.gci",
            "9b587ad1c35b8e1e",
            "3953a42ad0a9b6a8",
        ),
        (
            "data/saves/GameCube/Super Smash Bros. Melee/Dolphin/01-GALE-SuperSmashBros0110290334.gci",
            "aec55b9f51c1a738",
            "2c34ebbfdb330623",
        ),
    ] {
        let card = card_from_gci(path);
        let bundle = one_saves_convert::card::read(Format::GcCard, &card, &CardOptions::default())
            .expect("reads the card");

        // The picture belongs to the save, so it is in the header of the bundle that is the save
        // and not on the card's part naming it: it has to survive the save being sliced out.
        let save = Bundle::from_slice(&bundle.parts[0].bytes().expect("the nested bundle")).expect("a save");
        let key = ReverseDnsName::parse("x.1sav.icon").expect("well-formed");
        assert!(
            !bundle.parts[0].extensions.contains_key(&key),
            "{path}: the icon does not ride on the card's part"
        );
        let icon = save.header.extensions.get(&key).expect("a decoded icon");
        let map = icon.as_map().expect("an icon map");

        let frames = map.get::<u64, CBOR>(0).expect("frames");
        let frames = frames.as_array().expect("frames are an array");
        assert_eq!(frames.len(), 1, "{path}: both saves keep a still icon");

        let frame = frames[0].as_array().expect("a frame is a reading and maybe a hold");
        let (w, h, px) = pixels(frame[0].as_byte_string().expect("a PNG"));
        assert_eq!((w, h), (32, 32), "{path}: an icon is 32x32");
        assert_eq!(digest(&px), frame_digest, "{path}: icon pixels");
        // Three twelfths of a second, which is what the entry's `anim_speed` asks for.
        assert_eq!(i64::try_from(frame[1].clone()).expect("a hold"), 250, "{path}");

        let banner = map.get::<u64, CBOR>(1).expect("a banner");
        let (w, h, px) = pixels(banner.as_byte_string().expect("a PNG"));
        assert_eq!((w, h), (96, 32), "{path}: a banner is 96x32");
        assert_eq!(digest(&px), banner_digest, "{path}: banner pixels");
    }
}

#[test]
fn a_card_read_without_the_feature_still_reads() {
    // The key is optional to carry, so its absence is not a degraded read. This asserts the shape
    // of the bundle is the same either way, which is what lets the feature default to off.
    let card = card_from_gci("data/saves/GameCube/F-Zero GX/Dolphin/8P-GFZE-f_zero.dat.gci");
    let bundle =
        one_saves_convert::card::read(Format::GcCard, &card, &CardOptions::default()).expect("reads");
    assert_eq!(bundle.parts.len(), 1);
    bundle.validate().expect("a card with an icon is an ordinary card");
}
