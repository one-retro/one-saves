//! Nintendo 64 cartridge saves, and the one file a libretro core keeps a whole console's in.
//!
//! An N64 cartridge carries one of five chips ([`N64Save`]), and a bundle carries that chip's
//! memory in **normal form**: the bytes in the order the console reads them, at the chip's real
//! size, under the [`x.1sav.n64`](N64) key naming the chip. A flash cartridge and a cartridge
//! reader produce exactly that. Emulators differ from it in two ways, and both are undone here:
//!
//! - **Word order.** mupen64plus and Project64 keep SRAM and FlashRAM as host-order 32-bit words,
//!   which on every host they run on reverses the four bytes of each word, and the MiSTer core
//!   writes its files the same way round. EEPROM is never affected: the console reaches it over
//!   the Joybus a byte at a time, so it is a byte stream to everyone. [`WordOrder`] says which a
//!   file is in.
//! - **Length.** mupen64plus writes a 4 Kbit EEPROM as 512 bytes and wrote it as 2 KiB before
//!   2017, and Project64 writes each chip only as far as the game has written it. So a file can be
//!   shorter or longer than its chip, and [`normalize`] pads with the erased value and trims only
//!   what is erased.
//!
//! Neither the bytes nor their length say which chip a game has, which is why every call here
//! takes one. Where a caller does not know, [`read_srm`] can tell from a libretro save only what
//! that file can prove, and refuses the rest.
//!
//! # A libretro save
//!
//! Mupen64Plus-Next and ParaLLEl N64 both write one `.srm` per game holding every chip at once,
//! each at a fixed offset whatever the game uses, and all four Controller Paks after the EEPROM:
//!
//! | Offset    | Length        | What                                |
//! | --------- | ------------- | ----------------------------------- |
//! | `0x00000` | `0x800`       | EEPROM, 4 Kbit games in the first 512 bytes |
//! | `0x00800` | 4 × `0x8000`  | Controller Paks, port 1 to port 4   |
//! | `0x20800` | `0x8000`      | SRAM                                |
//! | `0x28800` | `0x20000`     | FlashRAM                            |
//!
//! That is a console's storage rather than one game's save, because the paks hold notes for games
//! the cartridge knows nothing about, so [`read_srm`] makes a [`device`](one_saves::Shape::Device)
//! of it: the cartridge as a nested save, and each pak that holds a note as a nested card.
//! [`game_save`] cuts one game out of such a device as a plain save, its cartridge beside only its
//! own notes, and [`write_srm`] takes either shape back.

use one_saves::extension::{N64, N64Save};
use one_saves::{Bundle, Game, Header, Part, PartKind, Shape, Slug, Source};

use crate::error::{Error, Result};

/// The system these saves are for.
pub const SYSTEM: &str = "n64";

/// The role of the part holding a cartridge's save memory.
pub const CARTRIDGE: &str = "cartridge";

/// What a game code is prefixed with to make the serial, as N64 product codes are.
const SERIAL_PREFIX: &str = "NUS-";

/// A game code as the serial a save names: `NKTE` is `NUS-NKTE`.
///
/// The four characters are what a cartridge's header and a Controller Pak note's entry both carry,
/// so a cartridge save and the game's notes name one serial.
#[must_use]
pub fn serial_of(game_code: &str) -> String {
    format!("{SERIAL_PREFIX}{game_code}")
}

/// The game code a serial carries. A bare code, which is what a bundle from before 0.7 named a
/// note's game by, comes back as it is.
#[must_use]
pub fn game_code_of(serial: &str) -> &str {
    serial.strip_prefix(SERIAL_PREFIX).unwrap_or(serial)
}

/// What a chip holds where nothing has been written: an erased EEPROM or FlashRAM cell reads as
/// all ones, and every emulator formats SRAM the same way.
const ERASED: u8 = 0xFF;

/// The order a file keeps a chip's 32-bit words in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordOrder {
    /// The console's own, big-endian. Normal form, and what a flash cartridge writes.
    Console,
    /// Each word's bytes reversed, as mupen64plus, Project64, the libretro cores and the MiSTer
    /// core keep SRAM and FlashRAM. Ignored for EEPROM, which nothing reverses.
    Swapped,
}

/// How to wrap a cartridge save.
#[derive(Debug, Clone, Default)]
pub struct N64Options {
    /// Who produced the bytes.
    pub source: Option<Source>,
    /// What is known about the game.
    pub game: Option<Game>,
    /// A human-readable note about the bundle.
    pub description: Option<String>,
    /// When the bundle was assembled, in whole epoch seconds.
    pub bundled_at: Option<i64>,
}

/// A chip's memory in normal form, from a file as an emulator wrote it.
///
/// A short file is padded with the erased value, since an emulator writing only as far as the game
/// has is not saying the rest holds anything. A long one is trimmed, but only where the excess is
/// erased: anything else past the end of the chip is data, and dropping data is not something to
/// do because a caller named a chip.
pub fn normalize(bytes: &[u8], save: N64Save, order: WordOrder) -> Result<Vec<u8>> {
    let size = save.size();
    if bytes.len() > size && !is_erased(&bytes[size..]) {
        return Err(Error::NotConvertible(format!(
            "this save holds data past the end of a {save} chip ({} bytes, the chip has {size})",
            bytes.len()
        )));
    }
    let mut normal = bytes[..bytes.len().min(size)].to_vec();
    normal.resize(size, ERASED);
    if order == WordOrder::Swapped && !save.is_eeprom() {
        swap_words(&mut normal);
    }
    Ok(normal)
}

