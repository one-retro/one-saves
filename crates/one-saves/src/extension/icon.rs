//! [`x.1sav.icon`](https://docs.1retro.com/specifications/extensions/x.1sav.icon/).

use std::fmt;

use dcbor::{CBOR, Map};

use super::{Extension, only_keys, want_map, want_uint};
use crate::name::ReverseDnsName;

/// The image a console shows for a save, decoded to PNG.
///
/// At the size and in the order the console stored it: nothing here scales, recolours or
/// reorders the picture. The native bytes stay in the payload.
#[derive(Clone, PartialEq, Eq)]
pub struct Icon {
    /// The frames in display order. A still icon is one frame, and there is always at least one.
    pub frames: Vec<IconFrame>,
    /// The wider still image, where a format keeps one.
    pub banner: Option<Vec<u8>>,
}

impl Icon {
    /// An icon of these frames, with no banner.
    #[must_use]
    pub fn new(frames: Vec<IconFrame>) -> Self {
        Icon { frames, banner: None }
    }

    /// Adds the banner.
    #[must_use]
    pub fn banner(mut self, png: Vec<u8>) -> Self {
        self.banner = Some(png);
        self
    }
}

/// One frame of an [`Icon`], and how long it shows before the next.
#[derive(Clone, PartialEq, Eq)]
pub struct IconFrame {
    /// The frame as a PNG, 1 to 65536 bytes.
    pub png: Vec<u8>,
    /// How long it shows, where the format says. A still icon says nothing.
    pub hold_ms: Option<u64>,
}

impl IconFrame {
    /// A frame with no stated hold.
    #[must_use]
    pub fn new(png: Vec<u8>) -> Self {
        IconFrame { png, hold_ms: None }
    }

    /// States how long the frame shows.
    #[must_use]
    pub fn hold_ms(mut self, hold_ms: u64) -> Self {
        self.hold_ms = Some(hold_ms);
        self
    }
}

impl Extension for Icon {
    const KEY: ReverseDnsName = ReverseDnsName::from_static("x.1sav.icon");
}

impl From<Icon> for CBOR {
    fn from(icon: Icon) -> Self {
        let frames: Vec<CBOR> = icon
            .frames
            .into_iter()
            .map(|frame| match frame.hold_ms {
                Some(hold) => vec![CBOR::to_byte_string(frame.png), CBOR::from(hold)].into(),
                None => vec![CBOR::to_byte_string(frame.png)].into(),
            })
            .collect();
        let mut map = Map::new();
        map.insert(0u64, frames);
        if let Some(banner) = icon.banner {
            map.insert(1u64, CBOR::to_byte_string(banner));
        }
        map.into()
    }
}

fn want_png(value: &CBOR) -> dcbor::Result<Vec<u8>> {
    let bytes = value.as_byte_string().ok_or(dcbor::Error::WrongType)?;
    if (1..=65_536).contains(&bytes.len()) { Ok(bytes.to_vec()) } else { Err(dcbor::Error::OutOfRange) }
}

impl TryFrom<CBOR> for Icon {
    type Error = dcbor::Error;

    fn try_from(value: CBOR) -> dcbor::Result<Self> {
        let map = want_map(&value)?;
        only_keys(map, &[0, 1])?;
        let frames = map.get::<u64, CBOR>(0).ok_or(dcbor::Error::MissingMapKey)?;
        let frames = frames.as_array().ok_or(dcbor::Error::WrongType)?;
        if frames.is_empty() {
            return Err(dcbor::Error::OutOfRange);
        }
        let frames = frames
            .iter()
            .map(|frame| {
                let frame = frame.as_array().ok_or(dcbor::Error::WrongType)?;
                match frame {
                    [png] => Ok(IconFrame::new(want_png(png)?)),
                    [png, hold] => Ok(IconFrame::new(want_png(png)?).hold_ms(want_uint(hold)?)),
                    _ => Err(dcbor::Error::OutOfRange),
                }
            })
            .collect::<dcbor::Result<_>>()?;
        Ok(Icon { frames, banner: map.get::<u64, CBOR>(1).map(|value| want_png(&value)).transpose()? })
    }
}

/// A PNG in a `Debug` line: its length, since its bytes would run to thousands of numbers.
struct Png<'a>(&'a [u8]);

impl fmt::Debug for Png<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<{} bytes of PNG>", self.0.len())
    }
}

impl fmt::Debug for Icon {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Icon")
            .field("frames", &self.frames)
            .field("banner", &self.banner.as_deref().map(Png))
            .finish()
    }
}

impl fmt::Debug for IconFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IconFrame").field("png", &Png(&self.png)).field("hold_ms", &self.hold_ms).finish()
    }
}

/// The pixel size a PNG states in its header, when it is a PNG.
fn dimensions(png: &[u8]) -> Option<(u32, u32)> {
    if png.get(..8)? != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let width = u32::from_be_bytes(png.get(16..20)?.try_into().ok()?);
    let height = u32::from_be_bytes(png.get(20..24)?.try_into().ok()?);
    Some((width, height))
}

impl fmt::Display for Icon {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let count = self.frames.len();
        write!(f, "{count} frame{}", if count == 1 { "" } else { "s" })?;
        if let Some((width, height)) = self.frames.first().and_then(|frame| dimensions(&frame.png)) {
            write!(f, ", {width}x{height}")?;
        }
        if let Some(banner) = &self.banner {
            match dimensions(banner) {
                Some((width, height)) => write!(f, ", banner {width}x{height}")?,
                None => f.write_str(", banner")?,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first 24 bytes of a PNG, which are all a listing reads.
    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes
    }

    #[test]
    fn round_trips_and_shows_its_size() {
        let icon = Icon::new(vec![IconFrame::new(png(16, 16)).hold_ms(166), IconFrame::new(png(16, 16))])
            .banner(png(96, 32));
        assert_eq!(Icon::try_from(CBOR::from(icon.clone())).unwrap(), icon);
        assert_eq!(icon.to_string(), "2 frames, 16x16, banner 96x32");
        // A picture's bytes are its length in a `Debug` line, not thousands of numbers.
        assert_eq!(
            format!("{:?}", icon.frames[0]),
            "IconFrame { png: <24 bytes of PNG>, hold_ms: Some(166) }"
        );
    }

    #[test]
    fn refuses_an_icon_with_no_frame() {
        assert!(Icon::try_from(CBOR::from(Icon::new(Vec::new()))).is_err());
    }
}
