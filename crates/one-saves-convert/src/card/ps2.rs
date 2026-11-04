//! PlayStation 2 memory cards.
//!
//! The filesystem itself lives in [`ps2_memcard`], which is a standalone crate because a PS2
//! card is a genuinely reusable format and nothing about reading one needs this container. What
//! is here is the adapter: the mapping between a card's saves and a bundle's nested parts.
//!
//! A PS2 save is a **directory** rather than a file — `BASLUS-20312/` holding `icon.sys`, an
//! icon and the game's own data — so its files are the parts of one nested bundle, told apart by
//! [`path`](one_saves::Part::path). The directory's own entry belongs to no file, and rides on
//! the outer `bundle` part alongside the slot.
//!
//! # The console's own folders
//!
//! Not every directory on a card is a game's. The browser keeps its play history in one, the
//! network setup disc keeps its settings in another, and a system update lives in a third. Each
//! is an ordinary directory with an `icon.sys` and files of its own, but no game wrote it, so it is
//! not a save. It travels as [`system`](PartKind::System) parts instead: one for the directory,
//! with an empty payload, then one per file in it, its `path` the directory's name and the file's
//! joined by a slash. They are the ones Sony's software is documented to write: `B?DATA-SYSTEM`,
//! `B?EXEC-SYSTEM`, `B?EXEC-DVDPLAYER`, `BWNETCNF` and `BWLINUX`, the `?` being the region.

use one_saves::{Bundle, Game, Header, Part, PartKind};
use ps2_memcard::{Capacity, CardBuilder, File, MemoryCard, Save};

use crate::CardOptions;
use crate::card::{card_header, civil_seconds, created_and_modified, slug};
use crate::detect::Format;
use crate::error::{Error, Result};
use crate::label::{shift_jis_field, value as label};

/// What the format is called, for error messages.
const FORMAT: &str = Format::Ps2Card.label();

/// The card format slug a bundle carries.
pub const CARD_FORMAT: &str = match Format::Ps2Card.card_format() {
    Some(slug) => slug,
    None => panic!("a PS2 card is a card format"),
};

/// The system slug these saves are for.
pub const SYSTEM: &str = match Format::Ps2Card.system() {
    Some(slug) => slug,
    None => panic!("a PS2 card holds saves for a system"),
};

/// Whether these bytes look like a PS2 memory card.
#[must_use]
pub fn detect(bytes: &[u8]) -> bool {
    MemoryCard::parse(bytes).is_ok()
}

/// Where a PS2 save keeps the line the browser lists it under.
///
/// In `icon.sys`, which is a file of the save's own rather than anything in the directory: 68
/// bytes of Shift-JIS at 0xc0. The browser breaks it across two lines at the offset held at 0x06,
/// which is layout rather than content — both halves are one title, so the break is not read here.
mod icon_sys {
    /// The file every PS2 save carries, and the only one this looks at.
    pub(super) const NAME: &str = "icon.sys";
    /// What it starts with, so a file of the right name and the wrong shape is not read as one.
    pub(super) const MAGIC: &[u8] = b"PS2D";
    /// Where the title sits, and how long it runs.
    pub(super) const TITLE: usize = 0xc0;
    pub(super) const TITLE_LEN: usize = 68;
}

/// The line the browser shows for a save, as `x.1sav.label` wants it.
fn title_of(save: &ps2_memcard::Save) -> Option<one_saves::extension::Label> {
    let file = save.files.iter().find(|file| file.name == icon_sys::NAME)?;
    if !file.data.starts_with(icon_sys::MAGIC) {
        return None;
    }
    let field = file.data.get(icon_sys::TITLE..icon_sys::TITLE + icon_sys::TITLE_LEN)?;
    label(shift_jis_field(field), None)
}

/// Where a PS2 directory entry keeps the times, and how it lays one out.
///
/// Two `sceMcStDateTime` structs, at 0x08 and 0x18: a reserved byte, then second, minute, hour,
/// day and month as plain binary, then a little-endian year. Confirmed against cards PCSX2 wrote,
/// whose three saves each dated a creation a few seconds before the write that followed it.
const CREATED_AT: usize = 0x08;
const MODIFIED_AT: usize = 0x18;

