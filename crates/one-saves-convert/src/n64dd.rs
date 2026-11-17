//! 64DD disks.
//!
//! A 64DD disk keeps a game's saves in its RAM area, the part of the disk a game can write, and
//! its save is that area: an `n64dd` save whose one `save` part, with no role, holds the area's
//! blocks in ascending order, each as the drive transfers it. mupen64plus, Project64 and MiSTer's
//! N64 core keep exactly that as a `.ram` file, so reading one is [`raw::wrap`](crate::raw::wrap)
//! with the system this module names, and writing one back is the part's payload.
//!
//! The disk's type, 0 to 6, fixes where the area starts, and it runs from there to the disk's last
//! block, so each type's area has a length of its own and the length is how the type is told back.
//! A type 6 disk has no area and no save.
//!
//! # A copy of the whole disk
//!
//! Other emulators keep the disk whole and write a game's saves into it: mupen64plus and Project64
//! as an `.ndr` when set to, Mupen64Plus-Next as a `.disk_save`, ares as a `.disk` in MAME's order,
//! and SummerCart64 into the `.ndd` itself. Such a copy is an image of the disk like any other, so
//! [`read_image`] takes the RAM area out of it, whichever layout it keeps, and [`write_image`] puts
//! one back into a copy of the image it is given. The rest of a copy is the game, which a save
//! does not carry; writing one back takes the disk's image as well.

use one_saves::extension::RtcN64dd;
use one_saves::{Bundle, Game, Header, Part, PartKind, Shape, Slug};

use crate::error::{Error, Result};
use crate::raw::{RawOptions, wrap};

/// The system a 64DD disk's save names.
pub const SYSTEM: &str = "n64dd";

/// The extension mupen64plus, Project64 and MiSTer's N64 core give a disk's RAM area.
pub const EXTENSION: &str = "ram";

/// Each disk type's RAM area, 0 to 5: the block it starts at, as libleo numbers blocks from the
/// first one after the system area, how many blocks it runs to, and its length in bytes.
pub const AREAS: [Area; 6] = [
    Area { first_block: 1418, blocks: 2874, len: 38_444_480 },
    Area { first_block: 1966, blocks: 2326, len: 29_501_120 },
    Area { first_block: 2514, blocks: 1778, len: 21_303_040 },
    Area { first_block: 3062, blocks: 1230, len: 13_850_240 },
    Area { first_block: 3610, blocks: 682, len: 7_142_720 },
    Area { first_block: 4088, blocks: 204, len: 1_942_080 },
];

/// Where one disk type's RAM area sits, and how long it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Area {
    /// The first block of the area, numbered as libleo numbers them.
    pub first_block: usize,
    /// How many blocks the area runs to.
    pub blocks: usize,
    /// The area's length in bytes, which is a disk save's payload length.
    pub len: usize,
}

/// The disk type whose RAM area is `len` bytes long, which is how a save tells its type.
#[must_use]
pub fn disk_type(len: usize) -> Option<u8> {
    AREAS.iter().position(|area| area.len == len).and_then(|index| u8::try_from(index).ok())
}

/// Whether a bundle is a 64DD disk's save: a save naming [`SYSTEM`].
#[must_use]
pub fn is_disk_save(bundle: &Bundle) -> bool {
    *bundle.shape() == one_saves::Shape::Save
        && bundle.header.system.as_ref().is_some_and(|system| system.as_str() == SYSTEM)
}

/// Whether these bytes are a whole 64DD disk, in either layout: an image, or an emulator's copy
/// of one with a game's saves written into it.
#[must_use]
pub fn is_image(bytes: &[u8]) -> bool {
    n64dd_disk::Disk::parse(bytes).is_ok()
}

