//! Resolving a ROM's digest into a canonical name through a DAT catalog.
//!
//! No-Intro, Redump, TOSEC and MAME all publish their sets as DAT files, in one of two shapes:
//! Logiqx XML, which is what the download sites serve, and the older ClrMamePro text format.
//! Both are accepted by default, since which one a user has is not their choice to make; the
//! `dat-cmpro` feature is what supplies the second, and with it off such a file is refused rather
//! than misread.
//!
//! Reading both is [`datary`]'s job. It models the dialects those projects actually publish and
//! types every digest, which is a larger job than a save converter should be doing inline — and a
//! job with more corners than it looks. The XML shape splits a run of text at every entity
//! reference and escapes attribute values separately, so a reader that misses either quietly
//! truncates a game's name. The ClrMamePro shape has no specification at all: `sample` is a bare
//! scalar where `rom` is a block, `crc` is `crc32` to ckmame, `forcepacking` is `forcezipping`,
//! and a header block may be called `clrmamepro` or `emulator`. What is left in this module is
//! the mapping onto one [`Entry`], since a catalog presents one shape whichever syntax it arrived
//! in.
//!
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
    /// Reads a catalog in either syntax.
    ///
    /// Which one it is comes from the bytes rather than the file name — both conventionally end
    /// in `.dat` — and [`datary`] does the detecting.
    ///
    /// A malformed checksum fails the whole catalog rather than being skipped. That is stricter
    /// than dropping the digest and carrying on, and deliberately so: an entry whose checksum
    /// went missing still matches on size and name, so a typo would quietly turn into a dump that
    /// resolves to the wrong game.
    ///
    /// Without `dat-cmpro`, a ClrMamePro catalog is refused and the error says which feature would
    /// have read it, rather than reporting it as unreadable XML.
    pub fn parse(text: &str) -> Result<Self> {
        #[cfg(not(feature = "dat-cmpro"))]
        if !is_xml(text) {
            return Err(Error::NotThisFormat {
                format: "DAT catalog",
                // Only two syntaxes exist, so not-XML means ClrMamePro, and the reason this build
                // cannot read it is a build-time choice rather than anything about the file.
                why: "this is not Logiqx XML, and the `dat-cmpro` feature that reads the \
                      ClrMamePro syntax is off in this build"
                    .to_owned(),
            });
        }
        let datafile = datary::from_str(text)
            .map_err(|error| Error::NotThisFormat { format: "DAT catalog", why: error.to_string() })?;
        Ok(Self::from_datafile(&datafile))
    }

    /// Reads a catalog from a file.
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self> {
        // DATs are Latin-1 as often as UTF-8, and a stray byte in one game's name is no reason to
        // refuse the whole set. `datary` reports the encoding rather than guessing at it, which is
        // right for a library and too strict for a converter handed whatever a user has.
        let bytes = std::fs::read(path)?;
        let text = String::from_utf8_lossy(&bytes);
        Self::parse(&text)
    }

    /// Maps a parsed datafile onto entries, one per dump.
    fn from_datafile(datafile: &datary::Datafile) -> Self {
        let name = datafile.header.as_ref().map(|header| header.name.clone());
        let entries = datafile
            .games
            .iter()
            .flat_map(|game| {
                game.roms.iter().map(move |rom| Entry {
                    name: game.name.clone(),
                    rom_name: rom.name.clone(),
                    // Logiqx makes `size` mandatory and ClrMamePro does not, where `datary`
                    // reports 0. Nothing here matches on size, so it is carried as stated.
                    size: Some(rom.size),
                    hashes: rom_hashes(rom),
                    serial: rom.serial.clone(),
                })
            })
            .collect();
        Self::index(entries, name)
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
}

/// Whether this is the XML syntax, for telling the two shapes apart before handing them over.
///
/// A Logiqx file opens with `<?xml` or with `<datafile` and a ClrMamePro one opens with a bare
/// word, so the first non-blank character settles it. A UTF-8 BOM survives the lossy decode in
/// [`Catalog::open`] as a character rather than as bytes, so it is skipped here.
#[cfg(not(feature = "dat-cmpro"))]
fn is_xml(text: &str) -> bool {
    text.trim_start_matches('\u{feff}').trim_start().starts_with('<')
}

