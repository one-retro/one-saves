//! Cards with nothing on them, made from nothing.
//!
//! A console formats a card before anything is saved to it, and so does an emulator, but only
//! when it makes the file itself. A card for a MiSTer, a flash cart, or an emulator pointed at a
//! file that is not there yet has to come from somewhere else, and the builder each format's crate
//! has for rebuilding a card is that somewhere: given no saves, it writes an empty, formatted one.
//!
//! What comes out is what [`write`](super::write()) writes for the format, so a blank card and a
//! card emptied of its saves are the same file: a PS2 card with its spare areas, a Saturn volume
//! packed, a Neo Geo card bare. [`Layout`] asks for the two wrappings some emulators keep instead.
//!
//! Reading one back gives a card bundle holding a single `card-image` part, which is how a card
//! with nothing on it is carried; see [`super`].

use one_saves::Bundle;

use crate::detect::Format;
use crate::error::{Error, Result};

/// A size a card of some format comes in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    /// What to call it when asking for it: `8M`, `251`, `32K`.
    pub name: &'static str,
    /// What it is, for a person choosing one.
    pub label: &'static str,
    /// The card's capacity as its card map records it, which is what
    /// [`BlankOptions::capacity`] takes. Not the length of the file, which a card's structure, a
    /// PS2 card's spare areas or a [`Layout`] all make longer.
    pub capacity: u64,
    /// Whether this is the size a card of its format is made in when nobody says.
    pub standard: bool,
}

impl Size {
    const fn new(name: &'static str, label: &'static str, capacity: u64) -> Self {
        Self { name, label, capacity, standard: false }
    }

    const fn standard(self) -> Self {
        Self { standard: true, ..self }
    }
}

/// The sizes a format's cards come in, smallest first. Empty for a format that is not a card.
///
/// Listed whether or not this build can write the format, like [`Format`] itself.
#[must_use]
pub fn sizes(format: Format) -> &'static [Size] {
    match format {
        Format::Ps1Card => PS1,
        Format::Ps2Card => PS2,
        Format::GcCard => GC,
        Format::N64Pak => N64,
        Format::Vmu => VMU,
        Format::SaturnBup => SATURN,
        Format::NeoGeoCard => NEOGEO,
        Format::Bundle | Format::Raw => &[],
    }
}

/// The size of a format's cards that goes by this name, ignoring case.
#[must_use]
pub fn size(format: Format, name: &str) -> Option<&'static Size> {
    sizes(format).iter().find(|size| size.name.eq_ignore_ascii_case(name))
}

const PS1: &[Size] = &[Size::new("128K", "128 KiB, 15 blocks", 131_072).standard()];

const PS2: &[Size] = &[
    Size::new("8M", "8 MB, Sony's own card", 8_388_608).standard(),
    Size::new("16M", "16 MB", 16_777_216),
    Size::new("32M", "32 MB", 33_554_432),
    Size::new("64M", "64 MB", 67_108_864),
];

// The capacity is what saves can occupy: the card's blocks, less the five its header, directory
// and table take. Which is also why the names are the counts the console shows.
const GC: &[Size] = &[
    Size::new("59", "59 blocks, 4 Mbit", 59 * 8192),
    Size::new("123", "123 blocks, 8 Mbit", 123 * 8192),
    Size::new("251", "251 blocks, 16 Mbit", 251 * 8192).standard(),
    Size::new("507", "507 blocks, 32 Mbit", 507 * 8192),
    Size::new("1019", "1019 blocks, 64 Mbit", 1019 * 8192),
    Size::new("2043", "2043 blocks, 128 Mbit", 2043 * 8192),
];

const N64: &[Size] = &[Size::new("32K", "32 KiB, 123 pages", 32_768).standard()];

const VMU: &[Size] = &[Size::new("128K", "128 KiB, 200 blocks", 102_400).standard()];

// The capacity is what saves can occupy, which is the volume less its two reserved blocks. The
// console's own memory allocates in 64-byte blocks, the carts in 512 or, the largest, 1024.
const SATURN: &[Size] = &[
    Size::new("32K", "32 KiB, the console's own memory", 32_640).standard(),
    Size::new("512K", "512 KiB, Sega's Backup RAM Cart", 523_264),
    Size::new("1M", "1 MiB cart", 1_047_552),
    Size::new("2M", "2 MiB cart", 2_096_128),
    Size::new("4M", "4 MiB cart", 4_192_256),
];

