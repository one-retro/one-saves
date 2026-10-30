//! Screenshots on their own, as a [`screenshot`](one_saves::Shape::Screenshot) bundle, and back.
//!
//! A screenshot bundle is one image of what a game showed, tied to the game and to what took it.
//! The image is kept as the emulator wrote it: a PNG stays a PNG and a JPEG a JPEG, since encoding
//! a JPEG again as PNG cannot recover what the first encode dropped and makes the file several
//! times larger. Those are the only two types a screenshot takes, because any image library reads
//! both, so anything else is refused here rather than carried under a type a consumer cannot show.
//!
//! A state's own picture is a `screenshot` part inside the state; see [`crate::state`].

use one_saves::{Bundle, Game, Header, Part, PartKind, Shape, Slug, Source};

use crate::error::{Error, Result};

/// The media type of a PNG or a JPEG, from its first bytes, or `None` for anything else.
#[must_use]
pub fn image_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else {
        None
    }
}

/// The extension a screenshot of a media type takes, without the dot.
#[must_use]
pub fn extension(content_type: &str) -> Option<&'static str> {
    match content_type {
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        _ => None,
    }
}

/// What a screenshot's header says beyond the image.
#[derive(Debug, Clone, Default)]
pub struct ScreenshotOptions {
    /// What took it: the app, and the core where one ran.
    pub source: Option<Source>,
    /// The system the game runs on.
    pub system: Option<Slug>,
    /// The game on the screen.
    pub game: Option<Game>,
    /// A note.
    pub description: Option<String>,
    /// When the bundle was assembled.
    pub bundled_at: Option<i64>,
    /// When the screenshot was taken: ordinarily the image file's modification time.
    pub taken_at: Option<i64>,
}

/// Wraps a PNG or a JPEG as a screenshot bundle.
///
/// # Errors
///
/// [`Error::NotThisFormat`] for an image that is neither, which a producer would have to encode as
/// PNG first.
pub fn read(image: &[u8], options: &ScreenshotOptions) -> Result<Bundle> {
    let content_type = image_type(image).ok_or_else(|| Error::NotThisFormat {
        format: "screenshot",
        why: "it is neither a PNG nor a JPEG, the two types a screenshot takes".into(),
    })?;
    let mut header = Header::new(Shape::Screenshot)
        .system(options.system.clone())
        .game(options.game.clone())
        .source(options.source.clone())
        .bundled_at(options.bundled_at)
        .taken_at(options.taken_at);
    header.description.clone_from(&options.description);
    let part = Part::new(0, image.to_vec()).kind(PartKind::Screenshot).content_type(content_type);
    let bundle = Bundle::new(header, vec![part]);
    bundle.validate()?;
    Ok(bundle)
}

/// The image a screenshot bundle holds, and its media type.
///
/// A state's picture comes out the same way, so this takes either shape.
///
/// # Errors
///
/// [`Error::NotConvertible`] for a bundle holding no screenshot.
pub fn write(bundle: &Bundle) -> Result<(Vec<u8>, String)> {
    let part = bundle
        .parts
        .iter()
        .find(|part| part.kind == PartKind::Screenshot)
        .ok_or_else(|| Error::NotConvertible("this bundle holds no screenshot".into()))?;
    let content_type = part.content_type.clone().unwrap_or_else(|| "image/png".to_owned());
    Ok((part.bytes()?.into_owned(), content_type))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_jpeg_stays_a_jpeg() {
        let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, b'J', b'F', b'I', b'F'];
        let bundle = read(&jpeg, &ScreenshotOptions::default()).unwrap();
        assert_eq!(bundle.header.shape, Shape::Screenshot);
        assert_eq!(bundle.parts[0].content_type.as_deref(), Some("image/jpeg"));
        assert_eq!(write(&bundle).unwrap(), (jpeg.to_vec(), "image/jpeg".to_owned()));
    }

    #[test]
    fn anything_else_is_refused_rather_than_mislabelled() {
        assert!(matches!(
            read(b"BM6\0\0\0", &ScreenshotOptions::default()),
            Err(Error::NotThisFormat { .. })
        ));
    }
}