/// Every digest a catalog entry carries, as this crate's values.
///
/// There is no hex to parse and no length to check: `datary` types its digests, so a value that
/// reached this point is already the right width. CRC-32 is the exception, being a `u32` rather
/// than a byte string; it is laid out big-endian, which is the order its eight hex digits read in.
fn rom_hashes(rom: &datary::Rom) -> Vec<HashValue> {
    let mut hashes = Vec::new();
    let mut push = |algorithm, digest: Vec<u8>| {
        if let Ok(value) = HashValue::new(algorithm, digest) {
            hashes.push(value);
        }
    };
    if let Some(crc) = rom.crc {
        push(HashAlgorithm::Crc32, crc.0.to_be_bytes().to_vec());
    }
    if let Some(md5) = rom.md5.as_ref() {
        push(HashAlgorithm::Md5, md5.as_bytes().to_vec());
    }
    if let Some(sha1) = rom.sha1.as_ref() {
        push(HashAlgorithm::Sha1, sha1.as_bytes().to_vec());
    }
    if let Some(sha256) = rom.sha256.as_ref() {
        push(HashAlgorithm::Sha256, sha256.as_bytes().to_vec());
    }
    hashes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds an expected digest from hex. Only tests construct one by hand; a catalog's
    /// digests arrive already typed from `datary`.
    fn hash_from_hex(algorithm: HashAlgorithm, text: &str) -> Option<HashValue> {
        if text.len() != algorithm.digest_len() * 2 {
            return None;
        }
        let mut digest = Vec::with_capacity(algorithm.digest_len());
        for pair in text.as_bytes().as_chunks::<2>().0 {
            digest.push(u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?);
        }
        HashValue::new(algorithm, digest).ok()
    }

    const LOGIQX: &str = r#"<?xml version="1.0"?>
<datafile>
  <header>
    <name>Nintendo - Game Boy</name>
    <version>20260101</version>
  </header>
  <game name="Pokemon - Red Version (USA, Europe)">
    <description>Pokemon - Red Version (USA, Europe)</description>
    <rom name="Pokemon - Red Version (USA, Europe).gb" size="1048576"
         serial="DMG-APAE-USA" crc="9F7FDD53" md5="3d45c1ee9abd5738df46d2bdda8b57dc"
         sha1="ea9bcae617fdf159b045185467ae58b2e4a48b9a"/>
  </game>
  <game name="Tetris (World) (Rev 1)">
    <rom name="Tetris (World) (Rev 1).gb" size="32768" crc="46166F60"
         sha1="35e4901a8e2c6f6a1d0e0d0e0d0e0d0e0d0e0d0e"/>
  </game>
</datafile>"#;

    #[cfg(feature = "dat-cmpro")]
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
    #[cfg(feature = "dat-cmpro")]
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
        // escaped wherever they appear: in the element the catalog names itself in, and in the
        // attributes carrying a game's name and a dump's product code.
        let dat = r#"<datafile>
          <header><name>Tom &amp; Jerry Collection</name></header>
          <game name="Tom &amp; Jerry (USA)">
            <description>Tom &amp; Jerry (USA)</description>
            <rom name="a" size="1" serial="DMG-T&amp;J-USA" crc="11111111"/>
          </game>
        </datafile>"#;
        let catalog = Catalog::parse(dat).expect("parses");

        assert_eq!(catalog.name.as_deref(), Some("Tom & Jerry Collection"));
        assert_eq!(catalog.entries()[0].name, "Tom & Jerry (USA)");
        assert_eq!(catalog.entries()[0].serial.as_deref(), Some("DMG-T&J-USA"));
    }

    #[test]
    fn a_malformed_digest_fails_the_whole_catalog() {
        // Skipping the digest and keeping the entry would leave a dump that still matches on
        // size and name, so a typo would resolve to the wrong game rather than to none.
        let bad = r#"<datafile><game name="g"><description>g</description>
            <rom name="a.gb" size="4" crc="zzzzzzzz"/></game></datafile>"#;
        assert!(Catalog::parse(bad).is_err(), "a bad digest must not be silently dropped");
        #[cfg(feature = "dat-cmpro")]
        assert!(Catalog::parse("game ( name g rom ( name a size 4 crc zzzzzzzz ) )").is_err());
    }

    #[test]
    #[cfg(not(feature = "dat-cmpro"))]
    fn a_clrmamepro_catalog_names_the_feature_that_would_read_it() {
        // The alternative is `datary` reporting it as XML that ran out, which sends a user looking
        // at their file for a fault that is in their build.
        let error = Catalog::parse(CLRMAMEPRO_ONLY).expect_err("cmpro is off");
        assert!(error.to_string().contains("dat-cmpro"), "unhelpful: {error}");
        // The XML syntax is unaffected, BOM and leading blank lines included.
        assert!(Catalog::parse(&format!("\u{feff}\n  {LOGIQX}")).is_ok());
    }

    /// A minimal ClrMamePro catalog, for the one test that needs the syntax without a reader.
    #[cfg(not(feature = "dat-cmpro"))]
    const CLRMAMEPRO_ONLY: &str = "clrmamepro ( name \"Nintendo - Game Boy\" )\n";

    #[test]
    #[cfg(feature = "dat-cmpro")]
    fn the_clrmamepro_dialects_are_read_as_published() {
        // None of this is hypothetical: `sample` is a bare scalar where `rom` is a block, ckmame
        // writes `crc32` where ClrMamePro writes `crc`, MAME's `-listinfo` names its header block
        // `emulator`, and a set may be spelled `set` rather than `game`.
        let dat = r#"emulator (
	name "MAME"
)

set (
	name pacman
	description "PuckMan (Japan set 1)"
	rom ( name namcopac.6e size 4096 crc32 0xfee263b3 )
	sample shot.wav
	sampleof galaxian
)
"#;
        let catalog = Catalog::parse(dat).expect("parses");
        assert_eq!(catalog.name.as_deref(), Some("MAME"));
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog.entries()[0].name, "pacman");
        // `0x`-prefixed and bare digests are the same digest.
        let crc = hash_from_hex(HashAlgorithm::Crc32, "fee263b3").expect("hex");
        assert_eq!(catalog.lookup(&crc).map(|e| e.rom_name.as_str()), Some("namcopac.6e"));
    }
}