// The capacity is the whole card, its header, directory and two tables included.
const NEOGEO: &[Size] = &[
    Size::new("2K", "2 KiB, SNK's own card", 2_048).standard(),
    Size::new("4K", "4 KiB", 4_096),
    Size::new("6K", "6 KiB", 6_144),
    Size::new("8K", "8 KiB, what a Neo Geo CD keeps inside", 8_192),
    Size::new("10K", "10 KiB", 10_240),
    Size::new("12K", "12 KiB", 12_288),
    Size::new("14K", "14 KiB", 14_336),
    Size::new("16K", "16 KiB", 16_384),
];

/// How a card sits in its file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layout {
    /// The card and nothing else, which is what [`write`](super::write()) writes and every
    /// emulator but the two below keeps.
    #[default]
    Bare,
    /// A Saturn volume as Yabause keeps it: the address space the console's 68000 sees, each byte
    /// behind the `0xFF` an even address reads, twice the length.
    Wide,
    /// A Neo Geo card as the MiSTer core keeps it: 64 KiB of the cabinet's own backup RAM, erased,
    /// then the card with the two bytes of every 16-bit word swapped. The core keeps 8 KiB for
    /// the card, so a larger one does not go in.
    MiSter,
}

impl Layout {
    /// The layout a name denotes: `bare`, `wide` or `mister`, ignoring case.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "bare" => Some(Layout::Bare),
            "wide" => Some(Layout::Wide),
            "mister" => Some(Layout::MiSter),
            _ => None,
        }
    }

    /// What it is called, as [`from_name`](Self::from_name) takes it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Layout::Bare => "bare",
            Layout::Wide => "wide",
            Layout::MiSter => "mister",
        }
    }

    /// The one format this layout is a layout of, or `None` for one every format comes in.
    const fn format(self) -> Option<Format> {
        match self {
            Layout::Bare => None,
            Layout::Wide => Some(Format::SaturnBup),
            Layout::MiSter => Some(Format::NeoGeoCard),
        }
    }
}

/// What kind of empty card to make.
#[derive(Debug, Clone, Default)]
pub struct BlankOptions {
    /// Its size, by the capacity a [`Size`] records. `None` is the format's standard size.
    pub capacity: Option<u64>,
    /// For a GameCube card, Shift-JIS names rather than ASCII, as a Japanese console formats it.
    ///
    /// A game compares the card's character set with the console's and refuses a card that
    /// disagrees, offering to format it, so a card for a Japanese console has to say Shift-JIS.
    pub shift_jis: bool,
    /// How the card sits in its file.
    pub layout: Layout,
}

/// An empty, formatted card of a format.
///
/// # Errors
///
/// [`Error::NoSuchCard`] for a size, character set or layout the format does not come in, and for
/// a format that is not a card. [`Error::Unsupported`] for a card format this build was not
/// compiled with.
pub fn blank(format: Format, options: &BlankOptions) -> Result<Vec<u8>> {
    let label = format.label();
    let sizes = sizes(format);
    if sizes.is_empty() {
        return Err(Error::NoSuchCard(format!("a {label} is not a card, so there is no empty one")));
    }
    let size = match options.capacity {
        None => sizes.iter().find(|size| size.standard).expect("every card format has a standard size"),
        Some(capacity) => sizes.iter().find(|size| size.capacity == capacity).ok_or_else(|| {
            Error::NoSuchCard(format!("no {label} holds {capacity} bytes; it comes in {}", names(sizes)))
        })?,
    };
    if options.shift_jis && format != Format::GcCard {
        return Err(Error::NoSuchCard(format!(
            "a {label} does not record a character set; only a GameCube card is formatted for one"
        )));
    }
    if let Some(only) = options.layout.format()
        && only != format
    {
        return Err(Error::NoSuchCard(format!(
            "the {} layout is a {}'s, and a {label} is kept bare",
            options.layout.name(),
            only.label()
        )));
    }
    build(format, size.capacity, options)
}

