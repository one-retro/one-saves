//! The spare-area error-correcting code a hardware dump carries.
//!
//! A PS2 memory card page is 512 bytes of data followed by 16 bytes of spare area, and the card
//! regenerates that spare area on every write. It holds a 3-byte code per 128-byte chunk, four
//! chunks to a page, which is the same Hamming-style scheme SmartMedia uses: a column parity byte
//! plus two line-parity bytes that between them locate a single flipped bit.
//!
//! This is why a raw dump of an 8 MB card is 8650752 bytes rather than 8388608: the spare area is
//! invisible to the filesystem, so it counts towards neither the card's capacity nor a save's
//! length.

/// How many bytes of data one code covers.
pub const CHUNK: usize = 128;

/// The byte a `0..256` table index stands for.
///
/// `usize::to_le_bytes` is const where `u8::try_from` is not, and taking the low byte of an index
/// this crate only ever calls with `0..256` is the same value without a narrowing cast to allow.
const fn table_index_byte(index: usize) -> u8 {
    index.to_le_bytes()[0]
}

/// The parity of each byte value: 1 when it has an odd number of bits set.
///
/// Built at compile time. The counter is the byte itself, which is what keeps this free of the
/// narrowing cast a `0..256` index would need and a `const fn` cannot write with `try_from`.
const PARITY: [u8; 256] = {
    let mut table = [0u8; 256];
    let mut index = 0usize;
    while index < 256 {
        let mut byte = table_index_byte(index);
        byte ^= byte >> 1;
        byte ^= byte >> 2;
        byte ^= byte >> 4;
        table[index] = byte & 1;
        index += 1;
    }
    table
};

/// The column-parity contribution of each byte value.
///
/// Bit *i* of the mask is the parity of the byte under the *i*th column mask, which is what lets
/// the per-byte loop below be a table lookup rather than seven nested parities.
const COLUMN_PARITY: [u8; 256] = {
    // Three masks splitting the bits one way, a hole, then three splitting them the other.
    let masks = [0x55u8, 0x33, 0x0F, 0x00, 0xAA, 0xCC, 0xF0];
    let mut table = [0u8; 256];
    let mut index = 0usize;
    while index < 256 {
        let mut mask = 0u8;
        let mut i = 0usize;
        while i < 7 {
            let mut byte = table_index_byte(index) & masks[i];
            byte ^= byte >> 1;
            byte ^= byte >> 2;
            byte ^= byte >> 4;
            mask |= (byte & 1) << i;
            i += 1;
        }
        table[index] = mask;
        index += 1;
    }
    table
};

/// Computes the three-byte code over one 128-byte chunk.
///
/// # Panics
///
/// Panics if `data` is not exactly [`CHUNK`] bytes.
#[must_use]
pub fn calculate(data: &[u8]) -> [u8; 3] {
    assert_eq!(data.len(), CHUNK, "an ECC code covers exactly {CHUNK} bytes");

    // The seeds are not zero, so an all-zero chunk codes to the seeds rather than to nothing.
    // That is deliberate: a spare area that reads as all zeroes is an unwritten one, which is a
    // different thing from a page of zeroes that was written on purpose.
    let mut column = 0x77u8;
    let mut line_complement = 0x7Fu8;
    let mut line = 0x7Fu8;

    for (index, &byte) in data.iter().enumerate() {
        column ^= COLUMN_PARITY[byte as usize];
        if PARITY[byte as usize] == 1 {
            // The index and its complement together give the two line-parity bytes, so a single
            // flipped bit shows up as a position rather than only as a mismatch.
            let index = u8::try_from(index).expect("an index inside a 128-byte chunk fits");
            line_complement ^= !index;
            line ^= index;
        }
    }

    [column, line_complement & 0x7F, line & 0x7F]
}

/// Fills a page's 16-byte spare area from its 512 bytes of data.
///
/// # Panics
///
/// Panics if the slices are not one page and one spare area.
pub fn fill_spare(page: &[u8], spare: &mut [u8]) {
    assert_eq!(page.len(), crate::PAGE, "a page is {} bytes", crate::PAGE);
    assert_eq!(spare.len(), crate::SPARE, "a spare area is {} bytes", crate::SPARE);

    spare.fill(0);
    for (chunk, code) in page.as_chunks::<CHUNK>().0.iter().zip(spare.as_chunks_mut::<3>().0.iter_mut()) {
        code.copy_from_slice(&calculate(chunk));
    }
    // The four bytes past the four codes are not ECC and are left clear.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_all_zero_chunk_codes_to_the_seeds() {
        // The known-good value for the standard PS2 code, and the check that the three seeds are
        // the right ones: nothing is XORed in, so the result is exactly what they start at.
        assert_eq!(calculate(&[0u8; CHUNK]), [0x77, 0x7F, 0x7F]);
    }

    #[test]
    fn a_flipped_bit_changes_the_code() {
        let mut data = [0u8; CHUNK];
        let clean = calculate(&data);
        for position in [0usize, 1, 63, 127] {
            data[position] ^= 0x01;
            assert_ne!(calculate(&data), clean, "a flip at {position} should show");
            data[position] ^= 0x01;
        }
    }

    #[test]
    fn the_code_locates_which_byte_moved() {
        // The two line-parity bytes are an index and its complement, so flipping the same bit in
        // two different places gives two different codes rather than only two mismatches.
        let mut first = [0u8; CHUNK];
        let mut second = [0u8; CHUNK];
        first[3] = 0x01;
        second[9] = 0x01;
        assert_ne!(calculate(&first), calculate(&second));
    }

    #[test]
    fn a_spare_area_holds_four_codes_and_four_clear_bytes() {
        let page: Vec<u8> = (0..crate::PAGE).map(|i| u8::try_from(i % 251).expect("masked")).collect();
        let mut spare = [0xFFu8; crate::SPARE];
        fill_spare(&page, &mut spare);

        for (index, chunk) in page.as_chunks::<CHUNK>().0.iter().enumerate() {
            assert_eq!(&spare[index * 3..index * 3 + 3], &calculate(chunk));
        }
        assert_eq!(&spare[12..], &[0, 0, 0, 0], "the tail is not ECC");
    }
}
