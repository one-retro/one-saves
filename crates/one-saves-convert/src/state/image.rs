//! What the state readers share: PNG chunks, framebuffers made into PNGs, and bounded inflation.
//!
//! Two of the three emulators keep a screenshot as a bare framebuffer, which the format carries
//! as a PNG, and mGBA keeps its whole state inside a PNG's chunks. Every one of them compresses.
//! A length a file states is the file's to make, so nothing here inflates past one, and nothing
//! takes one on trust beyond a cap that no real state comes near.

use std::io::{Read, Write};

use crate::error::{Error, Result};
use crate::state::PNG_SIGNATURE;

/// One PNG chunk: its type and what it holds.
pub(crate) struct Chunk<'a> {
    pub kind: [u8; 4],
    pub data: &'a [u8],
    /// The whole chunk as it sits in the file, length and checksum included.
    pub raw: &'a [u8],
}

/// The chunks of a PNG, in file order, refusing one that runs past the end.
pub(crate) fn chunks<'a>(png: &'a [u8], format: &'static str) -> Result<Vec<Chunk<'a>>> {
    let mut rest = png.strip_prefix(PNG_SIGNATURE).ok_or_else(|| Error::NotThisFormat {
        format,
        why: "it does not open with a PNG signature".into(),
    })?;
    let mut found = Vec::new();
    while !rest.is_empty() {
        let corrupt = || Error::Corrupt { format, why: "a PNG chunk runs past the end of the file".into() };
        let header = rest.get(..8).ok_or_else(corrupt)?;
        let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
        let end = length.checked_add(12).ok_or_else(corrupt)?;
        let raw = rest.get(..end).ok_or_else(corrupt)?;
        let kind = [header[4], header[5], header[6], header[7]];
        found.push(Chunk { kind, data: &raw[8..8 + length], raw });
        rest = &rest[end..];
        if &kind == b"IEND" {
            break;
        }
    }
    Ok(found)
}

/// One chunk, written out with its length and checksum.
///
/// PNG gives a chunk's length 31 bits, so data past 2 GiB is refused rather than written with a
/// length that wraps.
pub(crate) fn chunk(kind: [u8; 4], data: &[u8]) -> Result<Vec<u8>> {
    let length = u32::try_from(data.len())
        .ok()
        .filter(|&length| length <= 0x7FFF_FFFF)
        .ok_or_else(|| Error::NotConvertible(format!("{} bytes do not fit one PNG chunk", data.len())))?;
    let mut crc = flate2::Crc::new();
    crc.update(&kind);
    crc.update(data);
    let mut out = Vec::with_capacity(data.len() + 12);
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(&kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc.sum().to_be_bytes());
    Ok(out)
}

/// Encodes eight-bit pixels as a PNG: three channels to a pixel, or four.
pub(crate) fn encode(width: u32, height: u32, channels: usize, pixels: &[u8]) -> Result<Vec<u8>> {
    let color = if channels == 4 { png::ColorType::Rgba } else { png::ColorType::Rgb };
    let mut out = Vec::new();
    let failed = |e: png::EncodingError| Error::Corrupt { format: "screenshot", why: e.to_string() };
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(color);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(failed)?;
        writer.write_image_data(pixels).map_err(failed)?;
    }
    Ok(out)
}

/// The most pixels a picture decodes to, whatever the format it goes into would take: an 8K
/// frame, with a little to spare. As RGBA that is 128 MiB.
const MAX_PIXELS: u64 = 1 << 25;

/// A PNG's pixels as eight-bit RGBA, whatever its colour type and depth.
///
/// `largest` is the width and height the format the pixels go into takes at most. The size is
/// checked against it, and against [`MAX_PIXELS`], before anything is allocated for the pixels:
/// the size is the header's claim, and a PNG of a few hundred bytes can claim petabytes. png's own
/// limits do not reach this far, since the buffer the pixels go into is the caller's to allocate.
pub(crate) fn decode_rgba(image: &[u8], largest: (u32, u32)) -> Result<(u32, u32, Vec<u8>)> {
    let failed = |why: String| Error::Corrupt { format: "screenshot", why };
    let mut decoder = png::Decoder::new(std::io::Cursor::new(image));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|e| failed(e.to_string()))?;
    let (width, height) = (reader.info().width, reader.info().height);
    if width > largest.0 || height > largest.1 {
        let (most_wide, most_high) = largest;
        return Err(failed(format!(
            "it claims {width} by {height} pixels, past the {most_wide} by {most_high} it goes into"
        )));
    }
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(failed(format!("it claims {width} by {height} pixels, more than any screen has")));
    }
    let size = reader.output_buffer_size().ok_or_else(|| failed("too large to decode".into()))?;
    let mut buffer = vec![0; size];
    let frame = reader.next_frame(&mut buffer).map_err(|e| failed(e.to_string()))?;
    buffer.truncate(frame.buffer_size());
    let rgba = match frame.color_type {
        png::ColorType::Rgba => buffer,
        png::ColorType::Rgb => {
            buffer.as_chunks::<3>().0.iter().flat_map(|&[r, g, b]| [r, g, b, 0xFF]).collect()
        }
        png::ColorType::GrayscaleAlpha => {
            buffer.as_chunks::<2>().0.iter().flat_map(|&[gray, alpha]| [gray, gray, gray, alpha]).collect()
        }
        png::ColorType::Grayscale => buffer.iter().flat_map(|&g| [g, g, g, 0xFF]).collect(),
        png::ColorType::Indexed => return Err(failed("a palette the decoder did not expand".into())),
    };
    Ok((frame.width, frame.height, rgba))
}