/// A disk's save, from its RAM area alone, as a `.ram` holds it, or from a whole image; and with
/// ares's drive clock in [`rtc_sidecar`](RawOptions::rtc_sidecar), the device that clock goes on.
///
/// The drive keeps the 64DD's clock, so no disk's save carries it. A device does, on its header:
/// an `n64` device holding the disk's save in the role `64dd`, as the specification has it.
pub fn read(bytes: &[u8], options: &RawOptions) -> Result<Bundle> {
    let quiet = RawOptions { rtc_sidecar: None, ..options.clone() };
    let save = if is_image(bytes) {
        read_image(bytes, &quiet)?
    } else {
        wrap(bytes, &RawOptions { system: Some(SYSTEM.to_owned()), split_rtc: false, ..quiet })?
    };
    let clock =
        options.rtc_sidecar.as_ref().and_then(|(file, _)| crate::console_clock::parse_ares_n64dd(file));
    match clock {
        Some(clock) => with_drive(&save, clock, options),
        None => Ok(save),
    }
}

/// The device a disk's save and the drive's clock make together.
fn with_drive(save: &Bundle, clock: RtcN64dd, options: &RawOptions) -> Result<Bundle> {
    let slug = |text: &str| Slug::parse(text).expect("a slug");
    let mut header =
        Header::new(Shape::Device).system(slug(crate::n64::SYSTEM)).source(options.source.clone());
    // A clock whose stopped bit is set has no reading, and the chip's bytes still ride.
    if let Some(reading) = clock.reading() {
        header.extensions.insert(reading);
    }
    header.extensions.insert(clock);
    let disk = Part::new(0, save.to_vec()?)
        .kind(PartKind::Bundle)
        .role(slug(DRIVE_ROLE))
        .system(slug(SYSTEM))
        .game(save.header.game.clone());
    Ok(Bundle::new(header, vec![disk]))
}

/// The role a disk's save takes on an N64 device: the drive it is in.
pub const DRIVE_ROLE: &str = "64dd";

/// Whether a bundle is an N64 device holding a disk's save in the drive and nothing else, as
/// [`read`] makes of a disk's save and ares's drive clock.
#[must_use]
pub fn is_drive(bundle: &Bundle) -> bool {
    *bundle.shape() == Shape::Device
        && bundle.header.system.as_ref().is_some_and(|system| system.as_str() == crate::n64::SYSTEM)
        && matches!(bundle.parts.as_slice(), [part] if part.role.as_ref().is_some_and(|role| role.as_str() == DRIVE_ROLE))
}

/// The disk's save out of such a device, and the drive's clock as ares keeps it, anchored at
/// `now` when the bundle did not record when the clock was taken.
pub fn split_drive(bundle: &Bundle, now: i64) -> Result<(Bundle, Option<[u8; 16]>)> {
    if !is_drive(bundle) {
        return Err(Error::NotConvertible("this bundle is not a 64DD drive holding a disk".into()));
    }
    let save = Bundle::from_slice(&bundle.parts[0].payload)?;
    let clock = bundle
        .header
        .extensions
        .get::<RtcN64dd>()
        .map(|clock| crate::console_clock::build_ares_n64dd(clock, now));
    Ok((save, clock))
}

/// A disk image's save: its RAM area, as an `n64dd` save.
///
/// The game is what `options` names, or else what the disk ID does: its game code as a product
/// code, `NUD-` and the code. No digest is taken, since a copy that holds a game's saves is no
/// longer the dump a catalog lists.
pub fn read_image(bytes: &[u8], options: &RawOptions) -> Result<Bundle> {
    let disk = n64dd_disk::Disk::parse(bytes).map_err(|e| Error::NotConvertible(e.to_string()))?;
    let area = disk.ram_area().ok_or_else(|| {
        Error::NotConvertible("this is a type 6 disk, which has no RAM area and so no save".into())
    })?;
    let code = &disk.disk_id()[..4];
    let named = code
        .iter()
        .all(u8::is_ascii_alphanumeric)
        .then(|| Game::default().serial(format!("NUD-{}", String::from_utf8_lossy(code))));
    let options = RawOptions {
        system: Some(SYSTEM.to_owned()),
        game: options.game.clone().or(named),
        // A disk carries no clock; the drive keeps it.
        split_rtc: false,
        ..options.clone()
    };
    wrap(&area, &options)
}

