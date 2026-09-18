//! Turning bytes into a [`Bundle`] and back.
//!
//! The deterministic encoding is dcbor's job; everything here is the layer above it, where a
//! well-formed CBOR map either is or is not a header.

use std::collections::BTreeMap;

use dcbor::Simple;
use dcbor::prelude::*;

use crate::error::{Error, ErrorKind};
use crate::hash::{HashAlgorithm, HashValue};
use crate::model::{
    Bundle, Card, Extensions, Game, GameId, Header, Part, PartKind, Payload, Source, UnknownKeys,
};
use crate::name::{Name, ReverseDnsName, Slug};
use crate::shape::Shape;

/// How strictly to read a bundle.
///
/// The two differ over exactly one thing: an integer key this version does not define.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Strictness {
    /// What a shipped decoder does: ignore an unrecognised integer key and round-trip it
    /// unchanged, because the only thing it can be is a later minor version's field.
    #[default]
    Decoder,
    /// What the version 0.1 schema does: reject an integer key it does not list.
    ///
    /// A schema describes one version exactly and cannot state a rule about the next one, so
    /// this is the stricter reading and it is the one the conformance corpus tests. Do not ship
    /// a decoder in this mode; it will refuse bundles a later minor version writes.
    Schema,
}

/// The keys of one map, split into the two namespaces the format defines.
struct Fields {
    /// Integer keys, which belong to the specification.
    ints: BTreeMap<i64, CBOR>,
    /// Text keys, which belong to producers. Only the header and a part admit them.
    texts: Vec<(String, CBOR)>,
}

impl Fields {
    /// Splits a map, rejecting a key that is neither an integer nor text.
    fn split(map: &Map, path: &str) -> Result<Self, Error> {
        let mut ints = BTreeMap::new();
        let mut texts = Vec::new();
        for (key, value) in map.iter() {
            match key.as_case() {
                CBORCase::Unsigned(n) => {
                    let n = i64::try_from(*n)
                        .map_err(|_| Error::at(path, ErrorKind::Type { expected: "an integer key" }))?;
                    ints.insert(n, value.clone());
                }
                CBORCase::Negative(n) => {
                    // CBOR stores -1 - n, so the value is -(n + 1).
                    let magnitude = i64::try_from(*n)
                        .map_err(|_| Error::at(path, ErrorKind::Type { expected: "an integer key" }))?;
                    ints.insert(-1 - magnitude, value.clone());
                }
                CBORCase::Text(text) => texts.push((text.clone(), value.clone())),
                _ => {
                    return Err(Error::at(path, ErrorKind::Type { expected: "an integer or text key" }));
                }
            }
        }
        Ok(Fields { ints, texts })
    }

    /// Takes one integer key out, leaving the rest to become `unknown`.
    fn take(&mut self, key: i64) -> Option<CBOR> {
        self.ints.remove(&key)
    }

    /// Whatever integer keys are left, which belong to a later minor version.
    fn unknown(self, path: &str, strictness: Strictness) -> Result<UnknownKeys, Error> {
        if strictness == Strictness::Schema
            && let Some((&key, _)) = self.ints.iter().next()
        {
            return Err(Error::at(path, ErrorKind::UnknownIntegerKey(key)));
        }
        Ok(self.ints)
    }

    /// The text keys as extensions, checking each is a well-formed reverse-DNS name.
    ///
    /// An extension key is a reverse-DNS name or it is nothing: without that there is no way to
    /// tell whose key it is, which is the one thing the namespace buys.
    fn extensions(&self, path: &str) -> Result<Extensions, Error> {
        let mut extensions = Extensions::new();
        for (key, value) in &self.texts {
            let name = ReverseDnsName::parse(key)
                .map_err(|e| Error::at(format!("{path}[{key:?}]"), ErrorKind::Name(e)))?;
            extensions.insert(name, value.clone());
        }
        Ok(extensions)
    }

    /// Rejects text keys, for the maps that do not admit extensions.
    fn no_extensions(&self, path: &str) -> Result<(), Error> {
        if let Some((key, _)) = self.texts.first() {
            return Err(Error::at(
                format!("{path}[{key:?}]"),
                ErrorKind::Type { expected: "only integer keys in this map" },
            ));
        }
        Ok(())
    }
}

