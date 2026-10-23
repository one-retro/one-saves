//! Identify a disc image: which system it is for, and the serial on the disc.
//!
//! A `.cue` is a few hundred bytes of text pointing at the tracks; a `.bin` and
//! a `.iso` share extensions across half a dozen systems. So an extension
//! cannot name a disc's system, and hashing one is worse than useless — a
//! multi-track dump has no single digest a catalog would key on, and hashing
//! gigabytes to learn nothing is the expensive way to fail.
//!
//! Every disc system announces itself in the opening sectors instead. This
//! crate reads those, in bounded kilobytes, and answers both questions at once:
//! the system, for routing a save to the right game, and the serial, which is
//! the only identifier a memory card entry carries.
//!
//! It depends on nothing but the registry it names systems from, opens no
//! files, and never guesses: a signature it does not recognise is [`None`], not
//! a plausible answer.
//!
//! # I/O is the caller's
//!
//! This crate reads nothing. [`identify_prefix`] is handed whatever bytes a
//! caller has from the start of the image and answers with a
//! [`ControlFlow`]: break with the answer, or continue
//! with how many bytes it would need — so it composes with blocking reads, `tokio`, `futures`, an
//! SFTP stream, an inflating zip entry, or any stack of those, without picking
//! one. It never asks for more than [`sector::MAX_PREFIX`].
//!
//! [`identify`] is a convenience over that for a caller holding a plain
//! [`Read`].
//!
//! ```no_run
//! let file = std::fs::File::open("game.iso")?;
//! if let Some(id) = disc_id::identify(file)? {
//!     println!("{} {}", id.system.slug, id.serial.unwrap_or_default());
//! }
//! # Ok::<(), std::io::Error>(())
//! ```

#[cfg(feature = "chd")]
pub mod chd;
pub mod cue;
pub mod iso9660;
pub mod sector;

use std::io::{self, Read, Seek, SeekFrom};
use std::ops::ControlFlow;

use sector::Prefix;

pub use one_saves_registry::System;
use one_saves_registry::systems;

/// Every system whose games ship on discs, as the registry names it.
///
/// So a caller can ask whether one of *its* systems is disc-based without keeping a second copy of
/// this list. Deciding that from the system a dump was filed under is what lets a caller skip
/// reading a file it already knows is a cartridge, and a second list would answer that differently
/// here and there the moment a system is added.
pub const SYSTEMS: [System; 9] = [
    systems::SEGA_CD,
    systems::SATURN,
    systems::DREAMCAST,
    systems::GC,
    systems::WII,
    systems::PSX,
    systems::PS2,
    systems::NEOGEO_CD,
    systems::PCENGINECD,
];

/// What a disc image turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscId {
    /// The system whose console reads this disc, as the registry names it.
    pub system: System,
    /// The serial printed on the disc, when the format states one.
    ///
    /// `None` is a real answer: some formats carry no serial where this crate
    /// looks, and a guess would be worse than an absence, since a serial is
    /// what a memory card entry is keyed on.
    pub serial: Option<String>,
}

