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

pub mod gc;
pub mod n64;
pub mod ps1;
pub mod ps2;
pub mod vmu;

use one_saves::{Bundle, Part, PartKind, Slug};

use crate::error::{Error, Result};

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
#[derive(Debug, Clone)]
pub struct CardOptions {
    /// Which socket the card came out of, which becomes each part's `role`.
    ///
    /// A dump does not say which slot the card was in, so this is the caller's to state; it
    /// defaults to the first.
    pub role: Slug,
    /// Who produced the bytes.
    pub source: Option<one_saves::Source>,
}

impl Default for CardOptions {
    fn default() -> Self {
        CardOptions { role: Slug::parse("memcard-1").expect("valid slug"), source: None }
    }
}
