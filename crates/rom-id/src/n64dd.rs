//! The Nintendo 64DD: a disk's system area, and the game its disk ID names.
//!
//! A disk image is a retail dump in block order or MAME's image in physical order, told apart by
//! length; [`n64dd_disk`] reads both. The system record at the start of either says the disk is a
//! 64DD's, which is a header the drive checks, and the disk ID a few blocks on carries the game's
//! four-character code, as a cartridge's header carries an N64 game's.

use one_saves_registry::{System, systems};

use crate::header::found;
use crate::{RomId, View};

/// How much of the disk ID to read: the game code, its version, the disk's number, and the rest of
/// the fields a probe has no use for yet.
const DISK_ID: usize = 0x20;

/// Where an image of this length keeps its disk ID, for a probe to read.
pub(crate) fn disk_id_window(len: u64) -> Option<std::ops::Range<u64>> {
    let layout = n64dd_disk::Layout::of_len(usize::try_from(len).ok()?)?;
    let at = n64dd_disk::disk_id_offset(layout) as u64;
    Some(at..at + DISK_ID as u64)
}

/// A 64DD disk, in either layout. A development disk keeps its system record further in, where a
/// probe does not look, and goes by its name.
pub(crate) fn header(view: View<'_>, _: &[System]) -> Option<RomId> {
    let window = disk_id_window(view.size())?;
    let system = n64dd_disk::SystemArea::from_record(view.bytes(0, 0xE8)?)?;
    if system.region == n64dd_disk::Region::Development {
        return None;
    }
    let disk_id = view.bytes(window.start, DISK_ID)?;
    let code = &disk_id[..4];
    let serial =
        code.iter().all(u8::is_ascii_alphanumeric).then(|| String::from_utf8_lossy(code).into_owned());
    Some(RomId { serial, revision: Some(disk_id[4]), ..found(systems::N64DD, view) })
}

#[cfg(test)]
mod tests {
    use crate::{ByteOrder, Evidence, Probe, catalogued, identify_bytes};

    /// A retail dump with a Japanese disk's system record at its start and Mario Artist: Paint
    /// Studio's code in its disk ID, and nothing else.
    fn paint_studio() -> Vec<u8> {
        let mut image = vec![0u8; n64dd_disk::RETAIL_LEN];
        let mut record = [0u8; 0xE8];
        record[..4].copy_from_slice(&0xE848_D316u32.to_be_bytes());
        record[4] = 0x10;
        record[5] = 0x10;
        record[6..8].copy_from_slice(&0x0018u16.to_be_bytes());
        record[0x1C..0x20].copy_from_slice(&0x8000_0400u32.to_be_bytes());
        image[..0xE8].copy_from_slice(&record);
        let at = n64dd_disk::disk_id_offset(n64dd_disk::Layout::Retail);
        image[at..at + 5].copy_from_slice(b"DMPJ\x00");
        image
    }

    #[test]
    fn a_disk_names_its_game_by_the_code_its_disk_id_carries() {
        let image = paint_studio();
        let id = identify_bytes(None, &image).expect("a 64DD disk");
        assert_eq!((id.system.slug, id.evidence), ("n64dd", Evidence::Header));
        assert_eq!(id.serial.as_deref(), Some("DMPJ"));

        // A probe reads the disk ID where it sits, as well as the front of the image.
        let mut probe = Probe::new(image.len() as u64);
        for window in probe.windows() {
            let range = usize::try_from(window.start).unwrap()..usize::try_from(window.end).unwrap();
            probe.supply(window.start, image[range].to_vec());
        }
        assert_eq!(probe.identify(None), Some(id));
    }

    #[test]
    fn a_rom_is_hashed_as_its_catalog_holds_it() {
        // An N64 ROM in each of its byte orders comes back big-endian.
        let mut z64 = vec![0u8; 0x1000];
        z64[..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        z64[0x3B..0x3F].copy_from_slice(b"NSME");
        let v64: Vec<u8> = z64.chunks(2).flat_map(|w| [w[1], w[0]]).collect();
        let n64: Vec<u8> = z64.chunks(4).flat_map(|w| [w[3], w[2], w[1], w[0]]).collect();
        for (file, order) in
            [(&z64, ByteOrder::Native), (&v64, ByteOrder::Swapped16), (&n64, ByteOrder::Swapped32)]
        {
            let id = identify_bytes(None, file).expect("an N64 ROM");
            assert_eq!(id.byte_order, order);
            assert_eq!(catalogued(&id, file), z64.as_slice(), "{order:?}");
        }

        // An iNES header is the file's, not the cartridge's.
        let mut nes = b"NES\x1A\x02\x01\0\0\0\0\0\0\0\0\0\0".to_vec();
        nes.extend(vec![0xEA; 0x8000 + 0x2000]);
        let id = identify_bytes(None, &nes).expect("an NES ROM");
        assert_eq!(catalogued(&id, &nes), &nes[16..]);

        // A retail dump is already the form a disk is hashed in.
        let image = paint_studio();
        let id = identify_bytes(None, &image).unwrap();
        assert!(
            matches!(catalogued(&id, &image), std::borrow::Cow::Borrowed(bytes) if bytes.len() == image.len())
        );
    }
}
