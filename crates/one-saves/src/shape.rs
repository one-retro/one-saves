//! What a bundle *is*, as against what it holds.
//!
//! The format's three tiers — a save, a card, a collection of cards — are not written down in a
//! field. They fall out of the header and the part kinds read together, and the reading is easy
//! to get subtly wrong in both directions: a formatted card with nothing saved on it holds no
//! saves and is still a card, and a collection is recognised by what it *lacks* rather than by
//! anything it says. Anything handed an arbitrary bundle has to answer this before it can walk
//! it, so the rules live here once instead of in each consumer that re-derives them.

use std::fmt;

use crate::model::{Bundle, PartKind};

/// What a bundle is: one of the three tiers the format describes, or none of them.
///
/// Read it as "how do I walk this", which is the question a consumer handed an unknown bundle is
/// actually asking. It says nothing about whether the bundle is valid — call
/// [`validate`](Bundle::validate) for that — and nothing about whether its payloads are here;
/// a [thin](crate::Payload::External) card is a [`Card`](Shape::Card) like any other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Shape {
    /// One save, whose parts are its bytes.
    ///
    /// The floor of the format, and the bottom of every nesting: a bare dump with an empty
    /// header is this, and so is a save spanning several files, which is one part each.
    Save,
    /// A memory card, whose parts are what is on it.
    ///
    /// Either because the header carries a [`card`](crate::Card) map — which is what the map is
    /// for, and what a writer needs to rebuild the card — or because a part is a whole
    /// [`card-image`](crate::PartKind::CardImage). A card usually holds one nested bundle per
    /// save; a formatted card with nothing on it holds only the image, since there is no save to
    /// nest, and it is a card all the same.
    Card,
    /// A collection, whose parts are each another bundle and nothing else.
    ///
    /// Recognised by absence: no `system`, no `game` and no `card` of its own, because its
    /// entries do not agree on any of the three. This is how one file spans systems, and it is
    /// the deepest shape the format allows — an entry may itself be a card of saves.
    Collection,
    /// None of the three.
    ///
    /// A nested bundle beside a plain save part is the case the format names: one game whose
    /// state spans a cartridge and a memory card is neither a card, because the bundle is not
    /// one, nor a collection, because it has a system and a game. A consumer handles the parts
    /// one at a time rather than treating the bundle as a unit.
    Mixed,
}

impl Shape {
    /// The lowercase word for this shape.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Shape::Save => "save",
            Shape::Card => "card",
            Shape::Collection => "collection",
            Shape::Mixed => "mixed",
        }
    }
}

impl fmt::Display for Shape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Bundle {
    /// Which of the format's tiers this bundle sits in.
    ///
    /// ```
    /// use one_saves::{Bundle, Header, Part, Shape};
    ///
    /// let save = Bundle { header: Header::default(), parts: vec![Part::new(0, *b"SAVE")] };
    /// assert_eq!(save.shape(), Shape::Save);
    /// ```
    ///
    /// The reading, in order:
    ///
    /// 1. no [`bundle`](crate::PartKind::Bundle) part — a [`Save`](Shape::Save), unless the
    ///    bundle says it is a card, which makes it an empty [`Card`](Shape::Card);
    /// 2. every part a `bundle` part, under a header naming no `system`, `game` or `card` — a
    ///    [`Collection`](Shape::Collection);
    /// 3. otherwise a `Card` if it says it is one, and [`Mixed`](Shape::Mixed) if it does not.
    ///
    /// A header that names a system or a game is what keeps step 2 from swallowing a card whose
    /// `card` map went missing: a collection cannot name either, because its entries disagree.
    #[must_use]
    pub fn shape(&self) -> Shape {
        let nested = self.parts.iter().filter(|part| part.kind == PartKind::Bundle).count();

        if nested == 0 {
            return if self.says_it_is_a_card() { Shape::Card } else { Shape::Save };
        }
        // Absence on all three, which is the only thing a collection has to say for itself. The
        // `card` test is redundant with `says_it_is_a_card` below and kept for reading order:
        // this is the spec's rule as the spec states it.
        if nested == self.parts.len()
            && self.header.card.is_none()
            && self.header.system.is_none()
            && self.header.game.is_none()
        {
            return Shape::Collection;
        }
        if self.says_it_is_a_card() { Shape::Card } else { Shape::Mixed }
    }

