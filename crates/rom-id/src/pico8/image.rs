//! A cart in a picture, `.p8.png`: the cart in the low bits of the pixels, and its code at
//! `0x4300`, as text or compressed in either of PICO-8's two formats.
//!
//! Behind the `pico8-png` feature, since reading the picture is what takes a PNG decoder. Without
//! it, the module of the same name beside this one names such a cart and refuses it.

use std::io::Cursor;

use super::Error;

/// Where a cart keeps its code.
const CODE_AT: usize = 0x4300;

/// How long a cart is, and so how many pixels a `.p8.png` needs.
const CART_LEN: usize = 0x8000;

/// The most an image may take to decode. A PICO-8 cart is 160x205; this is room for one many times
/// that, and stops a large picture named `.p8.png`.
const MAX_DECODED: usize = 4 << 20;

/// The Lua code in `png_bytes`, a `.p8.png`, as its bytes.
pub(super) fn code(png_bytes: &[u8]) -> Result<Vec<u8>, Error> {
    let cart = cart_bytes(png_bytes)?;
    let data = &cart[CODE_AT..];
    let code = if data.starts_with(b"\0pxa") {
        uncompress_pxa(data)?
    } else if data.starts_with(b":c:\0") {
        uncompress_old(data)?
    } else {
        let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
        data[..end].to_vec()
    };
    Ok(code)
}

/// The cart a `.p8.png` hides in its pixels.
fn cart_bytes(png_bytes: &[u8]) -> Result<Vec<u8>, Error> {
    let image = |e: png::DecodingError| Error::Image(e.to_string());
    let mut decoder =
        png::Decoder::new_with_limits(Cursor::new(png_bytes), png::Limits { bytes: MAX_DECODED });
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(image)?;
    let size = reader
        .output_buffer_size()
        .filter(|&size| size <= MAX_DECODED)
        .ok_or_else(|| Error::Image("too large to be a cart".into()))?;
    let mut buf = vec![0; size];
    let info = reader.next_frame(&mut buf).map_err(image)?;
    let pixels = &buf[..info.buffer_size()];

    // As each pixel reads once made RGBA.
    let channels = info.color_type.samples();
    let mut cart = Vec::with_capacity(CART_LEN);
    for pixel in pixels.chunks_exact(channels).take(CART_LEN) {
        let (r, g, b, a) = match *pixel {
            [r, g, b, a] => (r, g, b, a),
            [r, g, b] => (r, g, b, 0xff),
            [l, a] => (l, l, l, a),
            [l] => (l, l, l, 0xff),
            _ => unreachable!("an 8-bit image has one to four samples a pixel"),
        };
        cart.push(((a & 3) << 6) | ((r & 3) << 4) | ((g & 3) << 2) | (b & 3));
    }
    if cart.len() < CART_LEN {
        return Err(Error::Image(format!(
            "{}x{} holds {} bytes, not a cart",
            info.width,
            info.height,
            cart.len()
        )));
    }
    Ok(cart)
}

/// Bits, least significant first, as the new format packs them. Past the end it reads zeros.
struct Bits<'a> {
    data: &'a [u8],
    at: usize,
    held: u64,
    count: u32,
}

impl Bits<'_> {
    fn take(&mut self, n: u32) -> usize {
        while self.count < n {
            let byte = self.data.get(self.at).copied().unwrap_or(0);
            self.at += 1;
            self.held |= u64::from(byte) << self.count;
            self.count += 8;
        }
        let value = self.held & ((1 << n) - 1);
        self.held >>= n;
        self.count -= n;
        usize::try_from(value).expect("at most 16 bits")
    }

    fn bit(&mut self) -> bool {
        self.take(1) == 1
    }
}