// ---- small readers ---------------------------------------------------------------------------

fn want_map<'a>(value: &'a CBOR, path: &str) -> Result<&'a Map, Error> {
    value.as_map().ok_or_else(|| Error::at(path, ErrorKind::Type { expected: "a map" }))
}

fn want_text(value: &CBOR, path: &str) -> Result<String, Error> {
    value
        .as_text()
        .map(ToOwned::to_owned)
        .ok_or_else(|| Error::at(path, ErrorKind::Type { expected: "a text string" }))
}

fn want_bytes(value: &CBOR, path: &str) -> Result<Vec<u8>, Error> {
    value
        .as_byte_string()
        .map(<[u8]>::to_vec)
        .ok_or_else(|| Error::at(path, ErrorKind::Type { expected: "a byte string" }))
}

fn want_uint(value: &CBOR, path: &str) -> Result<u64, Error> {
    match value.as_case() {
        CBORCase::Unsigned(n) => Ok(*n),
        _ => Err(Error::at(path, ErrorKind::Type { expected: "an unsigned integer" })),
    }
}

fn want_slug(value: &CBOR, path: &str) -> Result<Slug, Error> {
    let text = want_text(value, path)?;
    Slug::parse(&text).map_err(|e| Error::at(path, ErrorKind::Name(e)))
}

fn want_name(value: &CBOR, path: &str) -> Result<Name, Error> {
    let text = want_text(value, path)?;
    Name::parse(&text).map_err(|e| Error::at(path, ErrorKind::Name(e)))
}

/// Reads an epoch field: CBOR tag 1 over a whole number of seconds.
///
/// Tag 1 also admits a float, which this format forbids. Left open, one instant would have two
/// conforming encodings and the content hash would depend on which a producer picked.
fn want_epoch(value: &CBOR, path: &str) -> Result<i64, Error> {
    let CBORCase::Tagged(tag, inner) = value.as_case() else {
        return Err(Error::at(path, ErrorKind::Type { expected: "CBOR tag 1 over whole seconds" }));
    };
    if tag.value() != 1 {
        return Err(Error::at(path, ErrorKind::Type { expected: "CBOR tag 1 over whole seconds" }));
    }
    match inner.as_case() {
        CBORCase::Unsigned(n) => {
            i64::try_from(*n).map_err(|_| Error::at(path, ErrorKind::Type { expected: "seconds in range" }))
        }
        CBORCase::Negative(n) => i64::try_from(*n)
            .map(|magnitude| -1 - magnitude)
            .map_err(|_| Error::at(path, ErrorKind::Type { expected: "seconds in range" })),
        CBORCase::Simple(Simple::Float(_)) => Err(Error::at(path, ErrorKind::FloatEpoch)),
        _ => Err(Error::at(path, ErrorKind::Type { expected: "whole seconds" })),
    }
}

/// Reads a hash value: a raw digest under a tag naming its algorithm.
fn want_hash(value: &CBOR, path: &str) -> Result<HashValue, Error> {
    let CBORCase::Tagged(tag, inner) = value.as_case() else {
        return Err(Error::at(path, ErrorKind::Type { expected: "a tagged digest" }));
    };
    let digest = want_bytes(inner, path)?;
    HashValue::from_tagged(tag.value(), digest).map_err(|e| Error::at(path, ErrorKind::Hash(e)))
}

/// Reads a part's `sha256`, where only that one tag is legal.
///
/// The tag is there so a generic CBOR tool can name the digest without knowing this format, and
/// pinning the algorithm is what gives identity and dedup one answer rather than a choice.
fn want_sha256(value: &CBOR, path: &str) -> Result<HashValue, Error> {
    let hash = want_hash(value, path)?;
    if hash.algorithm() != HashAlgorithm::Sha256 {
        return Err(Error::at(path, ErrorKind::Type { expected: "a sha256 digest under tag 18540" }));
    }
    Ok(hash)
}

// ---- decoding --------------------------------------------------------------------------------

