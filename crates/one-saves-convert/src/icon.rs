//! Console icons, decoded to PNG for [`x.1sav.icon`].
//!
//! A producer decodes; a consumer should need none of these decoders. A PS1 icon is 4bpp against a
//! 16-colour CLUT, a GameCube icon is RGB5A3 or CI8 depending on two bits in the directory entry,
//! and a VMU icon is 4bpp against a palette in the VMS header — and a consumer that wanted to show
//! a save's picture would otherwise have to know all three.
//!
//! Nothing here improves the picture. Scaling it up, correcting its palette, compositing a
//! background behind transparency and reordering frames all produce something the card does not
//! hold, so the pixels come out at the size and in the order the console stored them. The native
//! bytes stay in the payload, where a writer reads them through the format's own key.
//!
//! [`x.1sav.icon`]: https://docs.1retro.com/specifications/extensions/x.1sav.icon/

use one_saves::ReverseDnsName;
use one_saves::dcbor::{CBOR, Map};

/// The `x.1sav.icon` key.
pub(crate) fn icon_key() -> ReverseDnsName {
    ReverseDnsName::parse("x.1sav.icon").expect("a spec name is well-formed")
}

/// One frame, and how long it shows before the next.
pub(crate) struct Frame {
    /// The frame as a PNG.
    pub png: Vec<u8>,
    /// How long it shows, where the format says. A still icon says nothing.
    pub hold_ms: Option<u64>,
}

/// The `x.1sav.icon` value: the frames in display order, and a banner where a format keeps one.
pub(crate) fn value(frames: Vec<Frame>, banner: Option<Vec<u8>>) -> Option<CBOR> {
    if frames.is_empty() {
        return None;
    }
    let frames: Vec<CBOR> = frames
        .into_iter()
        .map(|frame| match frame.hold_ms {
            Some(hold) => vec![CBOR::to_byte_string(frame.png), CBOR::from(hold)].into(),
            None => vec![CBOR::to_byte_string(frame.png)].into(),
        })
        .collect();

    let mut map = Map::new();
    map.insert(0u64, frames);
    if let Some(banner) = banner {
        map.insert(1u64, CBOR::to_byte_string(banner));
    }
    Some(map.into())
}

/// Straight-alpha RGBA, eight bits a channel, which is what a PNG here carries.
pub(crate) struct Rgba {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

impl Rgba {
    /// Reads a tiled texture, which is how a GameCube stores one and how nothing else here does.
    ///
    /// Pixels run row-major *within* a tile and the tiles run row-major across the image, so a
    /// reader that walks scanlines gets a shuffled picture rather than an obviously broken one.
    /// Tile geometry follows the pixel size: 4x4 for two bytes a pixel, 8x4 for one.
    pub fn from_tiles(
        width: usize,
        height: usize,
        tile: (usize, usize),
        mut pixel: impl FnMut(usize) -> [u8; 4],
    ) -> Self {
        let (tile_w, tile_h) = tile;
        let mut pixels = vec![0u8; width * height * 4];
        let mut index = 0;
        for top in (0..height).step_by(tile_h) {
            for left in (0..width).step_by(tile_w) {
                for y in top..top + tile_h {
                    for x in left..left + tile_w {
                        let at = (y * width + x) * 4;
                        pixels[at..at + 4].copy_from_slice(&pixel(index));
                        index += 1;
                    }
                }
            }
        }
        Self { width, height, pixels }
    }

    /// Encodes as a PNG, which is what the key carries.
    pub fn to_png(&self) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(
                &mut out,
                u32::try_from(self.width).ok()?,
                u32::try_from(self.height).ok()?,
            );
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().ok()?;
            writer.write_image_data(&self.pixels).ok()?;
        }
        // The schema caps a frame at 64 KiB. An icon that will not fit is dropped rather than
        // resized, since resizing it would be improving the picture.
        (!out.is_empty() && out.len() <= 65_536).then_some(out)
    }
}

/// One RGB5A3 pixel, the format a GameCube uses for an icon that carries its own colours.
///
/// The top bit picks the layout: set, the remaining fifteen are RGB555 and the pixel is opaque;
/// clear, they are a three-bit alpha over four-bit channels.
#[must_use]
#[cfg(feature = "gc")]
pub(crate) fn rgb5a3(value: u16) -> [u8; 4] {
    if value & 0x8000 != 0 {
        let (r, g, b) = ((value >> 10) & 31, (value >> 5) & 31, value & 31);
        [scale(r, 31), scale(g, 31), scale(b, 31), 255]
    } else {
        let (a, r, g, b) = ((value >> 12) & 7, (value >> 8) & 15, (value >> 4) & 15, value & 15);
        [scale(r, 15), scale(g, 15), scale(b, 15), scale(a, 7)]
    }
}

#[cfg(feature = "gc")]
/// Widens a channel to eight bits over its full range, so white stays white.
fn scale(value: u16, max: u16) -> u8 {
    u8::try_from(u32::from(value) * 255 / u32::from(max)).expect("a scaled channel is a byte")
}

/// Reads a big-endian `u16` at a pixel index, which is how every 16-bit format here is stored.
#[must_use]
#[cfg(feature = "gc")]
pub(crate) fn be16(bytes: &[u8], index: usize) -> u16 {
    let at = index * 2;
    bytes.get(at..at + 2).map_or(0, |pair| u16::from_be_bytes([pair[0], pair[1]]))
}
