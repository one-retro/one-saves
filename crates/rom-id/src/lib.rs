//! Identify a cartridge ROM dump: which system it is for, what its header says, and the hardware on
//! its cartridge.
//!
//! The cartridge counterpart of `disc-id`. A ROM's extension is a weak witness — `.bin` is half a
//! dozen systems, and a renamed file is any of them — but most consoles check something in the ROM
//! before they run it: a logo, a magic word, a checksum. This crate reads those, from a few fixed
//! places, and answers with the system, the title and code the header carries, and what the
//! cartridge holds besides the ROM: its save memory and its clock chip, which is what a save
//! manager needs to know to read the save beside it.
//!
//! ```
//! # let rom = vec![0u8; 0x8000];
//! if let Some(id) = rom_id::identify_bytes(Some("Tetris (World).gb"), &rom) {
//!     println!("{} ({:?})", id.system.slug, id.evidence);
//! }
//! ```
//!
//! # Two kinds of evidence
//!
//! A header the console checks settles the system, whatever the file is called:
//! [`Evidence::Header`]. Some consoles check nothing — an Atari 2600 or a PC Engine runs whatever is
//! mapped in — so their dumps say nothing about themselves, and the name is all there is:
//! [`Evidence::Name`], from the extension and, where the extension is shared, the folder the file
//! sits in. A caller filing a save on its own can insist on a header; one listing a library can
//! take either. When neither settles the system the answer is [`None`], never a guess.
//!
//! # I/O is the caller's
//!
//! Every header this crate reads sits at one of a few fixed places near the start of the file or at
//! its very end, and which ones depends only on the file's length. So [`Probe::windows`] lists them
//! up front, a caller reads them however it reads — blocking, async, out of an archive, over the
//! network — and [`Probe::identify`] decides. They come to about a kilobyte, whatever the ROM's
//! size. [`identify_file`] does the reading for a caller holding a [`RomFile`], and
//! [`identify_bytes`] for one holding the whole ROM.

use std::io::{self, Read, Seek, SeekFrom};
use std::ops::Range;

mod header;
mod name;

pub use header::gba_save;
pub use name::candidates;
pub use one_saves_registry::System;

/// What a ROM turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RomId {
    /// The system whose console runs this ROM, as the registry names it.
    pub system: System,
    /// How the system was told.
    pub evidence: Evidence,
    /// The title as the header spells it, which is often padded and shouty. `None` where the
    /// console's header has no title, and always for [`Evidence::Name`].
    pub title: Option<String>,
    /// The product code out of the header, where the console has one: a GBA's `AXVE`, an N64's
    /// `NSME`.
    pub serial: Option<String>,
    /// The mask ROM version the header states, where it states one.
    pub revision: Option<u8>,
    /// What the cartridge carries besides the ROM, as far as the header says.
    pub cartridge: Cartridge,
    /// The bytes of the file that are the ROM itself, which is what a catalog such as No-Intro
    /// hashes: everything but a dumper's or copier's header in front of it.
    pub payload: Range<u64>,
    /// How the file's bytes relate to the order the console reads them in.
    pub byte_order: ByteOrder,
}

/// How a ROM's system was told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Evidence {
    /// A header the console itself checks, or a dumper's container that names the system.
    Header,
    /// The file's name alone: its extension, narrowed by its folder where the extension is shared.
    /// What a headerless system's dump has to go on, and what a dump whose header does not parse
    /// falls back to.
    Name,
}

/// How a ROM file's bytes relate to the order the console reads them in.
///
/// Only Nintendo 64 dumps come in more than one: a cartridge is 16-bit words, and dumpers have
/// shipped them in all three orders. A catalog hashes the console's order, [`Native`](Self::Native).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ByteOrder {
    /// The console's own order. A `.z64`.
    #[default]
    Native,
    /// Each pair of bytes swapped. A `.v64`.
    Swapped16,
    /// Each four bytes reversed. A `.n64`.
    Swapped32,
}

/// What a cartridge carries besides its ROM.
///
/// Read from the header, or for the few consoles that do not say, from the game's product code.
/// `None` in a field means the header does not say, which for most systems is all a header ever
/// says about either.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cartridge {
    /// The memory that keeps the game's saves while the console is off.
    pub save: Option<Save>,
    /// The real-time clock chip, when there is one.
    pub clock: Option<Clock>,
}