/// A copy of `image`, in its own layout, holding a disk save's RAM area: the file an emulator that
/// keeps the disk whole reads the save from.
pub fn write_image(bundle: &Bundle, image: &[u8]) -> Result<Vec<u8>> {
    if !is_disk_save(bundle) {
        return Err(Error::NotConvertible("this bundle is not a 64DD disk's save".into()));
    }
    let area = &bundle
        .parts
        .first()
        .ok_or_else(|| Error::NotConvertible("this save holds no RAM area".into()))?
        .payload;
    let disk = n64dd_disk::Disk::parse(image).map_err(|e| Error::NotConvertible(e.to_string()))?;
    disk.with_ram_area(area).map_err(|e| Error::NotConvertible(e.to_string()))
}

/// The extension ares gives the copy of a disk it keeps a game's saves in.
pub const ARES_DISK_EXTENSION: &str = "disk";

/// A copy of `image` holding a disk save's RAM area, laid out as ares keeps it: in MAME's order,
/// whichever order `image` keeps. ares replaces the disk it loaded with this copy whole.
pub fn write_ares_disk(bundle: &Bundle, image: &[u8]) -> Result<Vec<u8>> {
    let copy = write_image(bundle, image)?;
    let disk = n64dd_disk::Disk::parse(&copy).map_err(|e| Error::NotConvertible(e.to_string()))?;
    Ok(disk.to_mame().map_err(|e| Error::NotConvertible(e.to_string()))?.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_type_s_area_runs_from_its_first_block_to_the_disk_s_last() {
        // The last block is the same for every type, and the length falls zone by zone.
        for area in AREAS {
            assert_eq!(area.first_block + area.blocks, 4292);
        }
        assert_eq!(disk_type(1_942_080), Some(5));
        assert_eq!(disk_type(38_444_480), Some(0));
        assert_eq!(disk_type(32_768), None);
    }

    #[test]
    fn a_ram_file_is_the_save_as_it_stands() {
        let area = vec![0x5a; AREAS[5].len];
        let options = RawOptions { system: Some(SYSTEM.to_owned()), ..RawOptions::default() };
        let bundle = wrap(&area, &options).expect("wraps");
        assert!(is_disk_save(&bundle));
        assert_eq!(bundle.parts.len(), 1);
        assert_eq!(bundle.parts[0].role, None, "the area takes no role");
        assert_eq!(bundle.parts[0].payload, area);
        let decoded = Bundle::from_slice(&bundle.to_vec().expect("a valid save")).expect("decodes");
        assert_eq!(decoded, bundle);

        let short = wrap(&area[1..], &options).expect("wraps");
        assert!(short.to_vec().is_err(), "no disk type's area is that long");
    }

    #[test]
    fn a_disk_s_save_and_ares_s_drive_clock_are_one_device() {
        let area = vec![0x5a; AREAS[5].len];
        let mut clock_file = vec![0x26, 0x11, 0x17, 0x09, 0x59, 0x58, 0xFF, 0xFF];
        clock_file.extend_from_slice(&1_794_909_600u64.to_le_bytes());
        let options = RawOptions { rtc_sidecar: Some((clock_file, 0)), ..RawOptions::default() };
        let device = read(&area, &options).expect("reads");
        device.validate().expect("a valid device");
        assert_eq!(*device.shape(), Shape::Device);
        assert_eq!(device.header.system.as_ref().map(Slug::as_str), Some("n64"));
        assert_eq!(device.parts[0].role.as_ref().map(Slug::as_str), Some(DRIVE_ROLE));
        let clock = device.header.extensions.get::<RtcN64dd>().expect("the drive's clock");
        assert_eq!(clock.taken_at, Some(1_794_909_600));
        assert_eq!(
            device.header.extensions.get::<one_saves::extension::Rtc>().map(|rtc| rtc.reading),
            Some(1_794_909_598)
        );
        let save = Bundle::from_slice(&device.parts[0].payload).unwrap();
        assert!(is_disk_save(&save));
        assert_eq!(save.parts[0].payload, area);

        // And apart again: the disk's save, and the file ares keeps the clock in.
        let (alone, file) = split_drive(&device, 0).expect("splits");
        assert_eq!(alone, save);
        assert_eq!(file.map(|file| file.to_vec()), options.rtc_sidecar.map(|(file, _)| file));

        // A clock ares never saved is none, and the save stands alone.
        let unset = RawOptions { rtc_sidecar: Some((vec![0xFF; 16], 0)), ..RawOptions::default() };
        assert!(is_disk_save(&read(&area, &unset).unwrap()));
    }

    /// A retail dump of a type 5 disk with the system record the drive checks, Mario Artist:
    /// Paint Studio's code in its disk ID, and every block of the RAM area holding its number.
    fn disk() -> Vec<u8> {
        let offsets = n64dd_disk::retail_offsets(5);
        let mut image = vec![0u8; n64dd_disk::RETAIL_LEN];
        let mut record = vec![0u8; 0xE8];
        record[..4].copy_from_slice(&0xE848_D316u32.to_be_bytes());
        record[4] = 0x10;
        record[5] = 0x15;
        record[6..8].copy_from_slice(&0x0018u16.to_be_bytes());
        record[0x1C..0x20].copy_from_slice(&0x8000_0400u32.to_be_bytes());
        for zone in 0..16 {
            record[8 + zone] = u8::try_from((zone + 1) * 12).unwrap();
            for defect in 0..12 {
                record[0x20 + zone * 12 + defect] =
                    u8::try_from(if zone == 0 { 12 } else { 1 } + defect * 8).unwrap();
            }
        }
        for block in [0, 1, 8, 9] {
            for sector in 0..85 {
                let at = offsets[block] + sector * 0xE8;
                image[at..at + 0xE8].copy_from_slice(&record);
            }
        }
        for sector in 0..85 {
            let at = offsets[n64dd_disk::DISK_ID_BLOCK] + sector * 0xE8;
            image[at..at + 4].copy_from_slice(b"DMPJ");
        }
        for block in AREAS[5].first_block + 24..n64dd_disk::BLOCKS {
            image[offsets[block]..offsets[block + 1]].fill(u8::try_from(block % 256).unwrap());
        }
        image
    }

    #[test]
    fn a_copy_of_the_whole_disk_is_its_ram_area_in_either_layout() {
        let retail = disk();
        let mame = n64dd_disk::Disk::parse(&retail).unwrap().to_mame().unwrap().into_owned();
        let from_retail = read_image(&retail, &RawOptions::default()).expect("reads a retail copy");
        let from_mame = read_image(&mame, &RawOptions::default()).expect("reads MAME's");
        assert_eq!(from_retail, from_mame, "one save, whichever layout kept it");
        assert!(is_disk_save(&from_retail));
        assert_eq!(from_retail.parts[0].payload.len(), AREAS[5].len);
        assert_eq!(
            from_retail.header.game.as_ref().and_then(|game| game.serial.as_deref()),
            Some("NUD-DMPJ")
        );
        from_retail.to_vec().expect("a valid save");

        // Written back onto either image, the save lands where it came from and nothing else moves.
        let mut save = from_retail.clone();
        save.parts[0].payload.fill(0xA5);
        save.parts[0].rehash();
        for image in [&retail, &mame] {
            let written = write_image(&save, image).expect("writes");
            assert_eq!(written.len(), image.len());
            let again = read_image(&written, &RawOptions::default()).unwrap();
            assert_eq!(again.parts[0].payload, save.parts[0].payload);
            assert_eq!(written[..0x4D08], image[..0x4D08], "the system area is the disk's");
            // And for ares, always in MAME's order.
            let ares = write_ares_disk(&save, image).expect("writes");
            assert_eq!(ares, write_image(&save, &mame).unwrap());
        }
    }
}