/// The zone a PS2 keeps its directory times in, in seconds east of UTC.
///
/// A PS2's clock runs on Japan Standard Time whatever the console's region, and the timezone a
/// later BIOS lets you set moves what the browser *shows* rather than what the filesystem writes.
/// So unlike a GameCube or a VMU, a PS2 reading can be qualified: the instant is the reading minus
/// this.
///
/// Established by controlling for the obvious alternative rather than by argument. A console left
/// at its defaults is configured as Japan, so times nine hours ahead of the host prove nothing on
/// their own. The BIOS was then set to a zone seven hours *behind* UTC and a card formatted, and
/// the entry that format wrote still read +9 — so the offset is the hardware's and not the
/// configuration's.
const JST: i32 = 9 * 3_600;

/// One `sceMcStDateTime`, as a wall-clock reading on the Unix scale.
fn entry_seconds(dirent: &[u8], at: usize) -> Option<i64> {
    let f = dirent.get(at..at + 8)?;
    civil_seconds(u16::from_le_bytes([f[6], f[7]]), f[5], f[4], f[3], f[2], f[1])
}

/// Reads a card into a bundle: one nested bundle per save, one part per file in it, and the
/// console's own folders as `system` parts beside them.
pub fn read(bytes: &[u8], options: &CardOptions) -> Result<Bundle> {
    let card = MemoryCard::parse(bytes).map_err(|e| into_error(&e))?;
    let saves = card.saves().map_err(|e| into_error(&e))?;
    let role = options.role_for(Format::Ps2Card);

    // Saves and the console's folders stay in the order the root lists them, so a writer that
    // keeps the part order lays the root down as it was.
    let mut parts = Vec::new();
    for (index, save) in saves.iter().enumerate() {
        let slot = u64::try_from(index).expect("fits");
        // The directory's record of the save: when it was made and when it was last written, both
        // qualified by the zone a PS2 keeps them in.
        let times = entry_seconds(&save.dirent, CREATED_AT)
            .zip(entry_seconds(&save.dirent, MODIFIED_AT))
            .map(|(created, modified)| created_and_modified(created, modified, Some(JST)));

        if is_system_folder(&save.name) {
            push_system_folder(&mut parts, save, slot, &role, times);
            continue;
        }

        // A directory is named for its game's serial only when it is a game's. One that is not,
        // and is not the console's either — a homebrew loader's, or a game that named its folder
        // its own way — is a save nothing identifies, and a save with no game has no `game` at
        // all rather than an empty one, which the format refuses.
        let game = serial_from_name(&save.name).map(|serial| Game::default().serial(serial));

        // Each file is a part of the nested bundle, carrying its own directory entry. That is
        // what a PS2 save needs and a PS1 save does not: it is a directory with an entry of its
        // own and an entry per file.
        let inner_parts: Vec<Part> = save
            .files
            .iter()
            .enumerate()
            .map(|(file_index, file)| {
                Part::new(u64::try_from(file_index).expect("fits"), file.data.clone())
                    .path(file.name.clone())
                    .dirent(file.dirent.clone())
            })
            .collect();

        if inner_parts.is_empty() {
            // A bundle holds at least one part, so a save with no files cannot be represented.
            continue;
        }

        // The title travels with the save, so it goes in the save's own header.
        let mut extensions = one_saves::Extensions::new();
        if let Some(title) = title_of(save) {
            extensions.insert(title);
        }
        // A PS2 save is a directory, so this nested bundle has a part per file — still one game's
        // state, and so still a save.
        let header = Header::new(one_saves::Shape::Save)
            .system(slug(SYSTEM))
            .game(game.clone())
            .extensions(extensions);
        let inner = Bundle::new(header, inner_parts);

        let mut part = Part::new(u64::try_from(parts.len()).expect("fits"), inner.to_vec()?)
            .kind(PartKind::Bundle)
            .role(role.clone())
            .path(save.name.clone())
            .slot(slot)
            // The directory's own entry, carrying its mode bits and timestamps, belongs to no file.
            .dirent(save.dirent.clone())
            .game(game)
            .system(slug(SYSTEM));
        if let Some(times) = times {
            part.extensions.insert(times);
        }
        parts.push(part);
    }

    if parts.is_empty() {
        parts.push(crate::card::card_image_part(0, bytes, &role));
    }

    // The capacity is the card's data capacity, which is not the length of a dump of it: an 8 MB
    // card dumps to 8650752 bytes because every page carries a 16-byte spare area. What on a PS2
    // card belongs to no save is whole directories, which are parts, rather than bytes at a fixed
    // place, so there is no system area to keep.
    Ok(Bundle::new(card_header(Format::Ps2Card, card.capacity(), None, options), parts))
}