/// A cartridge's save memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Save {
    /// What kind of memory it is.
    pub kind: SaveKind,
    /// Its size in bytes, when the header states it.
    pub size: Option<u32>,
    /// The byte lane of a 16-bit bus it sits on, for memory wired to one lane alone: most Mega
    /// Drive SRAM, on the odd one. `None` for memory as wide as its bus.
    pub lane: Option<Lane>,
}

/// One byte lane of a 16-bit bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    /// The odd addresses, the low byte of each word on a big-endian bus.
    Odd,
    /// The even addresses.
    Even,
}

/// A kind of save memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveKind {
    /// Static RAM, kept by a battery.
    Sram,
    /// EEPROM, which keeps its contents without one.
    Eeprom,
    /// Flash memory.
    Flash,
}

/// A cartridge's real-time clock chip, one for each chip key the Universal Saves Format defines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clock {
    /// An MBC3 or MBC30 with its timer: Pokémon Gold, Silver and Crystal.
    Mbc3,
    /// The clock in a Hudson HuC-3: the Robopon games.
    Huc3,
    /// The Toshiba TC8521 on the Bandai TAMA5 board, which only the Tamagotchi game uses.
    Tc8521,
    /// The TPP1 mapper's clock, when the header asks for it.
    Tpp1,
    /// The Seiko S-3511A, on a GBA cartridge or behind a WonderSwan's Bandai 2003 mapper.
    S3511a,
    /// The Sharp S-RTC in Daikaijuu Monogatari II.
    Srtc,
    /// The Epson RTC-4513 beside the SPC7110 in Tengai Makyou Zero.
    Rtc4513,
    /// The clock an N64 cartridge answers on the Joybus: Doubutsu no Mori, or a homebrew ROM whose
    /// header asks for it.
    Joybus,
}

impl Clock {
    /// The extension key a bundle carries this chip's state under.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Clock::Mbc3 => "x.1sav.rtc.mbc3",
            Clock::Huc3 => "x.1sav.rtc.huc3",
            Clock::Tc8521 => "x.1sav.rtc.tc8521",
            Clock::Tpp1 => "x.1sav.rtc.tpp1",
            Clock::S3511a => "x.1sav.rtc.s3511a",
            Clock::Srtc => "x.1sav.rtc.srtc",
            Clock::Rtc4513 => "x.1sav.rtc.rtc4513",
            Clock::Joybus => "x.1sav.rtc.joybus",
        }
    }
}

/// Bytes at known offsets of a file whose length is known: the whole file, or the windows a
/// [`Probe`] was given. What every header parser reads through, so the two paths cannot disagree.
#[derive(Debug, Clone, Copy)]
struct View<'a> {
    len: u64,
    whole: Option<&'a [u8]>,
    reads: &'a [(u64, Vec<u8>)],
}

impl<'a> View<'a> {
    fn whole(rom: &'a [u8]) -> Self {
        View { len: rom.len() as u64, whole: Some(rom), reads: &[] }
    }

    /// The file's length.
    fn size(self) -> u64 {
        self.len
    }

    /// `len` bytes from `at`, when they were read.
    fn bytes(self, at: u64, len: usize) -> Option<&'a [u8]> {
        if let Some(rom) = self.whole {
            return slice(rom, at, len);
        }
        self.reads.iter().find_map(|(start, bytes)| slice(bytes, at.checked_sub(*start)?, len))
    }

    /// `len` bytes ending `back` bytes before the end of the file.
    fn tail(self, back: u64, len: usize) -> Option<&'a [u8]> {
        self.bytes(self.len.checked_sub(back)?, len)
    }
}

/// `len` bytes of `bytes` from `at`, when it has them.
fn slice(bytes: &[u8], at: u64, len: usize) -> Option<&[u8]> {
    let at = usize::try_from(at).ok()?;
    bytes.get(at..at.checked_add(len)?)
}

/// The few reads identifying a ROM takes, gathered by a caller that does its own I/O.
///
/// ```
/// let rom = vec![0u8; 0x8000];
/// let mut probe = rom_id::Probe::new(rom.len() as u64);
/// for window in probe.windows() {
///     let range = window.start as usize..window.end as usize;
///     probe.supply(window.start, rom[range].to_vec());
/// }
/// assert_eq!(probe.identify(None), rom_id::identify_bytes(None, &rom));
/// ```
#[derive(Debug, Clone)]
pub struct Probe {
    len: u64,
    reads: Vec<(u64, Vec<u8>)>,
}

impl Probe {
    /// A probe of a file `len` bytes long, holding nothing yet.
    #[must_use]
    pub fn new(len: u64) -> Self {
        Probe { len, reads: Vec::new() }
    }