/// A chip's memory as a file in `order`, from normal form.
pub fn denormalize(normal: &[u8], save: N64Save, order: WordOrder) -> Result<Vec<u8>> {
    if normal.len() != save.size() {
        return Err(Error::NotConvertible(format!(
            "a {save} save is {} bytes, and this one is {}",
            save.size(),
            normal.len()
        )));
    }
    let mut bytes = normal.to_vec();
    if order == WordOrder::Swapped && !save.is_eeprom() {
        swap_words(&mut bytes);
    }
    Ok(bytes)
}

/// Wraps one chip's file in a save bundle, its memory in normal form on a `cartridge` part.
pub fn wrap(bytes: &[u8], save: N64Save, order: WordOrder, options: &N64Options) -> Result<Bundle> {
    if bytes.is_empty() {
        return Err(Error::NotConvertible("this save is zero bytes".into()));
    }
    let normal = normalize(bytes, save, order)?;
    let mut header = Header::new(Shape::Save)
        .bundled_at(options.bundled_at)
        .system(slug(SYSTEM))
        .game(options.game.clone())
        .source(options.source.clone());
    header.description.clone_from(&options.description);
    Ok(Bundle::new(header, vec![cartridge_part(0, normal, save)]))
}

/// The chip a bundle's cartridge part holds, and its memory as a file in `order`.
///
/// Reads a save, the cartridge alone or beside its notes, and a device, out of its `cartridge`
/// component.
pub fn unwrap(bundle: &Bundle, order: WordOrder) -> Result<(N64Save, Vec<u8>)> {
    let (save, normal) = cartridge_of(bundle)?
        .ok_or_else(|| Error::NotConvertible("this bundle holds no N64 cartridge save".into()))?;
    Ok((save, denormalize(&normal, save, order)?))
}

/// The cartridge part: the chip's memory in normal form, and the key saying which chip.
fn cartridge_part(id: u64, normal: Vec<u8>, save: N64Save) -> Part {
    Part::new(id, normal).role(slug(CARTRIDGE)).extension(N64::new(save))
}

/// The chip a bundle's cartridge holds and its memory in normal form, or `None` for a bundle
/// with no cartridge in it, such as a device of paks alone.
pub fn cartridge(bundle: &Bundle) -> Result<Option<(N64Save, Vec<u8>)>> {
    cartridge_of(bundle)
}

/// How many bundles deep a cartridge is looked for beneath the one it is asked of.
///
/// The shapes this version defines nest three deep at most — a collection holding a device
/// holding a card holding a save — and a device's cartridge is one bundle down. Validation holds a
/// bundle to that only for a shape it knows, though, so a file of nested bundles of a shape it
/// does not is cut off here rather than followed down. One level past the defined shapes leaves
/// room for a later version's.
const MAX_DEPTH: usize = 4;

/// The chip and its normal-form memory, out of a save's `cartridge` part or a device's.
///
/// Inside a device's cartridge component the save is the component's only socket, so its part
/// carries no role, and the key is what marks it.
fn cartridge_of(bundle: &Bundle) -> Result<Option<(N64Save, Vec<u8>)>> {
    let mut nested: Option<Bundle> = None;
    for _ in 0..=MAX_DEPTH {
        let current = nested.as_ref().unwrap_or(bundle);
        let Some(part) = current.parts.iter().find(|part| {
            part.role.as_ref().is_some_and(|r| r.as_str() == CARTRIDGE)
                || part.extensions.get::<N64>().is_some()
        }) else {
            return Ok(None);
        };
        if part.kind == PartKind::Bundle {
            nested = Some(Bundle::from_slice(&part.bytes()?)?);
            continue;
        }
        // A part with no key is a cartridge save whose chip nobody recorded, and the length of its
        // bytes is exactly the thing that cannot settle it.
        let save = part.extensions.get::<N64>().map(|n64| n64.save).ok_or_else(|| {
            Error::NotConvertible("this cartridge save does not say which chip it is for".into())
        })?;
        return Ok(Some((save, part.bytes()?.into_owned())));
    }
    Err(Error::NotConvertible(format!(
        "this bundle nests its cartridge more than {MAX_DEPTH} bundles deep, deeper than any shape does"
    )))
}

/// Reverses each 32-bit word's bytes, which is its own inverse.
fn swap_words(bytes: &mut [u8]) {
    for word in bytes.as_chunks_mut::<4>().0 {
        word.reverse();
    }
}

/// Whether a run of bytes holds nothing a game wrote.
fn is_erased(bytes: &[u8]) -> bool {
    bytes.iter().all(|&byte| byte == ERASED)
}

fn slug(text: &str) -> Slug {
    Slug::parse(text).expect("a spec slug is well-formed")
}

#[cfg(feature = "n64")]
pub use srm::{
    PAKS_LEN, PORTS, SRM_LEN, game_save, pak_ports, read_paks, read_srm, write_pak, write_paks, write_srm,
};

#[cfg(feature = "n64")]
mod srm {
    use n64_cpak::{ControllerPak, Note, PakBuilder};
    use one_saves::extension::{Label, N64Save};
    use one_saves::{Bundle, Game, Header, Part, PartKind, Shape, Slug};

    use super::{
        CARTRIDGE, ERASED, N64Options, SYSTEM, WordOrder, cartridge_of, cartridge_part, serial_of, slug,
    };
    use crate::card::{CardOptions, nested_saves};
    use crate::error::{Error, Result};

    /// How long a libretro N64 save is, whatever the game keeps in it.
    pub const SRM_LEN: usize = PAKS_AT + PORTS * n64_cpak::CAPACITY + SRAM_LEN + FLASH_LEN;

    /// How many controller ports a console has, and so how many paks one of these files holds.
    pub const PORTS: usize = 4;

    /// How long a file of all four paks is: mupen64plus keeps one per game, beside its chip file.
    pub const PAKS_LEN: usize = PORTS * n64_cpak::CAPACITY;