/// An empty card like this one: the same format and size, and on a GameCube the same character
/// set.
///
/// What formatting it on a console leaves. The saves go, and so does whatever the card said about
/// itself — a pak's label, a VMU's colour, a GameCube card's serial — as they do when a console
/// formats it. The character set stays, because a console formats with its own, and a card for a
/// Japanese one is still for a Japanese one afterwards.
///
/// # Errors
///
/// As [`blank`], and [`Error::NoSuchCard`] for a bundle that is not a card.
pub fn blank_like(card: &Bundle) -> Result<Vec<u8>> {
    let map =
        card.header.card.as_ref().ok_or_else(|| Error::NoSuchCard("this bundle is not a card".into()))?;
    let format = Format::from_card_format(map.format.as_str())
        .ok_or_else(|| Error::CannotWrite(map.format.as_str().to_owned()))?;
    #[cfg(feature = "gc")]
    let shift_jis =
        format == Format::GcCard && map.system_area.as_deref().is_some_and(gc_memcard::is_shift_jis);
    #[cfg(not(feature = "gc"))]
    let shift_jis = false;
    #[cfg(feature = "neogeo")]
    let capacity =
        if format == Format::NeoGeoCard { super::neogeo::card_capacity(map.capacity) } else { map.capacity };
    #[cfg(not(feature = "neogeo"))]
    let capacity = map.capacity;
    blank(format, &BlankOptions { capacity: Some(capacity), shift_jis, layout: Layout::Bare })
}

/// The sizes a format comes in, by name, for a message.
fn names(sizes: &[Size]) -> String {
    let names: Vec<&str> = sizes.iter().map(|size| size.name).collect();
    match names.as_slice() {
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
        [] => String::new(),
    }
}

/// Builds the card, its size, character set and layout already checked against its format.
// The size is read only by the formats that come in more than one, and the options only by those
// with a character set or a layout, so a build without all four of them leaves one or the other
// unread. That is the configuration working rather than something left over.
#[cfg_attr(
    not(all(feature = "gc", feature = "neogeo", feature = "ps2", feature = "saturn")),
    allow(unused_variables)
)]
fn build(format: Format, capacity: u64, options: &BlankOptions) -> Result<Vec<u8>> {
    let capacity = usize::try_from(capacity).expect("a card's capacity fits in memory");
    // An empty card fails to build only where its crate refuses what was asked, which a card too
    // large for a MiSTer save is, or where it and this table disagree about a size. Either way it
    // is said in the crate's words rather than unwrapped.
    match format {
        #[cfg(feature = "ps1")]
        Format::Ps1Card => {
            ps1_memcard::CardBuilder::new().build().map_err(|e| Error::NoSuchCard(e.to_string()))
        }
        #[cfg(feature = "ps2")]
        Format::Ps2Card => {
            let capacity = ps2_memcard::Capacity::from_bytes(capacity)
                .ok_or_else(|| Error::NoSuchCard(format!("no PS2 card holds {capacity} bytes")))?;
            // With its spare areas, as `write` writes a PS2 card: without them a console does not
            // see a card at all.
            ps2_memcard::CardBuilder::new(capacity)
                .build_with_spare()
                .map_err(|e| Error::NoSuchCard(e.to_string()))
        }
        #[cfg(feature = "gc")]
        Format::GcCard => gc_memcard::CardBuilder::new()
            .capacity(capacity)
            .shift_jis(options.shift_jis)
            .build()
            .map_err(|e| Error::NoSuchCard(e.to_string())),
        #[cfg(feature = "n64")]
        Format::N64Pak => n64_cpak::PakBuilder::new().build().map_err(|e| Error::NoSuchCard(e.to_string())),
        #[cfg(feature = "vmu")]
        Format::Vmu => dreamcast_vmu::VmuBuilder::new().build().map_err(|e| Error::NoSuchCard(e.to_string())),
        #[cfg(feature = "saturn")]
        Format::SaturnBup => {
            use saturn_backup::{BackupBuilder, Container, GEOMETRIES, Geometry};
            let (size, block) = GEOMETRIES
                .into_iter()
                .find(|&(size, block)| {
                    Geometry::new(size, block).is_some_and(|g| g.data_capacity() == capacity)
                })
                .ok_or_else(|| Error::NoSuchCard(format!("no Saturn volume holds {capacity} bytes")))?;
            let container = if options.layout == Layout::Wide { Container::Wide } else { Container::Packed };
            BackupBuilder::new(size, block)
                .and_then(|mut builder| builder.container(container).build())
                .map_err(|e| Error::NoSuchCard(e.to_string()))
        }
        #[cfg(feature = "neogeo")]
        Format::NeoGeoCard => {
            use neogeo_memcard::{CardBuilder, Container};
            let container =
                if options.layout == Layout::MiSter { Container::MiSter } else { Container::Bare };
            CardBuilder::new(capacity)
                .and_then(|mut builder| builder.container(container).build())
                .map_err(|e| Error::NoSuchCard(e.to_string()))
        }
        #[cfg_attr(
            all(
                feature = "gc",
                feature = "n64",
                feature = "neogeo",
                feature = "ps1",
                feature = "ps2",
                feature = "saturn",
                feature = "vmu"
            ),
            allow(unreachable_patterns)
        )]
        other => Err(other.unsupported()),
    }
}

