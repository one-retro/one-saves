//! What a bundle *is*, as against what it holds.
//!
//! Before 0.2 the format's tiers were not written down: they fell out of the header and the part
//! kinds read together, and the reading was easy to get subtly wrong in both directions. A bundle
//! now **names** its shape in required header key 0, and a decoder reads it rather than inferring
//! it. What the parts then mean is that shape's own document.

use std::fmt;

use crate::model::Bundle;
use crate::name::Slug;

/// What a bundle is: one of the four shapes this version defines, or one it does not know.
///
/// The vocabulary is spec-owned and closed — a producer may not mint one, because a shape needs a
/// document and rules of its own. A later minor version assigns a new one, which is why
/// [`Unknown`](Shape::Unknown) exists and why it round-trips rather than failing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub enum Shape {
    /// One game's state; the parts are the regions or files that state is made of.
    ///
    /// The default, and the floor of the format: a bare dump with an empty header is this.
    #[default]
    Save,
    /// One memory card; the parts are one `bundle` per save, plus the card's own image.
    ///
    /// The header's [`card`](crate::Card) map is what makes it one, and is required here.
    Card,
    /// One console's storage, read whole; the parts are one `bundle` per component, each roled.
    ///
    /// `system` is required, which is what tells a device from a [`Collection`](Shape::Collection);
    /// `game` is absent, since the components hold saves for many.
    Device,
    /// Several bundles in one file; the parts are one `bundle` per entry, of any shape but this.
    ///
    /// A collection has nothing of its own to say, so `system`, `game` and `card` are all absent.
    Collection,
    /// A shape a later minor version assigned, which this one does not define.
    ///
    /// A decoder **must** parse and round-trip such a bundle unchanged, exactly as it does an
    /// integer key it does not know. It **must not** guess what the shape means, and **must**
    /// decline any operation that depends on knowing — walking the parts, restoring a save,
    /// rebuilding a card. Refusing to act on a bundle is not the same as rejecting it, and only
    /// the second loses the bytes.
    Unknown(Slug),
}

impl Shape {
    /// The slug this shape is written as.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Shape::Save => "save",
            Shape::Card => "card",
            Shape::Device => "device",
            Shape::Collection => "collection",
            Shape::Unknown(slug) => slug.as_str(),
        }
    }

    /// The shape a slug names, taking one this version does not define as [`Unknown`](Shape::Unknown).
    ///
    /// Never fails: an unrecognised slug is a later version's shape, not a malformed bundle.
    #[must_use]
    pub fn from_slug(slug: Slug) -> Self {
        match slug.as_str() {
            "save" => Shape::Save,
            "card" => Shape::Card,
            "device" => Shape::Device,
            "collection" => Shape::Collection,
            _ => Shape::Unknown(slug),
        }
    }

    /// Whether this is a shape this version defines, and so one a consumer may act on.
    ///
    /// The guard to put in front of walking the parts: `false` means round-trip the bundle and
    /// decline the operation, never reject the bytes.
    #[must_use]
    pub fn is_known(&self) -> bool {
        !matches!(self, Shape::Unknown(_))
    }
}

impl fmt::Display for Shape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Bundle {
    /// Which shape this bundle names.
    ///
    /// ```
    /// use one_saves::{Bundle, Header, Part, Shape};
    ///
    /// let save = Bundle { header: Header::default(), parts: vec![Part::new(0, *b"SAVE")] };
    /// assert_eq!(save.shape(), &Shape::Save);
    /// ```
    ///
    /// This reads the header rather than deriving anything: since 0.2 the bundle says what it is.
    #[must_use]
    pub fn shape(&self) -> &Shape {
        &self.header.shape
    }
}
