//! The rules a schema cannot state, and the ones that tie a header and its parts to its shape.
//!
//! CDDL describes the decoded data model, so it sees neither the encoding beneath it nor
//! anything inside a byte string. Everything here is a rule that needs one of those: a
//! comparison across items, or a look into a payload. Which keys and which parts each shape
//! admits is here too, rather than in the decoder, so a bundle a caller built is held to it on the
//! way out exactly as one read off the wire is on the way in.

use crate::codec::{Strictness, check_path};
use crate::error::{Error, ErrorKind};
use crate::hash::HashValue;
use crate::model::{Bundle, Game, Header, Part, PartKind, Payload, Recorded, TakenWith};
use crate::name::ReverseDnsName;
use crate::shape::Shape;

/// The two image types a screenshot takes, which any image library reads.
const IMAGE_TYPES: [&str; 2] = ["image/png", "image/jpeg"];

impl Bundle {
    /// Checks every structural rule the format states.
    ///
    /// Called for you by [`from_slice`](Bundle::from_slice) and [`to_vec`](Bundle::to_vec), so a
    /// bundle that came from either has already passed. A rule broken at any depth fails the
    /// whole bundle: a card holding one malformed save is a malformed card, and nothing here
    /// keeps the parts it could read.
    ///
    /// # Cost
    ///
    /// Every [`bundle`](crate::PartKind::Bundle) part is decoded here, checked, and then
    /// discarded: the rules about a nested bundle are rules about what its payload decodes to, and
    /// keeping the result would mean either a second in-memory shape for a bundle or making every
    /// caller pay for an inner decode it may not want. A caller that then walks the nested bundles
    /// decodes each of them a second time, which is the expected pattern rather than a mistake —
    /// on a 16 MiB card it is the work twice. A compressed `bundle` part is not inflated: that
    /// would let a small file make every decode allocate whatever it claims, and the bundle it
    /// carries is validated when it is read. [`from_cbor`](Bundle::from_cbor) is the way out: it
    /// does not validate, so it does not step into a payload.
    pub fn validate(&self) -> Result<(), Error> {
        self.validate_inner()
    }

    /// Checks the rules, including what this bundle's shape may hold.
    ///
    /// Nesting is bounded by the shapes rather than by a depth counter: a save, a state and a
    /// screenshot hold no bundle, a card holds saves, a device holds cards and saves, and a
    /// collection holds anything but another collection.
    fn validate_inner(&self) -> Result<(), Error> {
        if self.parts.is_empty() {
            return Err(Error::at("parts", ErrorKind::EmptyContainer));
        }
        self.check_part_order()?;
        check_header(&self.header)?;
        // Before the addresses, since two screenshots can only ever share one: the rule a state
        // breaks by holding them is the one worth naming.
        self.check_parts_against_shape()?;
        self.check_part_addresses()?;

        for (index, part) in self.parts.iter().enumerate() {
            let path = format!("parts[{index}]");
            check_part_kind(part, &path)?;
            if let Some(game) = &part.game {
                check_game(game, &format!("{path}.game"))?;
            }
            check_key_placement(&part.extensions, Place::Part(&part.kind), &path)?;
            if part.kind == PartKind::Bundle {
                check_nested(part, &self.header.shape, &path)?;
            }
        }
        Ok(())
    }

    /// Parts are stored in ascending `id` order, always, and no two share an `id`.
    fn check_part_order(&self) -> Result<(), Error> {
        for (index, pair) in self.parts.windows(2).enumerate() {
            let [previous, next] = pair else { continue };
            if previous.id == next.id {
                return Err(Error::at(format!("parts[{}]", index + 1), ErrorKind::DuplicatePartId(next.id)));
            }
            if previous.id > next.id {
                return Err(Error::at("parts", ErrorKind::PartsOutOfOrder));
            }
        }
        Ok(())
    }

    /// No two parts of one kind may agree on `role`, `path` and `slot` together.
    ///
    /// Those three are how a consumer names a part to a user and matches it against a target, so
    /// two parts a consumer cannot tell apart is a bundle it cannot act on. With all three
    /// defaulting to absence, two bare saves collide on exactly this. Parts of different kinds
    /// never do, which is what lets a state part and its screenshot go without an address.
    fn check_part_addresses(&self) -> Result<(), Error> {
        for (index, part) in self.parts.iter().enumerate() {
            for earlier in &self.parts[..index] {
                if earlier.kind == part.kind && earlier.address() == part.address() {
                    return Err(Error::at(
                        format!("parts[{index}]"),
                        ErrorKind::PartsShareAnAddress { first: earlier.id, second: part.id },
                    ));
                }
            }
        }
        Ok(())
    }

