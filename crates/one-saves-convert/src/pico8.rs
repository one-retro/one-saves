//! PICO-8's cart data, and the files its runtimes keep it in.
//!
//! A PICO-8 cart saves by calling `cartdata("id")` and then `dset()`: 64 numbers, PICO-8's memory
//! from `0x5e00` to `0x5eff`, each a 32-bit 16.16 value kept little endian. That memory is the
//! save, and it is what a bundle holds, [`MEMORY_LEN`] bytes as a cart reads them with `peek()`.
//! Where it is kept differs:
//!
//! - **`<id>.p8d.txt`**, which official PICO-8, fake-08 and picante8 all write, in a folder every
//!   cart shares: 8 lines of 64 lowercase hex digits, each 8-digit group one number written most
//!   significant byte first. This is the file a PICO-8 save is written as when nothing says
//!   otherwise ([`text_of_memory`]), and the layout [`crate::layout`] reads and writes for the
//!   `pico-8` system.
//! - **A P8 core's `.sav`**, one per cart on a MiSTer, holding a block per id the cart opened
//!   ([`p8`]). A file of several games' saves, which is what [`crate::entries`] reads.
//!
//! The id names the save and is not in it. It is the file's name in a shared folder and the block's
//! label in a P8 file, so it travels beside the memory rather than inside it.

/// How many bytes a cart's data is: 64 numbers of four bytes.
pub const MEMORY_LEN: usize = 256;

/// The extension a runtime gives a cart's data in a shared folder: `<id>.p8d.txt`.
pub const TEXT_EXTENSION: &str = "p8d.txt";

/// How many numbers a cart keeps.
const NUMBERS: usize = MEMORY_LEN / 4;

/// How many hex digits a number is written in.
const DIGITS: usize = 8;

/// How many numbers a line of a `.p8d.txt` holds.
const PER_LINE: usize = 8;

/// The memory a `.p8d.txt` holds, read as PICO-8 reads one: whitespace anywhere is skipped, a file
/// cut short leaves the numbers it does not reach at zero, and a number cut short keeps the digits
/// it has at the top. `None` for a file holding anything but hex digits and whitespace, or more
/// digits than 64 numbers take.
#[must_use]
pub fn memory_of_text(text: &[u8]) -> Option<[u8; MEMORY_LEN]> {
    let digits: Vec<u8> = text.iter().copied().filter(|b| !b.is_ascii_whitespace()).collect();
    if digits.len() > NUMBERS * DIGITS || !digits.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    let mut memory = [0u8; MEMORY_LEN];
    for (i, word) in digits.chunks(DIGITS).enumerate() {
        let text = std::str::from_utf8(word).expect("hex digits are ASCII");
        let value = u32::from_str_radix(text, 16).expect("at most eight hex digits");
        let value = value << (4 * (DIGITS - word.len()));
        memory[i * 4..i * 4 + 4].copy_from_slice(&value.to_le_bytes());
    }
    Some(memory)
}

/// The `.p8d.txt` PICO-8 writes for `memory`: 520 bytes, every line ended.
#[must_use]
pub fn text_of_memory(memory: &[u8; MEMORY_LEN]) -> Vec<u8> {
    let mut text = Vec::with_capacity(NUMBERS * DIGITS + NUMBERS / PER_LINE);
    for line in memory.as_chunks::<{ 4 * PER_LINE }>().0 {
        for number in line.as_chunks::<4>().0 {
            let value = u32::from_le_bytes(*number);
            text.extend_from_slice(format!("{value:08x}").as_bytes());
        }
        text.push(b'\n');
    }
    text
}

/// Whether `bytes` reads as a `.p8d.txt` rather than as the memory itself: a payload stored as a
/// writer's file, from before bundles held the memory alone. Memory is never taken for text: it is
/// exactly [`MEMORY_LEN`] bytes, which no file PICO-8 writes is.
fn is_text(bytes: &[u8]) -> bool {
    bytes.len() != MEMORY_LEN && memory_of_text(bytes).is_some()
}

/// The memory out of `file`, a `.p8d.txt` as a runtime keeps it, or the file as it is when it is
/// neither the text nor the memory.
pub(crate) fn normalize(file: &[u8]) -> Vec<u8> {
    if file.len() == MEMORY_LEN {
        return file.to_vec();
    }
    memory_of_text(file).map_or_else(|| file.to_vec(), |memory| memory.to_vec())
}

/// The memory out of a stored payload: the memory as it is, or the text a bundle from before kept.
pub(crate) fn normalize_stored(payload: &[u8]) -> Vec<u8> {
    if is_text(payload) {
        return memory_of_text(payload).expect("is_text parsed it").to_vec();
    }
    payload.to_vec()
}

/// The `.p8d.txt` for `payload`, the memory; anything else as it is.
pub(crate) fn lay_out(payload: &[u8]) -> Vec<u8> {
    match <&[u8; MEMORY_LEN]>::try_from(payload) {
        Ok(memory) => text_of_memory(memory),
        Err(_) => payload.to_vec(),
    }
}

