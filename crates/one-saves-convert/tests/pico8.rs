//! PICO-8 saves through the converter as a caller meets them: a runtime's
//! `.p8d.txt` wrapped and written back out, and the P8 core's `.sav` read as
//! the saves it keeps.
//!
//! The `.sav` files under `data/saves/mister/p8/` are ones the P8 core wrote on
//! a MiSTer: `lootslime-1.p8.sav` keeps `lootslime`'s block and `mot_airpico.sav`
//! keeps `mot_planesim`'s, the id the launcher's game cart opens.

use std::path::Path;

use one_saves::Bundle;
use one_saves_convert::entries;
use one_saves_convert::pico8::{MEMORY_LEN, memory_of_text, text_of_memory};
use one_saves_convert::raw::{self, RawOptions, Reader};
use one_saves_convert::{Format, detect, layout};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/saves/mister/p8").join(name))
        .expect("a P8 save in data/saves/mister/p8")
}

fn memory(first: u32) -> [u8; MEMORY_LEN] {
    let mut memory = [0u8; MEMORY_LEN];
    memory[..4].copy_from_slice(&first.to_le_bytes());
    memory
}

fn wrapped(file: &[u8]) -> Bundle {
    raw::wrap(file, &RawOptions { system: Some("pico-8".into()), ..RawOptions::default() }).expect("wraps")
}

#[test]
fn a_real_p8_save_is_its_ids_saves() {
    for (file, id) in [("lootslime-1.p8.sav", "lootslime"), ("mot_airpico.sav", "mot_planesim")] {
        let bytes = fixture(file);
        let read = entries::read(&bytes, "pico-8").unwrap_or_else(|| panic!("{file} keeps entries"));
        assert_eq!(read.len(), 1, "{file}");
        assert_eq!(read[0].name, id, "{file}");
        // The entry is the block's data, as the text every other runtime writes.
        let data: [u8; MEMORY_LEN] = bytes[256..512].try_into().unwrap();
        assert_eq!(memory_of_text(&read[0].file), Some(data), "{file}");
    }
}

#[test]
fn a_real_p8_save_written_back_unchanged_is_the_same_bytes() {
    for file in ["lootslime-1.p8.sav", "mot_airpico.sav"] {
        let bytes = fixture(file);
        let entry = entries::read(&bytes, "pico-8").unwrap().remove(0);
        assert_eq!(entries::write(&bytes, "pico-8", &entry.name, &entry.file).unwrap(), bytes, "{file}");
    }
}

#[test]
fn a_real_p8_save_is_detected_as_a_flat_save() {
    assert_eq!(detect(&fixture("lootslime-1.p8.sav"), "sav").unwrap(), Format::Raw);
}

#[test]
fn a_runtimes_text_is_wrapped_as_the_memory_and_written_back_as_text() {
    let data = memory(0x0034_0000);
    let text = text_of_memory(&data);
    assert_eq!(detect(&text, "txt").unwrap(), Format::Raw);

    let bundle = wrapped(&text);
    assert_eq!(bundle.parts.len(), 1);
    assert_eq!(&*bundle.parts[0].payload, &data[..], "the cart's data, not its text");

    let out = raw::unwrap_as(&bundle, &Reader::default()).expect("unwraps");
    assert_eq!(out.save, text);
}

#[test]
fn text_and_a_p8_block_of_the_same_play_are_one_save() {
    let data = memory(7);
    let from_text = wrapped(&text_of_memory(&data));
    let block = entries::write(&[], "pico-8", "porklike", &data).unwrap();
    let from_p8 = wrapped(&entries::read(&block, "pico-8").unwrap()[0].file);
    assert_eq!(from_text.parts[0].payload, from_p8.parts[0].payload);
}

/// A bundle stored before bundles held the memory kept the writer's text. It
/// comes back out as the text all the same.
#[test]
fn a_bundle_holding_the_text_from_before_writes_the_text() {
    let data = memory(3);
    let text = text_of_memory(&data);
    let mut bundle = wrapped(&text);
    bundle.parts[0] = one_saves::Part::new(0, text.clone());
    assert_eq!(raw::unwrap_as(&bundle, &Reader::default()).unwrap().save, text);
    assert_eq!(layout::normalize_stored(&text, "pico-8", None, None), data);
}

#[test]
fn a_text_cut_short_reads_as_pico_8_reads_it() {
    // PICO-8 leaves what a short file does not reach at zero.
    let bundle = wrapped(b"00340000\n");
    assert_eq!(&*bundle.parts[0].payload, &memory(0x0034_0000)[..]);
}

#[test]
fn another_systems_text_is_no_concern_of_pico_8s() {
    assert!(detect(b"a note left on the card", "txt").is_err());
    assert_eq!(layout::normalize(b"0001", "gba", None, None), b"0001");
}
