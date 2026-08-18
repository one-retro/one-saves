//! The rules a schema cannot state.
//!
//! CDDL describes the decoded data model, so it sees neither the encoding beneath it nor
//! anything inside a byte string. Everything here is a rule that needs one of those: a
//! comparison across items, or a look into a payload.

use crate::codec::Strictness;
use crate::error::{Error, ErrorKind};
use crate::hash::HashValue;
use crate::model::{Bundle, Part, PartKind, Payload};

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
        self.validate_at_depth(0)
    }

    /// Checks the rules, knowing how deep this bundle already sits.
    ///
    /// Depth is capped at 2: a bundle's own parts are depth 0, a nested bundle's are depth 1,
    /// and one nested inside that is depth 2. A save, a card and a collection of cards fill all
    /// three tiers, so anything deeper is malformed and decoders enforce it to keep recursion
    /// bounded.
    fn validate_at_depth(&self, depth: usize) -> Result<(), Error> {
        if self.parts.is_empty() {
            return Err(Error::at("parts", ErrorKind::EmptyContainer));
        }
        self.check_part_order()?;
        self.check_part_addresses()?;

        for (index, part) in self.parts.iter().enumerate() {
            let path = format!("parts[{index}]");
            check_rom_hash_order(part, &path)?;
            if part.kind == PartKind::Bundle {
                check_no_inherited_keys(part, &path)?;
                check_nested(part, depth, &path)?;
            }
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

/// Namespaces a `bundle` part may not carry a key from, because a nested bundle inherits nothing.
///
/// A value that describes the bytes rather than the wrapper has to live inside the inner bundle,
/// where it stays correct once the save is sliced out. Put on the outer part it would be dropped
/// by the byte copy that extracting a save is.
///
/// These are whole subtrees, not exact names. Every clock key lives under `x.1sav.rtc` — the
/// portable reading is that name, and each chip is a label below it — so one label-boundary test
/// covers the family and a chip added later is covered without touching this list.
const NOT_ON_A_BUNDLE_PART: [&str; 1] = ["x.1sav.rtc"];

fn check_no_inherited_keys(part: &Part, path: &str) -> Result<(), Error> {
    for key in part.extensions.keys() {
        for root in NOT_ON_A_BUNDLE_PART {
            let root = crate::ReverseDnsName::parse(root).expect("a well-formed name");
            // Label-boundary matching, never a string prefix: `x.1sav.rtcx` is somebody else's.
            if key.is_under(&root) {
                return Err(Error::at(
                    format!("{path}[{:?}]", key.as_str()),
                    ErrorKind::NotOnABundlePart(key.as_str().to_owned()),
                ));
            }
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

/// Checks the four rules a `bundle` part carries, all of them past what a schema can see.
fn check_nested(part: &Part, depth: usize, path: &str) -> Result<(), Error> {
    if depth >= crate::MAX_NESTING_DEPTH {
        return Err(Error::at(path, ErrorKind::NestingTooDeep));
    }

    // A thin `bundle` part is a legitimate shape — that is what a thin card is — but its payload
    // is not here to check, so the rules below wait until it is resolved.
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

    inner.validate_at_depth(depth + 1).map_err(|e| e.within(&format!("{path}.payload")))
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
    fn rejects_a_clock_reading_on_a_bundle_part() {
        // A nested bundle inherits nothing from the header around it, so a reading put on the
        // wrapper would vanish the moment the save is sliced out. The spec makes that a MUST NOT.
        let inner = Bundle { header: Header::default(), parts: vec![Part::new(0, *b"SAVE")] };
        let mut outer = Part::new(0, inner.to_vec().unwrap());
        outer.kind = PartKind::Bundle;
        outer.extensions.insert(crate::ReverseDnsName::parse("x.1sav.rtc").unwrap(), dcbor::CBOR::from(1u64));

        let bundle = bundle_of(vec![outer]);
        assert_eq!(
            bundle.validate().unwrap_err().kind(),
            &ErrorKind::NotOnABundlePart("x.1sav.rtc".to_owned())
        );
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
    fn a_clock_reading_is_fine_on_an_ordinary_part() {
        // The restriction is about `bundle` parts specifically: a part whose bytes came off a
        // different medium may name its own clock.
        let mut part = Part::new(0, *b"SAVE");
        part.extensions.insert(crate::ReverseDnsName::parse("x.1sav.rtc").unwrap(), dcbor::CBOR::from(1u64));
        assert!(bundle_of(vec![part]).validate().is_ok());
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
