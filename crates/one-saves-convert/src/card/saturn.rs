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
//! [`x.1sav.dirent`]: https://docs.1retro.com/specifications/extensions/x.1sav.dirent/
//! [`x.1sav.label`]: https://docs.1retro.com/specifications/extensions/x.1sav.label/

use saturn_backup::{BackupBuilder, BackupRam, ENTRY_LEN, Geometry, Save, TAG_LEN};

use crate::CardOptions;
use crate::card::{
    card_header, card_image_part, dirent_key, image_only, modified_only, nested_saves, save_part_with, slug,
};
use crate::detect::Format;
use crate::error::{Error, Result};
use crate::label::{label_key, value as label};

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
            inner.insert(label_key(), value);
        }

        let mut part = save_part_with(Format::SaturnBup, index, save.data.clone(), None, options, inner)?;
        // `save_part_with` takes the caller's role or the format's default; neither knows which
        // medium this is, so the one worked out above replaces it.
        part.role = Some(role.clone());
        part.path = (!save.name.is_empty()).then(|| save.name.clone());
        part.slot = Some(u64::try_from(index).expect("slot fits"));
        // The entry's own thirty bytes, verbatim, like every other format's. Where they sit is
        // what makes this format unusual — the head of the save's first block rather than a
        // directory region — and nothing in them depends on that: name, language, comment, date
        // and length are the save's wherever it lands. Only the block list after them moves, and
        // a writer rebuilds that for every format here.
        part.dirent = Some(save.entry.clone());
        // The date again, decoded, for a consumer that will not parse thirty opaque bytes. The
        // volume records a write and no creation — the shape `x.1sav.dirent` admits for exactly
        // this. No zone: the console keeps a wall clock and does not know one.
        part.extensions.insert(dirent_key(), modified_only(save.written_at(), None));
        parts.push(part);
    }

    if parts.is_empty() {
        parts.push(card_image_part(0, volume.image(), &role));
    }

    Ok(one_saves::Bundle {
        header: card_header(
            Format::SaturnBup,
            volume.capacity(),
            Some(volume.system_area().to_vec()),
            options,
        ),
        parts,
    })
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
    let geometry = match card.system_area.as_deref().and_then(saturn_backup::block_size) {
        Some(block) => Geometry::new(capacity + Geometry::FIRST_DATA_BLOCK * block, block),
        None => Geometry::for_data_capacity(capacity),
    }
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

/// The comment to write back: the detail line of the save's own label.
///
/// A save that arrived without one gets a blank rather than a refusal — the bytes are the save,
/// and the console lists a blank comment perfectly happily.
fn comment_of(inner: &one_saves::Bundle) -> String {
    inner
        .header
        .extensions
        .get(&label_key())
        .and_then(|value| value.as_map())
        .and_then(|map| map.get::<u64, String>(1u64))
        .unwrap_or_default()
}

/// The write time out of a part's `x.1sav.dirent`, as the volume counts it.
///
/// The key holds an epoch and this format holds minutes since 1980, so a reading before that or
/// past what the field holds is dropped rather than wrapped: a date the volume cannot state is
/// worse written wrong than left at zero.
fn written_at(extensions: &one_saves::Extensions) -> Option<u32> {
    let map = extensions.get(&dirent_key())?.as_map()?;
    let arm = map.get::<u64, one_saves::dcbor::CBOR>(1u64)?;
    let reading = arm.as_array()?.first()?.clone();
    let one_saves::dcbor::CBORCase::Tagged(_, seconds) = reading.as_case() else {
        return None;
    };
    let seconds = i64::try_from(seconds.clone()).ok()?;
    u32::try_from((seconds - saturn_backup::EPOCH_1980).max(0) / 60).ok()
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

    #[test]
    fn something_that_is_not_a_volume_is_refused_in_this_format_s_words() {
        let error = read(&[1, 2, 3], &CardOptions::default()).expect_err("not a volume");
        assert!(matches!(error, Error::NotThisFormat { .. }), "{error:?}");
    }

    /// A real volume off the workspace's `data/saves`, for the tests that need a console to have
    /// written the bytes.
    fn fixture(game: &str, extension: &str) -> Vec<u8> {
        producer_fixture(game, "Beetle Saturn", extension)
    }

    /// The same, naming the producer.
    fn producer_fixture(game: &str, producer: &str, extension: &str) -> Vec<u8> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../data/saves/Saturn")
            .join(game)
            .join(producer);
        let entry = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
            .filter_map(std::result::Result::ok)
            .find(|e| e.path().extension().is_some_and(|x| x == extension))
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
        let wide = producer_fixture("NiGHTS into Dreams", "Yabause", "srm");
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
            assert_eq!(bundle.parts[0].bytes().expect("bytes").len(), size);
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
        assert!(bundle.parts[0].extensions.contains_key(&dirent_key()), "the date still rides");
        assert_eq!(write(&bundle).expect("writes"), image);
    }
}