impl Bundle {
    /// Reads a bundle from the bytes of a `.1saves` file.
    ///
    /// This is the lenient reading a shipped decoder wants: an integer key from a later minor
    /// version is kept and round-tripped rather than refused. Everything else the specification
    /// forbids is an error.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, Error> {
        Bundle::from_slice_with(bytes, Strictness::Decoder)
    }

    /// Reads a bundle at a chosen [`Strictness`].
    pub fn from_slice_with(bytes: &[u8], strictness: Strictness) -> Result<Self, Error> {
        let cbor =
            CBOR::try_from_data(bytes).map_err(|e| Error::at("", ErrorKind::Encoding(e.to_string())))?;
        let bundle = Bundle::from_cbor(&cbor, strictness)?;
        bundle.validate()?;
        Ok(bundle)
    }

    /// Reads a bundle out of an already-decoded CBOR value, without validating it.
    pub fn from_cbor(cbor: &CBOR, strictness: Strictness) -> Result<Self, Error> {
        let CBORCase::Tagged(tag, envelope) = cbor.as_case() else {
            return Err(Error::at("", ErrorKind::NotABundle { found: None }));
        };
        if tag.value() != crate::BUNDLE_TAG {
            return Err(Error::at("", ErrorKind::NotABundle { found: Some(tag.value()) }));
        }
        let items = envelope
            .as_array()
            .ok_or_else(|| Error::at("", ErrorKind::Type { expected: "a two-element array" }))?;
        let [header, parts] = items else {
            return Err(Error::at("", ErrorKind::Type { expected: "a two-element array" }));
        };

        let header = decode_header(header, strictness)?;
        let parts = decode_parts(parts, strictness)?;

        Ok(Bundle { header, parts })
    }
}

fn decode_header(value: &CBOR, strictness: Strictness) -> Result<Header, Error> {
    let map = want_map(value, "header")?;
    let mut fields = Fields::split(map, "header")?;
    let extensions = fields.extensions("header")?;

    // Key 0 is required, and a slug this version does not define is a later version's shape
    // rather than a malformed bundle: it parses, round-trips, and is declined at the point of use.
    let shape = fields
        .take(0)
        .ok_or_else(|| Error::at("header", ErrorKind::MissingKey("shape")))
        .and_then(|v| want_slug(&v, "header.shape"))
        .map(Shape::from_slug)?;
    // The schema pins the four shapes this version defines, so validating against it says "this
    // is a conforming 0.2 bundle" rather than "some decoder can read this". A decoder is looser
    // on purpose, and declines at the point of use instead.
    if strictness == Strictness::Schema
        && let Shape::Unknown(slug) = &shape
    {
        return Err(Error::at("header", ErrorKind::UnknownShape(slug.as_str().to_owned())));
    }
    let system = fields.take(1).map(|v| want_slug(&v, "header.system")).transpose()?;
    let game = fields.take(2).map(|v| decode_game(&v, "header.game", strictness)).transpose()?;
    let source = fields.take(3).map(|v| decode_source(&v, "header.source", strictness)).transpose()?;
    let card = fields.take(4).map(|v| decode_card(&v, "header.card", strictness)).transpose()?;
    let description = fields.take(5).map(|v| want_text(&v, "header.description")).transpose()?;
    let created_at = fields.take(6).map(|v| want_epoch(&v, "header.created_at")).transpose()?;

    // Each shape admits a different header, and the shape is read first, so a contradiction is a
    // decode error rather than something a consumer has to re-derive. A shape this version does
    // not define constrains nothing: its rules are in a document this decoder has not seen.
    let forbid = |present: bool, field: &'static str, shape: &'static str| {
        present.then(|| Error::at("header", ErrorKind::KeyNotInShape { shape, field })).map_or(Ok(()), Err)
    };
    match &shape {
        // A save is one game's state, and a card map is what would make it a card instead.
        Shape::Save => forbid(card.is_some(), "card", "save")?,
        // The card map is the claim proper: it carries the format and capacity a writer needs.
        Shape::Card => {
            if card.is_none() {
                return Err(Error::at("header", ErrorKind::MissingKey("card")));
            }
        }
        // `system` is what tells a device from a collection; its components hold saves for many
        // games rather than for one, so `game` says nothing at this level.
        Shape::Device => {
            if system.is_none() {
                return Err(Error::at("header", ErrorKind::MissingKey("system")));
            }
            forbid(game.is_some(), "game", "device")?;
            forbid(card.is_some(), "card", "device")?;
        }
        // A collection's entries do not agree on any of the three, so it names none of them.
        Shape::Collection => {
            forbid(system.is_some(), "system", "collection")?;
            forbid(game.is_some(), "game", "collection")?;
            forbid(card.is_some(), "card", "collection")?;
        }
        Shape::Unknown(_) => {}
    }

    Ok(Header {
        shape,
        created_at,
        system,
        game,
        source,
        card,
        description,
        extensions,
        unknown: fields.unknown("header", strictness)?,
    })
}