    const EEPROM_LEN: usize = 0x800;
    const PAKS_AT: usize = EEPROM_LEN;
    const SRAM_AT: usize = PAKS_AT + PORTS * n64_cpak::CAPACITY;
    const SRAM_LEN: usize = 0x8000;
    const FLASH_AT: usize = SRAM_AT + SRAM_LEN;
    const FLASH_LEN: usize = 0x20000;

    /// What the format is called, for error messages.
    const FORMAT: &str = "libretro N64 save";

    /// Where a chip's memory sits in the file, and how much room it has there.
    ///
    /// SRAM has 32 KiB, so Dezaemon 3D's three banks do not fit, and no libretro core saves it.
    fn region(save: N64Save) -> Option<(usize, usize)> {
        match save {
            N64Save::Eeprom4k | N64Save::Eeprom16k => Some((0, EEPROM_LEN)),
            N64Save::Sram256k => Some((SRAM_AT, SRAM_LEN)),
            N64Save::Sram768k => None,
            N64Save::Flash1m => Some((FLASH_AT, FLASH_LEN)),
        }
    }

    /// The role of the pak in a port, counted from 1 as the console's ports are.
    fn pak_role(port: usize) -> String {
        format!("controller-pak-{}", port + 1)
    }

    /// The port a `controller-pak-N` role names, where it names one this file has.
    fn port_of(role: &str) -> Option<usize> {
        let number: usize = role.strip_prefix("controller-pak-")?.parse().ok()?;
        (1..=PORTS).contains(&number).then(|| number - 1)
    }

    /// A region that holds nothing, either way an emulator may have left it.
    ///
    /// Every current core formats with ones, and a save of nothing but zeroes is not one a game
    /// could load from: it would fail any checksum it keeps.
    fn is_blank(bytes: &[u8]) -> bool {
        bytes.iter().all(|&b| b == ERASED) || bytes.iter().all(|&b| b == 0)
    }

    /// Reads a libretro save as the device it is: the cartridge, and every pak that holds a note.
    ///
    /// `save` is the chip the game has, when the caller knows it from the ROM or a catalog. When it
    /// does not, the file settles it only where it can prove it: exactly one chip's region has been
    /// written, and for EEPROM, a byte has been written past the first 512, which only a 16 Kbit
    /// part has. Anything less is refused rather than guessed at, because the guess decides which
    /// file a restore writes and a wrong one leaves the game looking somewhere else.
    ///
    /// A pak with no notes is left out: each core formats all four, so one nobody saved to holds
    /// nothing but the serial its core gave it.
    pub fn read_srm(bytes: &[u8], save: Option<N64Save>, options: &N64Options) -> Result<Bundle> {
        if bytes.len() != SRM_LEN {
            return Err(Error::WrongLength { format: FORMAT, expected: "296960 bytes", found: bytes.len() });
        }

        let mut parts = Vec::new();
        if let Some((save, normal)) = cartridge_in(bytes, save)? {
            let mut header = Header::new(Shape::Save).system(slug(SYSTEM)).game(options.game.clone());
            header.description.clone_from(&options.description);
            let inner = Bundle::new(header, vec![cartridge_part(0, normal, save).role(None)]);
            parts.push(
                Part::new(0, inner.to_vec()?)
                    .kind(PartKind::Bundle)
                    .role(slug(CARTRIDGE))
                    .game(options.game.clone())
                    .system(slug(SYSTEM)),
            );
        }

        paks_into(&mut parts, &bytes[PAKS_AT..PAKS_AT + PAKS_LEN], FORMAT)?;
        device(parts, options)
    }

    /// Reads mupen64plus's file of all four paks as a device of the ones that hold a note.
    pub fn read_paks(bytes: &[u8], options: &N64Options) -> Result<Bundle> {
        if bytes.len() != PAKS_LEN {
            return Err(Error::WrongLength {
                format: PAKS_FORMAT,
                expected: "131072 bytes",
                found: bytes.len(),
            });
        }
        let mut parts = Vec::new();
        paks_into(&mut parts, bytes, PAKS_FORMAT)?;
        device(parts, options)
    }

    /// What mupen64plus's file of four paks is called, for error messages.
    const PAKS_FORMAT: &str = "mupen64plus Controller Pak file";

    /// A device of these components, or a refusal where the file held nothing.
    fn device(parts: Vec<Part>, options: &N64Options) -> Result<Bundle> {
        if parts.is_empty() {
            return Err(Error::NotConvertible("this save holds nothing yet".into()));
        }
        let mut header = Header::new(Shape::Device)
            .bundled_at(options.bundled_at)
            .system(slug(SYSTEM))
            .source(options.source.clone());
        header.description.clone_from(&options.description);
        Ok(Bundle::new(header, parts))
    }

    /// Adds a component for each of four paks in a row that holds a note.
    fn paks_into(parts: &mut Vec<Part>, paks: &[u8], format: &'static str) -> Result<()> {
        for port in 0..PORTS {
            let image = &paks[port * n64_cpak::CAPACITY..(port + 1) * n64_cpak::CAPACITY];
            if is_blank(image) {
                continue;
            }
            let pak = ControllerPak::parse(image).map_err(|e| Error::Corrupt {
                format,
                why: format!("the pak in port {} is not one: {e}", port + 1),
            })?;
            if pak.notes().is_empty() {
                continue;
            }
            let card = crate::card::n64::read(
                image,
                &CardOptions { role: Some(slug(&pak_role(port))), source: None, system: None },
            )?;
            let id = u64::try_from(parts.len()).expect("part count fits");
            parts.push(
                Part::new(id, card.to_vec()?)
                    .kind(PartKind::Bundle)
                    .role(slug(&pak_role(port)))
                    .system(slug(SYSTEM)),
            );
        }
        Ok(())
    }

