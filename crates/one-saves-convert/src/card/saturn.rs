//! Sega Saturn backup RAM.
//!
//! The filesystem itself lives in [`saturn_backup`], which is a standalone crate because a Saturn
//! volume is a genuinely reusable format and nothing about reading one needs this container. What
//! is here is the adapter: the mapping between a volume's saves and a bundle's nested parts.
//!
//! # Where the entry lives
//!
//! Every other card here keeps a directory apart from the saves. A Saturn save carries its own
//! entry at the head of its first block: the name, the language, the comment, the date, the
//! length, and then the list of every block it occupies.
//!
//! The entry is still a `dirent` like any other, and rides as one. Nothing in those thirty bytes
//! depends on where the save sits, so they are carried verbatim and handed back untouched. What
//! *is* position-dependent is the block list after them, and the writer rebuilds that — which
//! every format here does for its own allocation table anyway. The specifications call Saturn out
//! as the one format where that rebuild reaches inside the save's own blocks rather than a
//! directory region; see [`saturn_backup::BackupBuilder`].
//!
//! Carrying it whole is what keeps the language byte, which no other field here exposes, and
//! whatever a game left in the padding of a field it did not fill — NiGHTS writes `A-life` into
//! ten bytes of comment and leaves an `0xEE` in the eighth.
//!
//! # Two containers, one volume
//!
//! Mednafen and its libretro fork write the volume's data. Yabause writes the address space the
//! 68000 sees, which is twice as long with each byte behind the `0xFF` an unmapped even address
//! reads. [`saturn_backup::strip_container`] tells them apart and reading is identical either
//! way; what a bundle records is the volume, so `extract` writes the packed form. See
//! [`write`](write()).
//!
//! # Two media, one format
//!
//! The console's internal memory is 32 KiB in 64-byte blocks; a Backup RAM Cart is larger in
//! larger ones. Both are `saturn-bup` and the volume states which it is, so nothing here has to
//! know the medium to read one. Which socket a dump came out of is a different question that the
//! bytes cannot answer, so it stays the caller's: `--role internal` or `--role ram-cart`.
//!
//! # What identifies a save
//!
//! The name the console holds, such as `PANDRA_3_01`, which is the [`path`](one_saves::Part::path).
//! The comment beside it rides as the detail line of [`x.1sav.label`], since it is what the BIOS
//! shows and not what tells two saves apart.
//!
//! # A console's files, one device
//!
//! Mednafen and Beetle Saturn keep three files a game: the internal memory, `.bkr` (Beetle under
//! RetroArch, `.srm`), a Backup RAM Cart's volume, `.bcr`, and the SMPC, `.smpc`, which holds the
//! console's clock and four bytes of its settings. The clock is the console's and not a save's, so
//! the three together are a [`device`](one_saves::Shape::Device): the internal memory in
//! `internal`, the cart in `ram-cart`, and the clock on the header as
//! [`x.1sav.rtc.smpc`](RtcSmpc), beside the [`x.1sav.rtc`](one_saves::extension::Rtc) reading it
//! gives. See
//! [`read_device`] and [`write_device`].
//!
//! [`x.1sav.dirent`]: https://docs.1retro.com/specifications/extensions/x.1sav.dirent/
//! [`x.1sav.label`]: https://docs.1retro.com/specifications/extensions/x.1sav.label/

use saturn_backup::{BackupBuilder, BackupRam, ENTRY_LEN, Geometry, Save, Smpc, TAG_LEN};

use crate::CardOptions;
use crate::card::{
    card_header, card_image_part, image_only, modified_only, nested_saves, path_of_name, save_part_with, slug,
};
use crate::detect::Format;
use crate::error::{Error, Result};
use crate::label::value as label;
use one_saves::extension::{Dirent, Label, RtcSmpc};
use one_saves::{Bundle, Header, Part, PartKind, Shape};

/// What the format is called, for error messages.
const FORMAT: &str = Format::SaturnBup.label();

/// The card format slug a bundle carries.
pub const CARD_FORMAT: &str = match Format::SaturnBup.card_format() {
    Some(slug) => slug,
    None => panic!("a Saturn backup RAM is a card format"),
};

/// The socket the console's own backup memory sits in.
const INTERNAL_ROLE: &str = "internal";

/// The socket a Backup RAM Cart sits in.
///
/// A cart extends the internal memory rather than replacing it, so a console can carry saves in
/// both at once and a dump of one says nothing about the other.
const RAM_CART_ROLE: &str = "ram-cart";