fn decode_game(value: &CBOR, path: &str, strictness: Strictness) -> Result<Game, Error> {
    let map = want_map(value, path)?;
    let mut fields = Fields::split(map, path)?;
    fields.no_extensions(path)?;

    let mut game_id = BTreeMap::new();
    if let Some(ids) = fields.take(0) {
        let path = format!("{path}.game_id");
        let ids = want_map(&ids, &path)?;
        // A resolver name is minted exactly as an extension key is, so it is checked the same way.
        for (key, id) in ids.iter() {
            let text = want_text(key, &path)?;
            let name = ReverseDnsName::parse(&text)
                .map_err(|e| Error::at(format!("{path}[{text:?}]"), ErrorKind::Name(e)))?;
            let id = match id.as_case() {
                CBORCase::Unsigned(n) => GameId::Uint(*n),
                CBORCase::Text(t) => GameId::Text(t.clone()),
                _ => {
                    return Err(Error::at(
                        format!("{path}[{text:?}]"),
                        ErrorKind::Type { expected: "an unsigned integer or a text string" },
                    ));
                }
            };
            game_id.insert(name, id);
        }
        if game_id.is_empty() {
            return Err(Error::at(path, ErrorKind::EmptyContainer));
        }
    }

    let mut rom_hashes = Vec::new();
    if let Some(hashes) = fields.take(1) {
        let path = format!("{path}.rom_hashes");
        let items =
            hashes.as_array().ok_or_else(|| Error::at(&path, ErrorKind::Type { expected: "an array" }))?;
        if items.is_empty() {
            return Err(Error::at(&path, ErrorKind::EmptyContainer));
        }
        for (index, item) in items.iter().enumerate() {
            rom_hashes.push(want_hash(item, &format!("{path}[{index}]"))?);
        }
    }

    let rom_filename = fields.take(2).map(|v| want_text(&v, path)).transpose()?;
    let serial = fields.take(3).map(|v| want_text(&v, path)).transpose()?;
    let title = fields.take(4).map(|v| want_text(&v, path)).transpose()?;
    let system = fields.take(5).map(|v| want_slug(&v, &format!("{path}.system"))).transpose()?;

    let game = Game {
        game_id,
        rom_hashes,
        rom_filename,
        serial,
        title,
        system,
        unknown: fields.unknown(path, strictness)?,
    };
    // An optional container has exactly one encoding of empty, and that is not being there.
    if game.is_empty() {
        return Err(Error::at(path, ErrorKind::EmptyContainer));
    }
    Ok(game)
}

fn decode_source(value: &CBOR, path: &str, strictness: Strictness) -> Result<Source, Error> {
    let map = want_map(value, path)?;
    let mut fields = Fields::split(map, path)?;
    fields.no_extensions(path)?;

    let device_kind = fields.take(0).map(|v| want_slug(&v, &format!("{path}.device_kind"))).transpose()?;
    let fingerprint = fields.take(1).map(|v| want_text(&v, path)).transpose()?;
    let app = fields.take(2).map(|v| want_name(&v, &format!("{path}.app"))).transpose()?;
    let app_version = fields.take(3).map(|v| want_text(&v, path)).transpose()?;

    Ok(Source { device_kind, fingerprint, app, app_version, unknown: fields.unknown(path, strictness)? })
}

fn decode_card(value: &CBOR, path: &str, strictness: Strictness) -> Result<Card, Error> {
    let map = want_map(value, path)?;
    let mut fields = Fields::split(map, path)?;
    fields.no_extensions(path)?;

    let format = fields
        .take(0)
        .ok_or_else(|| Error::at(path, ErrorKind::MissingKey("format")))
        .and_then(|v| want_slug(&v, &format!("{path}.format")))?;
    let capacity = fields
        .take(1)
        .ok_or_else(|| Error::at(path, ErrorKind::MissingKey("capacity")))
        .and_then(|v| want_uint(&v, &format!("{path}.capacity")))?;
    let system_area = fields.take(2).map(|v| want_bytes(&v, &format!("{path}.system_area"))).transpose()?;

    Ok(Card { format, capacity, system_area, unknown: fields.unknown(path, strictness)? })
}