    /// The cartridge's chip and its memory in normal form, or `None` where the game has not saved.
    fn cartridge_in(bytes: &[u8], save: Option<N64Save>) -> Result<Option<(N64Save, Vec<u8>)>> {
        let eeprom = &bytes[..EEPROM_LEN];
        let sram = &bytes[SRAM_AT..SRAM_AT + SRAM_LEN];
        let flash = &bytes[FLASH_AT..FLASH_AT + FLASH_LEN];

        let Some(save) = save.map_or_else(|| chip_written(eeprom, sram, flash), |save| Ok(Some(save)))?
        else {
            return Ok(None);
        };

        let (at, _) = region(save)
            .ok_or_else(|| Error::NotConvertible(format!("a libretro save has no room for a {save} chip")))?;
        let memory = &bytes[at..at + save.size()];
        if is_blank(memory) {
            return Ok(None);
        }
        // A 16 Kbit EEPROM's region is exactly its chip, and a 4 Kbit one's rest is the core's
        // padding, which normalizing a 512-byte chip leaves behind.
        Ok(Some((save, super::normalize(memory, save, WordOrder::Swapped)?)))
    }

    /// The chip a file's regions prove, where nobody said which: the one region written, and for
    /// EEPROM a byte past the first 512, which only a 16 Kbit chip reaches. `None` for a file
    /// nothing has been written to.
    fn chip_written(eeprom: &[u8], sram: &[u8], flash: &[u8]) -> Result<Option<N64Save>> {
        let written: Vec<&str> = [("EEPROM", eeprom), ("SRAM", sram), ("FlashRAM", flash)]
            .into_iter()
            .filter(|(_, region)| !is_blank(region))
            .map(|(name, _)| name)
            .collect();
        match written.as_slice() {
            [] => Ok(None),
            ["EEPROM"] if !is_blank(&eeprom[N64Save::Eeprom4k.size()..]) => Ok(Some(N64Save::Eeprom16k)),
            ["EEPROM"] => Err(Error::NotConvertible(
                "this EEPROM save fits a 4 Kbit chip and a 16 Kbit one alike; name the chip".into(),
            )),
            ["SRAM"] => Ok(Some(N64Save::Sram256k)),
            ["FlashRAM"] => Ok(Some(N64Save::Flash1m)),
            several => Err(Error::NotConvertible(format!(
                "this save holds {} at once, and a cartridge has one; name the chip",
                several.join(" and ")
            ))),
        }
    }

    /// Writes a libretro save, onto the one already there where there is one.
    ///
    /// A device puts each of its components back whole, a pak keeping the ID block of the one it
    /// replaces, so that it is still the pak the core gave that port. A save is one game's: its
    /// cartridge goes in its chip's region, and each of its notes replaces that game's notes on the
    /// pak in the port its role names, leaving every other game's notes where they were. Whatever
    /// the bundle says nothing about — another chip's region, a pak it holds no note for — is kept
    /// from `onto`, or written as a core formats it.
    pub fn write_srm(bundle: &Bundle, onto: Option<&[u8]>) -> Result<Vec<u8>> {
        let mut file = match onto {
            Some(bytes) if bytes.len() == SRM_LEN => bytes.to_vec(),
            Some(bytes) => {
                return Err(Error::WrongLength {
                    format: FORMAT,
                    expected: "296960 bytes",
                    found: bytes.len(),
                });
            }
            None => fresh()?,
        };

        if let Some((save, normal)) = cartridge_of(bundle)? {
            let (at, _) = region(save).ok_or_else(|| {
                Error::NotConvertible(format!("a libretro save has no room for a {save} chip"))
            })?;
            let bytes = super::denormalize(&normal, save, WordOrder::Swapped)?;
            file[at..at + bytes.len()].copy_from_slice(&bytes);
        }

        for port in pak_ports(bundle)? {
            let at = PAKS_AT + (port - 1) * n64_cpak::CAPACITY;
            let pak = pak_for(bundle, port, &file[at..at + n64_cpak::CAPACITY])?;
            file[at..at + n64_cpak::CAPACITY].copy_from_slice(&pak);
        }
        Ok(file)
    }

    /// Writes mupen64plus's file of all four paks, onto the one already there where there is one.
    ///
    /// What [`write_srm`] does to the paks, and nothing else: a cartridge in the bundle is the
    /// chip file's business.
    pub fn write_paks(bundle: &Bundle, onto: Option<&[u8]>) -> Result<Vec<u8>> {
        let mut file = match onto {
            Some(bytes) if bytes.len() == PAKS_LEN => bytes.to_vec(),
            Some(bytes) => {
                return Err(Error::WrongLength {
                    format: PAKS_FORMAT,
                    expected: "131072 bytes",
                    found: bytes.len(),
                });
            }
            None => formatted_pak()?.repeat(PORTS),
        };
        for port in pak_ports(bundle)? {
            let at = (port - 1) * n64_cpak::CAPACITY;
            let pak = pak_for(bundle, port, &file[at..at + n64_cpak::CAPACITY])?;
            file[at..at + n64_cpak::CAPACITY].copy_from_slice(&pak);
        }
        Ok(file)
    }

    /// Writes the pak in one port, numbered from 1, onto the pak already there where there is one.
    ///
    /// `None` when the bundle holds nothing for that port. For a producer that keeps a file per pak,
    /// as Project64 does (`<game>_Cont_<port>.mpk`) and a flash cartridge does for the one pak it
    /// emulates.
    pub fn write_pak(bundle: &Bundle, port: usize, onto: Option<&[u8]>) -> Result<Option<Vec<u8>>> {
        if !pak_ports(bundle)?.contains(&port) {
            return Ok(None);
        }
        let base = match onto {
            Some(bytes) => bytes.to_vec(),
            None => formatted_pak()?,
        };
        pak_for(bundle, port, &base).map(Some)
    }

