//! The rules a schema cannot state.
//!
//! CDDL describes the decoded data model, so it sees neither the encoding beneath it nor
//! anything inside a byte string. Everything here is a rule that needs one of those: a
//! comparison across items, or a look into a payload.

use crate::codec::Strictness;
use crate::error::{Error, ErrorKind};
use crate::hash::HashValue;
use crate::model::{Bundle, Part, PartKind, Payload};
use crate::shape::Shape;

impl Bundle {
    /// Checks every structural rule the format states.
    ///
    /// Called for you by [`from_slice`](Bundle::from_slice) and [`to_vec`](Bundle::to_vec), so a
    /// bundle that came from either has already passed.
    ///
    /// # Cost
    ///
    /// Every [`bundle`](crate::PartKind::Bundle) part is decoded here, checked, and then
    /// discarded: the rules about a nested bundle are rules about what its payload decodes to, and
    /// keeping the result would mean either a second in-memory shape for a bundle or making every
    /// caller pay for an inner decode it may not want. A caller that then walks the nested bundles
    /// decodes each of them a second time, which is the expected pattern rather than a mistake —
    /// on a 16 MiB card it is the work twice. [`from_cbor`](Bundle::from_cbor) is the way out: it
    /// does not validate, so it does not step into a payload.
    pub fn validate(&self) -> Result<(), Error> {
        self.validate_inner()
    }

