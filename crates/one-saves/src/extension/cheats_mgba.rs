//! [`x.1sav.cheats.mgba`](https://docs.1retro.com/specifications/extensions/x.1sav.cheats.mgba/).

use std::fmt;

use dcbor::{CBOR, Map};

use super::{Extension, only_keys, want_map};
use crate::name::ReverseDnsName;

/// The cheat list mGBA keeps inside each state it takes, set by set.
///
/// It sits on a state's [`state`](crate::PartKind::State) part and nowhere else, because mGBA
/// loads it with the core's data. It changes nothing in that data: a cheat that was on when the
/// state was taken has already changed the memory the state holds. What the list decides is which
/// cheats stay on afterwards, and only when mGBA's setting to load cheats from a state is on.
///
/// The order of the sets is mGBA's, and it matters: each set starts from where the one before it
/// left off, its code type included, so moving one can change how mGBA reads its codes. The list
/// is never empty. mGBA reads an empty cheat section as none at all, so the key is absent for both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheatsMgba {
    /// The sets, in mGBA's order.
    pub sets: Vec<CheatSet>,
}

impl CheatsMgba {
    /// A list of these sets.
    #[must_use]
    pub fn new(sets: Vec<CheatSet>) -> Self {
        CheatsMgba { sets }
    }
}

/// One of mGBA's cheat sets: a name, whether it is switched on, and the codes the player entered.
///
/// Every field defaults to absence, so a set mGBA wrote with no name, switched on, with no
/// directives and no codes is [`CheatSet::default`], and still a set.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheatSet {
    /// What follows `#` in mGBA's text. Never empty when present.
    pub name: Option<String>,
    /// Whether the set is switched off, which mGBA writes as `!disabled`.
    pub disabled: bool,
    /// What follows each other `!`, telling mGBA how to read the codes: for the GBA `GSAv1`,
    /// `GSAv1 raw`, `PARv3` and `PARv3 raw`. The Game Boy has none.
    pub directives: Vec<String>,
    /// The codes, one per line, as mGBA kept them.
    pub codes: Vec<String>,
}

impl CheatSet {
    /// A switched-on set of these codes, with no name and no directives.
    #[must_use]
    pub fn new(codes: impl IntoIterator<Item = impl Into<String>>) -> Self {
        CheatSet { codes: codes.into_iter().map(Into::into).collect(), ..CheatSet::default() }
    }

    /// Sets the name.
    #[must_use]
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Switches the set off.
    #[must_use]
    pub fn disabled(mut self) -> Self {
        self.disabled = true;
        self
    }

    /// Adds a directive.
    #[must_use]
    pub fn directive(mut self, directive: impl Into<String>) -> Self {
        self.directives.push(directive.into());
        self
    }
}

impl Extension for CheatsMgba {
    const KEY: ReverseDnsName = ReverseDnsName::from_static("x.1sav.cheats.mgba");
}

impl From<CheatsMgba> for CBOR {
    fn from(cheats: CheatsMgba) -> Self {
        cheats.sets.into_iter().map(CBOR::from).collect::<Vec<_>>().into()
    }
}

impl From<CheatSet> for CBOR {
    fn from(set: CheatSet) -> Self {
        // Each key defaults to absence, so only what the set says is written.
        let mut map = Map::new();
        if let Some(name) = set.name {
            map.insert(0u64, name);
        }
        if set.disabled {
            map.insert(1u64, true);
        }
        if !set.directives.is_empty() {
            map.insert(2u64, set.directives);
        }
        if !set.codes.is_empty() {
            map.insert(3u64, set.codes);
        }
        map.into()
    }
}

impl TryFrom<CBOR> for CheatsMgba {
    type Error = dcbor::Error;

    fn try_from(value: CBOR) -> dcbor::Result<Self> {
        let items = value.as_array().ok_or(dcbor::Error::WrongType)?;
        if items.is_empty() {
            return Err(dcbor::Error::msg("an empty list, which is written by leaving the key out"));
        }
        Ok(CheatsMgba { sets: items.iter().cloned().map(CheatSet::try_from).collect::<Result<_, _>>()? })
    }
}

impl TryFrom<CBOR> for CheatSet {
    type Error = dcbor::Error;

    fn try_from(value: CBOR) -> dcbor::Result<Self> {
        let map = want_map(&value)?;
        only_keys(map, &[0, 1, 2, 3])?;
        // Absence is each key's only default, so an empty name, a `false` and an empty list are
        // all a second spelling of it.
        let lines = |key: u64| -> dcbor::Result<Vec<String>> {
            let Some(value) = map.get::<u64, CBOR>(key) else { return Ok(Vec::new()) };
            let items = value.as_array().ok_or(dcbor::Error::WrongType)?;
            if items.is_empty() {
                return Err(dcbor::Error::msg("an empty list, which is written by leaving the key out"));
            }
            items.iter().cloned().map(String::try_from).collect()
        };
        let name = map.get::<u64, CBOR>(0).map(String::try_from).transpose()?;
        if name.as_deref() == Some("") {
            return Err(dcbor::Error::msg("an empty name, which is written by leaving the key out"));
        }
        let disabled = match map.get::<u64, CBOR>(1) {
            None => false,
            Some(value) if value == CBOR::from(true) => true,
            Some(_) => return Err(dcbor::Error::msg("`disabled` is `true` or absent")),
        };
        Ok(CheatSet { name, disabled, directives: lines(2)?, codes: lines(3)? })
    }
}

impl fmt::Display for CheatsMgba {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let count = self.sets.len();
        write!(f, "{count} cheat set{}", if count == 1 { "" } else { "s" })?;
        let names: Vec<&str> = self.sets.iter().filter_map(|set| set.name.as_deref()).collect();
        if !names.is_empty() {
            write!(f, ": {}", names.join(", "))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_the_schema() {
        let cheats = CheatsMgba::new(vec![
            CheatSet::new(["D8BAE4D9 4864DCE5"]).name("Infinite money").directive("GSAv1"),
            CheatSet::default().disabled(),
        ]);
        assert_eq!(CheatsMgba::try_from(CBOR::from(cheats.clone())).unwrap(), cheats);
        assert_eq!(cheats.to_string(), "2 cheat sets: Infinite money");
    }

    #[test]
    fn a_set_with_nothing_to_say_is_an_empty_map_and_still_a_set() {
        let empty = CBOR::from(CheatSet::default());
        assert_eq!(empty, Map::new().into());
        let read = CheatsMgba::try_from(CBOR::from(vec![empty])).unwrap();
        assert_eq!(read.sets, [CheatSet::default()]);
    }

    #[test]
    fn refuses_every_second_spelling_of_absence() {
        assert!(CheatsMgba::try_from(CBOR::from(Vec::<CBOR>::new())).is_err(), "an empty list");

        let set = |key: u64, value: CBOR| {
            let mut map = Map::new();
            map.insert(key, value);
            CheatsMgba::try_from(CBOR::from(vec![CBOR::from(map)]))
        };
        assert!(set(0, CBOR::from("")).is_err(), "an empty name");
        assert!(set(1, CBOR::from(false)).is_err(), "a set switched on, spelled out");
        assert!(set(2, CBOR::from(Vec::<String>::new())).is_err(), "no directives, spelled out");
        assert!(set(3, CBOR::from(Vec::<String>::new())).is_err(), "no codes, spelled out");
        assert!(set(4, CBOR::from(1u64)).is_err(), "a key the schema does not list");
    }
}