/// The system slug these saves are for.
pub const SYSTEM: &str = match Format::SaturnBup.system() {
    Some(slug) => slug,
    None => panic!("a Saturn backup RAM holds saves for a system"),
};

/// Whether these bytes look like a Saturn backup RAM.
#[must_use]
pub fn detect(bytes: &[u8]) -> bool {
    saturn_backup::detect(bytes)
}

/// Reads a volume into a bundle: one nested bundle per save.
pub fn read(bytes: &[u8], options: &CardOptions) -> Result<one_saves::Bundle> {
    let volume = BackupRam::parse(bytes).map_err(into_error)?;
    // The medium is the one thing about a Saturn dump that the bytes settle and the format does
    // not: the console's own memory is 32 KiB in 64-byte blocks and a Backup RAM Cart is larger
    // in larger ones. `Format::default_role` cannot see the bytes, so the refinement is here, and
    // a caller who says which socket it was still wins.
    let role = match &options.role {
        Some(role) => role.clone(),
        None => slug(if volume.geometry().is_internal() { INTERNAL_ROLE } else { RAM_CART_ROLE }),
    };

    let mut parts = Vec::new();
    for (index, save) in volume.saves().iter().enumerate() {
        // The name goes in the label as well as the path, the same way a Neo Geo title does: a
        // consumer after something to show should not have to know that this format files a save
        // under the one string. The comment is the line beside it, which is the detail.
        let mut inner = one_saves::Extensions::new();
        let comment = Some(save.comment.clone()).filter(|line| !line.is_empty());
        if let Some(value) = label(Some(save.name.clone()).filter(|n| !n.is_empty()), comment) {
            inner.insert(value);
        }

        let mut part = save_part_with(Format::SaturnBup, index, save.data.clone(), None, options, inner)?
            // `save_part_with` takes the caller's role or the format's default; neither knows which
            // medium this is, so the one worked out above replaces it.
            .role(role.clone())
            .slot(u64::try_from(index).expect("slot fits"))
            // The entry's own thirty bytes, verbatim, like every other format's. Where they sit is
            // what makes this format unusual — the head of the save's first block rather than a
            // directory region — and nothing in them depends on that: name, language, comment,
            // date and length are the save's wherever it lands. Only the block list after them
            // moves, and a writer rebuilds that for every format here.
            .dirent(save.entry.clone())
            // The date again, decoded, for a consumer that will not parse thirty opaque bytes. The
            // volume records a write and no creation — the shape `x.1sav.dirent` admits for exactly
            // this. No zone: the console keeps a wall clock and does not know one.
            .extension(modified_only(save.written_at(), None));
        part.path = path_of_name(&save.name);
        parts.push(part);
    }

    if parts.is_empty() {
        parts.push(card_image_part(0, volume.image(), &role));
    }

    Ok(Bundle::new(
        card_header(Format::SaturnBup, volume.capacity(), Some(volume.system_area().to_vec()), options),
        parts,
    ))
}