fn decode_parts(value: &CBOR, strictness: Strictness) -> Result<Vec<Part>, Error> {
    let items =
        value.as_array().ok_or_else(|| Error::at("parts", ErrorKind::Type { expected: "an array" }))?;
    // A bundle holding no part carries nothing, and absence is not available for `parts`.
    if items.is_empty() {
        return Err(Error::at("parts", ErrorKind::EmptyContainer));
    }
    items
        .iter()
        .enumerate()
        .map(|(index, item)| decode_part(item, &format!("parts[{index}]"), strictness))
        .collect()
}

fn decode_part(value: &CBOR, path: &str, strictness: Strictness) -> Result<Part, Error> {
    let map = want_map(value, path)?;
    let mut fields = Fields::split(map, path)?;
    let extensions = fields.extensions(path)?;

    let id = fields
        .take(0)
        .ok_or_else(|| Error::at(path, ErrorKind::MissingKey("id")))
        .and_then(|v| want_uint(&v, &format!("{path}.id")))?;

    let kind = match fields.take(1) {
        None => PartKind::Save,
        Some(v) => {
            let name = want_name(&v, &format!("{path}.kind"))?;
            // `kind` absent means `save`, so writing it out is a second spelling of one part.
            if name.as_str() == "save" {
                return Err(Error::at(format!("{path}.kind"), ErrorKind::DefaultSpelledOut("save")));
            }
            PartKind::from_name(name)
        }
    };

    let role = match fields.take(2) {
        None => None,
        Some(v) => {
            let slug = want_slug(&v, &format!("{path}.role"))?;
            if slug.as_str() == "primary" {
                return Err(Error::at(format!("{path}.role"), ErrorKind::DefaultSpelledOut("primary")));
            }
            Some(slug)
        }
    };

    let path_field = match fields.take(3) {
        None => None,
        Some(v) => {
            let text = want_text(&v, &format!("{path}.path"))?;
            check_path(&text, &format!("{path}.path"))?;
            Some(text)
        }
    };

    let slot = fields.take(4).map(|v| want_uint(&v, &format!("{path}.slot"))).transpose()?;
    let dirent = fields.take(5).map(|v| want_bytes(&v, &format!("{path}.dirent"))).transpose()?;
    let content_type = fields.take(6).map(|v| want_text(&v, &format!("{path}.content_type"))).transpose()?;

    let encoding = match fields.take(7) {
        None => None,
        Some(v) => {
            let text = want_text(&v, &format!("{path}.encoding"))?;
            match text.as_str() {
                "zstd" => Some(text),
                // "none" is the default and is written by leaving the key out.
                "none" => {
                    return Err(Error::at(format!("{path}.encoding"), ErrorKind::DefaultSpelledOut("none")));
                }
                _ => {
                    return Err(Error::at(format!("{path}.encoding"), ErrorKind::UnknownEncoding(text)));
                }
            }
        }
    };

    let size = fields.take(8).map(|v| want_uint(&v, &format!("{path}.size"))).transpose()?;
    let sha256 = fields
        .take(9)
        .ok_or_else(|| Error::at(path, ErrorKind::MissingKey("sha256")))
        .and_then(|v| want_sha256(&v, &format!("{path}.sha256")))?;

    let source =
        fields.take(10).map(|v| decode_source(&v, &format!("{path}.source"), strictness)).transpose()?;
    let game = fields.take(11).map(|v| decode_game(&v, &format!("{path}.game"), strictness)).transpose()?;
    let system = fields.take(12).map(|v| want_slug(&v, &format!("{path}.system"))).transpose()?;
    let binding = fields.take(13).map(|v| want_slug(&v, &format!("{path}.binding"))).transpose()?;

    let payload_value = fields.take(-1).ok_or_else(|| Error::at(path, ErrorKind::MissingKey("payload")))?;
    let payload = decode_payload(&payload_value, encoding.as_deref(), size, path)?;

    Ok(Part {
        id,
        kind,
        role,
        path: path_field,
        slot,
        dirent,
        content_type,
        sha256,
        source,
        game,
        system,
        binding,
        payload,
        extensions,
        unknown: fields.unknown(path, strictness)?,
    })
}