/// The P8 core's save files.
///
/// P8 is a PICO-8 core for the MiSTer. A MiSTer core keeps one save file per program it loaded, so
/// P8 keeps a cart's data in `saves/P8/<cart>.sav` rather than in a folder every cart shares. A cart
/// can open more than one id over a session, and a program bundles the carts it `load()`s, so the
/// file holds a block per id:
///
/// | offset | bytes | what |
/// |---|---|---|
/// | 0 | 4 | `P8D1` |
/// | 4 | 1 | the id's length, at most 64 |
/// | 8 | n | the id, as the cart wrote it |
/// | 256 | 256 | the cart's data, PICO-8's memory at `0x5e00` |
///
/// Blocks follow each other from the start of the file, at most [`p8::MAX_BLOCKS`] of them, and the
/// first that does not start with `P8D1` ends the list. The core reads them when a cart starts and
/// rewrites a block, where it is, about a second after its data changes. A file is created only
/// once a cart first changes its data.
pub mod p8 {
    use super::MEMORY_LEN;

    /// How long a block is.
    pub const BLOCK_LEN: usize = 512;

    /// The most blocks the core reads from a file.
    pub const MAX_BLOCKS: usize = 16;

    /// What a block starts with.
    pub const MAGIC: &[u8; 4] = b"P8D1";

    /// Where a block keeps the id.
    const ID_AT: usize = 8;

    /// Where a block keeps the data.
    const DATA_AT: usize = 256;

    /// The longest id a block holds.
    const MAX_ID_LEN: usize = 64;