    /// The ports, numbered from 1, a bundle holds a pak or a note for.
    ///
    /// A device names each pak as a component; a save names each note's. A role this format has no
    /// place for is refused rather than dropped, since dropping it would lose a component.
    pub fn pak_ports(bundle: &Bundle) -> Result<Vec<usize>> {
        if !matches!(bundle.header.shape, Shape::Device | Shape::Save) {
            return Err(Error::NotConvertible(format!(
                "a {} bundle is not an N64 save or a device",
                bundle.header.shape
            )));
        }
        let mut ports = Vec::new();
        for part in &bundle.parts {
            let role = part.role.as_ref().map(Slug::as_str).unwrap_or_default();
            if role == CARTRIDGE || part.extensions.get::<one_saves::extension::N64>().is_some() {
                continue;
            }
            let port = port_of(role)
                .ok_or_else(|| Error::NotConvertible(format!("an N64 save has no place for {role:?}")))?;
            if !ports.contains(&(port + 1)) {
                ports.push(port + 1);
            }
        }
        ports.sort_unstable();
        Ok(ports)
    }

    /// The pak a bundle puts in a port: a device's component whole, or a save's notes merged onto
    /// the pak that is there.
    fn pak_for(bundle: &Bundle, port: usize, there: &[u8]) -> Result<Vec<u8>> {
        let role = pak_role(port - 1);
        let parts: Vec<&Part> = bundle
            .parts
            .iter()
            .filter(|part| part.role.as_ref().is_some_and(|r| r.as_str() == role))
            .collect();
        if bundle.header.shape == Shape::Device {
            let [component] = parts.as_slice() else {
                return Err(Error::NotConvertible(format!(
                    "this device holds {} paks in {role}",
                    parts.len()
                )));
            };
            return crate::card::n64::write_onto(&Bundle::from_slice(&component.bytes()?)?, there);
        }
        merge_notes(there, &parts)
    }

    /// A save no game has written to: erased chips, and four freshly formatted paks.
    fn fresh() -> Result<Vec<u8>> {
        let mut file = vec![ERASED; SRM_LEN];
        file[PAKS_AT..PAKS_AT + PAKS_LEN].copy_from_slice(&formatted_pak()?.repeat(PORTS));
        Ok(file)
    }

    /// A pak as a console formats it, with nothing on it.
    fn formatted_pak() -> Result<Vec<u8>> {
        PakBuilder::new().build().map_err(|e| Error::NotConvertible(e.to_string()))
    }

    /// One game's notes written onto a pak, in place of the ones it already holds for that game.
    ///
    /// A note's game is its game code, which the entry carries. Every note the pak holds under one
    /// of the incoming codes goes, so a save the game deleted since does not linger beside the
    /// ones it kept. The pak keeps its own page 0, so it is the same pak afterwards.
    fn merge_notes(pak: &[u8], incoming: &[&Part]) -> Result<Vec<u8>> {
        let notes: Vec<Note> = incoming.iter().map(|part| note_of(part)).collect::<Result<_>>()?;
        let mut builder = PakBuilder::new();
        let existing = match ControllerPak::parse(pak) {
            Ok(existing) => {
                builder.id_area(existing.id_area().to_vec());
                existing.into_notes()
            }
            // What is there is not a pak, so there is nothing on it to keep.
            Err(_) => Vec::new(),
        };
        for note in existing {
            if !notes.iter().any(|incoming| incoming.game_code == note.game_code) {
                builder.add(note);
            }
        }
        for note in notes {
            builder.add(note);
        }
        builder.build().map_err(|e| Error::NotConvertible(format!("could not write the pak: {e}")))
    }

    /// A save's note part as the note a pak holds.
    fn note_of(part: &Part) -> Result<Note> {
        let dirent = part.dirent.clone().ok_or_else(|| {
            Error::NotConvertible("a note off a Controller Pak carries the pak's entry for it".into())
        })?;
        if dirent.len() != n64_cpak::NOTE_ENTRY {
            return Err(Error::NotConvertible(format!(
                "a Controller Pak entry is {} bytes, and this one is {}",
                n64_cpak::NOTE_ENTRY,
                dirent.len()
            )));
        }
        let path = part.path.clone().unwrap_or_default();
        let (name, extension) = crate::card::n64::name_and_extension(&path);
        Ok(Note {
            slot: 0,
            game_code: String::from_utf8_lossy(&dirent[..4]).into_owned(),
            name: name.to_owned(),
            extension: extension.to_owned(),
            dirent,
            data: part.bytes()?.into_owned(),
        })
    }

