//! Saves that arrived on their own rather than on a card.
//!
//! Both fixtures are `.psv`, the format a PS3 and a Vita export one save as. They are here because
//! the name a `.psv` carries and the name its *file* carries are not the same string, and because
//! a two-block save is the only kind that proves the length field is read rather than assumed.

use std::path::PathBuf;

use ps1_memcard::{BLOCK, CardBuilder, MemoryCard, read_single};

fn psv(rest: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/saves/PS1").join(rest);
    std::fs::read(path).expect("a vendored save")
}

const JAPANESE: &str = "Capcom vs SNK Millennium Fight 2000 Pro/PS3/BISLPM-87053.PSV";
const TWO_BLOCK: &str = "Final Fantasy Chronicles/PS3/BASLUS-01360464634.PSV";

#[test]
fn a_psv_gives_up_its_save() {
    let save = read_single(&psv(JAPANESE)).expect("reads");
    assert_eq!(save.name, "BISLPM-87053");
    assert_eq!(save.data.len(), BLOCK);
    // The payload is the save itself, which every PS1 save starts with.
    assert_eq!(&save.data[..2], b"SC");
    // Nothing but the name comes out of a `.psv` header, so the entry is left for a builder.
    assert!(save.dirent.is_empty());
}

#[test]
fn a_two_block_save_keeps_both_blocks() {
    let save = read_single(&psv(TWO_BLOCK)).expect("reads");
    assert_eq!(save.data.len(), 2 * BLOCK, "the header's length is read, not guessed");
    // The name lives in the header. The file it arrived in is `BASLUS-01360464634.PSV`, which is
    // the same name with its non-alphanumeric tail hex-escaped, so a reader that took the filename
    // would file this save under a name no console would answer to.
    assert_eq!(save.name, "BASLUS-01360FF4");
}

#[test]
fn a_psv_holding_a_ps2_save_is_refused() {
    let mut bytes = psv(JAPANESE);
    bytes[0x3C] = 2;
    // A PS2 save is a directory of files, not a run of blocks. Reading one here would hand back a
    // save of exactly the right length made of entirely the wrong bytes.
    assert!(read_single(&bytes).is_none());
}

#[test]
fn something_that_is_not_a_single_save_is_refused() {
    assert!(read_single(b"").is_none());
    assert!(read_single(&[0u8; 8192]).is_none());
    // A whole card is not one save, however much it is made of them.
    assert!(read_single(&vec![0u8; 128 * 1024]).is_none());
    // A truncated `.psv`: the header promises bytes the file does not have.
    let mut short = psv(JAPANESE);
    short.truncate(short.len() - 1);
    assert!(read_single(&short).is_none());
}

#[test]
fn an_mcs_keeps_the_directory_entry_a_psv_does_not_have() {
    // `.mcs` is the save's own entry followed by its blocks, so it round-trips through a card.
    let save = read_single(&psv(TWO_BLOCK)).expect("reads");
    let built = CardBuilder::new().add(save.clone()).build().expect("builds");
    let on_card = MemoryCard::parse(&built).expect("parses");
    let entry = on_card.saves()[0].dirent.clone();

    let mut mcs = entry;
    mcs.extend_from_slice(&save.data);
    let back = read_single(&mcs).expect("reads the mcs");
    assert_eq!(back.name, save.name);
    assert_eq!(back.data, save.data);
    assert_eq!(back.dirent.len(), 128, "and unlike a .psv it kept the entry");
}

#[test]
fn a_save_off_a_psv_files_onto_a_card_and_reads_back() {
    let saves: Vec<_> = [JAPANESE, TWO_BLOCK].iter().map(|f| read_single(&psv(f)).expect("reads")).collect();
    let mut builder = CardBuilder::new();
    for save in &saves {
        builder.add(save.clone());
    }
    let card = MemoryCard::parse(&builder.build().expect("builds")).expect("parses");
    assert_eq!(card.saves().len(), 2);
    for (before, after) in saves.iter().zip(card.saves()) {
        assert_eq!(after.name, before.name);
        assert_eq!(after.data, before.data);
    }
}
