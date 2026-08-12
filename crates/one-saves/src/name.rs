//! The two name grammars these specifications use, and the rule that keeps them apart.
//!
//! A [`Slug`] belongs to the specification or to a registry: `card-image`, `memcard-1`,
//! `ps1-mc`. A [`ReverseDnsName`] belongs to whoever holds the domain it reverses:
//! `io.mgba.rtc`, `com.1retro.forge`. The two are disjoint by construction — a slug is exactly
//! one DNS label and a reverse-DNS name is two or more joined by `.`, so no string is both.
//! That is what lets a producer mint a name without checking what a later version of the spec
//! might assign, and a later version assign one without auditing what producers have minted.
//!
//! Fields that admit either are typed [`Name`].

use core::fmt;
use core::str::FromStr;

/// Why a string is not a well-formed name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameError {
    /// The string was empty.
    Empty,
    /// A slug ran past 64 bytes, or a reverse-DNS name past 255.
    TooLong,
    /// A label was empty, ran past 63 bytes, or started or ended with `-`.
    MalformedLabel,
    /// A character outside the grammar: uppercase, `_`, or anything non-ASCII.
    ///
    /// Case is never folded to rescue a name. A producer that wrote `Memcard-1` meant something
    /// this format cannot represent, and quietly lowercasing it would invent a second spelling
    /// for a value that feeds the content hash.
    InvalidCharacter(char),
    /// A reverse-DNS name's first label was neither `x` nor two or more letters.
    MalformedTopLevel,
    /// A reverse-DNS name had only one label, so it is a slug and not a name.
    NotEnoughLabels,
}

impl fmt::Display for NameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NameError::Empty => f.write_str("name is empty"),
            NameError::TooLong => f.write_str("name is too long"),
            NameError::MalformedLabel => {
                f.write_str("label is empty, over 63 bytes, or starts or ends with '-'")
            }
            NameError::InvalidCharacter(c) => write!(f, "character {c:?} is outside the grammar"),
            NameError::MalformedTopLevel => {
                f.write_str("first label must be 'x' or two or more lowercase letters")
            }
            NameError::NotEnoughLabels => {
                f.write_str("a reverse-DNS name needs two or more labels joined by '.'")
            }
        }
    }
}

impl core::error::Error for NameError {}

/// The longest a slug may run, in bytes.
pub const MAX_SLUG_LEN: usize = 64;

/// The longest a reverse-DNS name may run, in bytes.
pub const MAX_REVERSE_DNS_LEN: usize = 255;

/// A bare slug: lowercase ASCII, runs separated by a single `-`, at most 64 bytes.
///
/// One grammar covers every slug the format uses, whether the spec invented the name
/// (`card-image`) or borrowed it from the world (`ps1-mc`, `atari-2600`). Only a new version of
/// the spec, or a PR against the registry a field names, adds one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Slug(String);

impl Slug {
    /// Parses a slug, checking the whole grammar.
    pub fn parse(text: &str) -> Result<Self, NameError> {
        Self::check(text)?;
        Ok(Slug(text.to_owned()))
    }

    /// Checks the grammar without allocating.
    pub fn check(text: &str) -> Result<(), NameError> {
        if text.is_empty() {
            return Err(NameError::Empty);
        }
        if text.len() > MAX_SLUG_LEN {
            return Err(NameError::TooLong);
        }
        // A run is one or more of [a-z0-9]; runs are joined by exactly one '-', with none at
        // either end. Tracking whether the previous byte was a dash catches the doubled and the
        // dangling cases in one pass.
        let mut previous_was_dash = true;
        for (index, byte) in text.bytes().enumerate() {
            match byte {
                b'a'..=b'z' | b'0'..=b'9' => previous_was_dash = false,
                b'-' => {
                    if previous_was_dash {
                        return Err(NameError::MalformedLabel);
                    }
                    previous_was_dash = true;
                }
                _ => {
                    let character = text[index..].chars().next().unwrap_or('\u{fffd}');
                    return Err(NameError::InvalidCharacter(character));
                }
            }
        }
        if previous_was_dash {
            return Err(NameError::MalformedLabel);
        }
        Ok(())
    }

    /// The slug as a string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Splits a numbered socket into its prefix and index, as the Save Roles registry defines it.
    ///
    /// A role ending in `-<n>` names the nth socket of the kind its prefix names, so a consumer
    /// that recognises `memcard-` matches `memcard-8` without that exact role being registered.
    /// This is the only place these specifications read structure out of a slug.
    ///
    /// The number carries no leading zero, so `memcard-01` is an ordinary slug that happens to
    /// end in digits rather than the first socket spelled oddly. Returning `None` for it is what
    /// keeps one socket from having two names.
    ///
    /// ```
    /// # use one_saves::name::Slug;
    /// let role = Slug::parse("memcard-8").unwrap();
    /// assert_eq!(role.numbered_socket(), Some(("memcard-", 8)));
    ///
    /// // No leading zero, and the prefix has to be there.
    /// assert_eq!(Slug::parse("memcard-08").unwrap().numbered_socket(), None);
    /// assert_eq!(Slug::parse("cartridge").unwrap().numbered_socket(), None);
    /// ```
    #[must_use]
    pub fn numbered_socket(&self) -> Option<(&str, u32)> {
        let index = self.0.rfind('-')?;
        let (prefix, digits) = self.0.split_at(index + 1);
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        if digits.len() > 1 && digits.starts_with('0') {
            return None;
        }
        Some((prefix, digits.parse().ok()?))
    }
}

