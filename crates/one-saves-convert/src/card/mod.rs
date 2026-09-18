//! Memory card formats, and what they share.
//!
//! Every format here follows the same shape: a card is a bundle whose `card` map holds the
//! card's own properties and whose parts are one nested bundle per save, each carrying that
//! save's directory entry. What differs between them is the layout, the `dirent` length and what
//! a writer has to regenerate.
//!
//! That representation is enough to write the saves back onto a card of the same kind. It is not
//! enough to reproduce the original card byte for byte, and it is not meant to be: a producer
//! that wants that keeps a [`card-image`](one_saves::PartKind::CardImage) part alongside.
//!
//! A card with **nothing on it** takes the other shape. A formatted card is still a real card and
//! worth carrying, so it becomes a single `card-image` part — the only part it can have, since
//! there is no save to nest — and writing it back is a byte copy rather than a rebuild.
//!
//! # Where the filesystems live
//!
//! None of them is here. Each is its own crate — [`ps1_memcard`], [`n64_cpak`], [`gc_memcard`],
//! [`dreamcast_vmu`], [`ps2_memcard`], [`neogeo_memcard`] — because a memory card is a reusable
//! format and nothing
//! about reading one needs this container. What is in this module is the adapter per format: the
//! mapping between a card's saves and a bundle's nested parts, and nothing else.
//!
//! Each adapter is behind the feature of the same name, and [`read`] and [`write`](write()) are
//! the one place that knows which are on.

/// Applies "this build has at least one card format" to everything it wraps.
///
/// `cfg` has no way to say "any feature in this group", so the list is written here once rather
/// than on each of the items below.
macro_rules! with_a_card_format {
    ($($item:item)*) => {
        $(
            #[cfg(any(
                feature = "gc",
                feature = "n64",
                feature = "ps1",
                feature = "neogeo",
                feature = "ps2",
                feature = "vmu"
            ))]
            $item
        )*
    };
}

#[cfg(feature = "gc")]
pub mod gc;
#[cfg(feature = "n64")]
pub mod n64;
#[cfg(feature = "neogeo")]
pub mod neogeo;
#[cfg(feature = "ps1")]
pub mod ps1;
#[cfg(feature = "ps2")]
pub mod ps2;
#[cfg(feature = "vmu")]
pub mod vmu;

use one_saves::{Bundle, Part, PartKind, Slug};

use crate::Format;
use crate::error::{Error, Result};

/// Reads a card into a bundle: one nested bundle per save.
///
/// `format` is what [`detect`](crate::detect()) worked out. A format this build was not compiled
/// with is [`Unsupported`](Error::Unsupported) rather than misread.
// A build with no card format at all reads neither argument, since every arm that would have is
// gone. That is the point of the configuration rather than an oversight in it.
#[allow(unused_variables)]
pub fn read(format: Format, bytes: &[u8], options: &CardOptions) -> Result<Bundle> {
    match format {
        #[cfg(feature = "ps1")]
        Format::Ps1Card => ps1::read(bytes, options),
        #[cfg(feature = "n64")]
        Format::N64Pak => n64::read(bytes, options),
        #[cfg(feature = "gc")]
        Format::GcCard => gc::read(bytes, options),
        #[cfg(feature = "vmu")]
        Format::Vmu => vmu::read(bytes, options),
        #[cfg(feature = "ps2")]
        Format::Ps2Card => ps2::read(bytes, options),
        #[cfg(feature = "neogeo")]
        Format::NeoGeoCard => neogeo::read(bytes, options),
        Format::Bundle | Format::Raw => {
            Err(Error::NotConvertible(format!("a {} is not a card", format.label())))
        }
        #[allow(unreachable_patterns)]
        other => Err(other.unsupported()),
    }
}

/// Writes a bundle back out as a card of the given format.
#[allow(unused_variables)]
pub fn write(format: Format, bundle: &Bundle) -> Result<Vec<u8>> {
    match format {
        #[cfg(feature = "ps1")]
        Format::Ps1Card => ps1::write(bundle),
        #[cfg(feature = "n64")]
        Format::N64Pak => n64::write(bundle),
        #[cfg(feature = "gc")]
        Format::GcCard => gc::write(bundle),
        #[cfg(feature = "vmu")]
        Format::Vmu => vmu::write(bundle),
        #[cfg(feature = "ps2")]
        Format::Ps2Card => ps2::write(bundle),
        #[cfg(feature = "neogeo")]
        Format::NeoGeoCard => neogeo::write(bundle),
        Format::Bundle | Format::Raw => {
            Err(Error::NotConvertible(format!("a {} is not a card", format.label())))
        }
        #[allow(unreachable_patterns)]
        other => Err(other.unsupported()),
    }
}

// What the adapters share. A build with no card format has no adapter to use any of it.
with_a_card_format! {
    /// The header a card bundle carries: the system, the card's own properties and who produced it.
    ///
    /// Every format builds the same shape out of its own three values, so it is written once here.
    pub(crate) fn card_header(
        format: Format,
        capacity: usize,
        system_area: Option<Vec<u8>>,
        options: &CardOptions,
    ) -> one_saves::Header {
        one_saves::Header {
            // The card map is what makes it a card, and the shape says so outright since 0.2.
            shape: one_saves::Shape::Card,
            system: format.system().map(slug),
            card: Some(one_saves::Card {
                format: slug(format.card_format().expect("a card format names a slug")),
                capacity: capacity as u64,
                system_area,
                unknown: one_saves::UnknownKeys::new(),
            }),
            source: options.source.clone(),
            ..one_saves::Header::default()
        }
    }

}