/// One of the console's folders as `system` parts: the directory, then each file in it.
///
/// The directory's part carries its entry and its place in the root, as a save's outer part does,
/// and nothing else: its contents are the parts after it. A file's `path` is the directory's name
/// and its own, which is the whole of where it goes back.
fn push_system_folder(
    parts: &mut Vec<Part>,
    folder: &Save,
    slot: u64,
    role: &one_saves::Slug,
    times: Option<one_saves::extension::Dirent>,
) {
    let next_id = |parts: &Vec<Part>| u64::try_from(parts.len()).expect("fits");

    let mut directory = Part::new(next_id(parts), Vec::new())
        .kind(PartKind::System)
        .role(role.clone())
        .path(folder.name.clone())
        .slot(slot)
        .dirent(folder.dirent.clone());
    if let Some(times) = times {
        directory.extensions.insert(times);
    }
    parts.push(directory);

    for file in &folder.files {
        let part = Part::new(next_id(parts), file.data.clone())
            .kind(PartKind::System)
            .role(role.clone())
            .path(format!("{}/{}", folder.name, file.name))
            .dirent(file.dirent.clone());
        parts.push(part);
    }
}

/// Writes a bundle back out as a card image.
///
/// The image is plain data pages by default. A hardware dump with spare areas is
/// [`write_with_spare`].
pub fn write(bundle: &Bundle) -> Result<Vec<u8>> {
    // A card with nothing on it carries a whole-card image rather than nested saves, and writing
    // it back is a byte copy.
    if let Some(image) = crate::card::image_only(bundle)? {
        return Ok(image);
    }
    builder_for(bundle)?.build().map_err(|e| into_error(&e))
}

/// Writes a bundle back out as a hardware dump, spare areas and ECC included.
pub fn write_with_spare(bundle: &Bundle) -> Result<Vec<u8>> {
    if let Some(image) = crate::card::image_only(bundle)? {
        return Ok(image);
    }
    builder_for(bundle)?.build_with_spare().map_err(|e| into_error(&e))
}

fn builder_for(bundle: &Bundle) -> Result<CardBuilder> {
    // A card that came from a bundle keeps its stated capacity where that is a size cards come
    // in; anything else gets the standard 8 MB card, since a capacity nothing makes is not a
    // card a console would accept.
    let capacity = bundle
        .header
        .card
        .as_ref()
        .and_then(|card| usize::try_from(card.capacity).ok())
        .and_then(Capacity::from_bytes)
        .unwrap_or(Capacity::Mb8);

    let mut builder = CardBuilder::new(capacity);
    for directory in directories(bundle)? {
        builder.add(directory);
    }
    Ok(builder)
}

