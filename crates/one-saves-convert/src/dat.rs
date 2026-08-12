//! Resolving a ROM's digest into a canonical name through a DAT catalog.
//!
//! No-Intro, Redump, TOSEC and MAME all publish their sets as DAT files, in one of two shapes:
//! Logiqx XML, which is what the download sites serve, and the older ClrMamePro text format.
//! Both are read here, since which one a user has is not their choice to make.
//!
//! A catalog is matched **by digest, never by filename**. A filename says what somebody called
//! the file; a digest says which dump it is, and that is the question a catalog answers.

use std::collections::HashMap;

use one_saves::{Game, HashAlgorithm, HashValue};

use crate::error::{Error, Result};

/// One entry in a catalog: a dump, and the game it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The game's canonical name, as the catalog spells it.
    pub name: String,
    /// The dump's filename in the set.
    pub rom_name: String,
    /// The dump's size in bytes, when the catalog states one.
    pub size: Option<u64>,
    /// Every digest the catalog carries for this dump, ascending by tag.
    pub hashes: Vec<HashValue>,
    /// The product code, when the catalog carries one.
    pub serial: Option<String>,
}

/// A parsed DAT catalog, indexed for lookup by digest.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    /// The catalog's own name, out of its header.
    pub name: Option<String>,
    entries: Vec<Entry>,
    /// Digest to entry index. One entry appears once per digest it carries.
    by_hash: HashMap<(u64, Vec<u8>), usize>,
}

impl Catalog {
    /// Reads a catalog, working out which of the two formats it is.
    ///
    /// The discriminator is the first non-space character: XML opens with `<`, and ClrMamePro
    /// opens with a keyword.
    pub fn parse(text: &str) -> Result<Self> {
        if text.trim_start().starts_with('<') {
            Self::parse_logiqx(text)
        } else {
            Self::parse_clrmamepro(text)
        }
    }

    /// Reads a catalog from a file.
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self> {
        // DATs are Latin-1 as often as UTF-8, and a stray byte in one game's name is no reason to
        // refuse the whole set.
        let bytes = std::fs::read(path)?;
        let text = String::from_utf8_lossy(&bytes);
        Self::parse(&text)
    }

    fn index(mut entries: Vec<Entry>, name: Option<String>) -> Self {
        let mut by_hash = HashMap::new();
        for (index, entry) in entries.iter_mut().enumerate() {
            entry.hashes.sort();
            for hash in &entry.hashes {
                // First writer wins: a set with two entries for one digest is a set with a
                // duplicate, and picking the later one would be just as arbitrary.
                by_hash.entry((hash.tag(), hash.digest().to_vec())).or_insert(index);
            }
        }
        Catalog { name, entries, by_hash }
    }

    /// How many dumps the catalog lists.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the catalog lists nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every entry, in the order the catalog listed them.
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The entry a digest identifies.
    #[must_use]
    pub fn lookup(&self, hash: &HashValue) -> Option<&Entry> {
        self.by_hash.get(&(hash.tag(), hash.digest().to_vec())).map(|&index| &self.entries[index])
    }