    /// Whether the bundle claims to be a whole card.
    ///
    /// The `card` map is the claim proper, and the only form of it that carries enough — a
    /// format and a capacity — to write the card back. A `card-image` part is the weaker claim a
    /// producer that could not name the format still makes, and it is worth honouring: bytes
    /// announced as a whole card image are not a save, and reading them as one is the worse
    /// mistake.
    fn says_it_is_a_card(&self) -> bool {
        self.header.card.is_some() || self.parts.iter().any(|part| part.kind == PartKind::CardImage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Card, Header, Part};
    use crate::name::Slug;

    fn slug(text: &str) -> Slug {
        Slug::parse(text).expect("a well-formed slug")
    }

    fn nested(id: u64) -> Part {
        let inner = Bundle { header: Header::default(), parts: vec![Part::new(0, *b"SAVE")] };
        let mut part = Part::new(id, inner.to_vec().expect("inner bundle encodes"));
        part.kind = PartKind::Bundle;
        part
    }

    fn card_map() -> Card {
        Card {
            format: slug("ps1-mc"),
            capacity: 131_072,
            system_area: None,
            unknown: crate::UnknownKeys::new(),
        }
    }

    #[test]
    fn a_bare_save_is_a_save() {
        let bundle = Bundle { header: Header::default(), parts: vec![Part::new(0, *b"SAVE")] };
        assert_eq!(bundle.shape(), Shape::Save);
    }

    #[test]
    fn a_save_of_several_files_is_still_one_save() {
        // A PS2 save is a directory, so its bundle has a part per file and no nesting at all.
        let mut parts = vec![Part::new(0, *b"icon.sys"), Part::new(1, *b"BASLUS")];
        parts[0].path = Some("icon.sys".to_owned());
        parts[1].path = Some("BASLUS-20552".to_owned());
        let header = Header { system: Some(slug("ps2")), ..Header::default() };
        assert_eq!(Bundle { header, parts }.shape(), Shape::Save);
    }

    #[test]
    fn a_card_holding_saves_is_a_card() {
        let header = Header { system: Some(slug("ps1")), card: Some(card_map()), ..Header::default() };
        let mut parts = vec![nested(0), nested(1)];
        parts[1].slot = Some(1);
        assert_eq!(Bundle { header, parts }.shape(), Shape::Card);
    }

    #[test]
    fn a_formatted_card_with_nothing_on_it_is_a_card() {
        // The trap the whole classification exists for: no save to nest, so the image is the only
        // part it can have, and a rule that looked for nested bundles would call it a save.
        let header = Header { system: Some(slug("ps1")), card: Some(card_map()), ..Header::default() };
        let mut image = Part::new(0, vec![0u8; 128]);
        image.kind = PartKind::CardImage;
        assert_eq!(Bundle { header, parts: vec![image] }.shape(), Shape::Card);
    }

    #[test]
    fn a_card_image_alone_is_a_card_even_with_no_card_map() {
        // A producer that could not name the card's format writes no `card` map. The bytes are
        // still announced as a whole card, and reading them as a save is the worse mistake.
        let mut image = Part::new(0, vec![0u8; 128]);
        image.kind = PartKind::CardImage;
        assert_eq!(Bundle { header: Header::default(), parts: vec![image] }.shape(), Shape::Card);
    }

    #[test]
    fn an_image_kept_beside_the_splits_is_a_card() {
        let header = Header { system: Some(slug("ps1")), card: Some(card_map()), ..Header::default() };
        let mut image = Part::new(1, vec![0u8; 128]);
        image.kind = PartKind::CardImage;
        assert_eq!(Bundle { header, parts: vec![nested(0), image] }.shape(), Shape::Card);
    }

    #[test]
    fn a_bundle_that_says_nothing_about_itself_and_holds_bundles_is_a_collection() {
        let mut parts = vec![nested(0), nested(1)];
        parts[1].slot = Some(1);
        assert_eq!(Bundle { header: Header::default(), parts }.shape(), Shape::Collection);
    }

    #[test]
    fn naming_a_system_or_a_game_is_enough_to_stop_being_a_collection() {
        // A collection's entries do not agree on a system, so a header that names one is
        // describing something else — a card whose `card` map is missing, most likely.
        for header in [
            Header { system: Some(slug("ps1")), ..Header::default() },
            Header {
                game: Some(crate::model::Game { name: Some("FF7".into()), ..Default::default() }),
                ..Header::default()
            },
        ] {
            let mut parts = vec![nested(0), nested(1)];
            parts[1].slot = Some(1);
            assert_eq!(Bundle { header, parts }.shape(), Shape::Mixed);
        }
    }

    #[test]
    fn a_save_spanning_a_cartridge_and_a_card_is_mixed() {
        // Neither a card, because the bundle is not one, nor a collection, because it has a
        // system and a game. A plain part beside a nested one is what covers it.
        let header = Header { system: Some(slug("gb")), ..Header::default() };
        let mut cartridge = Part::new(0, *b"SRAM");
        cartridge.role = Some(slug("cartridge"));
        let mut card = nested(1);
        card.role = Some(slug("memcard-1"));
        assert_eq!(Bundle { header, parts: vec![cartridge, card] }.shape(), Shape::Mixed);
    }

    #[test]
    fn every_shape_names_itself() {
        for shape in [Shape::Save, Shape::Card, Shape::Collection, Shape::Mixed] {
            assert_eq!(shape.to_string(), shape.as_str());
        }
    }
}