/// Writes a bundle back out as a volume.
///
/// Every save's entry and block list are written afresh, because on this format they are not
/// separable from the payload. What comes back from the bundle is the reserved region, so the
/// volume's signature and whatever block 1 held survive the round trip.
///
/// A volume read out of a **wide** dump comes back **packed**, the same way a Neo Geo card read
/// out of a MiSTer save comes back bare: the container is the producer's, not the volume's, and a
/// bundle records the volume. Nothing is lost — the wide form is the same data with an `0xFF`
/// before each byte, and [`saturn_backup::widen`] puts it back — but a consumer that needs the
/// shape Yabause reads has to ask for it, because the bundle does not say which it came from.
pub fn write(bundle: &one_saves::Bundle) -> Result<Vec<u8>> {
    // A volume with nothing on it carries a whole-card image rather than nested saves, and writing
    // it back is a byte copy.
    if let Some(image) = image_only(bundle)? {
        return Ok(image);
    }

    let card = bundle
        .header
        .card
        .as_ref()
        .ok_or_else(|| Error::NotConvertible("this bundle does not say what card it is".into()))?;
    let capacity = usize::try_from(card.capacity)
        .map_err(|_| Error::NotConvertible("this bundle states a capacity no volume has".into()))?;

    // The block size is the volume's rather than the format's, so it has to come from the bundle.
    // The reserved region states it outright — the signature repeats to fill block 0 — and that is
    // what the reader kept. Falling back to the capacity is for a bundle some other producer wrote
    // without one, and it is exact wherever it answers at all.
    //
    // Either way the result has to be a volume the backup library lays out. The builder allocates
    // the whole volume before it writes a save, so a capacity taken on trust is an allocation of
    // whatever size a bundle cares to state — a gigabyte for fifteen kilobytes of bundle.
    let geometry = match card.system_area.as_deref().and_then(saturn_backup::block_size) {
        Some(block) => Geometry::FIRST_DATA_BLOCK
            .checked_mul(block)
            .and_then(|reserved| capacity.checked_add(reserved))
            .and_then(|size| Geometry::new(size, block)),
        None => Geometry::for_data_capacity(capacity),
    }
    .filter(|g| saturn_backup::GEOMETRIES.contains(&(g.size, g.block)))
    .ok_or_else(|| Error::NotConvertible(format!("no Saturn volume holds {capacity} bytes of saves")))?;

    let mut builder = BackupBuilder::new(geometry.size, geometry.block).map_err(into_error)?;
    if let Some(area) = &card.system_area {
        builder.system_area(area.clone());
    }

    for nested in nested_saves(bundle)? {
        // The entry comes back whole where the bundle kept one, which is what carries the
        // language byte and whatever a game left in the padding of a field it did not fill.
        // The fields beside it are the fallback for a bundle that carries no entry at all.
        let entry = nested.dirent.filter(|bytes| bytes.len() == ENTRY_LEN - TAG_LEN).unwrap_or_default();
        builder.add(Save {
            block: 0,
            blocks: Vec::new(),
            name: nested.path.unwrap_or_default(),
            comment: comment_of(&nested.inner),
            language: 0,
            date: written_at(&nested.extensions).unwrap_or(0),
            data: nested.bytes,
            entry,
        });
    }
    builder.build().map_err(into_error)
}

/// The extension a Backup RAM Cart's volume takes beside the internal memory's, in Mednafen and
/// Beetle Saturn alike.
pub const CART_EXTENSION: &str = "bcr";

/// The extension the SMPC's twelve bytes take beside the internal memory's.
pub const SMPC_EXTENSION: &str = "smpc";

/// What Mednafen and Beetle Saturn keep for one game: the files a Saturn [device](read_device) is
/// read from and written back as.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceFiles {
    /// The console's internal memory, a 32 KiB volume.
    pub internal: Vec<u8>,
    /// A Backup RAM Cart's volume, where one was in the slot.
    pub cart: Option<Vec<u8>>,
    /// The SMPC's twelve bytes, where the emulator kept them.
    pub smpc: Option<Vec<u8>>,
}

/// Whether these bytes are a volume the size of the console's internal memory, which is what a
/// device of an emulator's files is read around.
#[must_use]
pub fn is_internal(bytes: &[u8]) -> bool {
    BackupRam::parse(bytes).is_ok_and(|volume| volume.geometry().is_internal())
}

/// Reads an emulator's files for one game into the device they make.
///
/// The internal memory goes in `internal` and a cart in `ram-cart`, whatever role `options` names:
/// the files say which socket each came out of. An SMPC whose clock was set puts the clock on the
/// header. One that was never set holds no reading, and nothing of it is kept.
///
/// A clock is no component, so the internal memory with neither a cart nor a set clock is no
/// device: it comes back as the card it is, as [`read`] reads it.
pub fn read_device(files: &DeviceFiles, options: &CardOptions) -> Result<Bundle> {
    let with_role = |role: &str| CardOptions { role: Some(slug(role)), ..options.clone() };
    if !is_internal(&files.internal) {
        return Err(Error::NotConvertible(
            "the console's memory is a 32 KiB volume, and this one is a Backup RAM Cart's".into(),
        ));
    }
    let internal = read(&files.internal, &with_role(INTERNAL_ROLE))?;

    let clock =
        files.smpc.as_deref().map(Smpc::parse).transpose().map_err(into_error)?.filter(|smpc| smpc.valid);
    if files.cart.is_none() && clock.is_none() {
        return Ok(internal);
    }

    let component = |id: u64, role: &str, inner: &Bundle| -> Result<Part> {
        Ok(Part::new(id, inner.to_vec()?).kind(PartKind::Bundle).role(slug(role)).system(slug(SYSTEM)))
    };
    let mut parts = vec![component(0, INTERNAL_ROLE, &internal)?];
    if let Some(cart) = &files.cart {
        if is_internal(cart) {
            return Err(Error::NotConvertible(
                "the cart's file holds a 32 KiB volume, which is the console's own memory and no cart".into(),
            ));
        }
        parts.push(component(1, RAM_CART_ROLE, &read(cart, &with_role(RAM_CART_ROLE))?)?);
    }

    let mut header = Header::new(Shape::Device).system(slug(SYSTEM)).source(options.source.clone());
    if let Some(smpc) = clock {
        // Neither emulator records when it took the time, so there is no anchor to carry.
        let clock = RtcSmpc { time: smpc.clock, smem: Some(smpc.save_mem), taken_at: None };
        // A time that is no date gives no reading, and the chip's bytes still ride.
        if let Some(reading) = clock.reading() {
            header.extensions.insert(reading);
        }
        header.extensions.insert(clock);
    }
    Ok(Bundle::new(header, parts))
}