/// Every directory the root holds, saves and the console's folders alike, in part order.
///
/// Part order is the order the root listed them in when the card was read, so keeping it lays the
/// root down as it was. `slot` says the same, but a writer is free to change that; the parts
/// themselves are what a bundle that moved a save around has kept in order.
fn directories(bundle: &Bundle) -> Result<Vec<Save>> {
    // The saves come out of their nested bundles in part order, so they line up one for one with
    // the `bundle` parts walked below.
    let mut saves = crate::card::nested_saves(bundle)?.into_iter();
    let mut directories: Vec<Save> = Vec::new();
    // Which of those a `system` part laid down, since a `system` file goes into one of the
    // console's folders and never into a game's save.
    let mut system: Vec<usize> = Vec::new();

    for part in &bundle.parts {
        match part.kind {
            PartKind::Bundle => {
                let save = saves.next().expect("a nested save for every `bundle` part");
                let name = save.path.clone().ok_or_else(|| {
                    Error::NotConvertible("a save has no path to name its directory".into())
                })?;
                let files = save
                    .inner
                    .parts
                    .iter()
                    .map(|part| {
                        Ok(File {
                            name: part.path.clone().unwrap_or_else(|| name.clone()),
                            dirent: part.dirent.clone().unwrap_or_default(),
                            data: part.bytes()?.into_owned(),
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                directories.push(Save { name, dirent: save.dirent.unwrap_or_default(), files });
            }
            PartKind::System => {
                // The format requires a path on every `system` part, so a bundle that decoded has
                // one; a bundle built in memory may not have been checked yet.
                let path = part
                    .path
                    .as_deref()
                    .ok_or_else(|| Error::NotConvertible("a system entry has no path to put it at".into()))?;
                let dirent = part.dirent.clone().unwrap_or_default();
                match path.split_once('/') {
                    // A name and nothing else is a directory in the root.
                    None => {
                        system.push(directories.len());
                        directories.push(Save { name: path.to_owned(), dirent, files: Vec::new() });
                    }
                    Some((folder, name)) => {
                        if name.contains('/') {
                            return Err(Error::NotConvertible(format!(
                                "{path} is a folder inside a folder, which a PS2 card's directories do not \
                                 hold"
                            )));
                        }
                        // A directory comes before what it holds, so the one this file goes in has
                        // been laid down already, and is the last one of that name.
                        let index = system
                            .iter()
                            .rev()
                            .copied()
                            .find(|&index| directories[index].name == folder)
                            .ok_or_else(|| {
                                Error::NotConvertible(format!(
                                    "{path} is in a folder no `system` part before it names"
                                ))
                            })?;
                        directories[index].files.push(File {
                            name: name.to_owned(),
                            dirent,
                            data: part.bytes()?.into_owned(),
                        });
                    }
                }
            }
            // A `card-image` kept beside the saves is the archive, not the source, and an `aux` part
            // is carried rather than read.
            _ => {}
        }
    }
    Ok(directories)
}

/// Whether a directory in the root is one of the console's own rather than a game's.
///
/// The ones Sony's software writes, and only those:
///
/// - `B?DATA-SYSTEM`, the browser's: its play history, written each time it boots a disc, and
///   listed as "Your System Configuration".
/// - `B?EXEC-SYSTEM`, a system update the boot ROM runs at startup, which is also where Free
///   McBoot installs itself.
/// - `B?EXEC-DVDPLAYER`, the DVD player a utility disc installs.
/// - `BWNETCNF`, the network settings Sony's network library writes, from the Network Adaptor
///   Start-up Disc or a game's own setup. No region letter.
/// - `BWLINUX`, the one folder the PS2 Linux kernel lets itself touch.
///
/// The `?` is the console's region as the browser names it: `I` for Japan, `A` for North America
/// and Asia, `E` for Europe, `C` for China. Those four are the whole list the browser picks from,
/// in ps2homebrew's OSD-Initialization-Libraries (`osd/common/OSDInit.c`), which reimplements it;
/// the network settings' path and files are in ps2sdk's `netcnf.c`.
///
/// A folder a homebrew loader keeps — Free McBoot's `BOOT` and `APPS`, OPL's `OPL` — is not here.
/// No game wrote it either, but neither did the console, and the format has a word for a folder
/// nothing identifies: a save with no game.
fn is_system_folder(name: &str) -> bool {
    const REGIONAL: [&str; 3] = ["DATA-SYSTEM", "EXEC-SYSTEM", "EXEC-DVDPLAYER"];
    const UNREGIONAL: [&str; 2] = ["BWNETCNF", "BWLINUX"];

    if UNREGIONAL.contains(&name) {
        return true;
    }
    match name.as_bytes() {
        [b'B', b'I' | b'A' | b'E' | b'C', ..] => REGIONAL.contains(&&name[2..]),
        _ => false,
    }
}

/// The product code out of a save directory's name.
///
/// A PS2 save directory is named region code, then the disc's product code, the same way a PS1
/// save file is: `BASLUS-20312` is `BA` and `SLUS-20312`.
fn serial_from_name(name: &str) -> Option<String> {
    if name.len() < 12 {
        return None;
    }
    let serial = &name[2..12];
    let shape = serial.as_bytes();
    let looks_right = shape[..4].iter().all(u8::is_ascii_uppercase)
        && shape[4] == b'-'
        && shape[5..].iter().all(u8::is_ascii_digit);
    looks_right.then(|| serial.to_owned())
}

fn into_error(source: &ps2_memcard::Error) -> Error {
    match source {
        ps2_memcard::Error::NotAMemoryCard => {
            Error::NotThisFormat { format: FORMAT, why: "no memory card magic".into() }
        }
        ps2_memcard::Error::TooShort(len) => {
            Error::WrongLength { format: FORMAT, expected: "a whole number of pages", found: *len }
        }
        other => Error::Corrupt { format: FORMAT, why: other.to_string() },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card_with_saves() -> Vec<u8> {
        let mut builder = CardBuilder::new(Capacity::Mb8);
        builder.add(Save {
            name: "BASLUS-20312".into(),
            dirent: vec![0u8; ps2_memcard::ENTRY],
            files: vec![
                File { name: "icon.sys".into(), dirent: vec![0u8; ps2_memcard::ENTRY], data: vec![1u8; 964] },
                File {
                    name: "list.ico".into(),
                    dirent: vec![0u8; ps2_memcard::ENTRY],
                    data: vec![2u8; 15168],
                },
                File {
                    name: "BASLUS-20312".into(),
                    dirent: vec![0u8; ps2_memcard::ENTRY],
                    data: vec![3u8; 40960],
                },
            ],
        });
        builder.build().expect("builds")
    }

    #[test]
    fn a_save_is_a_directory_so_its_files_are_the_nested_parts() {
        // The worked example from the Memory Cards specification.
        let bundle = read(&card_with_saves(), &CardOptions::default()).expect("reads");

        assert_eq!(bundle.header.system.as_ref().unwrap().as_str(), "ps2");
        let card = bundle.header.card.as_ref().unwrap();
        assert_eq!(card.format.as_str(), "ps2-mc");
        // The data capacity, not the 8650752 a hardware dump of the same card runs to.
        assert_eq!(card.capacity, 8_388_608);

        assert_eq!(bundle.parts.len(), 1);
        let outer = &bundle.parts[0];
        assert_eq!(outer.path.as_deref(), Some("BASLUS-20312"));
        assert_eq!(outer.game.as_ref().unwrap().serial.as_deref(), Some("SLUS-20312"));
        // The directory's own entry rides on the outer part.
        assert_eq!(outer.dirent.as_ref().map(Vec::len), Some(512));

        let inner = Bundle::from_slice(&outer.bytes().unwrap()).expect("inner bundle");
        assert_eq!(inner.parts.len(), 3);
        assert_eq!(inner.parts[0].path.as_deref(), Some("icon.sys"));
        assert_eq!(inner.parts[2].payload.len(), 40960);
        // Each file's entry is on its own inner part.
        assert_eq!(inner.parts[1].dirent.as_ref().map(Vec::len), Some(512));

        bundle.validate().expect("a card is a valid bundle");
    }

    #[test]
    fn a_card_round_trips_through_a_bundle() {
        let bundle = read(&card_with_saves(), &CardOptions::default()).expect("reads");
        let rebuilt = write(&bundle).expect("writes");

        // Not a byte-for-byte copy — a writer regenerates the allocation table and the tree — so
        // the check is that the saves survive, which is what the representation promises.
        let again = read(&rebuilt, &CardOptions::default()).expect("reads what it wrote");
        assert_eq!(again.parts.len(), bundle.parts.len());
        let before = Bundle::from_slice(&bundle.parts[0].bytes().unwrap()).unwrap();
        let after = Bundle::from_slice(&again.parts[0].bytes().unwrap()).unwrap();
        for (a, b) in before.parts.iter().zip(&after.parts) {
            assert_eq!((&a.path, a.bytes().unwrap()), (&b.path, b.bytes().unwrap()));
        }
    }

    #[test]
    fn a_hardware_dump_reads_the_same_as_a_plain_image() {
        let bundle = read(&card_with_saves(), &CardOptions::default()).expect("reads");
        let dump = write_with_spare(&bundle).expect("writes a dump");
        assert_eq!(dump.len(), 8_650_752, "a dump is longer than the card's capacity");

        let from_dump = read(&dump, &CardOptions::default()).expect("reads the dump");
        assert_eq!(from_dump.header.card.as_ref().unwrap().capacity, 8_388_608);
        assert_eq!(from_dump.parts.len(), bundle.parts.len());
    }

    fn file(name: &str, fill: u8) -> File {
        File { name: name.into(), dirent: vec![0u8; ps2_memcard::ENTRY], data: vec![fill; 100] }
    }

    /// A card with the network settings on it between two directories that are not the console's:
    /// a game's save, and a homebrew loader's folder that names no game.
    fn card_with_network_settings() -> Vec<u8> {
        let mut builder = CardBuilder::new(Capacity::Mb8);
        for (name, files) in [
            ("BASLUS-20312", vec![file("BASLUS-20312", 1)]),
            ("BWNETCNF", vec![file("icon.sys", 2), file("BWNETCNF", 3)]),
            ("OPL", vec![file("conf_opl.cfg", 4)]),
        ] {
            builder.add(Save { name: name.into(), dirent: vec![0u8; ps2_memcard::ENTRY], files });
        }
        builder.build().expect("builds")
    }

    /// The console's own folders are not saves, so they are not nested bundles: the directory is
    /// one `system` part and each file in it another. This once made the network settings a save
    /// with no game, which a consumer expecting every save to name one refused.
    #[test]
    fn the_consoles_own_folder_is_system_parts_rather_than_a_save() {
        let bundle = read(&card_with_network_settings(), &CardOptions::default()).expect("reads");
        bundle.validate().expect("a card with system parts is a valid bundle");

        let summary: Vec<(PartKind, Option<&str>, Option<u64>)> =
            bundle.parts.iter().map(|part| (part.kind.clone(), part.path.as_deref(), part.slot)).collect();
        assert_eq!(
            summary,
            [
                (PartKind::Bundle, Some("BASLUS-20312"), Some(0)),
                (PartKind::System, Some("BWNETCNF"), Some(1)),
                (PartKind::System, Some("BWNETCNF/icon.sys"), None),
                (PartKind::System, Some("BWNETCNF/BWNETCNF"), None),
                (PartKind::Bundle, Some("OPL"), Some(2)),
            ],
            "the root's order, the console's folder flattened into its entries"
        );

        let directory = &bundle.parts[1];
        assert!(directory.payload.is_empty(), "a directory holds nothing but what comes after it");
        assert_eq!(directory.dirent.as_ref().map(Vec::len), Some(ps2_memcard::ENTRY));
        assert_eq!(bundle.parts[3].bytes().unwrap().as_ref(), [3u8; 100]);
        assert!(bundle.parts[1..4].iter().all(|part| part.game.is_none()));

        // A folder that is not the console's stays a save even when it names no game, because the
        // format's word for that is unidentified rather than not a save.
        assert_eq!(bundle.parts[0].game.as_ref().unwrap().serial.as_deref(), Some("SLUS-20312"));
        assert_eq!(bundle.parts[4].game, None);
    }

    #[test]
    fn the_consoles_own_folder_goes_back_where_it_was() {
        let original = card_with_network_settings();
        let bundle = read(&original, &CardOptions::default()).expect("reads");
        let rebuilt = write(&bundle).expect("writes");

        let directories = |image: &[u8]| MemoryCard::parse(image).unwrap().saves().unwrap();
        assert_eq!(directories(&rebuilt), directories(&original));
    }

    /// A file can only go into a directory the parts have already laid down, and a PS2 card's
    /// directories hold files and nothing deeper.
    #[test]
    fn a_system_entry_with_nowhere_to_go_is_refused() {
        let mut bundle = read(&card_with_network_settings(), &CardOptions::default()).expect("reads");
        bundle.parts[2].path = Some("BWNETCNF/deeper/icon.sys".into());
        assert!(matches!(write(&bundle), Err(Error::NotConvertible(why)) if why.contains("inside a folder")));

        let mut bundle = read(&card_with_network_settings(), &CardOptions::default()).expect("reads");
        bundle.parts.remove(1);
        assert!(
            matches!(write(&bundle), Err(Error::NotConvertible(why)) if why.contains("no `system` part"))
        );

        // And a game's save is not one of the console's folders, whatever the path says.
        let mut bundle = read(&card_with_network_settings(), &CardOptions::default()).expect("reads");
        bundle.parts[2].path = Some("BASLUS-20312/icon.sys".into());
        assert!(
            matches!(write(&bundle), Err(Error::NotConvertible(why)) if why.contains("no `system` part"))
        );
    }

    #[test]
    fn the_consoles_folders_are_told_apart_by_name() {
        for name in [
            "BIDATA-SYSTEM",
            "BADATA-SYSTEM",
            "BEDATA-SYSTEM",
            "BCDATA-SYSTEM",
            "BIEXEC-SYSTEM",
            "BEEXEC-DVDPLAYER",
            "BWNETCNF",
            "BWLINUX",
        ] {
            assert!(is_system_folder(name), "{name} is the console's");
        }
        // A letter that is no region's, a name with something after it, and the folders a homebrew
        // loader keeps, which no game wrote and no console did either.
        for name in [
            "BASLUS-20312",
            "BXDATA-SYSTEM",
            "BADATA-SYSTEMS",
            "BDATA-SYSTEM",
            "badata-system",
            "BWNETCNF2",
            "OPL",
            "BOOT",
            "SYS-CONF",
        ] {
            assert!(!is_system_folder(name), "{name} is not the console's");
        }
    }

    #[test]
    fn reads_a_serial_only_out_of_a_name_shaped_like_one() {
        assert_eq!(serial_from_name("BASLUS-20312").as_deref(), Some("SLUS-20312"));
        assert_eq!(serial_from_name("BESLES-50213").as_deref(), Some("SLES-50213"));
        assert_eq!(serial_from_name("SHORT"), None);
    }
}