    /// Which kinds of part this bundle's shape holds, and how many of each.
    ///
    /// A shape this version does not define constrains nothing: its rules are in a document this
    /// decoder has not seen. Whether a `bundle` part holds a shape this one may contain is
    /// [`check_nested`]'s, since it takes reading the payload.
    fn check_parts_against_shape(&self) -> Result<(), Error> {
        let refuse = |index: usize, shape: &'static str, reason: &'static str| {
            Err(Error::at(format!("parts[{index}]"), ErrorKind::PartNotInShape { shape, reason }))
        };
        let positions = |kind: PartKind| -> Vec<usize> {
            self.parts
                .iter()
                .enumerate()
                .filter(|(_, part)| part.kind == kind)
                .map(|(index, _)| index)
                .collect()
        };

        match &self.header.shape {
            Shape::Unknown(_) => {}
            // Exactly one state part, since it is what the bundle loads, and at most one screenshot,
            // since that is the state's picture. Two states and an in-game save in one file are a
            // collection.
            Shape::State => {
                match positions(PartKind::State).as_slice() {
                    [] => {
                        return Err(Error::at(
                            "parts",
                            ErrorKind::PartNotInShape {
                                shape: "state",
                                reason: "holds exactly one `state` part",
                            },
                        ));
                    }
                    [_] => {}
                    [_, second, ..] => return refuse(*second, "state", "holds exactly one `state` part"),
                }
                if let [_, second, ..] = positions(PartKind::Screenshot).as_slice() {
                    return refuse(*second, "state", "holds at most one `screenshot` part");
                }
                if let Some(&index) = positions(PartKind::Bundle).first() {
                    return refuse(index, "state", "holds no nested bundle");
                }
            }
            // One image, and nothing else: two are two bundles, which a collection holds together.
            Shape::Screenshot => {
                let reason = "holds one `screenshot` part and nothing else";
                if self.parts.len() > 1 {
                    return refuse(1, "screenshot", reason);
                }
                if self.parts[0].kind != PartKind::Screenshot {
                    return refuse(0, "screenshot", reason);
                }
            }
            shape @ (Shape::Save | Shape::Card | Shape::Device | Shape::Collection) => {
                let name = shape_name(shape);
                // A consumer restoring an in-game save never meets a state's data or its picture,
                // because neither can sit anywhere but the shapes that hold them.
                if let Some(&index) = positions(PartKind::State).first() {
                    return refuse(index, name, "holds no `state` part; a save state is a bundle of its own");
                }
                if let Some(&index) = positions(PartKind::Screenshot).first() {
                    return refuse(
                        index,
                        name,
                        "holds no `screenshot` part; a screenshot is a bundle of its own",
                    );
                }
                if *shape == Shape::Save
                    && let Some(&index) = positions(PartKind::Bundle).first()
                {
                    return refuse(index, "save", "holds no nested bundle");
                }
                // A device's components are told apart by `role` and nothing else. Writing it is
                // required rather than advised, because `role` otherwise defaults to `primary`, and
                // two components that both fell back to it would be a device whose halves a
                // consumer cannot distinguish.
                if *shape == Shape::Device
                    && let Some(index) = self.parts.iter().position(|part| part.role.is_none())
                {
                    return refuse(index, "device", "names a role on every part");
                }
            }
        }
        Ok(())
    }
}

/// The name a shape this version defines is written as, for an error to name it by.
fn shape_name(shape: &Shape) -> &'static str {
    match shape {
        Shape::Save => "save",
        Shape::State => "state",
        Shape::Screenshot => "screenshot",
        Shape::Card => "card",
        Shape::Device => "device",
        Shape::Collection => "collection",
        Shape::Unknown(_) => "unknown",
    }
}

