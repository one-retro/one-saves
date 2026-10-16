//! [`x.1sav.label`](https://docs.1retro.com/specifications/extensions/x.1sav.label/).

use std::fmt;

use dcbor::{CBOR, Map};

use super::{Extension, only_keys, want_map};
use crate::name::ReverseDnsName;

/// What a console calls a save, and what it says beneath that.
///
/// Both lines are the save's own text, normalized only as far as transcoding it and trimming the
/// padding of a fixed-width field. Each runs 1 to 256 bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    /// What the console lists this save as.
    pub title: String,
    /// What it says about the save beyond its name, where it says anything.
    pub detail: Option<String>,
}

impl Label {
    /// A label with only a title.
    #[must_use]
    pub fn new(title: impl Into<String>) -> Self {
        Label { title: title.into(), detail: None }
    }

    /// Adds the line beneath the title.
    #[must_use]
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

impl Extension for Label {
    const KEY: ReverseDnsName = ReverseDnsName::from_static("x.1sav.label");
}

impl From<Label> for CBOR {
    fn from(label: Label) -> Self {
        let mut map = Map::new();
        map.insert(0u64, label.title);
        if let Some(detail) = label.detail {
            map.insert(1u64, detail);
        }
        map.into()
    }
}

impl TryFrom<CBOR> for Label {
    type Error = dcbor::Error;

    fn try_from(value: CBOR) -> dcbor::Result<Self> {
        let map = want_map(&value)?;
        only_keys(map, &[0, 1])?;
        let line = |value: CBOR| -> dcbor::Result<String> {
            let text = String::try_from(value)?;
            if (1..=256).contains(&text.len()) { Ok(text) } else { Err(dcbor::Error::OutOfRange) }
        };
        Ok(Label {
            title: line(map.get::<u64, CBOR>(0).ok_or(dcbor::Error::MissingMapKey)?)?,
            detail: map.get::<u64, CBOR>(1).map(line).transpose()?,
        })
    }
}

impl fmt::Display for Label {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.title)?;
        if let Some(detail) = &self.detail {
            write!(f, " / {detail}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_the_schema() {
        let label = Label::new("FINAL FANTASY VII").detail("Cloud  Lv 7");
        assert_eq!(Label::try_from(CBOR::from(label.clone())).unwrap(), label);
        assert_eq!(label.to_string(), "FINAL FANTASY VII / Cloud  Lv 7");
    }

    #[test]
    fn refuses_a_line_the_schema_does_not_admit() {
        assert!(Label::try_from(CBOR::from(Label::new(""))).is_err(), "an empty title");
        assert!(Label::try_from(CBOR::from(Label::new("a".repeat(257)))).is_err(), "past 256 bytes");
    }
}