/// Whether a bundle is a Saturn device, which [`write_device`] writes back as an emulator's files.
#[must_use]
pub fn is_device(bundle: &Bundle) -> bool {
    *bundle.shape() == Shape::Device
        && bundle.header.system.as_ref().is_some_and(|system| system.as_str() == SYSTEM)
}

/// Writes a Saturn device back out as the files Mednafen and Beetle Saturn keep.
///
/// Each volume is written as [`write`](fn@write) writes it, packed and uncompressed, which both
/// emulators read. The SMPC is written where the header carries
/// [`x.1sav.rtc.smpc`](RtcSmpc), with its clock set. Memory the key holds as null is written as
/// zeros, which is what both emulators give a console they start from nothing. A device whose
/// header carries only an `x.1sav.rtc` reading gets no SMPC, and the emulator sets the clock from
/// the host's, as it does by default.
pub fn write_device(bundle: &Bundle) -> Result<DeviceFiles> {
    if !is_device(bundle) {
        return Err(Error::NotConvertible("this bundle is not a Saturn device".into()));
    }
    let mut files = DeviceFiles::default();
    let mut internal = None;
    for part in &bundle.parts {
        let volume = write(&Bundle::from_slice(&part.payload)?)?;
        match part.role.as_ref().map(one_saves::Slug::as_str) {
            Some(INTERNAL_ROLE) => internal = Some(volume),
            Some(RAM_CART_ROLE) => files.cart = Some(volume),
            other => {
                return Err(Error::NotConvertible(format!(
                    "this device holds a {} component, which an emulator keeps no file for",
                    other.unwrap_or("primary")
                )));
            }
        }
    }
    files.internal =
        internal.ok_or_else(|| Error::NotConvertible("this device holds no internal memory".into()))?;
    files.smpc = bundle.header.extensions.get::<RtcSmpc>().map(|clock| {
        Smpc { valid: true, clock: clock.time, save_mem: clock.smem.unwrap_or_default() }.to_bytes().to_vec()
    });
    Ok(files)
}

/// The comment to write back: the detail line of the save's own label.
///
/// A save that arrived without one gets a blank rather than a refusal — the bytes are the save,
/// and the console lists a blank comment perfectly happily.
fn comment_of(inner: &one_saves::Bundle) -> String {
    inner.header.extensions.get::<Label>().and_then(|label| label.detail.clone()).unwrap_or_default()
}

/// The write time out of a part's `x.1sav.dirent`, as the volume counts it.
///
/// The key holds an epoch and this format holds minutes since 1980, so a reading before that or
/// past what the field holds is dropped rather than wrapped: a date the volume cannot state is
/// worse written wrong than left at zero.
fn written_at(extensions: &one_saves::Extensions) -> Option<u32> {
    let seconds = extensions.get::<Dirent>()?.modified_at?.seconds;
    // Saturating, because the reading is any 64-bit second a bundle states: one billions of years
    // before 1980 is still before it, and lands on zero like every other such reading.
    u32::try_from(seconds.saturating_sub(saturn_backup::EPOCH_1980).max(0) / 60).ok()
}