/// Read a fixed-width field, trimmed, or `None` if it is blank.
fn field(bytes: &[u8], at: usize, len: usize) -> Option<String> {
    let raw = bytes.get(at..at + len)?;
    let text: String = raw.iter().map(|b| *b as char).filter(|c| c.is_ascii_graphic() || *c == ' ').collect();
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Identify a disc image from a prefix of it.
///
/// The whole of the crate's logic, and the only entry point that does no I/O.
/// Start with sixty-four kibibytes or so; most discs answer from that, and a
/// PlayStation asks for whichever sector holds its `SYSTEM.CNF`.
///
/// - [`ControlFlow::Continue`] carries how many bytes from the **start** of the
///   image would satisfy the lookup that came up short. Fetch at least that
///   many and ask again. Never more than [`sector::MAX_PREFIX`], so the loop
///   always terminates. A caller that cannot supply them, because the file is
///   shorter or the stream ended, should treat the disc as unidentified.
/// - [`ControlFlow::Break`] carries the answer. `None` inside it means the
///   bytes match nothing this crate recognises, which is a real answer rather
///   than a failure.
///
/// ```
/// use std::ops::ControlFlow;
/// # let bytes = vec![0u8; 4096];
/// match disc_id::identify_prefix(&bytes) {
///     ControlFlow::Break(found) => { let _ = found; }
///     ControlFlow::Continue(want) => { let _ = want; }
/// }
/// ```
// No `#[must_use]`: `ControlFlow` already carries one.
pub fn identify_prefix(bytes: &[u8]) -> ControlFlow<Option<DiscId>, usize> {
    let mut prefix = Prefix::new(bytes);

    // GameCube and Wii images are flat dumps rather than CD sectors, so their
    // magic sits at an absolute offset and is checked before any sector maths.
    let head = prefix.head(WIA_DISC_HEAD + WIA_DISC_HEAD_LEN);
    if let Some(id) = nintendo(head) {
        return ControlFlow::Break(Some(id));
    }
    // A compressed Dolphin image keeps the same bytes in its header.
    if let Some(id) = wia_disc_head(head).and_then(nintendo) {
        return ControlFlow::Break(Some(id));
    }

    let answer = identify_disc(&mut prefix);
    match (answer, prefix.needed()) {
        // An answer stands even if some later lookup came up short.
        (Some(id), _) => ControlFlow::Break(Some(id)),
        // Nothing found, but something ran past the end: more may settle it.
        (None, Some(want)) if want > bytes.len() => ControlFlow::Continue(want),
        (None, _) => ControlFlow::Break(None),
    }
}

fn identify_disc(prefix: &mut Prefix<'_>) -> Option<DiscId> {
    let first = prefix.sector(0)?;

    if first.starts_with(b"SEGADISCSYSTEM") || first.starts_with(b"SEGABOOTDISC") {
        return Some(DiscId { system: systems::SEGA_CD, serial: sega_cd_serial(first) });
    }
    if first.starts_with(b"SEGA SEGASATURN") {
        return Some(DiscId {
            system: systems::SATURN,
            // Hardware header: maker at 0x10, product number at 0x20.
            serial: field(first, 0x20, 10),
        });
    }
    if first.starts_with(b"SEGA SEGAKATANA") {
        return Some(DiscId {
            system: systems::DREAMCAST,
            // IP.BIN: product number at 0x40.
            serial: field(first, 0x40, 10),
        });
    }

    // A PC Engine CD has no ISO 9660 at all: the system card reads a boot
    // header out of the second sector of the data track. Verified against two
    // discs, one stored cooked and one raw, which the geometry layer levels
    // out before this looks.
    if let Some(second) = prefix.sector(1)
        && second.get(0x20..0x37) == Some(&b"PC Engine CD-ROM SYSTEM"[..])
    {
        return Some(DiscId {
            system: systems::PCENGINECD,
            // The boot header names Hudson and NEC, not the title; no product
            // code appears where this looks.
            serial: None,
        });
    }

    if !iso9660::is_iso9660(prefix) {
        return None;
    }
    // Every Neo Geo CD game ships an IPL.TXT at the root: it is the loader
    // manifest the BIOS reads, so its presence is the disc's signature. Only
    // presence — the manifest's own bytes commonly sit far past any prefix.
    if iso9660::find_root_file(prefix, "IPL.TXT").is_some() {
        return Some(DiscId { system: systems::NEOGEO_CD, serial: None });
    }
    playstation(prefix)
}

/// How much to ask for first, and how much to add each time more is wanted.
const STEP: usize = 64 * 1024;

/// Identify a disc image by reading forward from `reader`.
///
/// A convenience over [`identify_prefix`] for a caller holding a plain
/// Identify a disc in a reader that can seek, which is what a file is.
///
/// [`identify`] reads forward only and gives up past [`sector::MAX_PREFIX`], on
/// the promise that identification never costs a disc's worth of reading. That
/// promise conflates two things: never *buffering* a disc, which is right, and
/// never *reaching* into one, which is too strict. A PlayStation disc states its
/// serial in `SYSTEM.CNF`, and where that file sits is up to whoever mastered
/// the disc. Measured on five real PlayStation 2 dumps, it lands anywhere from
/// 716 KB to **156 MB** in; the two deepest were unidentifiable for that reason
/// alone, with the serial sitting there intact.
///
/// Seeking costs nothing, so this reads the front of the disc as usual, and when
/// the answer lies in a file further in it locates that file's extent from the
/// directory (which is near the front) and seeks straight to it. Two reads,
/// whatever the depth, and the buffer stays bounded.
///
/// Reads are relative to wherever the reader already is, so a caller that has
/// positioned on a data track partway into a single-file image stays correct.
pub fn identify_seekable<R: Read + Seek>(mut reader: R) -> io::Result<Option<DiscId>> {
    let base = reader.stream_position()?;
    let mut head = vec![0u8; sector::MAX_PREFIX];
    let read = read_upto(&mut reader, &mut head)?;
    head.truncate(read);

    // The ordinary path first: most discs answer from the front alone.
    //
    // Only a positive answer ends it. `Break(None)` means the front was read and
    // nothing matched, which is exactly the case a far-away `SYSTEM.CNF`
    // produces: the sector lookup lands past the buffer, the shortfall is over
    // the cap so it is not even recorded, and identification reports "no" when
    // the answer is simply further in.
    if let ControlFlow::Break(Some(id)) = identify_prefix(&head) {
        return Ok(Some(id));
    }

    // Otherwise, the one case that reaches deeper: a PlayStation serial living
    // in a file past the buffer. `find_root_file` gives its extent without
    // reading it, so this is a lookup near the front followed by one seek.
    let mut prefix = sector::Prefix::new(&head);
    if !iso9660::is_iso9660(&mut prefix) {
        return Ok(None);
    }
    let Some(entry) = iso9660::find_root_file(&mut prefix, "SYSTEM.CNF") else {
        return Ok(None);
    };
    let Some(geometry) = prefix.geometry() else {
        return Ok(None);
    };
    // `base` is wherever the caller's reader stood, a cue sheet's offset among
    // them, so the sum is checked: a position past `u64` is a file that is not there.
    let Some(at) = (u64::from(entry.lba) * geometry.stride() as u64)
        .checked_add(geometry.offset() as u64)
        .and_then(|within| base.checked_add(within))
    else {
        return Ok(None);
    };
    reader.seek(SeekFrom::Start(at))?;
    let mut buf = vec![0u8; sector::USER_DATA];
    let read = read_upto(&mut reader, &mut buf)?;
    buf.truncate(read.min(entry.size as usize));

    Ok(iso9660::parse_system_cnf(&String::from_utf8_lossy(&buf)).map(|(generation, serial)| DiscId {
        system: if generation == "ps2" { systems::PS2 } else { systems::PSX },
        serial: Some(serial),
    }))
}

/// Fill `buf` from `reader`, returning how much was read before the end.
fn read_upto<R: Read>(reader: &mut R, buf: &mut [u8]) -> io::Result<usize> {
    let mut at = 0;
    while at < buf.len() {
        match reader.read(&mut buf[at..])? {
            0 => break,
            n => at += n,
        }
    }
    Ok(at)
}

/// [`Read`]: it pulls only as much as identification asks for, in steps, and
/// stops as soon as there is an answer.
pub fn identify<R: Read>(mut reader: R) -> io::Result<Option<DiscId>> {
    let mut buf: Vec<u8> = Vec::new();
    let mut want = STEP;
    loop {
        while buf.len() < want {
            let mut chunk = vec![0u8; want - buf.len()];
            match reader.read(&mut chunk)? {
                0 => break,
                n => buf.extend_from_slice(&chunk[..n]),
            }
        }
        match identify_prefix(&buf) {
            ControlFlow::Break(answer) => return Ok(answer),
            // Asked for more than the stream can give: this is all there is.
            ControlFlow::Continue(more) if more > buf.len() && buf.len() >= want => {
                want = more.max(want + STEP);
            }
            ControlFlow::Continue(_) => return Ok(None),
        }
    }
}

/// Identify a disc image already held in memory.
///
/// Convenience over [`identify_prefix`] for a caller with the whole image:
/// a request for more bytes means there are no more, so it reads as
/// unidentified.
#[must_use]
pub fn identify_bytes(bytes: &[u8]) -> Option<DiscId> {
    match identify_prefix(bytes) {
        ControlFlow::Break(answer) => answer,
        ControlFlow::Continue(_) => None,
    }
}

/// How many opening bytes [`is_disc_image`] needs to answer either way.
///
/// Through sector 16 of a cooked dump, where the ISO 9660 descriptor sits. A
/// raw dump is settled by its first sector, which this is well past.
///
/// 34 KiB, which is the point of stating it: a caller asking of a file that is
/// most likely a cartridge can afford this much, where identifying it as a
/// disc reads [`sector::MAX_PREFIX`].
pub const DISC_IMAGE_PREFIX: usize = 17 * sector::USER_DATA;

/// Whether these bytes are laid out as a disc at all.
///
/// A different question from [`identify_prefix`], which answers *which* disc,
/// and the one a caller is left with when nothing was recognised: an unlicensed
/// release bypasses the boot format every signature here looks for while still
/// being a CD, and it deserves to be treated as one.
///
/// Two layouts, one test each. A raw dump keeps the sector framing, and the
/// twelve-byte sync pattern it opens with is not something a file of game data
/// starts with by accident. A cooked dump has thrown that away, so the ISO 9660
/// descriptor at sector 16 is what is left to say a filesystem lives here.
///
/// Strict on purpose. A cartridge dump is a flat file, and a flat file is
/// indistinguishable from cooked sectors by size alone, so anything looser
/// calls every ROM a disc.
///
/// [`DISC_IMAGE_PREFIX`] opening bytes are always enough to answer.
#[must_use]
pub fn is_disc_image(bytes: &[u8]) -> bool {
    let mut prefix = Prefix::new(bytes);
    match prefix.geometry() {
        Some(sector::Geometry::RawMode1 | sector::Geometry::RawMode2) => true,
        Some(sector::Geometry::Cooked) => iso9660::is_iso9660(&mut prefix),
        None => false,
    }
}

/// The product code of a Sega CD disc.
///
/// The boot sector carries a Mega Drive ROM header at `0x100`, and its serial
/// field sits at `0x180`: two letters of software type, the product number,
/// then a version. Verified against six discs:
///
/// ```text
/// GM T-93175 -00    GM MK-4407 -00    GM TMM-00000-0
/// ```
///
/// The last is unlicensed and overruns the padded layout, so this reads the
/// field rather than slicing fixed columns. `None` when the header is absent,
/// which is the honest answer for a disc laid out differently.
fn sega_cd_serial(sector: &[u8]) -> Option<String> {
    // Licensed discs say `SEGA GENESIS` or `SEGA MEGA DRIVE`; without that the
    // bytes at 0x180 are not a serial field at all.
    if sector.get(0x100..0x105) != Some(&b"SEGA "[..]) {
        return None;
    }
    let raw = field(sector, 0x180, 14)?;

    // Drop the two-letter software type (`GM` for a game, `AI` for an aid).
    let rest = match raw.split_once(' ') {
        Some((kind, rest)) if kind.len() == 2 && kind.chars().all(|c| c.is_ascii_uppercase()) => rest.trim(),
        _ => raw.as_str(),
    };

    // The product number is the first run. A version may follow after a space,
    // or be crammed straight on when the number overruns its column.
    let token = rest.split_whitespace().next()?;
    let trimmed = match token.rsplit_once('-') {
        // Only a trailing `-NN` leaving a hyphen behind is a version, so
        // `T-93175` keeps its own.
        Some((head, tail))
            if head.contains('-')
                && (1..=2).contains(&tail.len())
                && tail.chars().all(|c| c.is_ascii_digit()) =>
        {
            head
        }
        _ => token,
    };
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Where a WIA or RVZ file keeps the opening bytes of the disc it holds.
///
/// The container compresses the image in chunks, but its header carries the
/// disc's first `0x80` bytes **uncompressed**: `WIAFileHead` runs to `0x48`,
/// then `WIADisc` puts `disc_head` at `+0x10`. Since GameCube and Wii both
/// state their identity in that region — game code at 0, magic at `0x1C` or
/// `0x18` — a compressed image identifies without decompressing anything.
///
/// That is why this needs no decompressor. It does mean nothing *else* inside
/// an RVZ is reachable, which is fine: for these two systems the disc head is
/// the whole identity, unlike a PlayStation whose serial is a file in the
/// filesystem.
const WIA_DISC_HEAD: usize = 0x58;

/// Bytes of the disc that a WIA or RVZ header carries.
const WIA_DISC_HEAD_LEN: usize = 0x80;

/// The disc head inside a WIA or RVZ container, if this is one.
fn wia_disc_head(head: &[u8]) -> Option<&[u8]> {
    // `RVZ\x01` is Dolphin's current format and `WIA\x01` its predecessor; the
    // header layout this reads is common to both.
    if !head.starts_with(b"RVZ\x01") && !head.starts_with(b"WIA\x01") {
        return None;
    }
    head.get(WIA_DISC_HEAD..WIA_DISC_HEAD + WIA_DISC_HEAD_LEN)
}

/// GameCube and Wii both open with a magic word at a fixed offset.
fn nintendo(bytes: &[u8]) -> Option<DiscId> {
    let magic_at = |at: usize| -> Option<u32> {
        let s = bytes.get(at..at + 4)?;
        Some(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    };
    // The six-character game id at offset 0 is the serial for both.
    if magic_at(0x1C) == Some(0xC233_9F3D) {
        return Some(DiscId { system: systems::GC, serial: field(bytes, 0, 6) });
    }
    if magic_at(0x18) == Some(0x5D1C_9EA3) {
        return Some(DiscId { system: systems::WII, serial: field(bytes, 0, 6) });
    }
    None
}

/// A PlayStation disc is an ISO 9660 volume whose `SYSTEM.CNF` says which
/// generation it is: `BOOT` for the first, `BOOT2` for the second.
fn playstation(prefix: &mut Prefix<'_>) -> Option<DiscId> {
    let cnf = iso9660::read_root_file(prefix, "SYSTEM.CNF")?;
    let text = String::from_utf8_lossy(cnf);
    let (generation, serial) = iso9660::parse_system_cnf(&text)?;
    Some(DiscId {
        system: if generation == "ps2" { systems::PS2 } else { systems::PSX },
        serial: Some(serial),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sector::USER_DATA;

    fn cooked(sectors: &[&[u8]]) -> Vec<u8> {
        let mut out = Vec::new();
        for s in sectors {
            let mut sector = s.to_vec();
            sector.resize(USER_DATA, 0);
            out.extend_from_slice(&sector);
        }
        out
    }

    #[test]
    fn sega_headers_name_their_system_and_serial() {
        let mut saturn = b"SEGA SEGASATURN ".to_vec();
        saturn.resize(0x20, b' ');
        saturn.extend_from_slice(b"T-1809G   ");
        let id = identify_bytes(&cooked(&[&saturn])).unwrap();
        assert_eq!(id.system, systems::SATURN);
        assert_eq!(id.serial.as_deref(), Some("T-1809G"));

        let mut dc = b"SEGA SEGAKATANA ".to_vec();
        dc.resize(0x40, b' ');
        dc.extend_from_slice(b"HDR-0119  ");
        let id = identify_bytes(&cooked(&[&dc])).unwrap();
        assert_eq!(id.system, systems::DREAMCAST);
        assert_eq!(id.serial.as_deref(), Some("HDR-0119"));
    }

    #[test]
    fn a_sega_cd_without_a_rom_header_reports_no_serial() {
        let id = identify_bytes(&cooked(&[b"SEGADISCSYSTEM  "])).unwrap();
        assert_eq!(id.system, systems::SEGA_CD);
        assert_eq!(id.serial, None, "no header means no field to read");
    }

    fn sega_cd_boot(serial: &[u8]) -> Vec<u8> {
        let mut s = b"SEGADISCSYSTEM  ".to_vec();
        s.resize(0x100, 0);
        s.extend_from_slice(b"SEGA GENESIS    ");
        s.resize(0x180, b' ');
        s.extend_from_slice(serial);
        s
    }

    /// The three shapes seen across six real discs, including the unlicensed
    /// one that overruns the padded columns.
    #[test]
    fn a_sega_cd_serial_is_read_out_of_its_mega_drive_header() {
        for (raw, want) in [
            (&b"GM MK-4407 -00"[..], "MK-4407"),
            (&b"GM T-93175 -00"[..], "T-93175"),
            (&b"GM TMM-00000-0"[..], "TMM-00000"),
        ] {
            let id = identify_bytes(&cooked(&[&sega_cd_boot(raw)])).unwrap();
            assert_eq!(id.system, systems::SEGA_CD);
            assert_eq!(id.serial.as_deref(), Some(want), "from {raw:?}");
        }
    }

    /// The distinction the fingerprint rests on. A cartridge dump is a flat
    /// file, and by length alone it is indistinguishable from cooked sectors,
    /// so calling it a disc would fingerprint bytes that were never framed and
    /// hand back an identifier no catalogue can hold.
    #[test]
    fn a_flat_file_of_game_data_is_not_a_disc() {
        let rom: Vec<u8> = (0..64 * 1024).map(|i| u8::try_from(i % 251).unwrap()).collect();
        assert!(!is_disc_image(&rom));
        assert!(!is_disc_image(&[]), "nothing at all is not a disc");
        assert!(!is_disc_image(&vec![0u8; USER_DATA / 2]), "less than a sector is not a disc");
    }

    #[test]
    fn a_raw_dump_is_a_disc_by_its_sync_pattern() {
        let mut raw = vec![0x00];
        raw.extend_from_slice(&[0xFFu8; 10]);
        raw.push(0x00);
        raw.extend_from_slice(&[0, 0, 0, 1]);
        raw.resize(sector::RAW, 0xAB);
        assert!(is_disc_image(&raw));
    }

    /// A cooked dump threw the framing away, so its filesystem is what is left
    /// to say a disc was ever there.
    #[test]
    fn a_cooked_dump_is_a_disc_by_its_filesystem() {
        let mut sectors: Vec<Vec<u8>> = (0..17).map(|_| vec![0x5Au8; USER_DATA]).collect();
        let cooked_without = cooked(&sectors.iter().map(std::vec::Vec::as_slice).collect::<Vec<_>>());
        assert!(!is_disc_image(&cooked_without));

        sectors[16][..8].copy_from_slice(b"\x01CD001\x01\x00");
        let cooked_with = cooked(&sectors.iter().map(std::vec::Vec::as_slice).collect::<Vec<_>>());
        assert!(is_disc_image(&cooked_with));
    }

    #[test]
    fn a_gamecube_image_is_flat_rather_than_sectored() {
        let mut gc = b"GAFE01".to_vec();
        gc.resize(0x1C, 0);
        gc.extend_from_slice(&0xC233_9F3Du32.to_be_bytes());
        gc.resize(USER_DATA, 0);
        let id = identify_bytes(&gc).unwrap();
        assert_eq!(id.system, systems::GC);
        assert_eq!(id.serial.as_deref(), Some("GAFE01"));
    }

    #[test]
    fn a_playstation_disc_is_read_through_its_filesystem() {
        // Sector 16 is the volume descriptor, whose root record points at 18;
        // the root directory there names SYSTEM.CNF, which lives at 20.
        let mut pvd = b"\x01CD001\x01".to_vec();
        pvd.resize(156, 0);
        let mut root = vec![34u8, 0];
        root.extend_from_slice(&18u32.to_le_bytes()); // extent
        root.extend_from_slice(&[0; 4]);
        root.extend_from_slice(&u32::try_from(USER_DATA).unwrap().to_le_bytes()); // length
        root.resize(34, 0);
        pvd.extend_from_slice(&root);

        let cnf_text = b"BOOT = cdrom:\\SCUS_941.63;1\nTCB = 4\n";
        let mut entry = vec![0u8; 33];
        entry[0] = 33 + 12;
        entry[2..6].copy_from_slice(&20u32.to_le_bytes());
        entry[10..14].copy_from_slice(&u32::try_from(cnf_text.len()).unwrap().to_le_bytes());
        entry[32] = 12;
        entry.extend_from_slice(b"SYSTEM.CNF;1");

        let mut image = cooked(&[&[], &[]]);
        image.resize(16 * USER_DATA, 0);
        image.extend_from_slice(&cooked(&[&pvd])); // 16
        image.extend_from_slice(&cooked(&[&[]])); // 17
        image.extend_from_slice(&cooked(&[&entry])); // 18
        image.extend_from_slice(&cooked(&[&[]])); // 19
        image.extend_from_slice(&cooked(&[cnf_text])); // 20

        let id = identify_bytes(&image).unwrap();
        assert_eq!(id.system, systems::PSX);
        assert_eq!(id.serial.as_deref(), Some("SCUS-94163"));
    }

    /// Verified against a real dump: Metal Slug (Japan), raw Mode 1 track 1,
    /// whose ISO 9660 root carries the loader manifest and no serial anywhere.
    #[test]
    fn a_neogeo_cd_is_recognised_by_its_loader_manifest() {
        let mut pvd = b"\x01CD001\x01".to_vec();
        pvd.resize(156, 0);
        let mut root = vec![34u8, 0];
        root.extend_from_slice(&18u32.to_le_bytes());
        root.extend_from_slice(&[0; 4]);
        root.extend_from_slice(&u32::try_from(USER_DATA).unwrap().to_le_bytes());
        root.resize(34, 0);
        pvd.extend_from_slice(&root);

        let mut entry = vec![0u8; 33];
        entry[0] = 33 + 9;
        entry[2..6].copy_from_slice(&20u32.to_le_bytes());
        entry[10..14].copy_from_slice(&16u32.to_le_bytes());
        entry[32] = 9;
        entry.extend_from_slice(b"IPL.TXT;1");

        let mut image = vec![0u8; 16 * USER_DATA];
        image.extend_from_slice(&cooked(&[&pvd])); // 16
        image.extend_from_slice(&cooked(&[&[]])); // 17
        image.extend_from_slice(&cooked(&[&entry])); // 18
        image.extend_from_slice(&cooked(&[&[]])); // 19
        image.extend_from_slice(&cooked(&[b"PROG.PRG,0,0\r\n"])); // 20, the manifest

        let id = identify_bytes(&image).unwrap();
        assert_eq!(id.system, systems::NEOGEO_CD);
        assert_eq!(id.serial, None);
    }

    /// Regression, from a real Metal Slug dump: its `IPL.TXT` sits at LBA 7238,
    /// seventeen megabytes in and far past what identification buffers. Reading
    /// the manifest to prove it exists failed on every disc it was meant to
    /// recognise; only its presence in the directory matters.
    #[test]
    fn a_manifest_far_past_the_buffer_cap_still_identifies_the_disc() {
        let mut pvd = b"\x01CD001\x01".to_vec();
        pvd.resize(156, 0);
        let mut root = vec![34u8, 0];
        root.extend_from_slice(&18u32.to_le_bytes());
        root.extend_from_slice(&[0; 4]);
        root.extend_from_slice(&u32::try_from(USER_DATA).unwrap().to_le_bytes());
        root.resize(34, 0);
        pvd.extend_from_slice(&root);

        // The entry points a long way out; nothing there is ever read.
        let mut entry = vec![0u8; 33];
        entry[0] = 33 + 7;
        entry[2..6].copy_from_slice(&7238u32.to_le_bytes());
        entry[10..14].copy_from_slice(&64u32.to_le_bytes());
        entry[32] = 7;
        entry.extend_from_slice(b"IPL.TXT"); // no `;1`, as the real disc writes it

        let mut image = vec![0u8; 16 * USER_DATA];
        image.extend_from_slice(&cooked(&[&pvd]));
        image.extend_from_slice(&cooked(&[&[]]));
        image.extend_from_slice(&cooked(&[&entry]));

        let id = identify_bytes(&image).expect("presence is enough");
        assert_eq!(id.system, systems::NEOGEO_CD);
    }

    /// The contract an async caller drives: too little says how much would do,
    /// and asking again with that much answers. No I/O anywhere in the loop.
    #[test]
    fn a_short_prefix_asks_for_what_it_needs_and_then_answers() {
        let mut saturn = b"SEGA SEGASATURN ".to_vec();
        saturn.resize(0x20, b' ');
        saturn.extend_from_slice(b"T-1809G   ");
        let image = cooked(&[&saturn]);

        // Two hundred bytes cannot even settle the geometry.
        let want = match identify_prefix(&image[..200]) {
            ControlFlow::Continue(n) => n,
            ControlFlow::Break(other) => panic!("expected a request for more, got {other:?}"),
        };
        assert!(want <= sector::MAX_PREFIX);

        match identify_prefix(&image[..want.min(image.len())]) {
            ControlFlow::Break(Some(id)) => assert_eq!(id.system, systems::SATURN),
            other => panic!("expected an answer, got {other:?}"),
        }
    }

    /// From two real discs: the boot header sits at 0x20 of the data track's
    /// second sector, and reads the same whether the track was stored cooked
    /// or raw.
    #[test]
    fn a_pc_engine_cd_is_recognised_by_its_boot_header() {
        let mut second = vec![0u8; 0x20];
        second.extend_from_slice(b"PC Engine CD-ROM SYSTEM");
        second.extend_from_slice(b"\x00Copyright HUDSON SOFT / NEC Home Electronics,Ltd.");
        let id = identify_bytes(&cooked(&[&[], &second])).unwrap();
        assert_eq!(id.system, systems::PCENGINECD);
        assert_eq!(id.system.slug, "pcenginecd");
        assert_eq!(id.serial, None);
    }

    /// From three real files: an RVZ carries the disc's first 0x80 bytes
    /// uncompressed, so a compressed image identifies with no decompressor and
    /// gives the same answer as the raw one beside it.
    #[test]
    fn a_dolphin_container_identifies_from_its_header() {
        for magic in [&b"RVZ\x01"[..], &b"WIA\x01"[..]] {
            let mut file = magic.to_vec();
            file.resize(0x58, 0);
            // The disc head, exactly as a flat image would open.
            let mut disc = b"GAFE01".to_vec();
            disc.resize(0x1C, 0);
            disc.extend_from_slice(&0xC233_9F3Du32.to_be_bytes());
            disc.resize(0x80, 0);
            file.extend_from_slice(&disc);

            let id = identify_bytes(&file).expect("header names the disc");
            assert_eq!(id.system, systems::GC);
            assert_eq!(id.serial.as_deref(), Some("GAFE01"));
        }
    }

    #[test]
    fn an_unrecognised_image_is_none_rather_than_a_guess() {
        assert_eq!(identify_bytes(&cooked(&[b"not a disc at all"])), None);
        assert_eq!(identify_bytes(&[0u8; 8]), None);
    }
}