    /// One game's save out of a device or a lone pak: its cartridge, and only its own notes beside
    /// it.
    ///
    /// `game_code` is the four characters a ROM's header and a note's entry both carry, such as
    /// `NKTE`, or the serial a note's save names, `NUS-NKTE`. The result is a plain save with
    /// ordinary parts: the cartridge, and each note on the pak's role with the pak's entry for it,
    /// which is everything [`write_srm`] needs to put the note back on a pak that also holds other
    /// games'. A lone pak has no cartridge, and its notes keep the role it was read with, which is
    /// the port they go back to.
    ///
    /// The save is that game's, so its header names it: as the cartridge or the pak was named
    /// when it was read, and by the serial every note carries where that names none. Its label is
    /// the one its notes are listed under, where they agree on one.
    pub fn game_save(bundle: &Bundle, game_code: &str) -> Result<Bundle> {
        let game_code = super::game_code_of(game_code);

        let mut parts = Vec::new();
        let mut labels = Vec::new();
        let mut game: Option<Game> = None;
        let mut description = None;
        let what = match &bundle.header.shape {
            Shape::Device => {
                for component in &bundle.parts {
                    let role = component.role.as_ref().map(Slug::as_str).unwrap_or_default();
                    let inner = Bundle::from_slice(&component.bytes()?)?;
                    if role == CARTRIDGE {
                        if let Some((save, normal)) = cartridge_of(&inner)? {
                            game.clone_from(&inner.header.game);
                            description.clone_from(&inner.header.description);
                            parts.push(cartridge_part(0, normal, save));
                        }
                        continue;
                    }
                    notes_into(&mut parts, &mut labels, &inner, component.role.as_ref(), game_code)?;
                }
                "device"
            }
            Shape::Card if is_pak(bundle) => {
                game.clone_from(&bundle.header.game);
                description.clone_from(&bundle.header.description);
                notes_into(&mut parts, &mut labels, bundle, None, game_code)?;
                "pak"
            }
            shape => {
                return Err(Error::NotConvertible(format!(
                    "a {shape} bundle is not a device or a Controller Pak"
                )));
            }
        };
        if parts.is_empty() {
            return Err(Error::NotConvertible(format!("this {what} holds nothing of {game_code}'s")));
        }
        for (id, part) in parts.iter_mut().enumerate() {
            part.id = u64::try_from(id).expect("part count fits");
        }

        let mut game = game.unwrap_or_default();
        if game.serial.is_none() {
            game = game.serial(serial_of(game_code));
        }
        let mut header = Header::new(Shape::Save)
            .bundled_at(bundle.header.bundled_at)
            .system(slug(SYSTEM))
            .game(game)
            .source(bundle.header.source.clone());
        header.description = description;
        if let [label] = labels.as_slice() {
            header = header.extension(label.clone());
        }
        Ok(Bundle::new(header, parts))
    }

    /// Whether a card is a Controller Pak, the one card whose notes [`game_save`] cuts.
    fn is_pak(bundle: &Bundle) -> bool {
        bundle.header.card.as_ref().is_some_and(|card| card.format.as_str() == crate::card::n64::CARD_FORMAT)
    }