fn into_error(source: saturn_backup::Error) -> Error {
    match source {
        saturn_backup::Error::WrongLength(found) => Error::WrongLength {
            format: FORMAT,
            expected: "a whole number of the blocks its own signature states",
            found,
        },
        saturn_backup::Error::NotBackupRam => Error::NotThisFormat {
            format: FORMAT,
            why: "this image does not open with `BackUpRam Format`".into(),
        },
        saturn_backup::Error::Corrupt(why) => Error::Corrupt { format: FORMAT, why },
        // Not a malformed volume: a whole one, inside something this build does not open. Saying
        // so beats the generic complaint, which blames the bundle for a file's container.
        saturn_backup::Error::Compressed => Error::NotThisFormat {
            format: FORMAT,
            why: "this file is a gzip member, which is how standalone Mednafen writes a Backup \
                  RAM Cart; decompress it first"
                .into(),
        },
        other => Error::NotConvertible(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Beetle Saturn's three files for one game, as RetroArch names them.
    fn beetle_set(game: &str) -> DeviceFiles {
        DeviceFiles {
            internal: fixture(game, "srm"),
            cart: Some(fixture(game, CART_EXTENSION)),
            smpc: Some(fixture(game, SMPC_EXTENSION)),
        }
    }

    #[test]
    fn an_emulator_s_files_for_a_game_are_one_device_with_the_console_s_clock() {
        let files = beetle_set("Panzer Dragoon Saga");
        let device = read_device(&files, &CardOptions::default()).expect("reads");
        device.validate().expect("a valid device");
        let roles: Vec<_> =
            device.parts.iter().map(|part| part.role.as_ref().map(one_saves::Slug::as_str)).collect();
        assert_eq!(roles, [Some(INTERNAL_ROLE), Some(RAM_CART_ROLE)]);

        // 2026-09-22 17:56:00, which is when RetroArch last wrote the files, in local time.
        let clock = device.header.extensions.get::<RtcSmpc>().expect("the clock");
        assert_eq!((clock.to_string(), clock.weekday()), ("2026-09-22 17:56:00".to_owned(), 2));
        assert_eq!(clock.smem, Some([0; 4]));
        assert_eq!(
            device.header.extensions.get::<one_saves::extension::Rtc>().map(|rtc| rtc.reading),
            Some(1_790_099_760)
        );

        assert_eq!(write_device(&device).expect("writes"), files, "the same three files");
    }

    #[test]
    fn the_console_s_settings_ride_beside_its_clock() {
        // Sonic 3D Blast's SMPC holds a byte of settings the BIOS wrote.
        let files = beetle_set("Sonic 3D Blast");
        let device = read_device(&files, &CardOptions::default()).expect("reads");
        assert_eq!(
            device.header.extensions.get::<RtcSmpc>().and_then(|clock| clock.smem),
            Some([0, 0, 4, 0])
        );
        assert_eq!(write_device(&device).expect("writes").smpc, files.smpc);

        // A clock kept somewhere with no memory beside it comes back with the memory a fresh
        // console has.
        let mut kept_elsewhere = device.clone();
        let clock = RtcSmpc { smem: None, ..*device.header.extensions.get::<RtcSmpc>().unwrap() };
        kept_elsewhere.header.extensions.insert(clock);
        let smpc = write_device(&kept_elsewhere).expect("writes").smpc.expect("an SMPC");
        assert_eq!(smpc[8..], [0; 4]);
        assert_eq!(smpc[..8], files.smpc.as_ref().unwrap()[..8]);
    }

    #[test]
    fn a_clock_never_set_is_no_reading_and_makes_no_device() {
        let mut files = beetle_set("Panzer Dragoon Saga");
        files.smpc.as_mut().unwrap()[0] = 0;
        let device = read_device(&files, &CardOptions::default()).expect("reads");
        assert_eq!(*device.shape(), Shape::Device, "the cart still makes one");
        assert!(device.header.extensions.is_empty(), "with no clock on it");
        assert_eq!(write_device(&device).expect("writes").smpc, None);

        files.cart = None;
        let alone = read_device(&files, &CardOptions::default()).expect("reads");
        assert_eq!(*alone.shape(), Shape::Card, "the internal memory alone is the card it is");
        assert_eq!(alone, read(&files.internal, &CardOptions::default()).unwrap());
    }

    #[test]
    fn a_cart_is_not_taken_for_the_console_s_memory_or_the_other_way_round() {
        let files = beetle_set("Panzer Dragoon Saga");
        let swapped = DeviceFiles {
            internal: files.cart.clone().unwrap(),
            cart: Some(files.internal.clone()),
            ..files.clone()
        };
        assert!(read_device(&swapped, &CardOptions::default()).is_err());
        let twice = DeviceFiles { cart: Some(files.internal.clone()), ..files };
        assert!(read_device(&twice, &CardOptions::default()).is_err());
    }

    /// Standalone Mednafen gzips the cart's volume, and a device read from its files writes the
    /// cart back plain, which Mednafen reads as well.
    #[cfg(feature = "saturn-gzip")]
    #[test]
    fn mednafen_s_gzipped_cart_is_read_and_written_back_plain() {
        let mednafen = |extension: &str| producer_fixture("NiGHTS into Dreams", "macos/mednafen", extension);
        let files = DeviceFiles {
            internal: mednafen("bkr"),
            cart: Some(mednafen(CART_EXTENSION)),
            smpc: Some(mednafen(SMPC_EXTENSION)),
        };
        assert_eq!(files.cart.as_ref().unwrap()[..2], saturn_backup::GZIP_MAGIC);
        let device = read_device(&files, &CardOptions::default()).expect("reads");
        device.validate().expect("a valid device");

        let written = write_device(&device).expect("writes");
        assert_eq!(written.internal, files.internal);
        assert_eq!(written.smpc, files.smpc);
        let cart = written.cart.expect("the cart");
        assert_eq!(cart.len(), 524_288, "inflated");
        assert_eq!(
            read(&cart, &CardOptions::default()).unwrap(),
            read(files.cart.as_ref().unwrap(), &CardOptions::default()).unwrap()
        );
    }

    #[test]
    fn something_that_is_not_a_volume_is_refused_in_this_format_s_words() {
        let error = read(&[1, 2, 3], &CardOptions::default()).expect_err("not a volume");
        assert!(matches!(error, Error::NotThisFormat { .. }), "{error:?}");
    }

    /// A real volume off the workspace's `data/saves`, for the tests that need a console to have
    /// written the bytes.
    fn fixture(game: &str, extension: &str) -> Vec<u8> {
        producer_fixture(game, "retroarch/beetle-saturn", extension)
    }

    /// The same, naming the producer's folder under `data/saves`.
    fn producer_fixture(game: &str, producer: &str, extension: &str) -> Vec<u8> {
        // One folder holds every game its producer saved, so the game picks out the file by the name
        // it was saved under.
        let stem = match game {
            "Panzer Dragoon Saga" => "Panzer Dragoon Saga (Europe) (Disc 1)",
            "NiGHTS into Dreams" => "Nights into Dreams... (USA, Brazil)",
            "Sonic 3D Blast" => "Sonic 3D Blast (USA)",
            "Saturn BIOS" => "mpr-17933",
            other => panic!("no Saturn fixture for {other}"),
        };
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/saves").join(producer);
        let entry = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
            .filter_map(std::result::Result::ok)
            .find(|e| {
                let path = e.path();
                path.extension().is_some_and(|x| x == extension)
                    && path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(stem))
            })
            .unwrap_or_else(|| panic!("a .{extension} fixture"));
        std::fs::read(entry.path()).expect("reads")
    }

    #[test]
    fn a_real_volume_round_trips_through_a_bundle_exactly() {
        // Carrying the entry as a `dirent` is what makes this exact. Decoding it into fields and
        // re-encoding from them used to cost one byte here — the 0xEE NiGHTS leaves after the
        // terminator of a six-character comment — and would cost the language byte on any save
        // that set one.
        let bytes = fixture("NiGHTS into Dreams", "srm");
        let bundle = read(&bytes, &CardOptions::default()).expect("reads");
        assert_eq!(write(&bundle).expect("writes"), bytes, "a real volume round-trips exactly");

        // And the byte that used to go missing is in the entry the parts carry.
        let alife = bundle.parts.iter().find(|p| p.path.as_deref() == Some("NIGHTS___02"));
        let entry = alife.expect("the A-life save").dirent.as_ref().expect("its entry");
        assert_eq!(entry.len(), 30, "the length the registry fixes for this format");
        assert_eq!(&entry[0x10 - 4..][..10], b"A-life\0\xee\0\0");
    }

    #[test]
    fn the_medium_a_dump_came_off_settles_its_role() {
        // Nothing in a Saturn volume names its medium, but the geometry gives it away: the
        // console's memory is 32 KiB in 64-byte blocks and a cart is larger in larger ones. The
        // two sockets are separate storage a console carries at once, so filing a cart under
        // `internal` is a claim about where the saves live, not a cosmetic slip.
        let internal = read(&fixture("Panzer Dragoon Saga", "srm"), &CardOptions::default());
        let internal = internal.expect("reads");
        for part in &internal.parts {
            assert_eq!(part.role.as_ref().map(one_saves::Slug::as_str), Some("internal"));
        }

        let cart = read(&fixture("Panzer Dragoon Saga", "bcr"), &CardOptions::default());
        let cart = cart.expect("reads");
        for part in &cart.parts {
            assert_eq!(part.role.as_ref().map(one_saves::Slug::as_str), Some("ram-cart"));
        }

        // And a caller who knows better still wins: a cart dumped through an adapter that
        // presents it as internal storage is the caller's to state, not ours to overrule.
        let stated = CardOptions {
            role: Some(one_saves::Slug::parse("internal").expect("a listed role")),
            ..CardOptions::default()
        };
        let forced = read(&fixture("Panzer Dragoon Saga", "bcr"), &stated).expect("reads");
        assert_eq!(forced.parts[0].role.as_ref().map(one_saves::Slug::as_str), Some("internal"));
    }

    #[test]
    fn a_wide_dump_reads_the_same_and_comes_back_packed() {
        // Yabause's container carries no information the packed form does not, so a bundle made
        // from one holds the same saves as a bundle made from the other. What it does not hold is
        // which container it came from, so `extract` writes the packed form — a stated decision
        // rather than an accident, and the same one a Neo Geo card read out of a MiSTer save gets.
        let wide = producer_fixture("NiGHTS into Dreams", "retroarch/yabause", "srm");
        let bundle = read(&wide, &CardOptions::default()).expect("reads");
        assert_eq!(bundle.parts.len(), 2);
        assert_eq!(bundle.parts[0].path.as_deref(), Some("NIGHTS___01"));

        let out = write(&bundle).expect("writes");
        assert_eq!(out.len(), 32_768, "packed, where the dump was 65536");
        assert_eq!(saturn_backup::widen(&out), wide, "and widening it gives the dump back");
    }

    #[test]
    fn a_volume_with_nothing_on_it_is_carried_whole() {
        // Sonic 3D Blast keeps no saves, so both its volumes are formatted and empty. A card with
        // nothing on it is still a card: it becomes one `card-image` part, and writing it back is
        // a byte copy rather than a rebuild.
        for (extension, size) in [("srm", 32_768), ("bcr", 524_288)] {
            let image = fixture("Sonic 3D Blast", extension);
            let bundle = read(&image, &CardOptions::default()).expect("reads");
            assert_eq!(bundle.parts.len(), 1, ".{extension}");
            assert_eq!(bundle.parts[0].kind, one_saves::PartKind::CardImage);
            assert_eq!(bundle.parts[0].payload.as_slice().len(), size);
            assert_eq!(write(&bundle).expect("writes"), image, ".{extension} is not a byte copy");
        }
    }

    #[test]
    fn a_bundle_that_kept_no_reserved_region_still_writes() {
        // The block size is the volume's rather than the format's, so a writer normally reads it
        // off the `system_area` the reader kept. A bundle from a producer that kept none falls
        // back to the capacity, which answers exactly or not at all.
        let image = fixture("Panzer Dragoon Saga", "bcr");
        let mut bundle = read(&image, &CardOptions::default()).expect("reads");
        bundle.header.card.as_mut().expect("a card").system_area = None;

        let out = write(&bundle).expect("writes");
        assert_eq!(out.len(), 524_288, "a cart, worked out from 523264 bytes of capacity");
        let back = saturn_backup::BackupRam::parse(&out).expect("reads");
        assert_eq!(back.geometry().block, 512);
        assert_eq!(back.saves().len(), 2);
        // The volume is rebuilt rather than reproduced: the reserved region is a fresh one, so
        // the bytes differ from the dump even though every save in it is the same.
        for (before, after) in
            saturn_backup::BackupRam::parse(&image).expect("reads").saves().iter().zip(back.saves())
        {
            assert_eq!(before.data, after.data, "{}", before.name);
        }
    }

    #[test]
    fn a_bundle_that_is_not_a_card_cannot_be_written_as_one() {
        // `write` needs the card map for the capacity, and there is nothing to guess from.
        let image = fixture("Panzer Dragoon Saga", "srm");
        let mut bundle = read(&image, &CardOptions::default()).expect("reads");
        bundle.header.card = None;
        let error = write(&bundle).expect_err("nothing says how big the volume is");
        assert!(matches!(error, Error::NotConvertible(_)), "{error:?}");

        // And a capacity no volume has is refused rather than rounded to one that exists.
        let mut odd = read(&image, &CardOptions::default()).expect("reads");
        let card = odd.header.card.as_mut().expect("a card");
        card.system_area = None;
        card.capacity = 1234;
        assert!(matches!(write(&odd), Err(Error::NotConvertible(_))));
    }

    /// The console's own 32 KiB, holding one save.
    fn internal_volume() -> Vec<u8> {
        let mut builder = BackupBuilder::new(32_768, 64).expect("a real geometry");
        builder.add(Save {
            block: 0,
            blocks: Vec::new(),
            name: "PANDRA_3_01".to_owned(),
            comment: "AZEL#1Lv01".to_owned(),
            language: 0,
            date: 24_576_100,
            data: vec![9u8; 1276],
            entry: Vec::new(),
        });
        builder.build().expect("writes")
    }

    #[test]
    fn a_capacity_no_volume_has_is_refused_before_anything_is_allocated_for_it() {
        // The builder lays the whole volume out before it writes a save, so these were an eight
        // megabyte volume — a gigabyte, for a bundle that asked — or an overflow working out how
        // long the volume is. Read off the reserved region's 64-byte blocks first.
        let mut bundle = read(&internal_volume(), &CardOptions::default()).expect("reads");
        for capacity in [8 * 1024 * 1024 - 128, u64::MAX] {
            bundle.header.card.as_mut().expect("a card").capacity = capacity;
            let error = write(&bundle).expect_err("no such volume");
            assert!(matches!(error, Error::NotConvertible(_)), "{capacity}: {error:?}");
        }

        // Then from the capacity alone, which is a search through block sizes. The second is the
        // largest multiple of every block size there is.
        bundle.header.card.as_mut().expect("a card").system_area = None;
        for capacity in [8 * 1024 * 1024 - 1024, u64::MAX - 2047] {
            bundle.header.card.as_mut().expect("a card").capacity = capacity;
            let error = write(&bundle).expect_err("no such volume");
            assert!(matches!(error, Error::NotConvertible(_)), "{capacity}: {error:?}");
        }
    }

    #[test]
    fn a_reserved_region_stating_blocks_too_small_for_an_entry_is_refused() {
        // A signature twice over states 32-byte blocks, which a builder used to lay a volume out
        // in — one whose every save then panicked the reader.
        let mut bundle = read(&internal_volume(), &CardOptions::default()).expect("reads");
        let card = bundle.header.card.as_mut().expect("a card");
        let mut area = saturn_backup::MAGIC.repeat(2);
        area.resize(2 * 32, 0);
        card.system_area = Some(area);
        card.capacity = 32_768 - 64;
        let error = write(&bundle).expect_err("no volume is laid out in 32-byte blocks");
        assert!(matches!(error, Error::NotConvertible(_)), "{error:?}");
    }

    #[test]
    fn a_date_outside_what_the_volume_counts_is_written_as_zero() {
        let at = |seconds: i64| {
            let mut extensions = one_saves::Extensions::new();
            extensions.insert(modified_only(seconds, None));
            written_at(&extensions)
        };
        assert_eq!(at(i64::MIN), Some(0), "before 1980 however long before, which overflowed");
        assert_eq!(at(saturn_backup::EPOCH_1980 - 60), Some(0));
        assert_eq!(at(saturn_backup::EPOCH_1980 + 120), Some(2));
        assert_eq!(at(i64::MAX), None, "past what 32 bits of minutes hold");

        // And a bundle stating the earliest second there is still writes, at zero.
        let mut bundle = read(&internal_volume(), &CardOptions::default()).expect("reads");
        bundle.parts[0].extensions.insert(modified_only(i64::MIN, None));
        bundle.parts[0].dirent = None;
        let volume = BackupRam::parse(&write(&bundle).expect("writes")).expect("reads");
        assert_eq!(volume.saves()[0].date, 0);
    }

    #[test]
    fn a_volume_round_trips_through_a_bundle() {
        let mut builder = BackupBuilder::new(32_768, 64).expect("a real geometry");
        builder.add(Save {
            block: 0,
            blocks: Vec::new(),
            name: "PANDRA_3_01".to_owned(),
            comment: "AZEL#1Lv01".to_owned(),
            language: 0,
            date: 24_576_100,
            data: vec![9u8; 1276],
            entry: Vec::new(),
        });
        let image = builder.build().expect("writes");

        let bundle = read(&image, &CardOptions::default()).expect("reads");
        assert_eq!(bundle.parts.len(), 1);
        assert_eq!(bundle.parts[0].path.as_deref(), Some("PANDRA_3_01"));
        assert_eq!(bundle.parts[0].dirent.as_ref().map(Vec::len), Some(30), "the entry rides whole");
        assert!(bundle.parts[0].extensions.get::<Dirent>().is_some(), "the date still rides");
        assert_eq!(write(&bundle).expect("writes"), image);
    }
}