/// Parses a slug the specification defines, which is always well-formed.
pub(crate) fn slug(text: &str) -> Slug {
    Slug::parse(text).expect("a spec slug is well-formed")
}

/// The `bundle` part one save becomes: its bytes nested, and its address on the outside.
///
/// Nothing is inherited across the nesting boundary, so the inner bundle repeats the system and
/// game. That is exactly what lets a save be sliced out as a byte copy and still say what it
/// belongs to; the outer copy is the index, so a consumer can list a card's contents from the head
/// region without stepping into payloads.
///
/// PS2 is not in the list below and does not use this: a PS2 save is a *directory*, so its nested
/// bundle has a part per file rather than the one part every other format's save is.
#[cfg(any(feature = "gc", feature = "n64", feature = "neogeo", feature = "ps1", feature = "vmu"))]
pub(crate) fn save_part(
    format: Format,
    id: usize,
    payload: Vec<u8>,
    game: Option<one_saves::Game>,
    options: &CardOptions,
) -> Result<Part> {
    let system = format.system().map(slug);
    let inner = Bundle {
        header: one_saves::Header {
            shape: one_saves::Shape::Save,
            system: system.clone(),
            game: game.clone(),
            ..one_saves::Header::default()
        },
        parts: vec![Part::new(0, payload)],
    };

    let mut part = Part::new(u64::try_from(id).expect("part count fits"), inner.to_vec()?);
    part.kind = PartKind::Bundle;
    part.role = Some(options.role_for(format));
    part.game = game;
    part.system = system;
    Ok(part)
}

/// One save pulled out of a card bundle, ready to be written back onto a card.
pub struct NestedSave {
    /// The save's bytes: the concatenated payloads of its nested bundle's parts.
    pub bytes: Vec<u8>,
    /// The name the card's directory held for it.
    pub path: Option<String>,
    /// The directory slot it occupied, which a writer is free to change.
    pub slot: Option<u64>,
    /// The directory entry itself, verbatim.
    pub dirent: Option<Vec<u8>>,
    /// The inner bundle, for the formats that need to look at its parts individually.
    pub inner: Bundle,
}

/// Pulls the saves out of a card bundle, in part order.
///
/// A card's saves are its `bundle` parts. Anything else — a `card-image` kept alongside, an
/// `aux` blob — is not a save and is skipped here rather than written back onto the card.
pub fn nested_saves(bundle: &Bundle) -> Result<Vec<NestedSave>> {
    let mut saves = Vec::new();
    for part in &bundle.parts {
        if part.kind != PartKind::Bundle {
            continue;
        }
        let payload = part.bytes()?;
        let inner = Bundle::from_slice(&payload)?;
        // A save made of several files is one run of bytes on the card; the files are told apart
        // by `path` inside the nested bundle, and their order is the part order.
        let mut bytes = Vec::new();
        for inner_part in &inner.parts {
            bytes.extend_from_slice(&inner_part.bytes()?);
        }
        saves.push(NestedSave {
            bytes,
            path: part.path.clone(),
            slot: part.slot,
            dirent: part.dirent.clone(),
            inner,
        });
    }
    if saves.is_empty() {
        return Err(Error::NotConvertible("this bundle holds no `bundle` parts, so it is not a card".into()));
    }
    Ok(saves)
}

/// The `card-image` part a card with nothing on it becomes.
///
/// A formatted card with no saves is still a real card and worth carrying, and a whole-card image
/// is the only part it can have, since there is no save to nest.
#[must_use]
pub fn card_image_part(id: u64, image: &[u8], role: &Slug) -> Part {
    let mut part = Part::new(id, image);
    part.kind = PartKind::CardImage;
    part.role = Some(role.clone());
    // `content_type` stays absent: a raw card dump has no registered media type to name.
    part
}

/// The whole-card image to write back, for a bundle that carries one and no saves.
///
/// Returns `Ok(None)` when the bundle has nested saves, which is the ordinary case and means the
/// card is rebuilt from them. A producer may carry an image *beside* the splits, and then the
/// splits are what a writer works from — the image is the archive, not the source.
pub fn image_only(bundle: &Bundle) -> Result<Option<Vec<u8>>> {
    if bundle.parts.iter().any(|part| part.kind == PartKind::Bundle) {
        return Ok(None);
    }
    match bundle.parts.iter().find(|part| part.kind == PartKind::CardImage) {
        Some(image) => Ok(Some(image.bytes()?.into_owned())),
        None => Ok(None),
    }
}

/// How to read a card.
#[derive(Debug, Clone, Default)]
pub struct CardOptions {
    /// Which socket the card came out of, which becomes each part's `role`.
    ///
    /// A dump does not say which slot the card was in, so this is the caller's to state. Left
    /// unset, each format falls back to the socket it is normally read from — see
    /// [`Format::default_role`].
    pub role: Option<Slug>,
    /// Who produced the bytes.
    pub source: Option<one_saves::Source>,
}

impl CardOptions {
    /// The role to put on this format's parts: what the caller said, or the format's own socket.
    #[must_use]
    pub fn role_for(&self, format: Format) -> Slug {
        self.role.clone().unwrap_or_else(|| slug(format.default_role()))
    }
}