    /// Adds a pak's notes for one game, each on the role it goes back to: `role` where the pak is
    /// a device's component, and otherwise the one its own part was read with. `labels` gathers
    /// the names they are listed under, each once.
    fn notes_into(
        parts: &mut Vec<Part>,
        labels: &mut Vec<Label>,
        pak: &Bundle,
        role: Option<&Slug>,
        game_code: &str,
    ) -> Result<()> {
        // `nested_saves` takes a card's `bundle` parts in order, so these line up with its saves.
        let roles =
            pak.parts.iter().filter(|part| part.kind == PartKind::Bundle).map(|part| part.role.as_ref());
        for (note, own) in nested_saves(pak)?.into_iter().zip(roles) {
            let code = note.dirent.as_deref().and_then(|entry| entry.get(..4));
            if code != Some(game_code.as_bytes()) {
                continue;
            }
            // A role naming no port would be a save nothing can write back, so it is refused here
            // rather than at the restore.
            let role = role.or(own).cloned();
            let name = role.as_ref().map(Slug::as_str).unwrap_or_default();
            if port_of(name).is_none() {
                return Err(Error::NotConvertible(format!(
                    "a note on {name:?} has no controller port to go back to"
                )));
            }
            if let Some(label) = note.inner.header.extensions.get::<Label>()
                && !labels.contains(label)
            {
                labels.push(label.clone());
            }
            let mut part = Part::new(0, note.bytes).role(role).slot(note.slot);
            part.path = note.path;
            part.dirent = note.dirent;
            parts.push(part);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sram_and_flash_words_are_reversed_and_eeprom_is_not() {
        let file: Vec<u8> = (0u8..8).collect();
        assert_eq!(
            normalize(&file, N64Save::Sram256k, WordOrder::Swapped).unwrap()[..8],
            [3, 2, 1, 0, 7, 6, 5, 4]
        );
        assert_eq!(
            normalize(&file, N64Save::Flash1m, WordOrder::Swapped).unwrap()[..8],
            [3, 2, 1, 0, 7, 6, 5, 4]
        );
        assert_eq!(
            normalize(&file, N64Save::Eeprom4k, WordOrder::Swapped).unwrap()[..8],
            [0, 1, 2, 3, 4, 5, 6, 7]
        );
        assert_eq!(
            normalize(&file, N64Save::Sram256k, WordOrder::Console).unwrap()[..8],
            [0, 1, 2, 3, 4, 5, 6, 7]
        );
    }

    #[test]
    fn a_short_file_is_padded_with_what_an_erased_chip_reads() {
        // Project64 writes a chip only as far as the game has: 16 bytes of a 2 KiB EEPROM.
        let normal = normalize(&[7u8; 16], N64Save::Eeprom16k, WordOrder::Swapped).unwrap();
        assert_eq!(normal.len(), 2048);
        assert_eq!(&normal[..16], &[7u8; 16]);
        assert!(normal[16..].iter().all(|&b| b == 0xFF));
    }

    #[test]
    fn a_long_file_is_trimmed_only_where_nothing_was_written() {
        // mupen64plus wrote every EEPROM as 2 KiB before 2017, whatever the chip.
        let mut old = vec![0xFFu8; 2048];
        old[..512].fill(1);
        assert_eq!(normalize(&old, N64Save::Eeprom4k, WordOrder::Console).unwrap(), vec![1u8; 512]);

        old[600] = 0;
        assert!(normalize(&old, N64Save::Eeprom4k, WordOrder::Console).is_err(), "that is data");
    }

    #[test]
    fn a_wrapped_chip_names_itself_and_comes_back_as_the_file() {
        let file: Vec<u8> = (0..32 * 1024).map(|i| u8::try_from(i % 251).unwrap()).collect();
        let bundle = wrap(&file, N64Save::Sram256k, WordOrder::Swapped, &N64Options::default()).unwrap();
        bundle.validate().unwrap();

        let part = &bundle.parts[0];
        assert_eq!(part.role.as_ref().unwrap().as_str(), "cartridge");
        assert_eq!(part.extensions.get::<N64>(), Some(&N64::new(N64Save::Sram256k)));
        assert_eq!(&part.bytes().unwrap()[..4], &[3, 2, 1, 0], "normal form inside");

        assert_eq!(unwrap(&bundle, WordOrder::Swapped).unwrap(), (N64Save::Sram256k, file.clone()));
        let (_, console) = unwrap(&bundle, WordOrder::Console).unwrap();
        assert_eq!(&console[..4], &[3, 2, 1, 0], "the same save for a flash cartridge");
    }

    #[test]
    fn a_cartridge_nested_past_any_shape_is_refused_rather_than_followed_down() {
        // A shape this version does not define is one validation cannot hold to a depth, so a file
        // of them nested thousands deep used to take the stack with it. Each level here holds the
        // last on the cartridge role, as a device holds its cartridge.
        let shape = || Shape::from_slug(slug("later-shape"));
        let mut bundle =
            wrap(&[1u8; 512], N64Save::Eeprom4k, WordOrder::Console, &N64Options::default()).unwrap();
        for depth in 1..=MAX_DEPTH + 1 {
            let part = Part::new(0, bundle.to_vec().unwrap()).kind(PartKind::Bundle).role(slug(CARTRIDGE));
            bundle = Bundle::new(Header::new(shape()), vec![part]);
            let found = cartridge(&bundle);
            if depth <= MAX_DEPTH {
                assert_eq!(found.unwrap().map(|(save, _)| save), Some(N64Save::Eeprom4k), "{depth} deep");
            } else {
                assert!(matches!(found, Err(Error::NotConvertible(_))), "{depth} deep: {found:?}");
                assert!(matches!(unwrap(&bundle, WordOrder::Console), Err(Error::NotConvertible(_))));
                #[cfg(feature = "n64")]
                assert!(matches!(write_srm(&bundle, None), Err(Error::NotConvertible(_))));
            }
        }
    }

    #[test]
    fn a_cartridge_part_without_its_chip_is_not_guessed_at() {
        let mut bundle =
            wrap(&[1u8; 512], N64Save::Eeprom4k, WordOrder::Console, &N64Options::default()).unwrap();
        bundle.parts[0].extensions = one_saves::Extensions::new();
        assert!(unwrap(&bundle, WordOrder::Console).is_err());
    }

    #[cfg(feature = "n64")]
    mod srm {
        use n64_cpak::{ControllerPak, Note, PakBuilder};

        use super::super::*;

        const EEPROM_AT: usize = 0;
        const SRAM_AT: usize = 0x20800;
        const FLASH_AT: usize = 0x28800;

        fn pak_at(port: usize) -> usize {
            0x800 + port * n64_cpak::CAPACITY
        }

        fn note(code: &str, name: &str, fill: u8) -> Note {
            Note {
                slot: 0,
                game_code: code.into(),
                name: name.into(),
                extension: String::new(),
                dirent: Vec::new(),
                data: vec![fill; n64_cpak::PAGE],
            }
        }

        fn pak(notes: &[Note]) -> Vec<u8> {
            let mut builder = PakBuilder::new();
            for note in notes {
                builder.add(note.clone());
            }
            builder.build().unwrap()
        }

        /// What a core writes for a game that has saved nothing: erased chips, formatted paks.
        fn blank_srm() -> Vec<u8> {
            write_srm(&Bundle::new(Header::new(Shape::Save), vec![]), None).unwrap()
        }

        fn notes_on(file: &[u8], port: usize) -> Vec<(String, String)> {
            ControllerPak::parse(&file[pak_at(port)..pak_at(port) + n64_cpak::CAPACITY])
                .unwrap()
                .notes()
                .iter()
                .map(|note| (note.game_code.clone(), note.name.clone()))
                .collect()
        }

        #[test]
        fn the_layout_is_the_one_both_cores_write() {
            assert_eq!(SRM_LEN, 0x48800);
            assert_eq!(blank_srm().len(), 296_960);
        }

        #[test]
        fn a_game_that_saved_nothing_is_not_a_save() {
            assert!(read_srm(&blank_srm(), None, &N64Options::default()).is_err());
        }

        #[test]
        fn the_chip_is_read_off_the_file_only_where_the_file_proves_it() {
            let mut file = blank_srm();
            file[SRAM_AT..SRAM_AT + 4].copy_from_slice(&[4, 3, 2, 1]);
            let device = read_srm(&file, None, &N64Options::default()).unwrap();
            device.validate().unwrap();
            assert_eq!(device.header.shape, Shape::Device);
            let (save, bytes) = unwrap(&device, WordOrder::Console).unwrap();
            assert_eq!(save, N64Save::Sram256k);
            assert_eq!(&bytes[..4], &[1, 2, 3, 4], "normal form, the core's word order undone");

            // Past 512 bytes only a 16 Kbit chip reaches.
            let mut file = blank_srm();
            file[EEPROM_AT + 1000] = 0;
            let device = read_srm(&file, None, &N64Options::default()).unwrap();
            assert_eq!(unwrap(&device, WordOrder::Console).unwrap().0, N64Save::Eeprom16k);

            // Within it, both chips fit and the file cannot say which.
            let mut file = blank_srm();
            file[EEPROM_AT + 10] = 0;
            assert!(read_srm(&file, None, &N64Options::default()).is_err());
            let device = read_srm(&file, Some(N64Save::Eeprom4k), &N64Options::default()).unwrap();
            let (save, bytes) = unwrap(&device, WordOrder::Console).unwrap();
            assert_eq!((save, bytes.len()), (N64Save::Eeprom4k, 512));

            // Two chips written is a file that changed hands between settings, and is not guessed at.
            file[FLASH_AT] = 0;
            assert!(read_srm(&file, None, &N64Options::default()).is_err());
        }

        #[test]
        fn a_libretro_save_is_a_device_and_comes_back_byte_for_byte() {
            let mut file = blank_srm();
            file[EEPROM_AT..EEPROM_AT + 512].fill(0x11);
            let mario_kart = pak(&[note("NKTE", "MARIOKART64", 1), note("NSME", "SUPER MARIO", 2)]);
            file[pak_at(1)..pak_at(1) + n64_cpak::CAPACITY].copy_from_slice(&mario_kart);

            let device = read_srm(&file, Some(N64Save::Eeprom4k), &N64Options::default()).unwrap();
            device.validate().unwrap();
            let roles: Vec<&str> = device.parts.iter().map(|p| p.role.as_ref().unwrap().as_str()).collect();
            assert_eq!(roles, ["cartridge", "controller-pak-2"], "the empty paks are left out");

            assert_eq!(write_srm(&device, Some(&file)).unwrap(), file);
        }

        #[test]
        fn one_game_is_cut_out_with_its_own_notes_and_put_back_beside_the_others() {
            let mut file = blank_srm();
            file[EEPROM_AT..EEPROM_AT + 512].fill(0x11);
            let shared = pak(&[note("NKTE", "MARIOKART64", 1), note("NSME", "SUPER MARIO", 2)]);
            file[pak_at(0)..pak_at(0) + n64_cpak::CAPACITY].copy_from_slice(&shared);

            let device = read_srm(&file, Some(N64Save::Eeprom4k), &N64Options::default()).unwrap();
            let save = game_save(&device, "NKTE").unwrap();
            save.validate().unwrap();
            assert_eq!(save.header.shape, Shape::Save);
            assert_eq!(save.parts.len(), 2, "the cartridge and Mario Kart's one note");
            assert_eq!(save.parts[1].role.as_ref().unwrap().as_str(), "controller-pak-1");
            assert_eq!(save.parts[1].path.as_deref(), Some("MARIOKART64"));

            // Onto a device whose pak has moved on: Mario Kart's note replaced, Mario 64's kept.
            let mut elsewhere = blank_srm();
            let moved_on = pak(&[note("NSME", "SUPER MARIO", 9), note("NKTE", "OLD GHOST", 5)]);
            elsewhere[pak_at(0)..pak_at(0) + n64_cpak::CAPACITY].copy_from_slice(&moved_on);
            let written = write_srm(&save, Some(&elsewhere)).unwrap();

            let mut notes = notes_on(&written, 0);
            notes.sort();
            assert_eq!(
                notes,
                [
                    ("NKTE".to_owned(), "MARIOKART64".to_owned()),
                    ("NSME".to_owned(), "SUPER MARIO".to_owned())
                ]
            );
            assert_eq!(&written[EEPROM_AT..EEPROM_AT + 512], &[0x11; 512]);
            assert_eq!(
                &written[pak_at(0)..pak_at(0) + n64_cpak::PAGE],
                &elsewhere[pak_at(0)..pak_at(0) + n64_cpak::PAGE],
                "the pak it went onto is still that pak"
            );
        }

        #[test]
        fn mupen_s_four_paks_are_a_device_and_come_back_byte_for_byte() {
            let formatted = pak(&[]);
            let mut file = [formatted.clone(), formatted.clone(), formatted.clone(), formatted].concat();
            let ghosts = pak(&[note("NKTE", "MARIOKART64", 1)]);
            file[n64_cpak::CAPACITY * 2..n64_cpak::CAPACITY * 3].copy_from_slice(&ghosts);

            let device = read_paks(&file, &N64Options::default()).unwrap();
            device.validate().unwrap();
            assert_eq!(device.parts.len(), 1);
            assert_eq!(device.parts[0].role.as_ref().unwrap().as_str(), "controller-pak-3");
            assert_eq!(pak_ports(&device).unwrap(), [3]);
            assert_eq!(write_paks(&device, Some(&file)).unwrap(), file);
            assert_eq!(write_paks(&device, None).unwrap(), file, "the empty ones are formatted alike");
        }

        #[test]
        fn one_port_s_pak_is_written_on_its_own() {
            let mut file = blank_srm();
            file[pak_at(1)..pak_at(1) + n64_cpak::CAPACITY].copy_from_slice(&pak(&[note(
                "NKTE",
                "MARIOKART64",
                1,
            )]));
            let device = read_srm(&file, None, &N64Options::default()).unwrap();

            assert_eq!(write_pak(&device, 1, None).unwrap(), None, "nothing in port 1");
            let written = write_pak(&device, 2, None).unwrap().unwrap();
            assert_eq!(written, &file[pak_at(1)..pak_at(1) + n64_cpak::CAPACITY]);

            // One game's note onto a pak of its own that holds another game's.
            let save = game_save(&device, "NKTE").unwrap();
            let theirs = pak(&[note("NSME", "SUPER MARIO", 3)]);
            let merged = ControllerPak::parse(&write_pak(&save, 2, Some(&theirs)).unwrap().unwrap()).unwrap();
            let mut codes: Vec<&str> = merged.notes().iter().map(|n| n.game_code.as_str()).collect();
            codes.sort_unstable();
            assert_eq!(codes, ["NKTE", "NSME"]);
        }

        #[test]
        fn dezaemon_s_three_banks_do_not_fit() {
            let save =
                wrap(&vec![1u8; 96 * 1024], N64Save::Sram768k, WordOrder::Console, &N64Options::default())
                    .unwrap();
            assert!(write_srm(&save, None).is_err());
        }
    }
}