#[cfg(test)]
mod tests {
    use one_saves::PartKind;

    use super::*;
    use crate::card::{CardOptions, read, write};
    use crate::detect;

    const CARDS: [Format; 7] = [
        Format::Ps1Card,
        Format::Ps2Card,
        Format::GcCard,
        Format::N64Pak,
        Format::Vmu,
        Format::SaturnBup,
        Format::NeoGeoCard,
    ];

    fn of(capacity: u64) -> BlankOptions {
        BlankOptions { capacity: Some(capacity), ..BlankOptions::default() }
    }

    /// The table's capacities are the format crates' arithmetic written out, so this is what holds
    /// them to it: every size reads back as a card of exactly that capacity, found by its bytes
    /// alone, holding nothing but the picture of itself an empty card is carried as.
    #[test]
    fn every_size_reads_back_as_an_empty_card_of_that_size() {
        for format in CARDS.into_iter().filter(|format| format.is_supported()) {
            for size in sizes(format) {
                let what = format!("{} {}", format.label(), size.name);
                // The two largest PS2 cards are the 16 MB one with more clusters, and building
                // either takes seconds in a debug build, nearly all of it spare-area ECC. What the
                // table could have wrong about them is the capacity, which the crate's own sizes
                // settle.
                #[cfg(feature = "ps2")]
                if format == Format::Ps2Card && size.capacity > 16 << 20 {
                    let capacity = usize::try_from(size.capacity).expect("fits");
                    assert!(ps2_memcard::Capacity::from_bytes(capacity).is_some(), "{what}");
                    continue;
                }
                let bytes = blank(format, &of(size.capacity)).unwrap_or_else(|e| panic!("{what}: {e}"));
                assert_eq!(detect(&bytes, "").ok(), Some(format), "{what}: known by its bytes");

                let bundle = read(format, &bytes, &CardOptions::default()).expect("reads");
                assert_eq!(
                    bundle.header.card.as_ref().map(|card| card.capacity),
                    Some(size.capacity),
                    "{what}"
                );
                let kinds: Vec<_> = bundle.parts.iter().map(|part| part.kind.clone()).collect();
                assert_eq!(kinds, [PartKind::CardImage], "{what}: nothing on it");
                assert_eq!(write(format, &bundle).expect("writes"), bytes, "{what}: and back as it was");
            }
        }
    }

    #[test]
    fn each_format_names_its_sizes_once_smallest_first_and_makes_one_unasked() {
        for format in CARDS {
            let all = sizes(format);
            assert_eq!(all.iter().filter(|size| size.standard).count(), 1, "{}", format.label());
            assert!(all.windows(2).all(|pair| pair[0].capacity < pair[1].capacity), "{}", format.label());
            for listed in all {
                assert_eq!(
                    size(format, &listed.name.to_ascii_lowercase()),
                    Some(listed),
                    "by name, any case"
                );
            }
        }
        assert!(sizes(Format::Raw).is_empty() && sizes(Format::Bundle).is_empty());
        assert_eq!(size(Format::GcCard, "251").map(|size| size.standard), Some(true));
        assert_eq!(size(Format::GcCard, "250"), None);
    }

    #[test]
    #[cfg(feature = "ps2")]
    fn a_card_asked_for_by_nothing_but_its_format_is_its_standard_one() {
        let bytes = blank(Format::Ps2Card, &BlankOptions::default()).expect("builds");
        // Sony's own 8 MB card, each 512-byte page followed by its sixteen spare bytes.
        assert_eq!(bytes.len(), 8_650_752);
    }