    /// Checks the rules, including what this bundle's shape may hold.
    ///
    /// Nesting is bounded by the shapes rather than by a depth counter: a save holds no bundle,
    /// a card holds saves, a device holds cards and saves, and a collection holds anything but
    /// another collection. That admits the same structures the old cap of 2 did, and says why
    /// rather than counting.
    fn validate_inner(&self) -> Result<(), Error> {
        if self.parts.is_empty() {
            return Err(Error::at("parts", ErrorKind::EmptyContainer));
        }
        self.check_part_order()?;
        self.check_part_addresses()?;

        // A device's components are told apart by `role` and nothing else. Writing it is required
        // rather than advised, because `role` otherwise defaults to `primary`, and two components
        // that both fell back to it would be a device whose halves a consumer cannot distinguish.
        if self.header.shape == Shape::Device
            && let Some(index) = self.parts.iter().position(|part| part.role.is_none())
        {
            return Err(Error::at(
                format!("parts[{index}]"),
                ErrorKind::PartNotInShape { shape: "device", reason: "names a role on every part" },
            ));
        }

        for (index, part) in self.parts.iter().enumerate() {
            let path = format!("parts[{index}]");
            check_rom_hash_order(part, &path)?;
            // On every part, not only a `bundle` one: these keys describe a save, and a save is
            // the bundle rather than the part that carries it.
            check_key_placement(&part.extensions, &path)?;
            if part.kind == PartKind::Bundle {
                check_nested(part, &self.header.shape, &path)?;
            }
        }
        // A card's or a collection's header is not a save's, so the clock keys have no business
        // there either: the reading belongs to one save, which is a nested bundle of its own.
        if self.header.shape != Shape::Save {
            check_key_placement(&self.header.extensions, "header")?;
        }
        if let Some(game) = &self.header.game {
            check_hashes(&game.rom_hashes, "header.game.rom_hashes")?;
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

    /// No two parts may agree on `role`, `path` and `slot` together.
    ///
    /// Those three are how a consumer names a part to a user and matches it against a target, so
    /// two parts a consumer cannot tell apart is a bundle it cannot act on. With all three
    /// defaulting to absence, two bare parts collide on exactly this.
    fn check_part_addresses(&self) -> Result<(), Error> {
        for (index, part) in self.parts.iter().enumerate() {
            for earlier in &self.parts[..index] {
                if earlier.address() == part.address() {
                    return Err(Error::at(
                        format!("parts[{index}]"),
                        ErrorKind::PartsShareAnAddress { first: earlier.id, second: part.id },
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Extension subtrees the specifications place on a save's bundle header and nowhere else.
///
/// A value that describes the bytes rather than the wrapper has to live inside the save's own
/// header, where it stays correct once the save is sliced out of whatever holds it. On a part it
/// would be dropped by the byte copy that extracting a save is.
///
/// These are whole subtrees, not exact names. Every clock key lives under `x.1sav.rtc` — the
/// portable reading is that name, and each chip is a label below it — so one label-boundary test
/// covers the family and a chip added later is covered without touching this list.
const SAVE_HEADER_ONLY: [&str; 1] = ["x.1sav.rtc"];

/// Whether a key sits under one of the subtrees above.
fn is_save_header_only(key: &crate::ReverseDnsName) -> bool {
    SAVE_HEADER_ONLY.iter().any(|root| {
        let root = crate::ReverseDnsName::parse(root).expect("a well-formed name");
        // Label-boundary matching, never a string prefix: `x.1sav.rtcx` is somebody else's.
        key.is_under(&root)
    })
}

fn check_key_placement(extensions: &crate::Extensions, path: &str) -> Result<(), Error> {
    for key in extensions.keys() {
        if is_save_header_only(key) {
            return Err(Error::at(
                format!("{path}[{:?}]", key.as_str()),
                ErrorKind::KeyOutOfPlace { key: key.as_str().to_owned(), allowed: "a save's bundle header" },
            ));
        }
    }
    Ok(())
}

fn check_rom_hash_order(part: &Part, path: &str) -> Result<(), Error> {
    if let Some(game) = &part.game {
        check_hashes(&game.rom_hashes, &format!("{path}.game.rom_hashes"))?;
    }
    Ok(())
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
    // A compressed `bundle` part has to be inflated before any of this can be read, and that is
    // the caller's business rather than a validation step.
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

    // What a shape may hold, which is what bounds the nesting now that the depth cap is gone. A
    // shape this version does not define is not inspected: its rules are in a document this
    // decoder has not seen, so it is round-tripped rather than judged.
    // An inner shape this version does not define is not judged on either side: its rules are in
    // a document this decoder has not seen, so the bundle round-trips rather than being rejected.
    if !outer.is_known() || !inner.shape().is_known() {
        return Ok(());
    }
    let (holds, shape, reason) = match outer {
        Shape::Save => (false, "save", "holds no nested bundle"),
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
        let inner = Bundle { header: Header::default(), parts: vec![Part::new(0, *b"SAVE")] };
        let mut outer = Part::new(0, inner.to_vec().unwrap());
        outer.kind = PartKind::Bundle;
        outer.extensions.insert(key.clone(), reading.clone());
        assert_eq!(
            bundle_of(vec![outer]).validate().unwrap_err().kind(),
            &ErrorKind::KeyOutOfPlace { key: "x.1sav.rtc".to_owned(), allowed: "a save's bundle header" }
        );

        // And on a plain part, which it now also covers.
        let mut plain = Part::new(0, *b"SAVE");
        plain.extensions.insert(key.clone(), reading.clone());
        assert!(bundle_of(vec![plain]).validate().is_err(), "not on a plain part either");

        // A save's own header is where it belongs.
        let mut save = bundle_of(vec![Part::new(0, *b"SAVE")]);
        save.header.extensions.insert(key, reading);
        save.validate().expect("a save's header carries the reading");
    }

    #[test]
    fn every_clock_key_is_refused_on_a_bundle_part() {
        // The rule covers the whole family, including a chip nobody has added yet: the check is a
        // label-boundary test against `x.1sav.rtc`, not a list of names to keep up to date.
        for key in ["x.1sav.rtc", "x.1sav.rtc.mbc3", "x.1sav.rtc.s3511a", "x.1sav.rtc.future-chip"] {
            let inner = Bundle { header: Header::default(), parts: vec![Part::new(0, *b"SAVE")] };
            let mut outer = Part::new(0, inner.to_vec().unwrap());
            outer.kind = PartKind::Bundle;
            outer.extensions.insert(crate::ReverseDnsName::parse(key).unwrap(), dcbor::CBOR::from(1u64));
            assert!(bundle_of(vec![outer]).validate().is_err(), "{key} on a bundle part");
        }
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