/// Reads the payload, and with it which of the two forms this part is in.
///
/// `size` is carried only where it cannot be derived, which since 0.2 means a compressed payload
/// and nothing else: the thin form that also needed it is gone.
fn decode_payload(
    value: &CBOR,
    encoding: Option<&str>,
    size: Option<u64>,
    path: &str,
) -> Result<Payload, Error> {
    let bytes = want_bytes(value, &format!("{path}.payload"))?;
    match (encoding, size) {
        (Some(_), Some(size)) => Ok(Payload::Compressed { bytes, size }),
        (Some(_), None) => Err(Error::at(path, ErrorKind::MissingSize)),
        // An embedded uncompressed payload already states its own length.
        (None, Some(_)) => Err(Error::at(format!("{path}.size"), ErrorKind::SizeOnUncompressedPart)),
        (None, None) => Ok(Payload::Embedded(bytes)),
    }
}

/// Checks a `path`: non-empty, at most 512 bytes, no leading `/` and no `..` segment.
fn check_path(text: &str, path: &str) -> Result<(), Error> {
    if text.is_empty() {
        return Err(Error::at(path, ErrorKind::DefaultSpelledOut("absent")));
    }
    // The cap is in bytes rather than characters, since that is what a consumer sizing the head
    // region actually has to budget for.
    if text.len() > crate::MAX_PATH_LEN {
        return Err(Error::at(path, ErrorKind::PathTooLong(text.len())));
    }
    if text.starts_with('/') || text.split('/').any(|segment| segment == "..") {
        return Err(Error::at(path, ErrorKind::PathForbidden));
    }
    Ok(())
}

// ---- encoding --------------------------------------------------------------------------------

impl Bundle {
    /// Writes the bundle as the bytes of a `.1saves` file.
    ///
    /// The bundle is validated first, so this cannot produce a file that
    /// [`from_slice`](Bundle::from_slice) would refuse.
    pub fn to_vec(&self) -> Result<Vec<u8>, Error> {
        self.validate()?;
        Ok(self.to_cbor().to_cbor_data())
    }

    /// Writes the bundle as a CBOR value, without validating it.
    #[must_use]
    pub fn to_cbor(&self) -> CBOR {
        let envelope: CBOR = vec![self.header.to_cbor(), encode_parts(&self.parts)].into();
        CBORCase::Tagged(Tag::new(crate::BUNDLE_TAG, "1sav"), envelope).into()
    }
}

/// Adds the integer keys a later minor version wrote, and the producer's own text keys.
///
/// Round-tripping these unchanged is the whole of what makes the format extensible: never drop
/// one just because you do not recognise it.
fn finish(mut map: Map, unknown: &UnknownKeys, extensions: &Extensions) -> CBOR {
    for (&key, value) in unknown {
        map.insert(key, value.clone());
    }
    for (name, value) in extensions {
        map.insert(name.as_str(), value.clone());
    }
    map.into()
}

fn epoch_cbor(seconds: i64) -> CBOR {
    CBORCase::Tagged(Tag::new(1u64, "epoch"), seconds.into()).into()
}

fn hash_cbor(hash: &HashValue) -> CBOR {
    CBORCase::Tagged(Tag::new(hash.tag(), hash.algorithm().name()), CBOR::to_byte_string(hash.digest()))
        .into()
}

impl Header {
    /// Writes the header as a CBOR map.
    #[must_use]
    pub fn to_cbor(&self) -> CBOR {
        let mut map = Map::new();
        map.insert(0u64, self.shape.as_str());
        if let Some(created_at) = self.created_at {
            map.insert(6u64, epoch_cbor(created_at));
        }
        if let Some(system) = &self.system {
            map.insert(1u64, system.as_str());
        }
        if let Some(game) = &self.game {
            map.insert(2u64, game.to_cbor());
        }
        if let Some(source) = &self.source {
            map.insert(3u64, source.to_cbor());
        }
        if let Some(card) = &self.card {
            map.insert(4u64, card.to_cbor());
        }
        if let Some(description) = &self.description {
            map.insert(5u64, description.as_str());
        }
        finish(map, &self.unknown, &self.extensions)
    }
}

