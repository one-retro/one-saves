//! A card whose saves are scattered, which is what a console leaves behind.
//!
//! Every card in `data/saves` has its saves packed from the front, because that is how a card
//! comes off a console that has only ever been written to in order. Delete a save from the middle
//! and write a longer one and the blocks no longer run consecutively — the directory entry for a
//! block points at wherever the next one landed. Nothing here had ever read one of those, and this
//! crate's own writer cannot produce one, so the fixture is built by hand.

const FRAME: usize = 128;
const BLOCK: usize = 8192;

fn checksum(frame: &[u8]) -> u8 {
    frame[..FRAME - 1].iter().fold(0u8, |acc, byte| acc ^ byte)
}

/// An empty formatted card: the magic frame, fifteen free entries, the broken-sector list and the
/// write-test frame a console leaves behind.
fn blank() -> Vec<u8> {
    let mut card = vec![0u8; 128 * 1024];
    card[..2].copy_from_slice(b"MC");
    card[FRAME - 1] = checksum(&card[..FRAME]);
    for slot in 1..=15usize {
        let frame = &mut card[slot * FRAME..(slot + 1) * FRAME];
        frame[..4].copy_from_slice(&0xA0u32.to_le_bytes());
        frame[8..10].copy_from_slice(&0xFFFFu16.to_le_bytes());
        let sum = checksum(frame);
        frame[FRAME - 1] = sum;
    }
    for index in 16..36usize {
        let frame = &mut card[index * FRAME..(index + 1) * FRAME];
        frame[..4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        frame[8..10].copy_from_slice(&0xFFFFu16.to_le_bytes());
        let sum = checksum(frame);
        frame[FRAME - 1] = sum;
    }
    let frame = &mut card[63 * FRAME..64 * FRAME];
    frame[..2].copy_from_slice(b"MC");
    let sum = checksum(frame);
    frame[FRAME - 1] = sum;
    card
}

/// Files each save at the blocks it is given, chaining the entries to match.
///
/// `CardBuilder` cannot do this — it packs from the front, on purpose — so laying a save across
/// blocks that do not run on from each other has to be done by hand.
fn file_at(card: &mut [u8], save: &ps1_memcard::Save, blocks: &[usize]) {
    for (index, &block) in blocks.iter().enumerate() {
        let from = index * BLOCK;
        let to = ((index + 1) * BLOCK).min(save.data.len());
        card[block * BLOCK..block * BLOCK + (to - from)].copy_from_slice(&save.data[from..to]);

        // The link is a zero-based index over the data blocks, so block 5 is written as 4.
        let next = blocks.get(index + 1).map_or(0xFFFFu16, |&b| u16::try_from(b - 1).expect("a block"));
        let state: u32 = match (index, blocks.len() - 1) {
            (0, _) => 0x51,
            (i, last) if i == last => 0x53,
            _ => 0x52,
        };
        let frame = &mut card[block * FRAME..(block + 1) * FRAME];
        if index == 0 && !save.dirent.is_empty() {
            frame.copy_from_slice(&save.dirent);
        } else {
            frame.fill(0);
        }
        frame[..4].copy_from_slice(&state.to_le_bytes());
        // The size belongs to the first entry and to no other, which is what a console writes.
        let size = if index == 0 { u32::try_from(save.data.len()).expect("a save") } else { 0 };
        frame[4..8].copy_from_slice(&size.to_le_bytes());
        frame[8..10].copy_from_slice(&next.to_le_bytes());
        let sum = checksum(frame);
        frame[FRAME - 1] = sum;
    }
}

/// A card holding three saves: one at blocks 1 and 5, one at 2, one at 3 and 4.
fn scattered() -> Vec<u8> {
    let mut card = blank();
    // Every byte of a block says which block it is, so a chain followed wrongly shows up as the
    // wrong bytes rather than as the wrong length.
    let save = |name: &str, blocks: &[usize]| ps1_memcard::Save {
        slot: 0,
        name: name.to_owned(),
        dirent: Vec::new(),
        data: blocks
            .iter()
            .flat_map(|&b| std::iter::repeat_n(u8::try_from(b).expect("a block index"), BLOCK))
            .collect(),
    };
    for (name, blocks) in
        [("BASCUS-00001A", &[1, 5][..]), ("BASCUS-00002B", &[2][..]), ("BASCUS-00003C", &[3, 4][..])]
    {
        let mut save = save(name, blocks);
        save.dirent = vec![0u8; FRAME];
        save.dirent[10..10 + name.len()].copy_from_slice(name.as_bytes());
        file_at(&mut card, &save, blocks);
    }
    card
}

#[test]
fn a_save_whose_blocks_are_not_consecutive_is_read_in_chain_order() {
    let parsed = ps1_memcard::MemoryCard::parse(&scattered()).expect("parses");
    let saves = parsed.saves();
    assert_eq!(saves.len(), 3);

    let blocks =
        |save: &ps1_memcard::Save| -> Vec<u8> { save.data.chunks(BLOCK).map(|block| block[0]).collect() };
    // The chain jumps over B to reach A's second block, which is the whole point.
    assert_eq!(saves[0].name, "BASCUS-00001A");
    assert_eq!(blocks(&saves[0]), vec![1, 5], "followed the links rather than reading on");
    assert_eq!(saves[1].name, "BASCUS-00002B");
    assert_eq!(blocks(&saves[1]), vec![2]);
    assert_eq!(saves[2].name, "BASCUS-00003C");
    assert_eq!(blocks(&saves[2]), vec![3, 4]);
}

#[test]
fn writing_a_scattered_card_back_packs_it_without_losing_a_byte() {
    let original = scattered();
    let parsed = ps1_memcard::MemoryCard::parse(&original).expect("parses");

    let mut builder = ps1_memcard::CardBuilder::new();
    for save in parsed.saves() {
        builder.add(save.clone());
    }
    let rebuilt = builder.build().expect("builds");
    let again = ps1_memcard::MemoryCard::parse(&rebuilt).expect("reads what it wrote");

    // Same saves, same bytes. Where they sit is not the same and is not meant to be: the format
    // stores no block positions, because the number is wrong the moment a save lands on another
    // card, so a writer packs from the front and a scattered card comes back tidy.
    assert_eq!(again.saves().len(), parsed.saves().len());
    for (before, after) in parsed.saves().iter().zip(again.saves()) {
        assert_eq!(after.name, before.name);
        assert_eq!(after.data, before.data, "{}", before.name);
    }
    assert_eq!(again.saves()[0].slot, 1, "and the first save now starts at the first block");
}

/// Where the vendored saves live, and how many blocks each takes.
fn real_saves() -> Vec<ps1_memcard::Save> {
    let at = |rest: &str| {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/saves/PS1").join(rest)
    };
    let mut saves = Vec::new();
    for rest in [
        "Gran Turismo/DuckStation/shared_card_1.mcd",
        "Castlevania Symphony of the Night/DuckStation/shared_card_2.mcd",
    ] {
        let bytes = std::fs::read(at(rest)).expect("a vendored card");
        saves.extend(ps1_memcard::MemoryCard::parse(&bytes).expect("parses").into_saves());
    }
    for rest in [
        "Capcom vs SNK Millennium Fight 2000 Pro/PS3/BISLPM-87053.PSV",
        "Final Fantasy Chronicles/PS3/BASLUS-01360464634.PSV",
    ] {
        let bytes = std::fs::read(at(rest)).expect("a vendored save");
        saves.push(ps1_memcard::read_single(&bytes).expect("a save"));
    }
    // A `.psv` brings no directory entry, so everything goes through the builder once to be given
    // one. Only the blocks move after that.
    let mut builder = ps1_memcard::CardBuilder::new();
    for save in &saves {
        builder.add(save.clone());
    }
    let packed = builder.build().expect("ten blocks fit");
    ps1_memcard::MemoryCard::parse(&packed).expect("parses").into_saves()
}

/// Five real saves over ten blocks, none of them where a packer would have put it.
///
/// The saves are what a console and a PS3 wrote; only the layout is arranged here. Gran Turismo's
/// five blocks land at 1, 3, 5, 6 and 8, so its chain both skips a block and runs on from one,
/// which are the two cases a reader can get right for the wrong reason.
///
/// The card this builds is vendored, because a PlayStation BIOS has listed it: the saves come up
/// with their own icons and Final Fantasy Chronicles and Symphony of the Night both load. That is
/// the check this crate cannot make of itself, so the bytes that passed it are kept and this test
/// is what says we still produce them.
#[test]
fn real_saves_survive_being_scattered_across_a_card() {
    let saves = real_saves();
    let layout: Vec<Vec<usize>> = {
        let mut free = vec![15, 14, 12, 11, 9, 8, 6, 5, 3, 1];
        saves
            .iter()
            .map(|save| (0..save.data.len() / BLOCK).map(|_| free.pop().expect("room")).collect())
            .collect()
    };
    assert_eq!(layout[0], vec![1, 3, 5, 6, 8], "the biggest save is the scattered one");

    let mut card = blank();
    for (save, blocks) in saves.iter().zip(&layout) {
        file_at(&mut card, save, blocks);
    }

    let back = ps1_memcard::MemoryCard::parse(&card).expect("reads a scattered card");
    assert_eq!(back.saves().len(), saves.len());
    for ((before, after), blocks) in saves.iter().zip(back.saves()).zip(&layout) {
        assert_eq!(after.name, before.name);
        assert_eq!(after.data, before.data, "{}", before.name);
        assert_eq!(after.slot, blocks[0], "a save is filed under its first block");
    }

    // And it is still the card a BIOS was shown, byte for byte.
    let vendored = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../data/saves/PS1/Fragmented card/one-saves/fragmented.mcd");
    assert_eq!(card, std::fs::read(vendored).expect("the vendored card"));
}

/// The vendored card reads the same whether or not this crate is the one that laid it out.
#[test]
fn the_vendored_fragmented_card_reads() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../data/saves/PS1/Fragmented card/one-saves/fragmented.mcd");
    let card =
        ps1_memcard::MemoryCard::parse(&std::fs::read(path).expect("the vendored card")).expect("parses");
    let listed: Vec<(usize, &str, usize)> =
        card.saves().iter().map(|s| (s.slot, s.name.as_str(), s.data.len() / BLOCK)).collect();
    assert_eq!(
        listed,
        vec![
            (1, "BASCUS-94194GT", 5),
            (9, "BASCUS-9415400827918", 1),
            (11, "BASLUS-00067DRAX00", 1),
            (12, "BISLPM-87053", 1),
            (14, "BASLUS-01360FF4", 2),
        ]
    );
}