    /// The entry any of these digests identifies, preferring the strongest that matches.
    ///
    /// Order matters: a CRC-32 collision is cheap to produce and a SHA-256 one is not, so a set
    /// that carries both should be matched on the stronger. Values with different algorithms
    /// never compare, so this is a sequence of lookups rather than one.
    #[must_use]
    pub fn lookup_any<'a>(&self, hashes: impl IntoIterator<Item = &'a HashValue>) -> Option<&Entry> {
        let mut candidates: Vec<&HashValue> = hashes.into_iter().collect();
        candidates.sort_by_key(|hash| match hash.algorithm() {
            HashAlgorithm::Sha256 => 0,
            HashAlgorithm::Sha1 => 1,
            HashAlgorithm::Md5 => 2,
            HashAlgorithm::Crc32 => 3,
        });
        candidates.into_iter().find_map(|hash| self.lookup(hash))
    }

    /// Fills in what a catalog knows on top of what a ROM's own header said.
    ///
    /// The catalog wins on `name`, because a canonical set name is what a consumer can group by,
    /// where a header title is padded, truncated and shouty. It does **not** overwrite a serial
    /// read out of the header: that came from the ROM itself and the catalog's is a transcription.
    pub fn enrich(&self, game: &mut Game) -> bool {
        let Some(entry) = self.lookup_any(&game.rom_hashes) else {
            return false;
        };
        game.name = Some(entry.name.clone());
        if game.serial.is_none() {
            game.serial.clone_from(&entry.serial);
        }
        // Take any digest the catalog has that we did not compute, so the bundle carries
        // everything a later consumer might match on.
        for hash in &entry.hashes {
            if !game.rom_hashes.iter().any(|have| have.algorithm() == hash.algorithm()) {
                game.rom_hashes.push(hash.clone());
            }
        }
        game.rom_hashes.sort();
        true
    }

    /// Reads the Logiqx XML shape, which is what the download sites serve.
    fn parse_logiqx(text: &str) -> Result<Self> {
        use quick_xml::Reader;
        use quick_xml::events::Event;

        let mut reader = Reader::from_str(text);
        // Not trimmed by the reader: it trims each event, and a run split by an entity reference
        // arrives as several. Trimming per fragment would eat the spaces around the entity and
        // turn `Tom &amp; Jerry` into `Tom&Jerry`. The joined text is trimmed instead.
        reader.config_mut().trim_text(false);

        let mut entries = Vec::new();
        let mut catalog_name = None;
        let mut game_name = String::new();
        let mut serial = None;
        let mut in_header = false;
        // The name of the element whose text we are inside. Tracked for every element, not only
        // the ones in the header, because a game's serial is an element of its own.
        let mut element = String::new();
        // Characters seen inside the current element, joined back up.
        let mut text = String::new();
        let mut buffer = Vec::new();

        loop {
            match reader.read_event_into(&mut buffer) {
                Err(error) => {
                    return Err(Error::NotThisFormat {
                        format: "Logiqx DAT",
                        why: format!("at position {}: {error}", reader.buffer_position()),
                    });
                }
                Ok(Event::Eof) => break,
                Ok(Event::Start(tag)) => {
                    let name = tag.name();
                    match name.as_ref() {
                        b"header" => in_header = true,
                        b"game" | b"machine" => {
                            game_name = attribute(&tag, b"name").unwrap_or_default();
                            serial = None;
                        }
                        _ => {}
                    }
                    element = String::from_utf8_lossy(name.as_ref()).into_owned();
                    text.clear();
                }
                // An element's text is settled at its close rather than as it arrives, because a
                // single run of characters reaches us as several events: an entity reference
                // splits it, so `Tom &amp; Jerry` is three. Acting on the first would keep `Tom`.
                Ok(Event::End(tag)) => {
                    let value = text.trim().to_owned();
                    match tag.name().as_ref() {
                        // A game names itself in an attribute, so a `name` element is the
                        // catalog's own and appears only in the header.
                        b"name" if in_header && catalog_name.is_none() => catalog_name = Some(value),
                        // No-Intro puts a disc's product code in an element of its own.
                        b"serial" if !in_header => serial = Some(value),
                        b"header" => in_header = false,
                        _ => {}
                    }
                    text.clear();
                    element.clear();
                }
                Ok(Event::Text(chunk)) => text.push_str(&text_of(&chunk)),
                // `&amp;` and `&#38;` alike, which the reader hands over separately from the text
                // around them.
                Ok(Event::GeneralRef(entity)) => {
                    if let Ok(name) = entity.decode() {
                        let written = format!("&{name};");
                        // An entity nothing defines is left as written rather than dropped.
                        let resolved = quick_xml::escape::unescape(&written)
                            .map_or_else(|_| written.clone(), std::borrow::Cow::into_owned);
                        text.push_str(&resolved);
                    }
                }
                Ok(Event::Empty(tag)) => {
                    if tag.name().as_ref() != b"rom" {
                        continue;
                    }
                    let hashes = [
                        (HashAlgorithm::Sha256, "sha256"),
                        (HashAlgorithm::Sha1, "sha1"),
                        (HashAlgorithm::Crc32, "crc"),
                        (HashAlgorithm::Md5, "md5"),
                    ]
                    .into_iter()
                    .filter_map(|(algorithm, key)| {
                        let text = attribute(&tag, key.as_bytes())?;
                        hash_from_hex(algorithm, &text)
                    })
                    .collect();

                    entries.push(Entry {
                        name: game_name.clone(),
                        rom_name: attribute(&tag, b"name").unwrap_or_default(),
                        size: attribute(&tag, b"size").and_then(|s| s.parse().ok()),
                        hashes,
                        serial: serial.clone(),
                    });
                }
                Ok(_) => {}
            }
            buffer.clear();
        }
        Ok(Self::index(entries, catalog_name))
    }

    /// Reads the older ClrMamePro text shape.
    ///
    /// Nothing here can fail today: an unparseable line is skipped rather than refused, since a
    /// DAT with one odd entry is still a usable catalog. The `Result` matches the other arm of
    /// [`parse`](Self::parse) so the two stay interchangeable.
    #[allow(clippy::unnecessary_wraps)]
    fn parse_clrmamepro(text: &str) -> Result<Self> {
        let mut entries = Vec::new();
        let mut catalog_name = None;
        let mut game_name = String::new();
        let mut serial: Option<String> = None;

        for line in text.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("name ") {
                let value = unquote(rest);
                // The first `name` belongs to the header block; the rest name games.
                if catalog_name.is_none() && game_name.is_empty() {
                    catalog_name = Some(value);
                } else {
                    game_name = value;
                }
                continue;
            }
            if let Some(rest) = line.strip_prefix("serial ") {
                serial = Some(unquote(rest));
                continue;
            }
            if line == ")" {
                continue;
            }
            let Some(rest) = line.strip_prefix("rom (") else {
                continue;
            };

            // `rom ( name "x.gb" size 1048576 crc 9F7FDD53 sha1 ... )` — a flat sequence of
            // keyword and value, where only the name is ever quoted.
            let body = rest.trim_end_matches(')').trim();
            let tokens = tokenize(body);
            let mut rom_name = String::new();
            let mut size = None;
            let mut hashes = Vec::new();
            let mut index = 0;
            while index + 1 < tokens.len() {
                let (key, value) = (&tokens[index], &tokens[index + 1]);
                match key.as_str() {
                    "name" => rom_name.clone_from(value),
                    "size" => size = value.parse().ok(),
                    "crc" => hashes.extend(hash_from_hex(HashAlgorithm::Crc32, value)),
                    "sha1" => hashes.extend(hash_from_hex(HashAlgorithm::Sha1, value)),
                    "sha256" => hashes.extend(hash_from_hex(HashAlgorithm::Sha256, value)),
                    "md5" => hashes.extend(hash_from_hex(HashAlgorithm::Md5, value)),
                    _ => {}
                }
                index += 2;
            }
            entries.push(Entry { name: game_name.clone(), rom_name, size, hashes, serial: serial.clone() });
        }
        Ok(Self::index(entries, catalog_name))
    }
}