impl Game {
    /// Writes the game hints as a CBOR map.
    #[must_use]
    pub fn to_cbor(&self) -> CBOR {
        let mut map = Map::new();
        if !self.game_id.is_empty() {
            let mut ids = Map::new();
            for (resolver, id) in &self.game_id {
                match id {
                    GameId::Uint(n) => ids.insert(resolver.as_str(), *n),
                    GameId::Text(t) => ids.insert(resolver.as_str(), t.as_str()),
                }
            }
            map.insert(0u64, ids);
        }
        if !self.rom_hashes.is_empty() {
            let hashes: Vec<CBOR> = self.rom_hashes.iter().map(hash_cbor).collect();
            map.insert(1u64, hashes);
        }
        if let Some(rom_filename) = &self.rom_filename {
            map.insert(2u64, rom_filename.as_str());
        }
        if let Some(serial) = &self.serial {
            map.insert(3u64, serial.as_str());
        }
        if let Some(title) = &self.title {
            map.insert(4u64, title.as_str());
        }
        if let Some(system) = &self.system {
            map.insert(5u64, system.as_str());
        }
        finish(map, &self.unknown, &Extensions::new())
    }
}

impl Source {
    /// Writes the source as a CBOR map.
    #[must_use]
    pub fn to_cbor(&self) -> CBOR {
        let mut map = Map::new();
        if let Some(device_kind) = &self.device_kind {
            map.insert(0u64, device_kind.as_str());
        }
        if let Some(fingerprint) = &self.fingerprint {
            map.insert(1u64, fingerprint.as_str());
        }
        if let Some(app) = &self.app {
            map.insert(2u64, app.as_str());
        }
        if let Some(app_version) = &self.app_version {
            map.insert(3u64, app_version.as_str());
        }
        finish(map, &self.unknown, &Extensions::new())
    }
}

impl Card {
    /// Writes the card as a CBOR map.
    #[must_use]
    pub fn to_cbor(&self) -> CBOR {
        let mut map = Map::new();
        map.insert(0u64, self.format.as_str());
        map.insert(1u64, self.capacity);
        if let Some(system_area) = &self.system_area {
            map.insert(2u64, CBOR::to_byte_string(system_area));
        }
        finish(map, &self.unknown, &Extensions::new())
    }
}

fn encode_parts(parts: &[Part]) -> CBOR {
    parts.iter().map(Part::to_cbor).collect::<Vec<_>>().into()
}

impl Part {
    /// Writes the part as a CBOR map.
    #[must_use]
    pub fn to_cbor(&self) -> CBOR {
        let mut map = Map::new();
        map.insert(0u64, self.id);
        if let Some(kind) = self.kind.as_str() {
            map.insert(1u64, kind);
        }
        if let Some(role) = &self.role {
            map.insert(2u64, role.as_str());
        }
        if let Some(path) = &self.path {
            map.insert(3u64, path.as_str());
        }
        if let Some(slot) = self.slot {
            map.insert(4u64, slot);
        }
        if let Some(dirent) = &self.dirent {
            map.insert(5u64, CBOR::to_byte_string(dirent));
        }
        if let Some(content_type) = &self.content_type {
            map.insert(6u64, content_type.as_str());
        }
        match &self.payload {
            Payload::Embedded(_) => {}
            Payload::Compressed { size, .. } => {
                map.insert(7u64, "zstd");
                map.insert(8u64, *size);
            }
        }
        map.insert(9u64, hash_cbor(&self.sha256));
        if let Some(source) = &self.source {
            map.insert(10u64, source.to_cbor());
        }
        if let Some(game) = &self.game {
            map.insert(11u64, game.to_cbor());
        }
        if let Some(system) = &self.system {
            map.insert(12u64, system.as_str());
        }
        if let Some(binding) = &self.binding {
            map.insert(13u64, binding.as_str());
        }
        map.insert(
            -1i64,
            match &self.payload {
                // Both go out as a byte string; what differs is whether keys 7 and 8 above
                // said the bytes are compressed.
                Payload::Embedded(bytes) | Payload::Compressed { bytes, .. } => CBOR::to_byte_string(bytes),
            },
        );
        finish(map, &self.unknown, &self.extensions)
    }
}