impl FromStr for Slug {
    type Err = NameError;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Slug::parse(text)
    }
}

impl fmt::Display for Slug {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for Slug {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// A reverse-DNS name: two or more labels joined by `.`, the first a TLD or the reserved `x`.
///
/// Nothing checks that the domain exists, resolves, or still belongs to the producer that minted
/// the name. The convention buys uniqueness, not authentication.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReverseDnsName(String);

impl ReverseDnsName {
    /// Parses a reverse-DNS name, checking the whole grammar.
    pub fn parse(text: &str) -> Result<Self, NameError> {
        Self::check(text)?;
        Ok(ReverseDnsName(text.to_owned()))
    }

    /// Checks the grammar without allocating.
    pub fn check(text: &str) -> Result<(), NameError> {
        if text.is_empty() {
            return Err(NameError::Empty);
        }
        if text.len() > MAX_REVERSE_DNS_LEN {
            return Err(NameError::TooLong);
        }
        let mut labels = text.split('.');
        let top = labels.next().ok_or(NameError::NotEnoughLabels)?;

        // The first label is the reversed top-level domain, so it is letters only and two or
        // more of them. `x` is the one exception: a single-letter TLD cannot exist, which is
        // what makes the tree safe to reserve for producers with no domain.
        let top_is_valid =
            top == "x" || ((2..=63).contains(&top.len()) && top.bytes().all(|b| b.is_ascii_lowercase()));
        if !top_is_valid {
            return Err(NameError::MalformedTopLevel);
        }

        let mut count = 0;
        for label in labels {
            count += 1;
            check_label(label)?;
        }
        if count == 0 {
            return Err(NameError::NotEnoughLabels);
        }
        Ok(())
    }

    /// The name as a string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The labels, in written order, so `io.mgba.rtc` yields `io`, `mgba`, `rtc`.
    pub fn labels(&self) -> impl Iterator<Item = &str> {
        self.0.split('.')
    }

    /// Whether this name sits at or below `prefix` in the tree.
    ///
    /// Matching happens on label boundaries, so `io.mgba` matches `io.mgba` and `io.mgba.rtc`
    /// and does **not** match `io.mgbahax`. A plain string prefix test is wrong here and will
    /// match names belonging to somebody else.
    ///
    /// ```
    /// # use one_saves::name::ReverseDnsName;
    /// let key = ReverseDnsName::parse("io.mgba.rtc").unwrap();
    /// let owner = ReverseDnsName::parse("io.mgba").unwrap();
    /// let impostor = ReverseDnsName::parse("io.mgbahax").unwrap();
    ///
    /// assert!(key.is_under(&owner));
    /// assert!(owner.is_under(&owner));
    /// assert!(!impostor.is_under(&owner));
    /// ```
    #[must_use]
    pub fn is_under(&self, prefix: &ReverseDnsName) -> bool {
        let mut ours = self.labels();
        let mut theirs = prefix.labels();
        loop {
            match (theirs.next(), ours.next()) {
                // Their labels ran out first, or exactly together: we are at or below them.
                (None, _) => return true,
                (Some(_), None) => return false,
                (Some(a), Some(b)) if a != b => return false,
                (Some(_), Some(_)) => {}
            }
        }
    }

    /// Whether this name is in the `x` tree, reserved for producers with no domain to reverse.
    #[must_use]
    pub fn is_x_tree(&self) -> bool {
        self.0 == "x" || self.0.starts_with("x.")
    }
}

/// Checks one non-leading label: 1 to 63 bytes of `[a-z0-9-]`, not starting or ending with `-`.
fn check_label(label: &str) -> Result<(), NameError> {
    if label.is_empty() || label.len() > 63 {
        return Err(NameError::MalformedLabel);
    }
    for (index, byte) in label.bytes().enumerate() {
        match byte {
            b'a'..=b'z' | b'0'..=b'9' | b'-' => {}
            _ => {
                let character = label[index..].chars().next().unwrap_or('\u{fffd}');
                return Err(NameError::InvalidCharacter(character));
            }
        }
    }
    if label.starts_with('-') || label.ends_with('-') {
        return Err(NameError::MalformedLabel);
    }
    Ok(())
}

impl FromStr for ReverseDnsName {
    type Err = NameError;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        ReverseDnsName::parse(text)
    }
}

impl fmt::Display for ReverseDnsName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for ReverseDnsName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// A minted name: a bare slug, or a reverse-DNS name.
///
/// Two fields hold one: a part's `kind` and a source's `app`. Both name something belonging to
/// the producer rather than to everybody, which is why a producer may mint into them at all.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Name {
    /// A bare slug, belonging to the spec or to a registry.
    Slug(Slug),
    /// A reverse-DNS name, belonging to whoever holds the domain.
    ReverseDns(ReverseDnsName),
}