    /// Yabause's Saturn volume and the MiSTer core's Neo Geo save each wrap the card, and reading
    /// either finds the same empty card inside as the bare file holds.
    ///
    /// What a bundle then carries of it differs, as it does for files the emulators wrote: an
    /// empty Saturn volume is kept as the bytes that were on disk, wrapping and all, and a Neo Geo
    /// card comes out of its MiSTer save bare.
    #[test]
    #[cfg(all(feature = "saturn", feature = "neogeo"))]
    fn the_layouts_two_emulators_keep_hold_the_same_card() {
        let unwrapped = |format, bytes: &[u8]| {
            let bundle = read(format, bytes, &CardOptions::default()).expect("reads");
            write(format, &bundle).expect("writes")
        };

        let packed = blank(Format::SaturnBup, &BlankOptions::default()).expect("builds");
        let wide =
            blank(Format::SaturnBup, &BlankOptions { layout: Layout::Wide, ..BlankOptions::default() })
                .unwrap();
        assert_eq!(wide.len(), packed.len() * 2);
        assert!(wide.iter().step_by(2).all(|&byte| byte == 0xFF), "every byte behind an 0xFF");
        assert_eq!(detect(&wide, "").ok(), Some(Format::SaturnBup));
        let volume = saturn_backup::BackupRam::parse(&wide).expect("reads");
        assert_eq!(volume.data(), packed.as_slice(), "the volume inside is the bare one");
        assert_eq!(unwrapped(Format::SaturnBup, &wide), wide);

        for name in ["2K", "8K"] {
            let capacity = size(Format::NeoGeoCard, name).expect("listed").capacity;
            let bare = blank(Format::NeoGeoCard, &of(capacity)).expect("builds");
            let mister = blank(Format::NeoGeoCard, &BlankOptions { layout: Layout::MiSter, ..of(capacity) })
                .expect("fits a MiSTer save");
            assert_eq!(mister.len(), 0x10000 + 0x2000, "{name}: the cabinet's RAM, then the card's room");
            assert_eq!(detect(&mister, "").ok(), Some(Format::NeoGeoCard));
            assert_eq!(unwrapped(Format::NeoGeoCard, &mister), bare, "{name}");
        }
    }

    #[test]
    fn what_a_format_does_not_come_in_is_refused_in_words() {
        let refused = |format, options: BlankOptions| match blank(format, &options) {
            Err(Error::NoSuchCard(why)) => why,
            other => panic!("{}: {other:?}", format.label()),
        };
        assert!(refused(Format::Raw, BlankOptions::default()).contains("not a card"));
        let why = refused(Format::Ps2Card, of(4_194_304));
        assert!(why.contains("8M, 16M, 32M or 64M"), "names what there is: {why}");
        assert!(
            refused(Format::Ps1Card, BlankOptions { shift_jis: true, ..BlankOptions::default() })
                .contains("GameCube")
        );
        let why = refused(Format::Ps2Card, BlankOptions { layout: Layout::Wide, ..BlankOptions::default() });
        assert!(why.contains("Saturn"), "says whose layout it is: {why}");
        #[cfg(feature = "neogeo")]
        {
            let largest = size(Format::NeoGeoCard, "16K").expect("listed").capacity;
            let why = refused(Format::NeoGeoCard, BlankOptions { layout: Layout::MiSter, ..of(largest) });
            assert!(why.contains("MiSTer"), "{why}");
        }
    }

    /// A card formatted on a console comes back the size it was, for the console it was for.
    #[test]
    #[cfg(feature = "gc")]
    fn a_card_emptied_like_another_keeps_its_size_and_its_character_set() {
        let mut builder = gc_memcard::CardBuilder::new();
        builder.capacity(1019 * 8192).shift_jis(true).add(gc_memcard::Save {
            slot: 0,
            game_code: "GZLJ".into(),
            maker_code: "01".into(),
            filename: "gczelda".into(),
            dirent: Vec::new(),
            data: vec![0x5A; 8192],
        });
        let card = builder.build().expect("writes");
        let bundle = read(Format::GcCard, &card, &CardOptions::default()).expect("reads");
        assert!(bundle.parts.iter().any(|part| part.kind == PartKind::Bundle), "a save on it");

        let emptied = blank_like(&bundle).expect("empties");
        assert_eq!(emptied.len(), card.len());
        assert!(gc_memcard::is_shift_jis(&emptied[..gc_memcard::BLOCK]));
        let options = BlankOptions { shift_jis: true, ..of(1019 * 8192) };
        assert_eq!(emptied, blank(Format::GcCard, &options).expect("builds"));

        let save = one_saves::Bundle::new(
            one_saves::Header::new(one_saves::Shape::Save),
            vec![one_saves::Part::new(0, vec![1, 2, 3])],
        );
        assert!(matches!(blank_like(&save), Err(Error::NoSuchCard(_))), "a save is not a card");
    }
}