/// Drops every fourth byte: RGBA, or a framebuffer's unused fourth channel, to RGB.
pub(crate) fn rgb_of(pixels: &[u8]) -> Vec<u8> {
    pixels.as_chunks::<4>().0.iter().flat_map(|&[r, g, b, _]| [r, g, b]).collect()
}

/// Inflates a zlib stream, stopping the moment it outgrows `limit`.
///
/// Returns what it inflated, which a caller that knows the length to expect compares against it.
pub(crate) fn inflate_zlib(bytes: &[u8], limit: usize, format: &'static str) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(bytes)
        .take(limit as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|e| Error::Corrupt { format, why: format!("a compressed block does not inflate: {e}") })?;
    if out.len() > limit {
        return Err(Error::Corrupt {
            format,
            why: format!("a compressed block inflates past {limit} bytes"),
        });
    }
    Ok(out)
}

/// Compresses as a zlib stream at the level every emulator here writes by default.
pub(crate) fn deflate_zlib(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
    encoder.write_all(bytes).expect("a Vec takes any write");
    encoder.finish().expect("a Vec takes any write")
}

/// Inflates a Zstandard frame, stopping the moment it outgrows `limit`.
#[cfg(feature = "zstd")]
pub(crate) fn inflate_zstd(bytes: &[u8], limit: usize, format: &'static str) -> Result<Vec<u8>> {
    let corrupt = |e: std::io::Error| Error::Corrupt {
        format,
        why: format!("a compressed block does not inflate: {e}"),
    };
    let mut out = Vec::new();
    zstd::stream::read::Decoder::new(bytes)
        .map_err(corrupt)?
        .take(limit as u64 + 1)
        .read_to_end(&mut out)
        .map_err(corrupt)?;
    if out.len() > limit {
        return Err(Error::Corrupt {
            format,
            why: format!("a compressed block inflates past {limit} bytes"),
        });
    }
    Ok(out)
}

/// What a build without `zstd` says about a block it cannot inflate.
#[cfg(not(feature = "zstd"))]
pub(crate) fn inflate_zstd(_bytes: &[u8], _limit: usize, format: &'static str) -> Result<Vec<u8>> {
    Err(Error::Unsupported { format, feature: "zstd" })
}

/// Reads a little-endian `u32` at an offset, or fails as running past the end.
pub(crate) fn le32(bytes: &[u8], at: usize, format: &'static str) -> Result<u32> {
    at.checked_add(4)
        .and_then(|end| bytes.get(at..end))
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| Error::Corrupt { format, why: format!("it ends before offset {at}") })
}

/// A PNG whose header claims a picture of `width` by `height` eight-bit RGBA pixels, and whose
/// image data is a few bytes of nothing: what a decompression bomb looks like before it goes off.
#[cfg(test)]
pub(crate) fn claiming(width: u32, height: u32) -> Vec<u8> {
    let mut header = [width.to_be_bytes(), height.to_be_bytes()].concat();
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    let chunks = [(*b"IHDR", header), (*b"IDAT", deflate_zlib(&[0; 64])), (*b"IEND", Vec::new())];
    let mut png = PNG_SIGNATURE.to_vec();
    for (kind, data) in chunks {
        png.extend_from_slice(&chunk(kind, &data).expect("a few bytes fit a chunk"));
    }
    png
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chunk_written_here_reads_back_with_a_checksum_png_accepts() {
        // IEND's checksum is the one every PNG ends with, so it pins the CRC to PNG's.
        assert_eq!(chunk(*b"IEND", &[]).unwrap(), b"\0\0\0\0IEND\xaeB`\x82");
    }

    #[test]
    fn a_framebuffer_comes_back_as_the_pixels_it_was() {
        let pixels: Vec<u8> = (0..4 * 3 * 2).map(|n| u8::try_from(n * 9).unwrap()).collect();
        let png = encode(4, 2, 3, &pixels).unwrap();
        let (width, height, rgba) = decode_rgba(&png, (4, 2)).unwrap();
        assert_eq!((width, height), (4, 2));
        assert_eq!(rgb_of(&rgba), pixels);
    }

    #[test]
    fn a_picture_is_refused_on_the_size_it_claims_before_its_pixels_are_allocated() {
        // A few dozen bytes claiming a petabyte of pixels: allocating for the claim would abort
        // the process, so this passing at all is the check that nothing was.
        let bomb = claiming(1 << 24, 1 << 24);
        assert!(bomb.len() < 100);
        for largest in [(u32::MAX, u32::MAX), (4096, 4096)] {
            match decode_rgba(&bomb, largest) {
                Err(Error::Corrupt { why, .. }) => assert!(why.contains("claims"), "{why}"),
                other => panic!("{largest:?}: {other:?}"),
            }
        }
        // Within the pixel cap but past what the format takes on one side.
        assert!(matches!(decode_rgba(&claiming(5000, 8), (4096, 4096)), Err(Error::Corrupt { .. })));
    }

    #[test]
    fn inflation_stops_at_its_limit() {
        let bomb = deflate_zlib(&vec![0; 1 << 20]);
        assert!(inflate_zlib(&bomb, 1 << 20, "test").is_ok());
        assert!(matches!(inflate_zlib(&bomb, 4096, "test"), Err(Error::Corrupt { .. })));
    }
}
