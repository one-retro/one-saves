//! The rules a schema cannot state, and the ones that tie a header and its parts to its shape.
//!
//! CDDL describes the decoded data model, so it sees neither the encoding beneath it nor
//! anything inside a byte string. Everything here is a rule that needs one of those: a
//! comparison across items, or a look into a payload. Which keys and which parts each shape
//! admits is here too, rather than in the decoder, so a bundle a caller built is held to it on the
//! way out exactly as one read off the wire is on the way in.

use std::collections::{HashMap, HashSet};

use dcbor::prelude::*;

use crate::codec::{BINDINGS, Strictness, check_path};
use crate::error::{Error, ErrorKind};
use crate::extension::{Extension, Extensions, N64, N64Save};
use crate::hash::HashValue;
use crate::model::{Bundle, Game, Header, Part, PartKind, Recorded, Source, TakenWith};
use crate::name::Slug;
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
    /// on a 16 MiB card it is the work twice. Every part's payload is hashed as well, since a part
    /// whose `sha256` names other bytes is malformed. [`from_cbor`](Bundle::from_cbor) is the way
    /// out: it does not validate, so it neither hashes nor steps into a payload.
    pub fn validate(&self) -> Result<(), Error> {
        self.validate_inner()
    }

    /// Checks the rules, including what this bundle's shape may hold.
    ///
    /// Nesting is bounded by the shapes rather than by a depth counter: a save, a state and a
    /// screenshot hold no bundle, a card holds saves, a device holds cards and saves, and a
    /// collection holds anything but another collection.
    ///
    /// [`Checker`] applies these same rules a part at a time, for the streaming reader. A rule
    /// added here belongs there too; `tests/stream.rs` holds the two to the same verdict.
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
        self.check_system_entries()?;
        self.check_card_format()?;

        let mut across = Across::new(&self.header);
        for (index, part) in self.parts.iter().enumerate() {
            let path = format!("parts[{index}]");
            check_part_kind(part, &path)?;
            check_part_fields(part, &self.header, &path)?;
            across.part(part, part.payload.len() as u64, &path)?;
            check_part_extensions(
                &self.header,
                &part.kind,
                part.role.as_ref(),
                part.payload.len() as u64,
                &part.extensions,
                &path,
            )?;
            if part.kind == PartKind::Bundle {
                check_nested(part, &self.header, &path)?;
            } else if !part.verify() {
                // A decoder handing this payload out would hand out something corrupt.
                return Err(Error::at(path, ErrorKind::PayloadHashMismatch));
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
    ///
    /// A set rather than a comparison of every pair: the number of parts is the file's to choose,
    /// and pairwise, a few megabytes of them kept a decoder busy for minutes.
    fn check_part_addresses(&self) -> Result<(), Error> {
        // A collection's entries are bundles of their own, each its own content hash, and go back
        // to no socket: two loose saves in one file need nothing more to tell them apart.
        if self.header.shape == Shape::Collection {
            return Ok(());
        }
        let mut seen = HashMap::with_capacity(self.parts.len());
        for (index, part) in self.parts.iter().enumerate() {
            if let Some(first) = seen.insert((&part.kind, part.address()), part.id) {
                return Err(Error::at(
                    format!("parts[{index}]"),
                    ErrorKind::PartsShareAnAddress { first, second: part.id },
                ));
            }
        }
        Ok(())
    }

    /// Every `system` file sits in a directory with a `system` part of its own, ahead of the
    /// file's.
    ///
    /// A writer lays a card's entries down in part order, so a file whose directory comes later,
    /// or never, is one it has nowhere to put. Only a card holds `system` parts, which
    /// [`check_parts_against_shape`](Self::check_parts_against_shape) has already settled.
    fn check_system_entries(&self) -> Result<(), Error> {
        let mut laid_down: HashSet<&str> = HashSet::new();
        for (index, part) in self.parts.iter().enumerate() {
            // A part without a path is refused on its own account, by `check_system_part`.
            let Some(path) = part.path.as_deref().filter(|_| part.kind == PartKind::System) else {
                continue;
            };
            if let Some((directory, _)) = path.rsplit_once('/')
                && !laid_down.contains(&directory)
            {
                return Err(Error::at(
                    format!("parts[{index}]"),
                    ErrorKind::SystemEntryBeforeItsDirectory { directory: directory.to_owned() },
                ));
            }
            laid_down.insert(path);
        }
        Ok(())
    }

    /// What a card's format fixes: how long its entries are, how long its system area is, and how
    /// a save's name is spelled where the card holds something other than text.
    ///
    /// A format this version does not list constrains nothing. A consumer that does not know one
    /// must not write the card anyway, so nothing it would rebuild depends on these.
    fn check_card_format(&self) -> Result<(), Error> {
        let Some(format) = card_format(&self.header) else {
            return Ok(());
        };
        check_system_area(&self.header)?;
        for (index, part) in self.parts.iter().enumerate() {
            let path = format!("parts[{index}]");
            check_dirent(part, format, &path)?;
            if part.kind == PartKind::Bundle
                && let Some(name) = part.path.as_deref()
            {
                check_spelling(name, format, &format!("{path}.path"))?;
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
                if let Some(&index) = positions(PartKind::System).first() {
                    return refuse(index, "state", SYSTEM_ONLY_ON_A_CARD);
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
                // A card's saves are its `bundle` parts, so a bare one is a save flattened out of
                // the bundle it belongs in.
                if *shape == Shape::Card
                    && let Some(&index) = positions(PartKind::Save).first()
                {
                    return refuse(index, "card", BARE_SAVE_ON_A_CARD);
                }
                // A device's components and a collection's entries are bundles, and nothing else.
                if matches!(shape, Shape::Device | Shape::Collection)
                    && let Some(index) = self.parts.iter().position(|part| part.kind != PartKind::Bundle)
                {
                    return refuse(index, name, ONLY_BUNDLES);
                }
                // An entry belonging to no game is the card's, so only a card has one. A device
                // holds it inside the card it came off, which is where a writer looks for it.
                if *shape != Shape::Card
                    && let Some(&index) = positions(PartKind::System).first()
                {
                    return refuse(index, name, SYSTEM_ONLY_ON_A_CARD);
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

/// The format of the card a bundle is, where it is one: what its entries are held to.
pub(crate) fn card_format(header: &Header) -> Option<&str> {
    header.card.as_ref().filter(|_| header.shape == Shape::Card).map(|card| card.format.as_str())
}

/// A card's `system_area` is as long as its format makes one.
fn check_system_area(header: &Header) -> Result<(), Error> {
    let Some(card) = header.card.as_ref().filter(|_| header.shape == Shape::Card) else {
        return Ok(());
    };
    let format = card.format.as_str();
    if let (Some(expected), Some(area)) = (system_area_len(format), &card.system_area)
        && area.len() != expected
    {
        return Err(Error::at(
            "header.card.system_area",
            ErrorKind::SystemAreaWrongLength { format: format.to_owned(), expected, found: area.len() },
        ));
    }
    Ok(())
}

/// Why a `system` part is refused anywhere but on a card.
const SYSTEM_ONLY_ON_A_CARD: &str = "holds no `system` part; an entry belonging to no game is a card's";

/// Why a part with no `kind` is refused on a card.
const BARE_SAVE_ON_A_CARD: &str = "holds each save as a `bundle` part, never as a bare save part";

/// Why anything but a `bundle` part is refused on a device or a collection.
const ONLY_BUNDLES: &str = "holds `bundle` parts and nothing else";

/// The rules that tie one part to the parts before it, beyond its address: a device's roles, a
/// card's socket, an N64 cartridge's one chip and a 64DD disk's one area.
///
/// [`Bundle::validate`] walks every part through one of these, and [`Checker`] keeps one as the
/// parts go by, so the two cannot disagree.
struct Across {
    /// The shape the header named.
    shape: Shape,
    /// The header's `system`.
    system: Option<String>,
    /// Each role a device's part has taken, to the `id` that took it.
    roles: HashMap<Slug, u64>,
    /// The socket a card's first part named, once one has gone by.
    card_socket: Socket,
    /// The `id` of the part with the role `cartridge` in a bundle naming `n64`.
    cartridge: Option<u64>,
    /// Whether a 64DD disk's save has held its area yet.
    area: bool,
}

impl Across {
    fn new(header: &Header) -> Self {
        Across {
            shape: header.shape.clone(),
            system: header.system.as_ref().map(|system| system.as_str().to_owned()),
            roles: HashMap::new(),
            card_socket: Socket::Unseen,
            cartridge: None,
            area: false,
        }
    }

    /// Checks one part, whose payload is `len` bytes, against the ones before it.
    fn part(&mut self, part: &Part, len: u64, path: &str) -> Result<(), Error> {
        match &self.shape {
            Shape::Device => {
                if let Some(role) = &part.role
                    && let Some(first) = self.roles.insert(role.clone(), part.id)
                {
                    return Err(Error::at(path, ErrorKind::RoleShared { first, second: part.id }));
                }
            }
            // A card sat in one slot when it was read, so its parts name one socket or none.
            Shape::Card => match &self.card_socket {
                Socket::Unseen => self.card_socket = Socket::Named(part.role.clone()),
                Socket::Named(role) if *role != part.role => {
                    return Err(Error::at(path, ErrorKind::CardRolesDisagree));
                }
                Socket::Named(_) => {}
            },
            // A save on a card is a save of its own, so no part of one names a card's slot.
            Shape::Save | Shape::State => {
                if let Some(role) = part.role.as_ref().filter(|role| names_a_card_slot(role.as_str())) {
                    return Err(Error::at(
                        format!("{path}.role"),
                        ErrorKind::CardSlotOnASavePart(role.as_str().to_owned()),
                    ));
                }
            }
            _ => {}
        }

        if self.system.as_deref() == Some("n64")
            && part.role.as_ref().is_some_and(|role| role.as_str() == "cartridge")
        {
            if let Some(first) = self.cartridge {
                return Err(Error::at(path, ErrorKind::SecondCartridge { first, second: part.id }));
            }
            self.cartridge = Some(part.id);
        }

        // A 64DD disk's save holds its RAM area as its one `save` part, with no role, which is how
        // a decoder finds it; the area's length is how a consumer tells the disk's type.
        if self.shape == Shape::Save && self.system.as_deref() == Some("n64dd") && part.kind == PartKind::Save
        {
            if self.area {
                return Err(Error::at(
                    path,
                    ErrorKind::N64ddArea("holds its RAM area in one `save` part, and holds another"),
                ));
            }
            self.area = true;
            if part.role.is_some() {
                return Err(Error::at(
                    format!("{path}.role"),
                    ErrorKind::N64ddArea("holds its RAM area in a `save` part with no role"),
                ));
            }
            if !N64DD_AREAS.contains(&len) {
                return Err(Error::at(
                    format!("{path}.payload"),
                    ErrorKind::N64ddWrongLength(usize::try_from(len).unwrap_or(usize::MAX)),
                ));
            }
        }
        Ok(())
    }
}

/// The socket a card's parts name: none yet, or the one its first part named, which may be none.
enum Socket {
    /// No part has gone by.
    Unseen,
    /// The role the first part named.
    Named(Option<Slug>),
}

/// How long a 64DD disk's RAM area is for each disk type, 0 to 5. A type 6 disk has none.
const N64DD_AREAS: [u64; 6] = [38_444_480, 29_501_120, 21_303_040, 13_850_240, 7_142_720, 1_942_080];

/// Whether a role names a memory card's slot: `memcard-<n>`, `controller-pak-<n>` or
/// `neogeo-card`.
///
/// A prefix covers a whole number from 1 up with no leading zero, so `memcard-01` is no slot this
/// version knows, and is not refused for being one.
fn names_a_card_slot(role: &str) -> bool {
    let numbered = |prefix: &str| {
        role.strip_prefix(prefix).is_some_and(|number| {
            !number.is_empty() && !number.starts_with('0') && number.bytes().all(|byte| byte.is_ascii_digit())
        })
    };
    role == "neogeo-card" || numbered("memcard-") || numbered("controller-pak-")
}

/// Where `x.1sav.n64` belongs, as an error names it.
const N64_PLACE: &str = "the `cartridge` save part of a bundle naming `n64`";

/// Checks the extension keys on a part, which follow its payload: `x.1sav.n64` where an N64
/// cartridge's chip is, and nowhere else.
///
/// It is the one extension value a decoder looks inside, since the chip fixes how long the payload
/// beside it is. Every other key is round-tripped wherever it sits.
pub(crate) fn check_part_extensions(
    header: &Header,
    kind: &PartKind,
    role: Option<&Slug>,
    len: u64,
    extensions: &Extensions,
    path: &str,
) -> Result<(), Error> {
    let chip_part = header.system.as_ref().is_some_and(|system| system.as_str() == "n64")
        && *kind == PartKind::Save
        && role.is_some_and(|role| role.as_str() == "cartridge");
    let value = extensions.cbor(&N64::KEY);
    if !chip_part {
        return match value {
            Some(_) => Err(n64_out_of_place(path)),
            None => Ok(()),
        };
    }
    let Some(value) = value else {
        return Err(Error::at(path, ErrorKind::N64ChipMissing));
    };
    let chip = n64_chip(&value).map_err(|kind| Error::at(format!("{path}[\"x.1sav.n64\"]"), kind))?;
    if len != chip.size() as u64 {
        return Err(Error::at(
            format!("{path}.payload"),
            ErrorKind::N64PayloadWrongLength {
                chip: chip.as_str(),
                expected: chip.size(),
                found: usize::try_from(len).unwrap_or(usize::MAX),
            },
        ));
    }
    Ok(())
}

/// The chip an `x.1sav.n64` value names: a map whose key 0 is a chip the key's page lists.
///
/// Any other integer key is a later version's, and is round-tripped rather than refused; a chip a
/// later version lists is not, since nothing could say how long its payload is.
fn n64_chip(value: &CBOR) -> Result<N64Save, ErrorKind> {
    let unknown = |what: String| ErrorKind::N64ChipUnknown(what);
    let map = value.as_map().ok_or_else(|| unknown("it is not a map".to_owned()))?;
    let mut chip = None;
    for (key, value) in map.iter() {
        match key.as_case() {
            CBORCase::Unsigned(0) => {
                let name = value.as_text().ok_or_else(|| unknown("its chip is not text".to_owned()))?;
                chip = Some(N64Save::from_name(name).ok_or_else(|| unknown(format!("`{name}` is none")))?);
            }
            CBORCase::Unsigned(_) => {}
            _ => return Err(unknown("it has a key that is not an unsigned integer".to_owned())),
        }
    }
    chip.ok_or_else(|| unknown("it has no key 0".to_owned()))
}

fn n64_out_of_place(path: &str) -> Error {
    Error::at(
        format!("{path}[\"x.1sav.n64\"]"),
        ErrorKind::KeyOutOfPlace { key: N64::KEY.as_str().to_owned(), allowed: N64_PLACE },
    )
}

/// `x.1sav.n64` on a header, in a `game` map or in `taken_with`, none of which holds a chip.
fn check_no_n64(extensions: &Extensions, path: &str) -> Result<(), Error> {
    if extensions.contains_key(&N64::KEY) { Err(n64_out_of_place(path)) } else { Ok(()) }
}

/// The rules on one part's own keys: what it may carry for its kind, what it may not repeat of the
/// header, and what is never empty.
fn check_part_fields(part: &Part, header: &Header, path: &str) -> Result<(), Error> {
    // A part's `game` or `source` replaces the header's whole, so one equal to it is the header's
    // spelled out again: on a card of one game, an index copy on every save.
    if part.game.is_some() && part.game == header.game {
        return Err(Error::at(format!("{path}.game"), ErrorKind::RepeatsTheHeader("game")));
    }
    if part.source.is_some() && part.source == header.source {
        return Err(Error::at(format!("{path}.source"), ErrorKind::RepeatsTheHeader("source")));
    }
    if let Some(source) = &part.source {
        check_source(source, &format!("{path}.source"))?;
    }
    if let Some(game) = &part.game {
        check_game(game, &format!("{path}.game"))?;
    }
    if let Some(found) = &part.path {
        check_path(found, &format!("{path}.path"))?;
    }
    if part.dirent.as_ref().is_some_and(Vec::is_empty) {
        return Err(Error::at(format!("{path}.dirent"), ErrorKind::EmptyContainer));
    }
    if part.content_type.as_ref().is_some_and(String::is_empty) {
        return Err(Error::at(format!("{path}.content_type"), ErrorKind::EmptyContainer));
    }

    // Key 12 indexes a nested bundle's `system`, so only a `bundle` part has one to copy.
    if part.system.is_some() && part.kind != PartKind::Bundle {
        return Err(Error::at(
            path,
            ErrorKind::KeyNotOnPart { kind: kind_name(&part.kind), field: "system" },
        ));
    }
    match part.kind {
        // Its media type is this format's by construction, and a binding belongs on the part
        // inside that holds the bound bytes.
        PartKind::Bundle => {
            for (field, present) in [
                ("content_type", part.content_type.is_some()),
                ("binding", part.binding.is_some()),
                ("bound_to", part.bound_to.is_some()),
            ] {
                if present {
                    return Err(Error::at(path, ErrorKind::KeyNotOnPart { kind: "bundle", field }));
                }
            }
        }
        // A whole card belongs to no one game, and is no entry in a directory.
        PartKind::CardImage => {
            for (field, present) in [("game", part.game.is_some()), ("dirent", part.dirent.is_some())] {
                if present {
                    return Err(Error::at(path, ErrorKind::KeyNotOnPart { kind: "card-image", field }));
                }
            }
        }
        _ => {}
    }
    check_bound(part, path)
}

/// `bound_to` goes beside `binding`, and names only what that binding binds a payload to.
fn check_bound(part: &Part, path: &str) -> Result<(), Error> {
    let Some(bound_to) = &part.bound_to else { return Ok(()) };
    let path = format!("{path}.bound_to");
    let Some(binding) = &part.binding else {
        return Err(Error::at(path, ErrorKind::BoundToWithoutBinding));
    };
    if bound_to.is_empty()
        || [&bound_to.device, &bound_to.account, &bound_to.medium].into_iter().flatten().any(Vec::is_empty)
    {
        return Err(Error::at(path, ErrorKind::EmptyContainer));
    }
    // A binding a later version added names targets this one cannot know.
    let Some((_, keys)) = BINDINGS.iter().find(|(name, _)| *name == binding.as_str()) else {
        return Ok(());
    };
    if let Some(key) = bound_to.keys().find(|key| !keys.contains(key)) {
        return Err(Error::at(
            path,
            ErrorKind::BoundToNamesAnotherTarget { binding: binding.as_str().to_owned(), key },
        ));
    }
    Ok(())
}

/// A `source` names something, and every text in it is filled.
fn check_source(source: &Source, path: &str) -> Result<(), Error> {
    if source.is_empty() {
        return Err(Error::at(path, ErrorKind::EmptyContainer));
    }
    for (field, text) in [
        ("fingerprint", &source.fingerprint),
        ("app_version", &source.app_version),
        ("core_version", &source.core_version),
        ("platform_version", &source.platform_version),
    ] {
        if text.as_ref().is_some_and(String::is_empty) {
            return Err(Error::at(format!("{path}.{field}"), ErrorKind::EmptyContainer));
        }
    }
    if source.utc_offset == Some(0) {
        return Err(Error::at(format!("{path}.utc_offset"), ErrorKind::DefaultSpelledOut("UTC")));
    }
    Ok(())
}

/// The name a kind is written as, for an error to name it by.
fn kind_name(kind: &PartKind) -> &'static str {
    match kind {
        PartKind::Save => "save",
        PartKind::State => "state",
        PartKind::Screenshot => "screenshot",
        PartKind::CardImage => "card-image",
        PartKind::Bundle => "bundle",
        PartKind::System => "system",
        PartKind::Aux | PartKind::Other(_) => "aux",
    }
}

/// A part's kind and the address a consumer names it by: its `role`, `path` and `slot`.
type Address = (PartKind, Option<crate::name::Slug>, Option<String>, Option<u64>);

/// The same rules [`Bundle::validate`] checks, applied a part at a time as a streaming reader meets
/// them.
///
/// Each rule is judged the moment everything it needs has arrived: the header's on its own, a
/// part's from the keys ahead of its payload, its extension keys once they follow, and the one
/// rule that needs every part — a state holds a `state` part — at the end. The rules themselves
/// are the functions `validate` calls; what is restated here is only the bookkeeping that carries
/// one part's facts to the next, which `validate` does by walking the whole list.
pub(crate) struct Checker {
    /// The shape the header named.
    shape: Shape,
    /// The card's format, where the bundle is a card.
    card_format: Option<String>,
    /// The `id` of the part before, which the next one has to exceed.
    previous: Option<u64>,
    /// How many `state` parts have gone by.
    states: u64,
    /// How many `screenshot` parts have gone by.
    screenshots: u64,
    /// Every part's address so far, by kind, to the `id` that took it.
    addresses: HashMap<Address, u64>,
    /// Every `system` part's path so far, which is where a later one may sit.
    laid_down: HashSet<String>,
    /// The header, which a part may not repeat and which says what an N64 part is.
    header: Header,
    /// What ties each part to the ones before it.
    across: Across,
}

impl Checker {
    /// Checks a header on its own, which is everything `validate` says about one without its parts.
    pub(crate) fn new(header: &Header) -> Result<Self, Error> {
        check_header(header)?;
        check_system_area(header)?;
        Ok(Checker {
            shape: header.shape.clone(),
            card_format: card_format(header).map(str::to_owned),
            previous: None,
            states: 0,
            screenshots: 0,
            addresses: HashMap::new(),
            laid_down: HashSet::new(),
            header: header.clone(),
            across: Across::new(header),
        })
    }

    /// The shape the header named.
    pub(crate) fn shape(&self) -> &Shape {
        &self.shape
    }

    /// The format of the card the bundle is, where it is one.
    pub(crate) fn card_format(&self) -> Option<&str> {
        self.card_format.as_deref()
    }

    /// Checks how many parts the bundle declares, which only a screenshot bounds.
    pub(crate) fn parts(&self, count: u64) -> Result<(), Error> {
        if self.shape == Shape::Screenshot && count > 1 {
            return Err(Error::at(
                "parts[1]",
                ErrorKind::PartNotInShape {
                    shape: "screenshot",
                    reason: "holds one `screenshot` part and nothing else",
                },
            ));
        }
        Ok(())
    }

    /// Checks a part's keys against the header and the parts before it.
    ///
    /// `part` carries everything ahead of its payload, and `len` is how long its payload is; its
    /// payload's bytes, extension keys and unknown keys are not looked at.
    pub(crate) fn part(&mut self, index: u64, part: &Part, len: u64) -> Result<(), Error> {
        let path = format!("parts[{index}]");
        if let Some(previous) = self.previous {
            if previous == part.id {
                return Err(Error::at(&path, ErrorKind::DuplicatePartId(part.id)));
            }
            if previous > part.id {
                return Err(Error::at("parts", ErrorKind::PartsOutOfOrder));
            }
        }
        self.previous = Some(part.id);

        self.check_shape(index, part)?;

        if self.shape != Shape::Collection {
            let address = (part.kind.clone(), part.role.clone(), part.path.clone(), part.slot);
            if let Some(&first) = self.addresses.get(&address) {
                return Err(Error::at(&path, ErrorKind::PartsShareAnAddress { first, second: part.id }));
            }
            self.addresses.insert(address, part.id);
        }

        if part.kind == PartKind::System
            && let Some(entry) = part.path.as_deref()
        {
            if let Some((directory, _)) = entry.rsplit_once('/')
                && !self.laid_down.contains(directory)
            {
                return Err(Error::at(
                    &path,
                    ErrorKind::SystemEntryBeforeItsDirectory { directory: directory.to_owned() },
                ));
            }
            self.laid_down.insert(entry.to_owned());
        }

        if let Some(format) = &self.card_format {
            check_dirent(part, format, &path)?;
            if part.kind == PartKind::Bundle
                && let Some(name) = part.path.as_deref()
            {
                check_spelling(name, format, &format!("{path}.path"))?;
            }
        }

        check_part_kind(part, &path)?;
        check_part_fields(part, &self.header, &path)?;
        self.across.part(part, len, &path)
    }

    /// What [`check_parts_against_shape`](Bundle::check_parts_against_shape) says about this one
    /// part, given the ones before it.
    fn check_shape(&mut self, index: u64, part: &Part) -> Result<(), Error> {
        let refuse = |shape: &'static str, reason: &'static str| {
            Err(Error::at(format!("parts[{index}]"), ErrorKind::PartNotInShape { shape, reason }))
        };
        match &self.shape {
            Shape::Unknown(_) => {}
            Shape::State => match part.kind {
                PartKind::State => {
                    self.states += 1;
                    if self.states > 1 {
                        return refuse("state", "holds exactly one `state` part");
                    }
                }
                PartKind::Screenshot => {
                    self.screenshots += 1;
                    if self.screenshots > 1 {
                        return refuse("state", "holds at most one `screenshot` part");
                    }
                }
                PartKind::Bundle => return refuse("state", "holds no nested bundle"),
                PartKind::System => return refuse("state", SYSTEM_ONLY_ON_A_CARD),
                _ => {}
            },
            Shape::Screenshot => {
                // A second part is refused by `parts`, before any part arrives.
                if index > 0 || part.kind != PartKind::Screenshot {
                    return refuse("screenshot", "holds one `screenshot` part and nothing else");
                }
            }
            shape @ (Shape::Save | Shape::Card | Shape::Device | Shape::Collection) => {
                let name = shape_name(shape);
                match part.kind {
                    PartKind::State => {
                        return refuse(name, "holds no `state` part; a save state is a bundle of its own");
                    }
                    PartKind::Screenshot => {
                        return refuse(
                            name,
                            "holds no `screenshot` part; a screenshot is a bundle of its own",
                        );
                    }
                    PartKind::Bundle if *shape == Shape::Save => {
                        return refuse("save", "holds no nested bundle");
                    }
                    PartKind::System if *shape != Shape::Card => return refuse(name, SYSTEM_ONLY_ON_A_CARD),
                    PartKind::Save if *shape == Shape::Card => return refuse("card", BARE_SAVE_ON_A_CARD),
                    PartKind::Bundle => {}
                    _ if matches!(shape, Shape::Device | Shape::Collection) => {
                        return refuse(name, ONLY_BUNDLES);
                    }
                    _ => {}
                }
                if *shape == Shape::Device && part.role.is_none() {
                    return refuse("device", "names a role on every part");
                }
            }
        }
        Ok(())
    }

    /// Checks the extension keys that follow a part's payload, which was `len` bytes.
    pub(crate) fn part_extensions(
        &self,
        index: u64,
        kind: &PartKind,
        role: Option<&Slug>,
        len: u64,
        extensions: &Extensions,
    ) -> Result<(), Error> {
        check_part_extensions(&self.header, kind, role, len, extensions, &format!("parts[{index}]"))
    }

    /// Checks what takes every part to say, once the last has gone by.
    pub(crate) fn finish(&self) -> Result<(), Error> {
        if self.shape == Shape::State && self.states == 0 {
            return Err(Error::at(
                "parts",
                ErrorKind::PartNotInShape { shape: "state", reason: "holds exactly one `state` part" },
            ));
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

    check_no_n64(&header.extensions, "header")?;
    if header.description.as_ref().is_some_and(String::is_empty) {
        return Err(Error::at("header.description", ErrorKind::EmptyContainer));
    }
    if let Some(source) = &header.source {
        check_source(source, "header.source")?;
    }
    if let Some(game) = &header.game {
        check_game(game, "header.game")?;
    }
    if let Some(taken_with) = &header.taken_with {
        check_taken_with(taken_with, "header.taken_with")?;
    }
    if header.card.as_ref().and_then(|card| card.system_area.as_ref()).is_some_and(Vec::is_empty) {
        return Err(Error::at("header.card.system_area", ErrorKind::EmptyContainer));
    }
    Ok(())
}

/// How long a `dirent` is on each card format this version lists, as the memory cards
/// specification fixes it.
fn dirent_len(format: &str) -> Option<usize> {
    match format {
        "ps1-mc" => Some(128),
        "ps2-mc" => Some(512),
        "n64-cpak" | "vmu" => Some(32),
        "gc-mc" => Some(64),
        "neogeo-mc" => Some(4),
        "saturn-bup" => Some(30),
        _ => None,
    }
}

/// How long a card's `system_area` is, on the one format that fixes it: a Neo Geo card's is its
/// whole header block, region and all.
fn system_area_len(format: &str) -> Option<usize> {
    (format == "neogeo-mc").then_some(64)
}

/// A part's entry is as long as its card's format makes one.
pub(crate) fn check_dirent(part: &Part, format: &str, path: &str) -> Result<(), Error> {
    if let (Some(expected), Some(dirent)) = (dirent_len(format), &part.dirent)
        && dirent.len() != expected
    {
        return Err(Error::at(
            format!("{path}.dirent"),
            ErrorKind::DirentWrongLength { format: format.to_owned(), expected, found: dirent.len() },
        ));
    }
    Ok(())
}

/// Every character an N64 pak's table has, from `0x0F` to `0x94`, which is all a note's name and
/// extension can be spelled in. The table is N64brew's, as the memory cards specification gives
/// it.
const N64_CHARACTERS: &str = concat!(
    " 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ!\"#'*+,-./:=?@",
    "。゛゜ァィゥェォッャュョヲン",
    "アイウエオカキクケコサシスセソタチツテトナニヌネノハヒフヘホマミムメモヤユヨラリルレロワ",
    "ガギグゲゴザジズゼゾダヂヅデドバビブベボパピプペポ",
);

/// A save's name on a card that holds something other than text for one is spelled the way the
/// specification spells it there.
fn check_spelling(name: &str, format: &str, path: &str) -> Result<(), Error> {
    let (spelled, spelling) = match format {
        "neogeo-mc" => (
            is_neogeo_save_name(name),
            "the NGH number in four lowercase hex digits, a dash, then the sub-number in one",
        ),
        "n64-cpak" => (
            is_n64_note_name(name),
            "a name of up to 16 characters of the pak's table, then a dot and up to 4 more where the \
             note has an extension",
        ),
        _ => return Ok(()),
    };
    if spelled {
        Ok(())
    } else {
        Err(Error::at(path, ErrorKind::PathMisspelled { format: format.to_owned(), spelling }))
    }
}

/// `0201-0`: Metal Slug's NGH number, then which of its saves this is.
fn is_neogeo_save_name(name: &str) -> bool {
    let hex = |byte: &u8| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte);
    match name.as_bytes() {
        [ngh @ .., b'-', sub] => ngh.len() == 4 && ngh.iter().all(hex) && hex(sub),
        _ => false,
    }
}

/// A note's 16-character name, then a dot and its extension of up to 4 where it has one.
///
/// A dot is in the table too, so a name can hold one, and a spelling with a dot in it is a note's
/// when any one dot splits it into a name and an extension that fit.
fn is_n64_note_name(name: &str) -> bool {
    const NAME: usize = 16;
    const EXTENSION: usize = 4;
    let characters: Vec<char> = name.chars().collect();
    characters.iter().all(|&character| N64_CHARACTERS.contains(character))
        && (characters.len() <= NAME
            || characters.iter().enumerate().any(|(at, &character)| {
                character == '.' && at <= NAME && (1..=EXTENSION).contains(&(characters.len() - at - 1))
            }))
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
        PartKind::System => return check_system_part(part, path),
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
        ("bound_to", part.bound_to.is_some()),
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

/// Checks the keys a `system` part takes.
///
/// Its `path` is the only thing that says where on the card the entry goes, so it is required.
/// And it names no game, because belonging to none is what makes it a `system` part rather than a
/// save: one that named a game would be a save filed under the wrong kind.
fn check_system_part(part: &Part, path: &str) -> Result<(), Error> {
    if part.path.is_none() {
        return Err(Error::at(path, ErrorKind::MissingKey("path")));
    }
    // No game, since it belongs to none, and no `system`, which only indexes a nested bundle.
    if part.game.is_some() {
        return Err(Error::at(path, ErrorKind::KeyNotOnPart { kind: "system", field: "game" }));
    }
    if part.system.is_some() {
        return Err(Error::at(path, ErrorKind::KeyNotOnPart { kind: "system", field: "system" }));
    }
    Ok(())
}

/// Checks a `game` map, wherever it sits.
///
/// It describes a game rather than one save of it, so it holds no N64 chip, and nothing in it is
/// empty.
fn check_game(game: &Game, path: &str) -> Result<(), Error> {
    if game.is_empty() {
        return Err(Error::at(path, ErrorKind::EmptyContainer));
    }
    for (field, text) in
        [("rom_filename", &game.rom_filename), ("serial", &game.serial), ("title", &game.title)]
    {
        if text.as_ref().is_some_and(String::is_empty) {
            return Err(Error::at(format!("{path}.{field}"), ErrorKind::EmptyContainer));
        }
    }
    if game.game_id.values().any(|id| matches!(id, crate::GameId::Text(text) if text.is_empty())) {
        return Err(Error::at(format!("{path}.game_id"), ErrorKind::EmptyContainer));
    }
    check_hashes(&game.rom_hashes, &format!("{path}.rom_hashes"))?;
    check_no_n64(&game.extensions, path)
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
    check_no_n64(&taken_with.extensions, path)
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

/// Whether a bundle of one shape may hold a bundle of another, for two shapes this version defines.
pub(crate) fn check_holds(outer: &Shape, inner: &Shape) -> Result<(), ErrorKind> {
    let (holds, shape, reason) = match outer {
        Shape::Save | Shape::State | Shape::Screenshot => {
            (false, shape_name(outer), "holds no nested bundle")
        }
        Shape::Card => (matches!(inner, Shape::Save), "card", "holds saves and nothing else"),
        Shape::Device => (matches!(inner, Shape::Card | Shape::Save), "device", "holds cards and saves"),
        Shape::Collection => {
            (!matches!(inner, Shape::Collection), "collection", "does not hold another collection")
        }
        Shape::Unknown(_) => return Ok(()),
    };
    if holds { Ok(()) } else { Err(ErrorKind::PartNotInShape { shape, reason }) }
}

/// Checks the rules a `bundle` part carries, all of them past what a schema can see.
fn check_nested(part: &Part, header: &Header, path: &str) -> Result<(), Error> {
    let outer = &header.shape;
    let bytes = &part.payload;

    let inner = Bundle::from_cbor(
        &crate::codec::decode_cbor(bytes).map_err(|kind| Error::at(path, kind))?,
        Strictness::Decoder,
    )
    .map_err(|e| Error::at(path, ErrorKind::NestedPayloadNotABundle(Box::new(e))))?;

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
    check_holds(outer, inner.shape()).map_err(|kind| Error::at(path, kind))?;

    // A save made of several files carries an entry on each, and those are the card's entries as
    // much as the one on the outer part is.
    if let Some(card) = header.card.as_ref().filter(|_| *outer == Shape::Card) {
        for (index, inner_part) in inner.parts.iter().enumerate() {
            check_dirent(inner_part, card.format.as_str(), &format!("{path}.payload.parts[{index}]"))?;
        }
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
    fn a_bundle_of_very_many_parts_is_checked_in_linear_time() {
        // The part count is the file's to choose. Comparing every pair of parts, this many kept a
        // decoder busy for minutes; it is under a second with a set, and still finds the duplicate.
        let count = 100_000u64;
        let mut parts: Vec<Part> = (0..count).map(|id| Part::new(id, *b"a").slot(id)).collect();
        let started = std::time::Instant::now();
        assert!(bundle_of(parts.clone()).validate().is_ok());

        parts.last_mut().unwrap().slot = Some(0);
        assert_eq!(
            bundle_of(parts).validate().unwrap_err().kind(),
            &ErrorKind::PartsShareAnAddress { first: 0, second: count - 1 }
        );
        assert!(started.elapsed().as_secs() < 30, "took {:?}", started.elapsed());
    }

    #[test]
    fn a_nested_bundle_nested_without_end_is_an_error_and_not_an_abort() {
        // A `bundle` part's payload is decoded during validation, so the depth limit has to hold
        // there as well as at the top: a few kilobytes of array heads overflowed the stack.
        let mut payload = vec![0x81; 100_000];
        payload.push(0x00);
        let part = Part::new(0, payload).kind(PartKind::Bundle);
        let error = collection_of(vec![part]).validate().unwrap_err();
        assert_eq!(error.kind(), &ErrorKind::TooDeep { limit: crate::codec::MAX_NESTING });
    }

    #[test]
    fn a_role_is_enough_to_tell_two_parts_apart() {
        let mut parts = vec![Part::new(0, *b"a"), Part::new(1, *b"b")];
        parts[1].role = Some(Slug::parse("cartridge").unwrap());
        assert!(bundle_of(parts).validate().is_ok());
    }

    #[test]
    fn an_extension_key_is_kept_wherever_it_sits() {
        // Since 0.10 the rules on an extension's page bind producers only: a decoder round-trips
        // the key whatever it holds and wherever it sits. A clock reading on a `bundle` part, in a
        // `game` map, on a state part or in `taken_with`, and mGBA's cheats on a header, are each
        // somewhere a producer should not put them, and none makes the bundle malformed.
        use crate::extension::{CheatSet, CheatsMgba};
        let key = |name: &str| crate::ReverseDnsName::parse(name).unwrap();
        let reading = || dcbor::CBOR::from(1u64);

        for name in ["x.1sav.rtc", "x.1sav.rtc.mbc3", "x.1sav.rtc.future-chip"] {
            let mut outer = nested_save(0);
            outer.extensions.insert_cbor(key(name), reading());
            collection_of(vec![outer]).validate().unwrap_or_else(|e| panic!("{name} on a bundle part: {e}"));
        }

        let mut game = Game { title: Some("Pokemon Ruby".into()), ..Game::default() };
        game.extensions.insert_cbor(key("x.1sav.rtc"), reading());
        let mut save = bundle_of(vec![Part::new(0, *b"SAVE")]);
        save.header.game = Some(game);
        save.validate().expect("a clock reading in a game map");

        let mut part = state_part(0);
        part.extensions.insert_cbor(key("x.1sav.rtc.s3511a"), reading());
        state_of(vec![part]).validate().expect("a clock on a state part");

        let mut taken_with = TakenWith::default();
        taken_with.extensions.insert_cbor(key("x.1sav.rtc"), reading());
        let mut state = state_of(vec![state_part(0)]);
        state.header.taken_with = Some(taken_with);
        state.validate().expect("a clock reading in taken_with");

        let mut state = state_of(vec![state_part(0)]);
        state.header.extensions.insert(CheatsMgba::new(vec![CheatSet::new(["D8BAE4D9 4864DCE5"])]));
        state.validate().expect("mGBA's cheats on a state's header");
    }

    #[test]
    fn a_device_reads_the_consoles_clock_on_its_header() {
        // A console's own clock belongs to no one game, so a device carries it.
        let mut device = Bundle {
            header: Header::new(Shape::Device).system(Slug::parse("neogeo").unwrap()),
            parts: vec![nested_save(0).role(Slug::parse("internal").unwrap())],
        };
        device.header.extensions.insert(crate::extension::Rtc::new(987_251_696));
        device.validate().expect("an MVS board's clock on its device");
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
    fn a_card_a_device_and_a_collection_were_never_taken() {
        // Each is read whole from storage at whatever moment, and a collection's entries keep
        // their own, so none of the three has a moment of one machine to name.
        let card = Bundle {
            header: Header::new(Shape::Card).card(crate::Card::new(Slug::parse("ps1-mc").unwrap(), 122_880)),
            parts: vec![nested_save(0)],
        };
        let mut device = Bundle {
            header: Header::new(Shape::Device).system(Slug::parse("n64").unwrap()),
            parts: vec![nested_save(0)],
        };
        device.parts[0].role = Some(Slug::parse("cartridge").unwrap());
        for bundle in [card, device, collection_of(vec![nested_save(0)])] {
            let shape = shape_name(&bundle.header.shape);
            bundle.validate().unwrap_or_else(|e| panic!("a {shape} with nothing taken: {e}"));
            let refused = |header: Header, field: &'static str| {
                assert_eq!(
                    Bundle { header, ..bundle.clone() }.validate().unwrap_err().kind(),
                    &ErrorKind::KeyNotInShape { shape, field },
                    "{field} on a {shape}"
                );
            };
            refused(bundle.header.clone().taken_at(1), "taken_at");
            refused(bundle.header.clone().taken_with(TakenWith::default().language("ja")), "taken_with");
            refused(bundle.header.clone().origin(crate::Origin::Auto), "origin");
        }
    }

    /// A PS2 card with its network settings on it: a save, then the folder and its one file.
    fn card_with_a_system_folder() -> Bundle {
        Bundle {
            header: Header::new(Shape::Card)
                .card(crate::Card::new(Slug::parse("ps2-mc").unwrap(), 8_388_608)),
            parts: vec![
                nested_save(0),
                Part::new(1, Vec::new()).kind(PartKind::System).path("BWNETCNF"),
                Part::new(2, *b"NET").kind(PartKind::System).path("BWNETCNF/BWNETCNF"),
            ],
        }
    }

    #[test]
    fn a_card_carries_entries_belonging_to_no_game_beside_its_saves() {
        card_with_a_system_folder().validate().expect("a card holds system entries");

        // The path is the whole of where the entry goes, so without one there is nowhere.
        let mut pathless = card_with_a_system_folder();
        pathless.parts[2].path = None;
        assert_eq!(pathless.validate().unwrap_err().kind(), &ErrorKind::MissingKey("path"));

        // And one naming a game is a save filed under the wrong kind.
        let mut claimed = card_with_a_system_folder();
        claimed.parts[2].game = Some(Game::default().serial("SCUS-97129"));
        assert_eq!(
            claimed.validate().unwrap_err().kind(),
            &ErrorKind::KeyNotOnPart { kind: "system", field: "game" }
        );

        // `system` indexes a nested bundle, and an entry is not one.
        let mut indexed = card_with_a_system_folder();
        indexed.parts[1].system = Some(Slug::parse("ps2").unwrap());
        assert_eq!(
            indexed.validate().unwrap_err().kind(),
            &ErrorKind::KeyNotOnPart { kind: "system", field: "system" }
        );
    }

    /// A card of this format holding one save, addressed by this name and carrying this entry.
    fn card_of(format: &str, name: &str, dirent: &[u8]) -> Bundle {
        Bundle {
            header: Header::new(Shape::Card).card(crate::Card::new(Slug::parse(format).unwrap(), 32_768)),
            parts: vec![nested_save(0).path(name).dirent(dirent.to_vec())],
        }
    }

    #[test]
    fn an_entry_is_as_long_as_its_card_s_format_makes_one() {
        card_of("n64-cpak", "MARIOKART64", &[0; 32]).validate().expect("a note's entry is 32 bytes");
        assert_eq!(
            card_of("n64-cpak", "MARIOKART64", &[0; 31]).validate().unwrap_err().kind(),
            &ErrorKind::DirentWrongLength { format: "n64-cpak".into(), expected: 32, found: 31 }
        );
        // A format this version does not list fixes nothing.
        card_of("x-mc", "ANYTHING", &[0; 3]).validate().expect("nothing to hold it to");

        // A PS2 save's files carry entries of their own, inside the save, and they are the card's.
        let file = |dirent: Vec<u8>| {
            let inner = Bundle {
                header: Header::default(),
                parts: vec![Part::new(0, *b"DATA").path("BASLUS-20312").dirent(dirent)],
            };
            Bundle {
                header: Header::new(Shape::Card)
                    .card(crate::Card::new(Slug::parse("ps2-mc").unwrap(), 8_388_608)),
                parts: vec![
                    Part::new(0, inner.to_vec().unwrap())
                        .kind(PartKind::Bundle)
                        .path("BASLUS-20312")
                        .dirent(vec![0; 512]),
                ],
            }
        };
        file(vec![0; 512]).validate().expect("a file's entry is 512 bytes too");
        let error = file(vec![0; 128]).validate().unwrap_err();
        assert_eq!(error.path(), "parts[0].payload.parts[0].dirent");
    }

    #[test]
    fn a_neo_geo_card_keeps_its_whole_header_and_spells_each_save_by_its_numbers() {
        let card = |name: &str, area: usize| {
            let mut card = card_of("neogeo-mc", name, &[0, 0x02, 0x01, 5]);
            card.header.card = card.header.card.map(|card| card.system_area(vec![0; area]));
            card
        };
        card("0201-0", 64).validate().expect("Metal Slug's first save");
        card("0250-f", 64).validate().expect("sub-number 15 is one hex digit");
        assert_eq!(
            card("0201-0", 16).validate().unwrap_err().kind(),
            &ErrorKind::SystemAreaWrongLength { format: "neogeo-mc".into(), expected: 64, found: 16 }
        );
        for name in ["0201-A", "201-0", "0201-10", "0201_0", "METAL SLUG"] {
            let error = card(name, 64).validate().unwrap_err();
            assert!(matches!(error.kind(), ErrorKind::PathMisspelled { .. }), "{name}: {error}");
            assert_eq!(error.path(), "parts[0].path");
        }
    }

    #[test]
    fn an_n64_note_is_spelled_through_its_pak_s_table() {
        for name in [
            "MARIOKART64",
            "MARIOKART64.A",
            "GAME.ABCD",
            // Sixteen characters of name and four of extension, the most a note holds.
            "ABCDEFGHIJKLMNOP.QRST",
            // A dot of the name's own, and no extension.
            "MR.DRILLER",
            "GAME.ABCDE",
            // Katakana and its marks are the table's as much as the letters are.
            "ゼルダノデンセツ",
            "POKEMON STADIUM'",
        ] {
            card_of("n64-cpak", name, &[0; 32]).validate().unwrap_or_else(|e| panic!("{name}: {e}"));
        }
        // No lowercase, and nothing past the name's sixteen or the extension's four. A short
        // spelling with a dot is a name holding one, so only a long one can overrun.
        for name in
            ["MarioKart64", "MARIOKART64DELUXE", "ABCDEFGHIJKLMNOP.ABCDE", "ABCDEFGHIJKLMNOPQ.A", "GAME`"]
        {
            let error = card_of("n64-cpak", name, &[0; 32]).validate().unwrap_err();
            assert!(matches!(error.kind(), ErrorKind::PathMisspelled { .. }), "{name}: {error}");
        }
        assert_eq!(N64_CHARACTERS.chars().count(), 0x94 - 0x0F + 1, "the table from 0x0F to 0x94");
    }

    /// A writer lays entries down in part order, so a file's directory comes ahead of it.
    #[test]
    fn a_system_file_comes_after_its_directory() {
        let outside = || ErrorKind::SystemEntryBeforeItsDirectory { directory: "BWNETCNF".into() };

        let mut orphan = card_with_a_system_folder();
        orphan.parts.remove(1);
        assert_eq!(orphan.validate().unwrap_err().kind(), &outside());

        let mut late = card_with_a_system_folder();
        late.parts.swap(1, 2);
        late.parts[1].id = 1;
        late.parts[2].id = 2;
        let error = late.validate().unwrap_err();
        assert_eq!(error.kind(), &outside());
        assert_eq!(error.path(), "parts[1]", "the file, which is where it went wrong");

        // A save's directory is not a system part's to put a file in.
        let mut misfiled = card_with_a_system_folder();
        misfiled.parts[2].path = Some("BASLUS-20312/icon.sys".into());
        assert_eq!(
            misfiled.validate().unwrap_err().kind(),
            &ErrorKind::SystemEntryBeforeItsDirectory { directory: "BASLUS-20312".into() }
        );
    }

    #[test]
    fn only_a_card_carries_an_entry_belonging_to_no_game() {
        let entry = || Part::new(1, *b"NET").kind(PartKind::System).path("BWNETCNF/BWNETCNF");
        let reason = SYSTEM_ONLY_ON_A_CARD;

        let save = bundle_of(vec![Part::new(0, *b"SAVE"), entry()]);
        assert_eq!(save.validate().unwrap_err().kind(), &ErrorKind::PartNotInShape { shape: "save", reason });

        // A device and a collection hold `bundle` parts and nothing else, a `system` part
        // included: it belongs inside the card it came off.
        let mut device = Bundle {
            header: Header::new(Shape::Device).system(Slug::parse("ps2").unwrap()),
            parts: vec![nested_save(0), entry().role(Slug::parse("memcard-1").unwrap())],
        };
        device.parts[0].role = Some(Slug::parse("memcard-2").unwrap());
        assert_eq!(
            device.validate().unwrap_err().kind(),
            &ErrorKind::PartNotInShape { shape: "device", reason: ONLY_BUNDLES }
        );

        let state = state_of(vec![state_part(0), entry()]);
        assert_eq!(
            state.validate().unwrap_err().kind(),
            &ErrorKind::PartNotInShape { shape: "state", reason }
        );

        let collection = collection_of(vec![nested_save(0), entry()]);
        assert_eq!(
            collection.validate().unwrap_err().kind(),
            &ErrorKind::PartNotInShape { shape: "collection", reason: ONLY_BUNDLES }
        );
    }

    #[test]
    fn each_firmware_file_names_its_hashes_once_each_in_tag_order() {
        use crate::hash::HashAlgorithm;
        let with = |firmware: std::collections::BTreeMap<String, Vec<HashValue>>| {
            let mut state = state_of(vec![state_part(0)]);
            state.header.taken_with = Some(TakenWith::default().firmware(firmware));
            state.validate()
        };
        let bios =
            |hashes: Vec<HashValue>| std::collections::BTreeMap::from([("scph5501.bin".to_owned(), hashes)]);
        // sha256 is tag 18540 and sha1 18542, so this is their order.
        let sha256 = HashValue::new(HashAlgorithm::Sha256, vec![1; 32]).unwrap();
        let sha1 = HashValue::new(HashAlgorithm::Sha1, vec![2; 20]).unwrap();
        with(bios(vec![sha256.clone(), sha1.clone()])).expect("two algorithms in tag order");

        // The rules a game's `rom_hashes` keeps, and for the reason it keeps them: two digests of
        // one algorithm name two files, and nothing says which one the machine booted.
        let error = with(bios(vec![sha1.clone(), sha256.clone()])).unwrap_err();
        assert_eq!(error.kind(), &ErrorKind::RomHashesOutOfOrder);
        assert_eq!(error.path(), r#"header.taken_with.firmware["scph5501.bin"]"#, "names the file");
        assert_eq!(
            with(bios(vec![sha256.clone(), HashValue::new(HashAlgorithm::Sha256, vec![3; 32]).unwrap()]))
                .unwrap_err()
                .kind(),
            &ErrorKind::RomHashesDuplicateAlgorithm(HashAlgorithm::Sha256)
        );

        // A file with no hash says nothing a null would not, and no file at all is what absence says.
        assert_eq!(with(bios(vec![])).unwrap_err().kind(), &ErrorKind::EmptyContainer);
        let error = with(std::collections::BTreeMap::new()).unwrap_err();
        assert_eq!((error.kind(), error.path()), (&ErrorKind::EmptyContainer, "header.taken_with.firmware"));
    }

    #[test]
    fn taken_with_carries_anyones_key_but_an_n64_chip() {
        // An extension key there describes the machine. An N64 chip is the one key a decoder
        // places, and it belongs on the cartridge's part.
        let with = |key: &str| {
            let mut taken_with = TakenWith::default();
            taken_with
                .extensions
                .insert_cbor(crate::ReverseDnsName::parse(key).unwrap(), dcbor::CBOR::from(1u64));
            let mut state = state_of(vec![state_part(0)]);
            state.header.taken_with = Some(taken_with);
            state.validate()
        };
        with("org.duckstation.renderer").expect("a key of the emulator's own is enough on its own");
        let error = with("x.1sav.n64").unwrap_err();
        assert!(matches!(error.kind(), ErrorKind::KeyOutOfPlace { .. }), "{error}");
        assert!(error.path().starts_with("header.taken_with"), "{error}");
    }

    /// An N64 cartridge's save: one part with the role `cartridge`, holding `len` bytes of `chip`.
    fn n64_save(chip: N64Save, len: usize) -> Bundle {
        let part =
            Part::new(0, vec![0xFF; len]).role(Slug::parse("cartridge").unwrap()).extension(N64::new(chip));
        Bundle { header: Header::default().system(Slug::parse("n64").unwrap()), parts: vec![part] }
    }

    #[test]
    fn an_n64_cartridge_names_its_chip_and_holds_all_of_it() {
        for chip in N64Save::ALL {
            n64_save(chip, chip.size()).validate().unwrap_or_else(|e| panic!("{chip}: {e}"));
        }
        assert_eq!(
            n64_save(N64Save::Eeprom16k, 512).validate().unwrap_err().kind(),
            &ErrorKind::N64PayloadWrongLength { chip: "eeprom-16k", expected: 2048, found: 512 }
        );

        let mut missing = n64_save(N64Save::Eeprom4k, 512);
        missing.parts[0].extensions = crate::Extensions::new();
        assert_eq!(missing.validate().unwrap_err().kind(), &ErrorKind::N64ChipMissing);

        // The key alone is not enough to make a part the chip's: without `n64` it is out of place.
        let mut elsewhere = n64_save(N64Save::Eeprom4k, 512);
        elsewhere.header.system = Some(Slug::parse("gb").unwrap());
        assert!(matches!(elsewhere.validate().unwrap_err().kind(), ErrorKind::KeyOutOfPlace { .. }));

        // A chip's bytes the producer could not tell go as `aux`, with no key and no length.
        let mut unknown = n64_save(N64Save::Flash1m, 1000);
        unknown.parts[0] = Part::new(0, vec![0; 1000]).kind(PartKind::Aux).path("game.sav");
        unknown.validate().expect("a file whose chip nobody could tell");
    }

    #[test]
    fn an_n64_chip_is_a_map_naming_one_the_page_lists() {
        let with = |value: dcbor::CBOR| {
            let mut save = n64_save(N64Save::Eeprom4k, 512);
            save.parts[0].extensions.insert_cbor(N64::KEY, value);
            save.validate()
        };
        let mut later = dcbor::Map::new();
        later.insert(0u64, "eeprom-4k");
        later.insert(1u64, "a later version's");
        with(later.into()).expect("an integer key a later version assigns is round-tripped");

        let mut undefined = dcbor::Map::new();
        undefined.insert(0u64, "eeprom-512b");
        for value in [undefined.into(), dcbor::CBOR::from("eeprom-4k")] {
            assert!(matches!(with(value).unwrap_err().kind(), ErrorKind::N64ChipUnknown(_)));
        }
    }

    #[test]
    fn an_n64_bundle_holds_one_cartridge() {
        let mut two = n64_save(N64Save::Eeprom4k, 512);
        let mut second = two.parts[0].clone().path("again");
        second.id = 1;
        two.parts.push(second);
        assert_eq!(two.validate().unwrap_err().kind(), &ErrorKind::SecondCartridge { first: 0, second: 1 });
    }

    #[test]
    fn a_save_part_never_names_a_cards_slot() {
        for role in ["memcard-1", "controller-pak-4", "neogeo-card"] {
            let part = Part::new(0, *b"NOTE").role(Slug::parse(role).unwrap());
            assert!(
                matches!(
                    bundle_of(vec![part]).validate().unwrap_err().kind(),
                    ErrorKind::CardSlotOnASavePart(_)
                ),
                "{role}"
            );
        }
        // A number this version would not write is a role it does not know, not a slot.
        for role in ["memcard-0", "memcard-01", "transfer-pak-1"] {
            bundle_of(vec![Part::new(0, *b"SAVE").role(Slug::parse(role).unwrap())])
                .validate()
                .unwrap_or_else(|e| panic!("{role}: {e}"));
        }
    }

    #[test]
    fn a_64dd_disks_save_is_its_ram_area_and_nothing_less() {
        let disk = |parts: Vec<Part>| Bundle {
            header: Header::default().system(Slug::parse("n64dd").unwrap()),
            parts,
        };
        disk(vec![Part::new(0, vec![0; 1_942_080])]).validate().expect("a type 5 disk's area");
        assert_eq!(
            disk(vec![Part::new(0, vec![0; 32_768])]).validate().unwrap_err().kind(),
            &ErrorKind::N64ddWrongLength(32_768)
        );
        assert!(matches!(
            disk(vec![Part::new(0, vec![0; 1_942_080]).role(Slug::parse("cartridge").unwrap())])
                .validate()
                .unwrap_err()
                .kind(),
            ErrorKind::N64ddArea(_)
        ));
        let two = vec![Part::new(0, vec![0; 1_942_080]), Part::new(1, vec![0; 1_942_080]).path("b")];
        assert!(matches!(disk(two).validate().unwrap_err().kind(), ErrorKind::N64ddArea(_)));
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