    /// One id's block.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Block {
        /// Which block of the file, from 0.
        pub index: usize,
        /// The id, as the cart wrote it: bytes, because the core keeps what the cart passed
        /// whether or not PICO-8 would have saved under it.
        pub id: Vec<u8>,
        /// The cart's data.
        pub memory: [u8; MEMORY_LEN],
    }

    /// Whether `file` is one of the core's save files: whole blocks, the first a block.
    #[must_use]
    pub fn is_save(file: &[u8]) -> bool {
        !file.is_empty() && file.len().is_multiple_of(BLOCK_LEN) && !blocks(file).is_empty()
    }

    /// The blocks in `file`, as the core reads them.
    #[must_use]
    pub fn blocks(file: &[u8]) -> Vec<Block> {
        let mut blocks = Vec::new();
        for (index, block) in file.as_chunks::<BLOCK_LEN>().0.iter().take(MAX_BLOCKS).enumerate() {
            let len = usize::from(block[4]);
            if &block[..4] != MAGIC || len == 0 || len > MAX_ID_LEN {
                break;
            }
            blocks.push(Block {
                index,
                id: block[ID_AT..ID_AT + len].to_vec(),
                memory: block[DATA_AT..].try_into().expect("a block ends with the data"),
            });
        }
        blocks
    }

    /// `file` with the block for `id` holding `memory`: the block that has it rewritten in place,
    /// or a new block after the last when there is none. In place, because the core writes a block
    /// back by its position, and a block moved under a running cart would be written over with
    /// another id's data.
    ///
    /// `None` when the id is empty or longer than a block holds, or the file already holds as many
    /// blocks as the core reads.
    #[must_use]
    pub fn with_block(file: &[u8], id: &[u8], memory: &[u8; MEMORY_LEN]) -> Option<Vec<u8>> {
        if id.is_empty() || id.len() > MAX_ID_LEN {
            return None;
        }
        let blocks = blocks(file);
        let index = match blocks.iter().find(|b| b.id == id) {
            Some(block) => block.index,
            None if blocks.len() < MAX_BLOCKS => blocks.len(),
            None => return None,
        };
        let mut out = file.to_vec();
        let end = (index + 1) * BLOCK_LEN;
        if out.len() < end {
            out.resize(end, 0);
        }
        let block = &mut out[index * BLOCK_LEN..end];
        block.fill(0);
        block[..4].copy_from_slice(MAGIC);
        block[4] = u8::try_from(id.len()).expect("checked above");
        block[ID_AT..ID_AT + id.len()].copy_from_slice(id);
        block[DATA_AT..].copy_from_slice(memory);
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::p8::{BLOCK_LEN, MAGIC, MAX_BLOCKS, blocks, is_save, with_block};
    use super::*;

    /// The memory with one number set.
    fn memory_with(slot: usize, value: u32) -> [u8; MEMORY_LEN] {
        let mut m = [0u8; MEMORY_LEN];
        m[slot * 4..slot * 4 + 4].copy_from_slice(&value.to_le_bytes());
        m
    }

    #[test]
    fn the_text_is_each_number_most_significant_byte_first() {
        // Pico Dino's high score of 52, `dset(0, 52)`, as fake-08 and picante8 both wrote it.
        let memory = memory_with(0, 0x0034_0000);
        let text = text_of_memory(&memory);
        assert_eq!(text.len(), 520);
        assert!(text.starts_with(b"00340000000000000000000000000000"));
        assert_eq!(text.split(|&b| b == b'\n').count(), 9, "eight lines, each ended");
        assert_eq!(memory_of_text(&text), Some(memory));
    }

    #[test]
    fn text_is_read_as_pico_8_reads_it() {
        // Whitespace anywhere, and a file cut short: the rest is zero.
        assert_eq!(
            memory_of_text(b"0001 0000\r\n00020000"),
            Some({
                let mut m = memory_with(0, 0x0001_0000);
                m[4..8].copy_from_slice(&0x0002_0000u32.to_le_bytes());
                m
            })
        );
        // A number cut short keeps its digits at the top.
        assert_eq!(memory_of_text(b"0034"), Some(memory_with(0, 0x0034_0000)));
        assert_eq!(memory_of_text(b""), Some([0; MEMORY_LEN]));
        // Anything but hex digits, or too many of them, is no cart data.
        assert_eq!(memory_of_text(b"0034000g"), None);
        assert_eq!(memory_of_text(&[b'0'; 513]), None);
    }

    #[test]
    fn memory_stays_memory_and_text_becomes_it() {
        let memory = memory_with(3, 0x0143_a56e);
        let text = text_of_memory(&memory);
        assert_eq!(normalize(&text), memory);
        assert_eq!(normalize(&memory), memory);
        assert_eq!(normalize_stored(&text), memory);
        assert_eq!(normalize_stored(&memory), memory);
        assert_eq!(lay_out(&memory), text);
        // A file that is neither goes as it is.
        assert_eq!(normalize(b"not cart data"), b"not cart data");
        assert_eq!(lay_out(b"short"), b"short");
    }

    /// A block as the core writes one.
    fn block(id: &[u8], memory: &[u8; MEMORY_LEN]) -> Vec<u8> {
        let mut b = vec![0u8; BLOCK_LEN];
        b[..4].copy_from_slice(MAGIC);
        b[4] = u8::try_from(id.len()).unwrap();
        b[8..8 + id.len()].copy_from_slice(id);
        b[256..].copy_from_slice(memory);
        b
    }

    #[test]
    fn blocks_read_until_one_is_not_a_block() {
        let mut file = block(b"mot_gpp", &memory_with(14, 3 << 16));
        file.extend(block(b"mot_gpp_results", &memory_with(30, 1 << 16)));
        file.extend(vec![0u8; BLOCK_LEN]);
        file.extend(block(b"after_the_end", &memory_with(0, 1)));

        let found = blocks(&file);
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].index, found[0].id.as_slice()), (0, b"mot_gpp".as_slice()));
        assert_eq!(found[0].memory, memory_with(14, 3 << 16));
        assert_eq!((found[1].index, found[1].id.as_slice()), (1, b"mot_gpp_results".as_slice()));
        assert!(is_save(&file));
    }

    #[test]
    fn a_file_that_is_no_save_has_no_blocks() {
        assert_eq!(blocks(&[]), []);
        assert_eq!(blocks(&[0xff; BLOCK_LEN]), []);
        let mut too_long = block(b"x", &[0; MEMORY_LEN]);
        too_long[4] = 65;
        assert_eq!(blocks(&too_long), []);
        assert!(!is_save(&too_long));
        // Cart data as text is no P8 file, and neither is a block cut short.
        assert!(!is_save(&text_of_memory(&[0; MEMORY_LEN])));
        assert!(!is_save(&block(b"x", &[0; MEMORY_LEN])[..300]));
    }

    /// A block is rewritten where it is, and nothing else in the file moves.
    #[test]
    fn a_block_is_rewritten_in_place() {
        let mut file = block(b"first", &memory_with(0, 1));
        file.extend(block(b"second", &memory_with(1, 2)));
        let memory = memory_with(5, 0x0034_0000);

        let out = with_block(&file, b"second", &memory).unwrap();
        assert_eq!(out.len(), file.len());
        assert_eq!(&out[..BLOCK_LEN], &file[..BLOCK_LEN]);
        let found = blocks(&out);
        assert_eq!((found[1].id.as_slice(), found[1].memory), (b"second".as_slice(), memory));
    }

    #[test]
    fn a_new_id_goes_after_the_last_block() {
        let file = block(b"first", &memory_with(0, 1));
        let memory = memory_with(2, 7);
        let out = with_block(&file, b"new", &memory).unwrap();
        assert_eq!(out.len(), 2 * BLOCK_LEN);
        let found = blocks(&out);
        assert_eq!((found[1].index, found[1].id.as_slice(), found[1].memory), (1, b"new".as_slice(), memory));
        // And a cart with no file yet gets one.
        assert_eq!(blocks(&with_block(&[], b"new", &memory).unwrap())[0].memory, memory);
    }

    #[test]
    fn a_full_file_takes_no_new_id() {
        let mut file = Vec::new();
        for i in 0..MAX_BLOCKS {
            file.extend(block(format!("id{i}").as_bytes(), &[0; MEMORY_LEN]));
        }
        let memory = [0; MEMORY_LEN];
        assert_eq!(with_block(&file, b"one_more", &memory), None);
        assert!(with_block(&file, b"id3", &memory).is_some());
        assert_eq!(with_block(&file, &[b'a'; 65], &memory), None);
        assert_eq!(with_block(&file, b"", &memory), None);
    }
}