/// Checks the header against its shape, and what sits inside it.
///
/// Each shape admits a different header, and the shape is read first, so a contradiction names
/// the key that broke it rather than leaving a consumer to re-derive the shape. A shape this
/// version does not define constrains nothing.
fn check_header(header: &Header) -> Result<(), Error> {
    let shape = shape_name(&header.shape);
    let forbid = |present: bool, field: &'static str| {
        if present { Err(Error::at("header", ErrorKind::KeyNotInShape { shape, field })) } else { Ok(()) }
    };
    let require = |present: bool, field: &'static str| {
        if present { Ok(()) } else { Err(Error::at("header", ErrorKind::MissingKey(field))) }
    };
    // `taken_at` is when a state or a screenshot was taken, which no other shape has a moment
    // for; `taken_with` and `origin` describe a state alone.
    let forbid_taking = |taken_at_too: bool| {
        forbid(taken_at_too && header.taken_at.is_some(), "taken_at")?;
        forbid(header.taken_with.is_some(), "taken_with")?;
        forbid(header.origin.is_some(), "origin")
    };

    match &header.shape {
        // A save is one game's in-game save, and a card map is what would make it a card instead.
        Shape::Save => {
            forbid(header.card.is_some(), "card")?;
            forbid_taking(true)?;
        }
        // A state loads only into the core that took it, so its `source` is where it can go and
        // has to say so.
        Shape::State => {
            forbid(header.card.is_some(), "card")?;
            let Some(source) = &header.source else {
                return Err(Error::at("header", ErrorKind::MissingKey("source")));
            };
            if source.core.is_none() && source.app.is_none() {
                return Err(Error::at("header.source", ErrorKind::StateLoadsIntoNothing));
            }
        }
        Shape::Screenshot => {
            forbid(header.card.is_some(), "card")?;
            forbid_taking(false)?;
        }
        // The card map is the claim proper: it carries the format and capacity a writer needs.
        Shape::Card => {
            require(header.card.is_some(), "card")?;
            forbid_taking(true)?;
        }
        // `system` is what tells a device from a collection; its components hold saves for many
        // games rather than for one, so `game` says nothing at this level.
        Shape::Device => {
            require(header.system.is_some(), "system")?;
            forbid(header.game.is_some(), "game")?;
            forbid(header.card.is_some(), "card")?;
            forbid_taking(true)?;
        }
        // A collection's entries do not agree on any of these, so it names none of them.
        Shape::Collection => {
            forbid(header.system.is_some(), "system")?;
            forbid(header.game.is_some(), "game")?;
            forbid(header.card.is_some(), "card")?;
            forbid_taking(true)?;
        }
        Shape::Unknown(_) => {}
    }

    check_key_placement(&header.extensions, Place::Header(&header.shape), "header")?;
    if let Some(game) = &header.game {
        check_game(game, "header.game")?;
    }
    if let Some(taken_with) = &header.taken_with {
        check_taken_with(taken_with, "header.taken_with")?;
    }
    Ok(())
}

/// Checks the keys a `state` or a `screenshot` part takes, wherever it sits.
///
/// A state part is the core's own data, which has no media type, and takes `slot` for the number
/// the emulator gave it and nothing else of a part's addressing. A screenshot takes none of it,
/// and names its image type, which is one any image library reads.
fn check_part_kind(part: &Part, path: &str) -> Result<(), Error> {
    let kind = match part.kind {
        PartKind::State => "state",
        PartKind::Screenshot => "screenshot",
        _ => return Ok(()),
    };
    let screenshot = part.kind == PartKind::Screenshot;
    let present = [
        ("role", part.role.is_some()),
        ("path", part.path.is_some()),
        ("slot", screenshot && part.slot.is_some()),
        ("dirent", part.dirent.is_some()),
        ("content_type", !screenshot && part.content_type.is_some()),
        ("source", part.source.is_some()),
        ("game", part.game.is_some()),
        ("system", part.system.is_some()),
        ("binding", part.binding.is_some()),
    ];
    if let Some((field, _)) = present.iter().find(|(_, present)| *present) {
        return Err(Error::at(path, ErrorKind::KeyNotOnPart { kind, field }));
    }
    if screenshot {
        match part.content_type.as_deref() {
            None => return Err(Error::at(path, ErrorKind::MissingKey("content_type"))),
            Some(found) if !IMAGE_TYPES.contains(&found) => {
                return Err(Error::at(
                    format!("{path}.content_type"),
                    ErrorKind::ScreenshotType(found.to_owned()),
                ));
            }
            Some(_) => {}
        }
    }
    Ok(())
}