/// A text event's content: decoded, end-of-lines normalised, and entities resolved.
///
/// Those are three separate operations in quick-xml and only the first two come from the event.
/// Skipping the third leaves a catalog full of `Tom &amp; Jerry` — which no digest lookup would
/// notice, since matching is on bytes, but every listing would show.
fn text_of(text: &quick_xml::events::BytesText<'_>) -> String {
    let Ok(decoded) = text.xml10_content() else {
        return String::new();
    };
    // Not trimmed: this is one fragment of a run that may continue after an entity reference, so
    // its edges are interior once the pieces are joined.
    match quick_xml::escape::unescape(&decoded) {
        Ok(unescaped) => unescaped.into_owned(),
        // An entity this crate cannot resolve is left as written rather than dropping the name.
        Err(_) => decoded.into_owned(),
    }
}

fn attribute(tag: &quick_xml::events::BytesStart<'_>, key: &[u8]) -> Option<String> {
    tag.attributes().flatten().find(|a| a.key.as_ref() == key).map(|a| {
        // Attribute values are escaped too, and a game names itself in one. DAT files carry no
        // XML declaration, which is the case `Implicit1_0` names.
        a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map_or_else(|_| String::from_utf8_lossy(&a.value).into_owned(), std::borrow::Cow::into_owned)
    })
}

