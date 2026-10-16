//! [`com.1retro.forge`](https://docs.1retro.com/specifications/extensions/com.1retro.forge/).

use std::fmt;

use dcbor::{CBOR, Map};

use super::{Extension, diagnostic, only_keys, want_map};
use crate::name::ReverseDnsName;

/// What a Forge parser read out of a save, and which parser read it.
///
/// The two are one key because they are useless apart: the parsed values say nothing without the
/// name of the parser that knows how to read them. What `parsed` holds is Forge's and changes with
/// the game and the parser version, so it is kept as the map it is and never interpreted here.
#[derive(Clone, PartialEq)]
pub struct Forge {
    /// Which parser read the save, such as `alttp`. Free text, compared by equality.
    pub parser: String,
    /// What it found, opaque to anything that is not Forge.
    pub parsed: Option<Map>,
}

impl Forge {
    /// What a parser that found nothing to report writes.
    #[must_use]
    pub fn new(parser: impl Into<String>) -> Self {
        Forge { parser: parser.into(), parsed: None }
    }

    /// Adds what the parser found.
    #[must_use]
    pub fn parsed(mut self, parsed: Map) -> Self {
        self.parsed = Some(parsed);
        self
    }
}

impl Extension for Forge {
    const KEY: ReverseDnsName = ReverseDnsName::from_static("com.1retro.forge");
}

impl From<Forge> for CBOR {
    fn from(forge: Forge) -> Self {
        let mut map = Map::new();
        map.insert(0u64, forge.parser);
        if let Some(parsed) = forge.parsed {
            map.insert(1u64, parsed);
        }
        map.into()
    }
}

impl TryFrom<CBOR> for Forge {
    type Error = dcbor::Error;

    fn try_from(value: CBOR) -> dcbor::Result<Self> {
        let map = want_map(&value)?;
        only_keys(map, &[0, 1])?;
        let parser = String::try_from(map.get::<u64, CBOR>(0).ok_or(dcbor::Error::MissingMapKey)?)?;
        let parsed = map.get::<u64, CBOR>(1).map(|parsed| want_map(&parsed).cloned()).transpose()?;
        Ok(Forge { parser, parsed })
    }
}

impl fmt::Display for Forge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.parser)?;
        match &self.parsed {
            Some(parsed) => write!(f, ": {}", diagnostic(&parsed.clone().into())),
            None => Ok(()),
        }
    }
}

impl fmt::Debug for Forge {
    /// `parsed` in diagnostic notation, since `dcbor`'s own `Debug` spells out every value's type.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        struct Parsed<'a>(&'a Map);
        impl fmt::Debug for Parsed<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&diagnostic(&self.0.clone().into()))
            }
        }
        f.debug_struct("Forge")
            .field("parser", &self.parser)
            .field("parsed", &self.parsed.as_ref().map(Parsed))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_without_reading_what_was_parsed() {
        let mut parsed = Map::new();
        parsed.insert("rupees", 255u64);
        parsed.insert("hearts", 20u64);
        let forge = Forge::new("alttp").parsed(parsed);
        assert_eq!(Forge::try_from(CBOR::from(forge.clone())).unwrap(), forge);
        assert_eq!(forge.to_string(), r#"alttp: {"hearts": 20, "rupees": 255}"#);
        assert_eq!(
            format!("{forge:?}"),
            r#"Forge { parser: "alttp", parsed: Some({"hearts": 20, "rupees": 255}) }"#
        );
        assert_eq!(Forge::new("ff7").to_string(), "ff7");
    }

    #[test]
    fn refuses_parsed_values_that_are_not_a_map() {
        let mut map = Map::new();
        map.insert(0u64, "alttp");
        map.insert(1u64, "not a map");
        assert!(Forge::try_from(CBOR::from(map)).is_err());
    }
}