    /// The ranges of the file to read: every place a header this crate knows can sit, clipped to
    /// the file and merged where they touch. About a kilobyte in all.
    #[must_use]
    pub fn windows(&self) -> Vec<Range<u64>> {
        header::windows(self.len)
    }

    /// Hands over bytes read from `offset`. Anything short of a window, or past it, is fine: a
    /// header the reads do not cover is a header not found.
    pub fn supply(&mut self, offset: u64, bytes: Vec<u8>) {
        self.reads.push((offset, bytes));
    }

    /// What the file is, from its name and the bytes supplied.
    #[must_use]
    pub fn identify(&self, name: Option<&str>) -> Option<RomId> {
        identify_view(name, View { len: self.len, whole: None, reads: &self.reads })
    }
}

/// A file a ROM can be identified from: something with a length that can be read at an offset.
///
/// Implemented for a byte slice, for [`std::fs::File`], and through [`Seekable`] for anything
/// [`Read`] and [`Seek`]. A caller with a source of its own — an entry in an archive, a file on a
/// remote — implements the two methods, and [`identify_file`] asks it for a kilobyte or so.
pub trait RomFile {
    /// The file's length in bytes.
    fn size(&mut self) -> io::Result<u64>;
    /// Fills `buf` from `offset`. Never asked for bytes past the length [`size`](Self::size) gave.
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()>;
}

impl RomFile for [u8] {
    fn size(&mut self) -> io::Result<u64> {
        Ok(self.len() as u64)
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        let bytes = slice(self, offset, buf.len()).ok_or(io::ErrorKind::UnexpectedEof)?;
        buf.copy_from_slice(bytes);
        Ok(())
    }
}

impl RomFile for std::fs::File {
    fn size(&mut self) -> io::Result<u64> {
        Ok(self.metadata()?.len())
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        Seekable(self).read_at(offset, buf)
    }
}

/// Anything [`Read`] and [`Seek`], as a [`RomFile`]: a [`Cursor`](std::io::Cursor), a buffered
/// file, a decompressor that can seek.
#[derive(Debug)]
pub struct Seekable<R>(pub R);

impl<R: Read + Seek> RomFile for Seekable<R> {
    fn size(&mut self) -> io::Result<u64> {
        let here = self.0.stream_position()?;
        let end = self.0.seek(SeekFrom::End(0))?;
        self.0.seek(SeekFrom::Start(here))?;
        Ok(end)
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        self.0.seek(SeekFrom::Start(offset))?;
        self.0.read_exact(buf)
    }
}

/// What a ROM file is, read through [`RomFile`].
///
/// `name` is the file's name or path, which is what settles a headerless dump. Reads only the
/// windows [`Probe::windows`] lists.
pub fn identify_file<F: RomFile + ?Sized>(name: Option<&str>, file: &mut F) -> io::Result<Option<RomId>> {
    let mut probe = Probe::new(file.size()?);
    for window in probe.windows() {
        let len = usize::try_from(window.end - window.start).expect("a window is a few hundred bytes");
        let mut buf = vec![0; len];
        file.read_at(window.start, &mut buf)?;
        probe.supply(window.start, buf);
    }
    Ok(probe.identify(name))
}

/// What a ROM is, from the whole of it in memory.
#[must_use]
pub fn identify_bytes(name: Option<&str>, rom: &[u8]) -> Option<RomId> {
    identify_view(name, View::whole(rom))
}

/// The decision itself: the first header that holds, else the name.
///
/// A strong header settles the system against anything the name says. A weak one — two bytes, or
/// a footer with nothing but its shape to check — counts only where the name does not point
/// elsewhere, so a `.a26` that happens to open with ColecoVision's magic stays a 2600.
fn identify_view(name: Option<&str>, view: View<'_>) -> Option<RomId> {
    let named = name.map(candidates).unwrap_or_default();
    for (strength, parse) in header::PARSERS {
        let Some(found) = parse(view, &named) else { continue };
        if *strength == header::Strength::Weak && !named.is_empty() && !named.contains(&found.system) {
            continue;
        }
        return Some(found);
    }
    match named.as_slice() {
        [only] => Some(RomId {
            system: *only,
            evidence: Evidence::Name,
            title: None,
            serial: None,
            revision: None,
            cartridge: Cartridge::default(),
            payload: 0..view.size(),
            byte_order: ByteOrder::Native,
        }),
        _ => None,
    }
}