/// The new format (`\0pxa`): characters by a move-to-front table, and runs copied from earlier in
/// the code.
fn uncompress_pxa(data: &[u8]) -> Result<Vec<u8>, Error> {
    let size =
        (usize::from(*data.get(4).ok_or(Error::Code)?) << 8) | usize::from(*data.get(5).ok_or(Error::Code)?);
    let mut bits = Bits { data, at: 8, held: 0, count: 0 };
    let mut table: [u8; 256] = std::array::from_fn(|i| u8::try_from(i).expect("below 256"));
    let mut out: Vec<u8> = Vec::with_capacity(size);
    while out.len() < size {
        if bits.bit() {
            let mut extra = 0;
            while bits.bit() {
                extra += 1;
                if extra > 4 {
                    return Err(Error::Code);
                }
            }
            let index = bits.take(4 + extra) + (((1 << extra) - 1) << 4);
            if index >= table.len() {
                return Err(Error::Code);
            }
            let ch = table[index];
            table.copy_within(0..index, 1);
            table[0] = ch;
            out.push(ch);
            continue;
        }
        let offset_bits = if bits.bit() { if bits.bit() { 5 } else { 10 } } else { 15 };
        let offset = bits.take(offset_bits) + 1;
        if offset == 1 && offset_bits == 10 {
            // Raw bytes, up to a zero.
            loop {
                let ch = u8::try_from(bits.take(8)).expect("eight bits");
                if ch == 0 || out.len() >= size {
                    break;
                }
                out.push(ch);
            }
            continue;
        }
        let mut count = 3;
        loop {
            let part = bits.take(3);
            count += part;
            if part != 7 || count > size {
                break;
            }
        }
        if offset > out.len() {
            return Err(Error::Code);
        }
        for _ in 0..count {
            out.push(out[out.len() - offset]);
        }
    }
    out.truncate(size);
    Ok(out)
}

/// What each byte of the old format below `0x3c` stands for.
const OLD_TABLE: &[u8; 59] = b"\n 0123456789abcdefghijklmnopqrstuvwxyz!#%(){}[]<>+=/*:;.,~_";

/// The old format (`:c:\0`): common characters as one byte, and runs copied from earlier in the
/// code.
fn uncompress_old(data: &[u8]) -> Result<Vec<u8>, Error> {
    let mut at = 8;
    let mut out = Vec::new();
    while at < data.len() {
        let ch = data[at];
        at += 1;
        if ch == 0 {
            let next = *data.get(at).ok_or(Error::Code)?;
            at += 1;
            if next == 0 {
                break;
            }
            out.push(next);
        } else if ch <= 0x3b {
            out.push(OLD_TABLE[usize::from(ch) - 1]);
        } else {
            let next = *data.get(at).ok_or(Error::Code)?;
            at += 1;
            let count = usize::from(next >> 4) + 2;
            let offset = (usize::from(ch - 0x3c) << 4) + usize::from(next & 0xf);
            if offset == 0 || offset > out.len() {
                return Err(Error::Code);
            }
            for _ in 0..count {
                out.push(out[out.len() - offset]);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pico8::read;

    /// A cart in an image: `code` stored as plain text at `0x4300`, in the pixels' low bits.
    fn png_cart(code: &[u8]) -> Vec<u8> {
        let mut cart = vec![0u8; 160 * 205];
        cart[CODE_AT..CODE_AT + code.len()].copy_from_slice(code);
        let mut rgba = Vec::with_capacity(cart.len() * 4);
        for byte in cart {
            // High bits set as a picture's would be, the cart in the low two.
            rgba.extend([
                0x40 | ((byte >> 4) & 3),
                0x80 | ((byte >> 2) & 3),
                0xc0 | (byte & 3),
                0xfc | (byte >> 6),
            ]);
        }
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, 160, 205);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header().unwrap().write_image_data(&rgba).unwrap();
        out
    }

    #[test]
    fn an_image_keeps_the_cart_in_its_low_bits() {
        let cart = png_cart(b"cartdata(\"porklike\")\0");
        assert_eq!(read(&cart).unwrap().ids, ["porklike"]);
    }

    #[test]
    fn a_picture_too_small_is_not_a_cart() {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, 16, 16);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header().unwrap().write_image_data(&[0; 16 * 16 * 4]).unwrap();
        assert!(matches!(read(&out), Err(Error::Image(_))));
    }

    #[test]
    fn the_old_format_spells_common_characters_in_one_byte() {
        // "ab" from the table, then "ab" again copied from two back.
        let mut data = b":c:\0\0\0\0\0".to_vec();
        data.extend([13, 14, 0x3c, 0x02, 0, 0]);
        assert_eq!(uncompress_old(&data).unwrap(), b"abab");
    }

    #[test]
    fn a_copy_from_before_the_start_is_refused() {
        let mut data = b":c:\0\0\0\0\0".to_vec();
        data.extend([0x3c, 0x05]);
        assert_eq!(uncompress_old(&data), Err(Error::Code));
    }
}