/// Where an extension key sits, which is what its specification places it by.
#[derive(Clone, Copy)]
enum Place<'a> {
    /// A bundle's header, of this shape.
    Header(&'a Shape),
    /// A part, of this kind.
    Part(&'a PartKind),
    /// A `game` map or `taken_with`, which describe a game or a machine rather than one save or
    /// one state of it.
    Elsewhere,
}

/// An extension subtree the specifications place, and where.
struct Placement {
    /// The subtree, matched at label boundaries.
    root: ReverseDnsName,
    /// Whether a place is one the key's specification admits.
    admits: fn(Place<'_>) -> bool,
    /// Those places, as an error names them.
    allowed: &'static str,
}

/// The extension keys the specifications place, as whole subtrees rather than exact names.
///
/// Every clock key lives under `x.1sav.rtc` — the portable reading is that name, and each chip is
/// a label below it — so one label-boundary test covers the family and a chip added later is
/// covered without touching this list. A clock reading describes the bytes rather than the
/// wrapper, so it lives in the save's or the state's own header, where it stays correct once the
/// bundle is sliced out of whatever holds it; on a part it would be dropped by the byte copy that
/// extracting a save is. mGBA's cheat list loads with the core's data, so it rides on the state
/// part and stays behind when an in-game save is lifted out of the state.
const PLACEMENTS: [Placement; 2] = [
    Placement {
        root: ReverseDnsName::from_static("x.1sav.rtc"),
        admits: on_a_save_or_state_header,
        allowed: "a save's or a state's bundle header",
    },
    Placement {
        root: ReverseDnsName::from_static("x.1sav.cheats.mgba"),
        admits: on_a_state_part,
        allowed: "a state's `state` part",
    },
];

/// A shape this version does not define constrains nothing, its header included.
fn on_a_save_or_state_header(place: Place<'_>) -> bool {
    matches!(place, Place::Header(Shape::Save | Shape::State | Shape::Unknown(_)))
}

fn on_a_state_part(place: Place<'_>) -> bool {
    matches!(place, Place::Part(PartKind::State))
}

fn check_key_placement(extensions: &crate::Extensions, place: Place<'_>, path: &str) -> Result<(), Error> {
    for key in extensions.keys() {
        // Label-boundary matching, never a string prefix: `x.1sav.rtcx` is somebody else's.
        if let Some(placement) = PLACEMENTS.iter().find(|placement| key.is_under(&placement.root))
            && !(placement.admits)(place)
        {
            return Err(Error::at(
                format!("{path}[{:?}]", key.as_str()),
                ErrorKind::KeyOutOfPlace { key: key.as_str().to_owned(), allowed: placement.allowed },
            ));
        }
    }
    Ok(())
}

/// Checks a `game` map, wherever it sits.
///
/// Its extension keys describe the game and nothing about one save of it, so a clock reading is
/// out of place here even inside a save's own header: two saves of one game carry one `game`.
fn check_game(game: &Game, path: &str) -> Result<(), Error> {
    check_hashes(&game.rom_hashes, &format!("{path}.rom_hashes"))?;
    check_key_placement(&game.extensions, Place::Elsewhere, path)
}

/// Checks what a state was taken with.
///
/// Absence is the only empty, here as everywhere: an empty map, an empty `firmware` and empty
/// `options` all have it to say instead.
fn check_taken_with(taken_with: &TakenWith, path: &str) -> Result<(), Error> {
    if taken_with.is_empty() {
        return Err(Error::at(path, ErrorKind::EmptyContainer));
    }
    if let Some(Recorded::Known(firmware)) = &taken_with.firmware {
        let path = format!("{path}.firmware");
        if firmware.is_empty() {
            return Err(Error::at(path, ErrorKind::EmptyContainer));
        }
        for (file, hashes) in firmware {
            let path = format!("{path}[{file:?}]");
            check_path(file, &path)?;
            if hashes.is_empty() {
                return Err(Error::at(path, ErrorKind::EmptyContainer));
            }
            check_hashes(hashes, &path)?;
        }
    }
    if let Some(Recorded::Known(language)) = &taken_with.language
        && !is_canonical_language(language)
    {
        return Err(Error::at(format!("{path}.language"), ErrorKind::LanguageNotCanonical(language.clone())));
    }
    if let Some(Recorded::Known(options)) = &taken_with.options
        && options.is_empty()
    {
        return Err(Error::at(format!("{path}.options"), ErrorKind::EmptyContainer));
    }
    check_key_placement(&taken_with.extensions, Place::Elsewhere, path)
}

/// Whether a language tag is BCP 47 in its canonical case, stopping at the region.
///
/// A language of two or three lower-case letters, then optionally a script of four letters in
/// title case, then optionally a region of two upper-case letters or three digits. That is as far
/// as any console's language setting goes, and it gives one setting one spelling.
fn is_canonical_language(tag: &str) -> bool {
    let mut subtags = tag.split('-');
    let language = subtags.next().unwrap_or_default();
    if !(2..=3).contains(&language.len()) || !language.bytes().all(|byte| byte.is_ascii_lowercase()) {
        return false;
    }
    let mut next = subtags.next();
    if let Some(script) = next
        && script.len() == 4
    {
        let (first, rest) = script.split_at(1);
        if !first.bytes().all(|byte| byte.is_ascii_uppercase())
            || !rest.bytes().all(|byte| byte.is_ascii_lowercase())
        {
            return false;
        }
        next = subtags.next();
    }
    if let Some(region) = next {
        let letters = region.len() == 2 && region.bytes().all(|byte| byte.is_ascii_uppercase());
        let digits = region.len() == 3 && region.bytes().all(|byte| byte.is_ascii_digit());
        if !letters && !digits {
            return false;
        }
        next = subtags.next();
    }
    next.is_none()
}

/// Hash values sort ascending by tag, and hold at most one entry per algorithm.
///
/// Two digests of one algorithm name two different files, and nothing says which one the bundle
/// means.
fn check_hashes(hashes: &[HashValue], path: &str) -> Result<(), Error> {
    for pair in hashes.windows(2) {
        let [previous, next] = pair else { continue };
        if previous.algorithm() == next.algorithm() {
            return Err(Error::at(path, ErrorKind::RomHashesDuplicateAlgorithm(next.algorithm())));
        }
        if previous.tag() > next.tag() {
            return Err(Error::at(path, ErrorKind::RomHashesOutOfOrder));
        }
    }
    Ok(())
}

/// Checks the rules a `bundle` part carries, all of them past what a schema can see.
fn check_nested(part: &Part, outer: &Shape, path: &str) -> Result<(), Error> {
    // A compressed `bundle` part is left to whoever inflates it. Inflating here would let a file of
    // a few kilobytes make every decode allocate whatever its parts claim, and the bundle it
    // carries is checked anyway the moment it is read, since reading one validates it.
    let Payload::Embedded(bytes) = &part.payload else {
        return Ok(());
    };

    let inner = Bundle::from_cbor(
        &dcbor::CBOR::try_from_data(bytes)
            .map_err(|e| Error::at(path, ErrorKind::Encoding(e.to_string())))?,
        Strictness::Decoder,
    )
    .map_err(|e| Error::at(path, ErrorKind::NestedPayloadNotABundle(Box::new(e))))?;

    // Every inner part is uncompressed with its payload embedded, so an inner bundle's file hash
    // and content hash are the same value. Compression belongs on the outer part, where it does
    // not change `sha256` and compresses better anyway.
    for (index, inner_part) in inner.parts.iter().enumerate() {
        if !matches!(inner_part.payload, Payload::Embedded(_)) {
            return Err(Error::at(
                format!("{path}.payload.parts[{index}]"),
                ErrorKind::NestedPartNotNormalized,
            ));
        }
    }

    // The outer `sha256` is the inner bundle's identity, which is what makes extracting a save a
    // byte copy rather than a re-encode.
    if part.sha256 != HashValue::sha256_of(bytes) {
        return Err(Error::at(path, ErrorKind::NestedHashMismatch));
    }

    // What a shape may hold, which is what bounds the nesting now that the depth cap is gone. An
    // inner or outer shape this version does not define is not judged: its rules are in a
    // document this decoder has not seen, so the bundle round-trips rather than being rejected.
    if !outer.is_known() || !inner.shape().is_known() {
        return Ok(());
    }
    let (holds, shape, reason) = match outer {
        Shape::Save | Shape::State | Shape::Screenshot => {
            (false, shape_name(outer), "holds no nested bundle")
        }
        Shape::Card => (matches!(inner.shape(), Shape::Save), "card", "holds saves and nothing else"),
        Shape::Device => {
            (matches!(inner.shape(), Shape::Card | Shape::Save), "device", "holds cards and saves")
        }
        Shape::Collection => {
            (!matches!(inner.shape(), Shape::Collection), "collection", "does not hold another collection")
        }
        Shape::Unknown(_) => return Ok(()),
    };
    if !holds {
        return Err(Error::at(path, ErrorKind::PartNotInShape { shape, reason }));
    }

    inner.validate_inner().map_err(|e| e.within(&format!("{path}.payload")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Header;
    use crate::name::Slug;

    fn bundle_of(parts: Vec<Part>) -> Bundle {
        Bundle { header: Header::default(), parts }
    }

    /// A collection holding these parts, which is the shape that holds a `bundle` part of any
    /// other.
    fn collection_of(parts: Vec<Part>) -> Bundle {
        Bundle { header: Header::new(Shape::Collection), parts }
    }

    /// A save bundle as a `bundle` part, ready for a collection to hold.
    fn nested_save(id: u64) -> Part {
        let inner = Bundle { header: Header::default(), parts: vec![Part::new(0, *b"SAVE")] };
        Part::new(id, inner.to_vec().unwrap()).kind(PartKind::Bundle)
    }

    /// The smallest valid state: the core it loads into, and the core's data.
    fn state_of(parts: Vec<Part>) -> Bundle {
        let source = crate::Source::default().core(crate::Name::parse("mgba").unwrap());
        Bundle { header: Header::new(Shape::State).source(source), parts }
    }

    fn state_part(id: u64) -> Part {
        Part::new(id, *b"CORE").kind(PartKind::State)
    }

    fn screenshot_part(id: u64) -> Part {
        Part::new(id, *b"\x89PNG").kind(PartKind::Screenshot).content_type("image/png")
    }

    #[test]
    fn rejects_a_bundle_with_no_parts() {
        assert_eq!(bundle_of(vec![]).validate().unwrap_err().kind(), &ErrorKind::EmptyContainer);
    }

    #[test]
    fn rejects_parts_out_of_order() {
        let parts = vec![Part::new(1, *b"a"), Part::new(0, *b"b")];
        assert_eq!(bundle_of(parts).validate().unwrap_err().kind(), &ErrorKind::PartsOutOfOrder);
    }

    #[test]
    fn rejects_two_parts_that_share_an_address() {
        // Two bare parts collide on role, path and slot, all three of which default to absence.
        let parts = vec![Part::new(0, *b"a"), Part::new(1, *b"b")];
        assert_eq!(
            bundle_of(parts).validate().unwrap_err().kind(),
            &ErrorKind::PartsShareAnAddress { first: 0, second: 1 }
        );
    }

    #[test]
    fn a_role_is_enough_to_tell_two_parts_apart() {
        let mut parts = vec![Part::new(0, *b"a"), Part::new(1, *b"b")];
        parts[1].role = Some(Slug::parse("cartridge").unwrap());
        assert!(bundle_of(parts).validate().is_ok());
    }

    #[test]
    fn a_clock_reading_belongs_on_a_saves_header_and_nowhere_else() {
        // 0.2 narrowed this: the clock keys are a save's bundle header, full stop. A reading put
        // on any part would vanish the moment the save is sliced out, and one on a card's header
        // would belong to no save in particular.
        let key = crate::ReverseDnsName::parse("x.1sav.rtc").unwrap();
        let reading = dcbor::CBOR::from(1u64);

        // On a `bundle` part, which is what the rule used to be about.
        let mut outer = nested_save(0);
        outer.extensions.insert_cbor(key.clone(), reading.clone());
        assert_eq!(
            collection_of(vec![outer]).validate().unwrap_err().kind(),
            &ErrorKind::KeyOutOfPlace {
                key: "x.1sav.rtc".to_owned(),
                allowed: "a save's or a state's bundle header"
            }
        );

        // And on a plain part, which it now also covers.
        let mut plain = Part::new(0, *b"SAVE");
        plain.extensions.insert_cbor(key.clone(), reading.clone());
        assert!(bundle_of(vec![plain]).validate().is_err(), "not on a plain part either");

        // A save's own header is where it belongs.
        let mut save = bundle_of(vec![Part::new(0, *b"SAVE")]);
        save.header.extensions.insert_cbor(key, reading);
        save.validate().expect("a save's header carries the reading");
    }

    #[test]
    fn a_clock_reading_is_refused_in_a_game_map() {
        // A key in `game` describes the game, and a clock reading describes one save of it. That
        // holds on a save's header too, which is the one place the reading itself may sit.
        let key = crate::ReverseDnsName::parse("x.1sav.rtc").unwrap();
        let mut game = Game { title: Some("Pokemon Ruby".into()), ..Game::default() };
        game.extensions.insert_cbor(key, dcbor::CBOR::from(1u64));

        let mut save = bundle_of(vec![Part::new(0, *b"SAVE")]);
        save.header.game = Some(game.clone());
        assert_eq!(
            save.validate().unwrap_err().kind(),
            &ErrorKind::KeyOutOfPlace {
                key: "x.1sav.rtc".to_owned(),
                allowed: "a save's or a state's bundle header"
            }
        );

        let mut part = Part::new(0, *b"SAVE");
        part.game = Some(game);
        assert!(bundle_of(vec![part]).validate().is_err(), "nor in a part's game map");
    }

    #[test]
    fn every_clock_key_is_refused_on_a_bundle_part() {
        // The rule covers the whole family, including a chip nobody has added yet: the check is a
        // label-boundary test against `x.1sav.rtc`, not a list of names to keep up to date.
        for key in ["x.1sav.rtc", "x.1sav.rtc.mbc3", "x.1sav.rtc.s3511a", "x.1sav.rtc.future-chip"] {
            let mut outer = nested_save(0);
            outer.extensions.insert_cbor(crate::ReverseDnsName::parse(key).unwrap(), dcbor::CBOR::from(1u64));
            assert!(
                matches!(
                    collection_of(vec![outer]).validate().unwrap_err().kind(),
                    ErrorKind::KeyOutOfPlace { .. }
                ),
                "{key} on a bundle part"
            );
        }
    }

    #[test]
    fn a_clock_reading_belongs_on_a_states_header_too() {
        // A state's clock sits inside the core's data, and the keys are a reading of it, which an
        // in-game save lifted out of the state takes with it.
        let key = crate::ReverseDnsName::parse("x.1sav.rtc.s3511a").unwrap();
        let mut state = state_of(vec![state_part(0)]);
        state.header.extensions.insert_cbor(key.clone(), dcbor::CBOR::from(1u64));
        state.validate().expect("a state's header carries the reading");

        let mut part = state_part(0);
        part.extensions.insert_cbor(key, dcbor::CBOR::from(1u64));
        assert!(state_of(vec![part]).validate().is_err(), "but not its state part");
    }

    #[test]
    fn mgbas_cheats_ride_on_the_state_part_and_nowhere_else() {
        use crate::extension::{CheatSet, CheatsMgba};
        let cheats = CheatsMgba::new(vec![CheatSet::new(["D8BAE4D9 4864DCE5"])]);

        let mut part = state_part(0);
        part.extensions.insert(cheats.clone());
        state_of(vec![part]).validate().expect("the state part carries mGBA's cheats");

        let mut state = state_of(vec![state_part(0)]);
        state.header.extensions.insert(cheats.clone());
        assert_eq!(
            state.validate().unwrap_err().kind(),
            &ErrorKind::KeyOutOfPlace {
                key: "x.1sav.cheats.mgba".to_owned(),
                allowed: "a state's `state` part"
            },
            "not on the header"
        );

        // Nor on the in-game save beside it, which a consumer lifts out and leaves the cheats behind.
        let mut save = Part::new(1, *b"SAVE");
        save.extensions.insert(cheats);
        assert!(state_of(vec![state_part(0), save]).validate().is_err(), "nor on a save part");
    }

    #[test]
    fn a_state_holds_exactly_one_state_part_and_at_most_one_screenshot() {
        state_of(vec![state_part(0), screenshot_part(1)]).validate().expect("a state and its picture");
        state_of(vec![state_part(0)]).validate().expect("a state without one");

        let refused = |bundle: Bundle| match bundle.validate().unwrap_err().kind() {
            ErrorKind::PartNotInShape { shape: "state", reason } => *reason,
            other => panic!("refused for the wrong reason: {other:?}"),
        };
        assert_eq!(refused(state_of(vec![screenshot_part(0)])), "holds exactly one `state` part");
        let two = vec![state_part(0).slot(1), state_part(1).slot(2)];
        assert_eq!(refused(state_of(two)), "holds exactly one `state` part");
        let pictures = vec![state_part(0), screenshot_part(1), screenshot_part(2).content_type("image/jpeg")];
        assert_eq!(refused(state_of(pictures)), "holds at most one `screenshot` part");
        assert_eq!(refused(state_of(vec![state_part(0), nested_save(1)])), "holds no nested bundle");

        // An in-game save beside the core's data and an `aux` part for the rest of the file are
        // both what a state holds.
        let aux = Part::new(2, *b"ACHV").kind(PartKind::Aux).path("ACHV");
        state_of(vec![state_part(0), Part::new(1, *b"SAVE"), aux])
            .validate()
            .expect("a save and an aux part");
    }

    #[test]
    fn neither_kind_of_its_own_sits_in_any_other_shape() {
        let refused = |bundle: Bundle| {
            matches!(bundle.validate().unwrap_err().kind(), ErrorKind::PartNotInShape { .. })
        };
        assert!(refused(bundle_of(vec![state_part(0)])), "a state part in a save");
        assert!(
            refused(bundle_of(vec![Part::new(0, *b"SAVE"), screenshot_part(1)])),
            "a screenshot in a save"
        );
        assert!(refused(collection_of(vec![state_part(0)])), "a state part in a collection");
    }

    #[test]
    fn a_screenshot_is_one_image_of_a_type_any_library_reads() {
        let screenshot = |parts| Bundle { header: Header::new(Shape::Screenshot), parts };
        screenshot(vec![screenshot_part(0)]).validate().expect("one PNG");
        screenshot(vec![screenshot_part(0).content_type("image/jpeg")]).validate().expect("one JPEG");

        assert!(screenshot(vec![screenshot_part(0), screenshot_part(1)]).validate().is_err(), "two images");
        assert!(screenshot(vec![Part::new(0, *b"SAVE")]).validate().is_err(), "a save");
        assert_eq!(
            screenshot(vec![screenshot_part(0).content_type("image/webp")]).validate().unwrap_err().kind(),
            &ErrorKind::ScreenshotType("image/webp".to_owned())
        );
        let mut untyped = screenshot_part(0);
        untyped.content_type = None;
        assert_eq!(
            screenshot(vec![untyped]).validate().unwrap_err().kind(),
            &ErrorKind::MissingKey("content_type")
        );
        assert_eq!(
            screenshot(vec![screenshot_part(0).slot(1)]).validate().unwrap_err().kind(),
            &ErrorKind::KeyNotOnPart { kind: "screenshot", field: "slot" }
        );
    }

    #[test]
    fn a_state_part_takes_a_slot_and_no_other_address() {
        state_of(vec![state_part(0).slot(3)]).validate().expect("the number the emulator gave it");
        assert_eq!(
            state_of(vec![state_part(0).content_type("application/octet-stream")])
                .validate()
                .unwrap_err()
                .kind(),
            &ErrorKind::KeyNotOnPart { kind: "state", field: "content_type" }
        );
        assert_eq!(
            state_of(vec![state_part(0).path("SLOT1")]).validate().unwrap_err().kind(),
            &ErrorKind::KeyNotOnPart { kind: "state", field: "path" }
        );
    }

    #[test]
    fn a_state_says_what_it_loads_into() {
        let mut state = state_of(vec![state_part(0)]);
        state.header.source = None;
        assert_eq!(state.validate().unwrap_err().kind(), &ErrorKind::MissingKey("source"));

        state.header.source = Some(crate::Source::default().core_version("0.10.5"));
        assert_eq!(state.validate().unwrap_err().kind(), &ErrorKind::StateLoadsIntoNothing);

        // An app is enough where nothing runs a core, as a flashcart's firmware takes its own.
        state.header.source = Some(crate::Source::default().app(crate::Name::parse("io.everdrive").unwrap()));
        state.validate().expect("an app names what it loads into");
    }

    #[test]
    fn taking_a_state_is_said_on_a_state_and_nowhere_else() {
        let forbidden = |bundle: Bundle, field: &'static str| {
            assert_eq!(
                bundle.validate().unwrap_err().kind(),
                &ErrorKind::KeyNotInShape {
                    shape: crate::Shape::as_str(&bundle.header.shape).to_owned().leak(),
                    field
                },
            );
        };
        let save = bundle_of(vec![Part::new(0, *b"SAVE")]);
        forbidden(Bundle { header: save.header.clone().taken_at(1), ..save.clone() }, "taken_at");
        forbidden(
            Bundle { header: save.header.clone().origin(crate::Origin::Auto), ..save.clone() },
            "origin",
        );
        let screenshot =
            Bundle { header: Header::new(Shape::Screenshot).taken_at(1), parts: vec![screenshot_part(0)] };
        screenshot.validate().expect("a screenshot has a moment it was taken");
        let with = crate::TakenWith::default().byte_order(crate::ByteOrder::Little);
        forbidden(Bundle { header: screenshot.header.clone().taken_with(with), ..screenshot }, "taken_with");
    }

    #[test]
    fn taken_with_holds_one_spelling_of_each_setting() {
        use crate::{Recorded, TakenWith};
        let with = |taken_with: TakenWith| {
            let mut state = state_of(vec![state_part(0)]);
            state.header.taken_with = Some(taken_with);
            state.validate()
        };
        with(TakenWith::default().language("ja")).expect("a language");
        with(TakenWith::default().language("zh-Hans-CN")).expect("a script and a region");
        with(TakenWith::default().language("es-419")).expect("a numeric region");
        with(TakenWith::default().firmware(Recorded::Unknown)).expect("firmware nobody could name");
        for tag in ["EN-us", "en-us", "en_US", "zh-hans", "en-US-x-private", "e", ""] {
            assert!(
                matches!(
                    with(TakenWith::default().language(tag)).unwrap_err().kind(),
                    ErrorKind::LanguageNotCanonical(_)
                ),
                "{tag}"
            );
        }
        assert_eq!(with(TakenWith::default()).unwrap_err().kind(), &ErrorKind::EmptyContainer);
        assert_eq!(
            with(TakenWith::default().options(std::collections::BTreeMap::new())).unwrap_err().kind(),
            &ErrorKind::EmptyContainer
        );
        let bios = HashValue::sha256_of(b"BIOS");
        let firmware = std::collections::BTreeMap::from([("../bios.bin".to_owned(), vec![bios])]);
        assert_eq!(
            with(TakenWith::default().firmware(firmware)).unwrap_err().kind(),
            &ErrorKind::PathForbidden
        );
    }

    #[test]
    fn rejects_duplicate_rom_hash_algorithms() {
        use crate::hash::HashAlgorithm;
        let hashes = [
            HashValue::new(HashAlgorithm::Sha256, vec![1; 32]).unwrap(),
            HashValue::new(HashAlgorithm::Sha256, vec![2; 32]).unwrap(),
        ];
        assert_eq!(
            check_hashes(&hashes, "x").unwrap_err().kind(),
            &ErrorKind::RomHashesDuplicateAlgorithm(HashAlgorithm::Sha256)
        );
    }

    #[test]
    fn rejects_rom_hashes_out_of_order() {
        use crate::hash::HashAlgorithm;
        // sha1 is tag 18542 and sha256 is 18540, so this pair is reversed.
        let hashes = [
            HashValue::new(HashAlgorithm::Sha1, vec![1; 20]).unwrap(),
            HashValue::new(HashAlgorithm::Sha256, vec![2; 32]).unwrap(),
        ];
        assert_eq!(check_hashes(&hashes, "x").unwrap_err().kind(), &ErrorKind::RomHashesOutOfOrder);
    }
}