fn unquote(text: &str) -> String {
    text.trim().trim_matches('"').to_owned()
}

/// Splits a ClrMamePro line into tokens, keeping a quoted run together.
fn tokenize(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for character in line.chars() {
        match character {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Reads a hex digest, checking it against the length its algorithm implies.
fn hash_from_hex(algorithm: HashAlgorithm, text: &str) -> Option<HashValue> {
    let text = text.trim();
    if text.len() != algorithm.digest_len() * 2 {
        return None;
    }
    let mut digest = Vec::with_capacity(algorithm.digest_len());
    for pair in text.as_bytes().chunks_exact(2) {
        let hex = std::str::from_utf8(pair).ok()?;
        digest.push(u8::from_str_radix(hex, 16).ok()?);
    }
    HashValue::new(algorithm, digest).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOGIQX: &str = r#"<?xml version="1.0"?>
<datafile>
  <header>
    <name>Nintendo - Game Boy</name>
    <version>20260101</version>
  </header>
  <game name="Pokemon - Red Version (USA, Europe)">
    <description>Pokemon - Red Version (USA, Europe)</description>
    <serial>DMG-APAE-USA</serial>
    <rom name="Pokemon - Red Version (USA, Europe).gb" size="1048576"
         crc="9F7FDD53" md5="3d45c1ee9abd5738df46d2bdda8b57dc"
         sha1="ea9bcae617fdf159b045185467ae58b2e4a48b9a"/>
  </game>
  <game name="Tetris (World) (Rev 1)">
    <rom name="Tetris (World) (Rev 1).gb" size="32768" crc="46166F60"
         sha1="35e4901a8e2c6f6a1d0e0d0e0d0e0d0e0d0e0d0e"/>
  </game>
</datafile>"#;

    const CLRMAMEPRO: &str = r#"clrmamepro (
	name "Nintendo - Game Boy"
	version 20260101
)

game (
	name "Pokemon - Red Version (USA, Europe)"
	serial "DMG-APAE-USA"
	rom ( name "Pokemon - Red Version (USA, Europe).gb" size 1048576 crc 9F7FDD53 sha1 ea9bcae617fdf159b045185467ae58b2e4a48b9a )
)
"#;

    #[test]
    fn reads_the_logiqx_shape() {
        let catalog = Catalog::parse(LOGIQX).expect("parses");
        assert_eq!(catalog.name.as_deref(), Some("Nintendo - Game Boy"));
        assert_eq!(catalog.len(), 2);

        let entry = &catalog.entries()[0];
        assert_eq!(entry.name, "Pokemon - Red Version (USA, Europe)");
        assert_eq!(entry.size, Some(1_048_576));
        assert_eq!(entry.serial.as_deref(), Some("DMG-APAE-USA"));
        // Three digests, sorted ascending by tag: sha1 (18542), crc32 (46010), md5 (46011).
        assert_eq!(entry.hashes.len(), 3);
        assert!(entry.hashes.windows(2).all(|w| w[0].tag() < w[1].tag()));
    }

    #[test]
    fn reads_the_clrmamepro_shape_to_the_same_result() {
        let catalog = Catalog::parse(CLRMAMEPRO).expect("parses");
        assert_eq!(catalog.name.as_deref(), Some("Nintendo - Game Boy"));
        assert_eq!(catalog.len(), 1);

        let entry = &catalog.entries()[0];
        assert_eq!(entry.name, "Pokemon - Red Version (USA, Europe)");
        assert_eq!(entry.size, Some(1_048_576));
        assert_eq!(entry.serial.as_deref(), Some("DMG-APAE-USA"));
        // A quoted name with spaces survives tokenizing.
        assert_eq!(entry.rom_name, "Pokemon - Red Version (USA, Europe).gb");
    }

    #[test]
    fn looks_up_by_digest_and_not_by_name() {
        let catalog = Catalog::parse(LOGIQX).expect("parses");
        let crc = hash_from_hex(HashAlgorithm::Crc32, "9F7FDD53").unwrap();
        assert_eq!(
            catalog.lookup(&crc).map(|e| e.name.as_str()),
            Some("Pokemon - Red Version (USA, Europe)")
        );

        // A digest nothing carries resolves to nothing rather than to a near miss.
        let absent = hash_from_hex(HashAlgorithm::Crc32, "00000000").unwrap();
        assert_eq!(catalog.lookup(&absent), None);
    }

    #[test]
    fn prefers_the_strongest_digest_that_matches() {
        // Two entries whose CRC-32 agrees and whose SHA-1 does not: matching on the weak hash
        // would pick either, and matching on the strong one settles it.
        let dat = r#"<datafile>
          <game name="Real"><rom name="a" crc="11111111" sha1="aa39a3ee5e6b4b0d3255bfef95601890afd80709"/></game>
          <game name="Collision"><rom name="b" crc="11111111" sha1="bb39a3ee5e6b4b0d3255bfef95601890afd80709"/></game>
        </datafile>"#;
        let catalog = Catalog::parse(dat).expect("parses");

        let crc = hash_from_hex(HashAlgorithm::Crc32, "11111111").unwrap();
        let sha1 = hash_from_hex(HashAlgorithm::Sha1, "bb39a3ee5e6b4b0d3255bfef95601890afd80709").unwrap();
        assert_eq!(catalog.lookup_any([&crc, &sha1]).map(|e| e.name.as_str()), Some("Collision"));
    }

    #[test]
    fn enriching_takes_the_catalog_name_and_keeps_the_header_serial() {
        let catalog = Catalog::parse(LOGIQX).expect("parses");
        let mut game = Game {
            name: Some("POKEMON RED".into()),
            serial: Some("FROM-HEADER".into()),
            rom_hashes: vec![hash_from_hex(HashAlgorithm::Crc32, "9F7FDD53").unwrap()],
            ..Game::default()
        };

        assert!(catalog.enrich(&mut game));
        // The canonical set name replaces the shouty header title.
        assert_eq!(game.name.as_deref(), Some("Pokemon - Red Version (USA, Europe)"));
        // The serial came off the ROM itself, so the catalog's transcription does not overwrite it.
        assert_eq!(game.serial.as_deref(), Some("FROM-HEADER"));
        // Digests the catalog had and we did not are taken, still ascending by tag.
        assert_eq!(game.rom_hashes.len(), 3);
        assert!(game.rom_hashes.windows(2).all(|w| w[0].tag() < w[1].tag()));
    }

    #[test]
    fn enriching_a_rom_no_catalog_lists_changes_nothing() {
        let catalog = Catalog::parse(LOGIQX).expect("parses");
        let mut game = Game {
            name: Some("UNKNOWN".into()),
            rom_hashes: vec![hash_from_hex(HashAlgorithm::Crc32, "DEADBEEF").unwrap()],
            ..Game::default()
        };
        let before = game.clone();
        assert!(!catalog.enrich(&mut game));
        assert_eq!(game, before);
    }

    #[test]
    fn entities_are_resolved_in_both_names_and_elements() {
        // A catalog full of ampersands is the common case rather than an edge one, and they are
        // escaped wherever they appear: in the attribute a game names itself with, and in the
        // element a serial sits in.
        let dat = r#"<datafile>
          <header><name>Tom &amp; Jerry Collection</name></header>
          <game name="Tom &amp; Jerry (USA)">
            <serial>DMG-T&amp;J-USA</serial>
            <rom name="a" crc="11111111"/>
          </game>
        </datafile>"#;
        let catalog = Catalog::parse(dat).expect("parses");

        assert_eq!(catalog.name.as_deref(), Some("Tom & Jerry Collection"));
        assert_eq!(catalog.entries()[0].name, "Tom & Jerry (USA)");
        assert_eq!(catalog.entries()[0].serial.as_deref(), Some("DMG-T&J-USA"));
    }

    #[test]
    fn a_digest_of_the_wrong_length_is_not_a_digest() {
        assert_eq!(hash_from_hex(HashAlgorithm::Crc32, "9F7F"), None);
        assert_eq!(hash_from_hex(HashAlgorithm::Sha1, "nothex..."), None);
    }
}