impl Name {
    /// Parses a name in whichever of the two grammars it is written.
    ///
    /// The two are disjoint, so the `.` decides which applies and there is no ambiguity to
    /// resolve. A dotted string that is not a well-formed reverse-DNS name fails as one rather
    /// than falling back to being a bad slug, which is what makes the error useful.
    pub fn parse(text: &str) -> Result<Self, NameError> {
        if text.contains('.') {
            ReverseDnsName::parse(text).map(Name::ReverseDns)
        } else {
            Slug::parse(text).map(Name::Slug)
        }
    }

    /// The name as a string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Name::Slug(slug) => slug.as_str(),
            Name::ReverseDns(name) => name.as_str(),
        }
    }

    /// The slug, when this name is one.
    #[must_use]
    pub fn as_slug(&self) -> Option<&Slug> {
        match self {
            Name::Slug(slug) => Some(slug),
            Name::ReverseDns(_) => None,
        }
    }
}

impl FromStr for Name {
    type Err = NameError;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Name::parse(text)
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_slugs_the_specifications_use() {
        for text in ["card-image", "cartridge-reader", "memcard-1", "atari-2600", "ps1-mc", "gb", "x"] {
            assert!(Slug::parse(text).is_ok(), "{text} should parse");
        }
    }

    #[test]
    fn rejects_the_malformed_slugs_the_corpus_names() {
        // Each of these is an invalid conformance case: the pre-coalescing underscore spellings,
        // a doubled or dangling dash, uppercase, and a slug past 64 bytes.
        for text in ["memcard_1", "card_image", "card--image", "-lead", "trail-", "Primary", ""] {
            assert!(Slug::parse(text).is_err(), "{text:?} should not parse");
        }
        assert_eq!(Slug::parse(&"a".repeat(65)), Err(NameError::TooLong));
        assert!(Slug::parse(&"a".repeat(64)).is_ok());
    }

    #[test]
    fn accepts_the_reverse_dns_names_the_specifications_use() {
        for text in ["io.mgba", "io.mgba.rtc", "com.1retro.forge", "org.hasheous", "x.1sav.rtc", "x.example"]
        {
            assert!(ReverseDnsName::parse(text).is_ok(), "{text} should parse");
        }
    }

    #[test]
    fn rejects_malformed_reverse_dns_names() {
        // A bare word is a slug, not a name; `_` is out so reversing yields a valid domain; a
        // single-letter TLD other than the reserved `x` cannot exist; empty labels are not labels.
        for text in ["hasheous", "io.mgba_rtc", "IO.mgba", "q.vendor", "io..mgba", "io.mgba.", ".io.mgba"] {
            assert!(ReverseDnsName::parse(text).is_err(), "{text:?} should not parse");
        }
    }

    #[test]
    fn the_two_grammars_are_disjoint() {
        // No string parses as both, which is what lets a producer mint without coordinating
        // with a later version of the spec.
        for text in ["card-image", "io.mgba", "x.example", "memcard-1", "com.1retro.forge"] {
            let slug = Slug::parse(text).is_ok();
            let name = ReverseDnsName::parse(text).is_ok();
            assert!(slug ^ name, "{text:?} parsed as slug={slug} reverse_dns={name}");
        }
    }

    #[test]
    fn prefix_matching_respects_label_boundaries() {
        let owner = ReverseDnsName::parse("io.mgba").unwrap();
        assert!(ReverseDnsName::parse("io.mgba.rtc").unwrap().is_under(&owner));
        assert!(ReverseDnsName::parse("io.mgba.a.b.c").unwrap().is_under(&owner));
        assert!(!ReverseDnsName::parse("io.mgbahax").unwrap().is_under(&owner));
        assert!(!ReverseDnsName::parse("io.mgbahax.rtc").unwrap().is_under(&owner));

        // Containment runs one way: a name is not under something below it.
        let key = ReverseDnsName::parse("io.mgba.rtc").unwrap();
        assert!(!owner.is_under(&key));
    }

    #[test]
    fn numbered_sockets_split_only_where_the_registry_says_they_do() {
        let cases = [
            ("memcard-1", Some(("memcard-", 1))),
            ("memcard-8", Some(("memcard-", 8))),
            ("controller-pak-3", Some(("controller-pak-", 3))),
            ("disk-2", Some(("disk-", 2))),
            // No leading zero, so this is an ordinary slug rather than socket 1 spelled twice.
            ("memcard-01", None),
            ("cartridge", None),
            ("atari-2600", Some(("atari-", 2600))),
        ];
        for (text, want) in cases {
            assert_eq!(Slug::parse(text).unwrap().numbered_socket(), want, "{text}");
        }
    }
}
