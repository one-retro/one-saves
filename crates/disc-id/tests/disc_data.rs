//! The `disc-data` fingerprint of one disc, in every container it comes in, and
//! of the discs that differ from it.
//!
//! The disc is made here, shaped like the PC Engine CDs that need every data
//! track read: music on track one, a program on track two, music, a second
//! program on track four, and music to close. Each program sits behind a pregap
//! that opens as silence and turns into empty data sectors, and is followed by
//! empty data sectors of its own, as a postgap is. Its fingerprint is that of
//! the two programs and nothing else, so that is the value every container has
//! to give.
//!
//! The CHDs under `tests/data` were made from it, and from a GD-ROM laid out
//! here too, by chdman 0.289:
//!
//! ```console
//! DISC_ID_FIXTURE=dir cargo test -p disc-id --all-features --test disc_data -- --ignored
//! cd dir
//! chdman createcd -i "Two (split).cue" -o two.chd
//! chdman createcd -i "Two (cooked).cue" -o two-cooked.chd
//! chdman createcd -i gd.gdi -o gd.chd
//! ```
//!
//! `cooked.chd` and `dvd.chd` are the Redump tests' (see `tests/redump.rs`).

#![cfg(feature = "disc-data")]

use std::collections::HashMap;
use std::io::{self, Read};

use disc_id::disc_data::{self, DiscData, TrackData};
use disc_id::sector::{RAW, USER_DATA};

const SYNC: [u8; 12] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];

/// Frames of each stretch of the disc, in order.
const TRACK_1: usize = 10;
/// Track two's pregap: silence written as audio, then empty data sectors.
const PREGAP_2: [usize; 2] = [3, 3];
const PROGRAM_A: usize = 32;
/// Empty data sectors closing track two, as its postgap.
const POSTGAP_2: usize = 4;
const PREGAP_3: usize = 2;
const TRACK_3: usize = 8;
const PREGAP_4: [usize; 2] = [2, 1];
const PROGRAM_B: usize = 36;
const POSTGAP_4: usize = 2;
const PREGAP_5: usize = 2;
const TRACK_5: usize = 6;

/// Where program B's sectors start among the program pattern's, so its
/// sectors are not program A's.
const B: usize = 64;

fn bcd(n: usize) -> u8 {
    u8::try_from(n / 10 * 16 + n % 10).unwrap()
}

/// A raw Mode 1 sector at `lba`. The last 288 bytes stand where the error
/// correction would; nothing here reads them, and chdman stores them as they are.
fn data(lba: usize, user: &[u8]) -> Vec<u8> {
    let mut sector = SYNC.to_vec();
    let msf = lba + 150;
    sector.extend([bcd(msf / 4500), bcd(msf / 75 % 60), bcd(msf % 75), 1]);
    let mut payload = user.to_vec();
    payload.resize(USER_DATA, 0);
    sector.extend(payload);
    sector.extend((0..288).map(|i| u8::try_from((lba + i) % 256).unwrap()));
    sector
}

/// A sector of music, varied by `tune` so two discs can differ in it alone.
fn music(tune: usize, lba: usize) -> Vec<u8> {
    (0..RAW).map(|i| u8::try_from((lba * 31 + i * 7 + tune) % 251).unwrap()).collect()
}

/// A program's sector `n`, 2048 bytes: a PC Engine boot header in the second,
/// where the system card looks, and a pattern everywhere else. The same as
/// `tests/redump.rs`, whose `cooked.chd` holds sectors 0 to 31 of it.
fn program(n: usize) -> Vec<u8> {
    let mut sector = if n == 1 {
        let mut boot = vec![0u8; 0x20];
        boot.extend_from_slice(b"PC Engine CD-ROM SYSTEM\0Copyright HUDSON SOFT / NEC Home Electronics,Ltd.");
        boot
    } else {
        (0..USER_DATA).map(|i| u8::try_from((n * 13 + i) % 256).unwrap()).collect()
    };
    sector.resize(USER_DATA, 0);
    sector
}

/// What a disc may differ in.
#[derive(Clone, Copy, Default)]
struct Variant {
    /// Its music.
    tune: usize,
    /// One byte of program B changed, as a translation patches the copy near
    /// the end of a disc.
    patched: bool,
}

/// The user data of programs A and B, cooked.
fn programs(variant: Variant) -> [Vec<Vec<u8>>; 2] {
    let a: Vec<Vec<u8>> = (0..PROGRAM_A).map(program).collect();
    let mut b: Vec<Vec<u8>> = (0..PROGRAM_B).map(|n| program(n + B)).collect();
    if variant.patched {
        b[20][100] ^= 0x40;
    }
    [a, b]
}

/// The disc's five tracks as Redump cuts them: each from its pregap.
fn tracks(variant: Variant) -> [Vec<u8>; 5] {
    let [a, b] = programs(variant);
    let mut lba = 0;
    let mut next = |sector: &dyn Fn(usize) -> Vec<u8>| {
        let made = sector(lba);
        lba += 1;
        made
    };
    let silence = |_| vec![0u8; RAW];
    let empty = |lba| data(lba, &[]);
    let tune = move |lba| music(variant.tune, lba);

    let one: Vec<u8> = (0..TRACK_1).flat_map(|_| next(&tune)).collect();
    let mut two: Vec<u8> = (0..PREGAP_2[0]).flat_map(|_| next(&silence)).collect();
    two.extend((0..PREGAP_2[1]).flat_map(|_| next(&empty)));
    two.extend(a.iter().flat_map(|user| next(&|lba| data(lba, user))));
    two.extend((0..POSTGAP_2).flat_map(|_| next(&empty)));
    let mut three: Vec<u8> = (0..PREGAP_3).flat_map(|_| next(&silence)).collect();
    three.extend((0..TRACK_3).flat_map(|_| next(&tune)));
    let mut four: Vec<u8> = (0..PREGAP_4[0]).flat_map(|_| next(&silence)).collect();
    four.extend((0..PREGAP_4[1]).flat_map(|_| next(&empty)));
    four.extend(b.iter().flat_map(|user| next(&|lba| data(lba, user))));
    four.extend((0..POSTGAP_4).flat_map(|_| next(&empty)));
    let mut five: Vec<u8> = (0..PREGAP_5).flat_map(|_| next(&silence)).collect();
    five.extend((0..TRACK_5).flat_map(|_| next(&tune)));
    [one, two, three, four, five]
}

fn msf(frames: usize) -> String {
    format!("{:02}:{:02}:{:02}", frames / 4500, frames / 75 % 60, frames % 75)
}

const TRACK_2_PREGAP: usize = PREGAP_2[0] + PREGAP_2[1];
const TRACK_4_PREGAP: usize = PREGAP_4[0] + PREGAP_4[1];

/// Redump's shape: a file per track, each opening with its pregap.
fn split_sheet() -> String {
    format!(
        "FILE \"Two (Track 1).bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n\
         FILE \"Two (Track 2).bin\" BINARY\n  TRACK 02 MODE1/2352\n    INDEX 00 00:00:00\n    INDEX 01 {}\n\
         FILE \"Two (Track 3).bin\" BINARY\n  TRACK 03 AUDIO\n    INDEX 00 00:00:00\n    INDEX 01 {}\n\
         FILE \"Two (Track 4).bin\" BINARY\n  TRACK 04 MODE1/2352\n    INDEX 00 00:00:00\n    INDEX 01 {}\n\
         FILE \"Two (Track 5).bin\" BINARY\n  TRACK 05 AUDIO\n    INDEX 00 00:00:00\n    INDEX 01 {}\n",
        msf(TRACK_2_PREGAP),
        msf(PREGAP_3),
        msf(TRACK_4_PREGAP),
        msf(PREGAP_5),
    )
}

/// Where each track's pregap starts in one image of them all.
fn starts() -> [usize; 5] {
    let lengths = tracks(Variant::default()).map(|track| track.len() / RAW);
    let mut at = 0;
    lengths.map(|length| {
        let start = at;
        at += length;
        start
    })
}

/// Every track in one image, as a merge of Redump's files writes it, with
/// `INDEX 00` where each pregap starts.
fn single_sheet() -> String {
    let [_, two, three, four, five] = starts();
    format!(
        "FILE \"Two.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n\
         \x20 TRACK 02 MODE1/2352\n    INDEX 00 {}\n    INDEX 01 {}\n\
         \x20 TRACK 03 AUDIO\n    INDEX 00 {}\n    INDEX 01 {}\n\
         \x20 TRACK 04 MODE1/2352\n    INDEX 00 {}\n    INDEX 01 {}\n\
         \x20 TRACK 05 AUDIO\n    INDEX 00 {}\n    INDEX 01 {}\n",
        msf(two),
        msf(two + TRACK_2_PREGAP),
        msf(three),
        msf(three + PREGAP_3),
        msf(four),
        msf(four + TRACK_4_PREGAP),
        msf(five),
        msf(five + PREGAP_5),
    )
}

/// The same image, with a sheet that states no pregaps: each track runs on to
/// where the next one's music or program starts, so each program's track ends
/// in the next track's silence. Sheets in the wild do this, and it is the same
/// disc.
fn loose_sheet() -> String {
    single_sheet().lines().filter(|line| !line.contains("INDEX 00")).flat_map(|line| [line, "\n"]).collect()
}

/// The same image as CloneCD names it, with the unpadded numbers its sheets
/// carry.
fn clonecd_sheet() -> String {
    single_sheet()
        .replace("Two.bin", "Two.img")
        .lines()
        .map(|line| {
            let words: Vec<&str> = line.split_whitespace().collect();
            match words.as_slice() {
                ["TRACK", n, mode] => format!("   TRACK {} {mode}\n", n.trim_start_matches('0')),
                ["INDEX", n, at] => format!("   INDEX {} {at}\n", n.parse::<u32>().unwrap()),
                _ => format!("{line}\n"),
            }
        })
        .collect()
}

/// The programs cooked to `.iso` files of their user data, postgaps and all,
/// with the music raw beside them.
fn cooked_sheet() -> String {
    format!(
        "FILE \"Two (Track 1).bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n\
         FILE \"Two (Track 2).iso\" BINARY\n  TRACK 02 MODE1/2048\n    PREGAP {}\n    INDEX 01 00:00:00\n\
         FILE \"Two (Track 3).bin\" BINARY\n  TRACK 03 AUDIO\n    INDEX 00 00:00:00\n    INDEX 01 {}\n\
         FILE \"Two (Track 4).iso\" BINARY\n  TRACK 04 MODE1/2048\n    PREGAP {}\n    INDEX 01 00:00:00\n\
         FILE \"Two (Track 5).bin\" BINARY\n  TRACK 05 AUDIO\n    INDEX 00 00:00:00\n    INDEX 01 {}\n",
        msf(TRACK_2_PREGAP),
        msf(PREGAP_3),
        msf(TRACK_4_PREGAP),
        msf(PREGAP_5),
    )
}

/// A cooked program track: its program, then its postgap's empty sectors.
fn cooked_track(program: &[Vec<u8>], postgap: usize) -> Vec<u8> {
    let mut track = program.concat();
    track.resize(track.len() + postgap * USER_DATA, 0);
    track
}

/// Every file of the disc in every shape, by the name its sheet gives it.
fn files(variant: Variant) -> HashMap<&'static str, Vec<u8>> {
    let tracks = tracks(variant);
    let [a, b] = programs(variant);
    let image = tracks.concat();
    let mut files = HashMap::new();
    for (name, bytes) in [
        "Two (Track 1).bin",
        "Two (Track 2).bin",
        "Two (Track 3).bin",
        "Two (Track 4).bin",
        "Two (Track 5).bin",
    ]
    .into_iter()
    .zip(tracks)
    {
        files.insert(name, bytes);
    }
    files.insert("Two (Track 2).iso", cooked_track(&a, POSTGAP_2));
    files.insert("Two (Track 4).iso", cooked_track(&b, POSTGAP_4));
    files.insert("Two.img", image.clone());
    files.insert("Two.bin", image);
    files
}

/// Reads like an archive member: forward only, and a few bytes at a time.
struct Member<'a>(&'a [u8]);

impl Read for Member<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = buf.len().min(self.0.len()).min(1000);
        buf[..n].copy_from_slice(&self.0[..n]);
        self.0 = &self.0[n..];
        Ok(n)
    }
}

/// The fingerprint of `sheet` over `files`, each opened as a member is, and
/// the names it asked for.
fn from_sheet(sheet: &str, files: &HashMap<&str, Vec<u8>>) -> (Option<DiscData>, Vec<String>) {
    let mut asked = Vec::new();
    let found = disc_data::sheet(sheet, |name| {
        asked.push(name.to_string());
        files.get(name).map(|bytes| Member(bytes)).ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    })
    .unwrap();
    (found, asked)
}

/// One program's share of the fingerprint, taken from its cooked sectors
/// alone.
fn track_of(program: &[Vec<u8>]) -> TrackData {
    disc_data::image(Member(&program.concat())).unwrap().expect("a program").tracks[0]
}

/// The value the disc must give: its two programs, and nothing else.
fn expected(variant: Variant) -> DiscData {
    let [a, b] = programs(variant);
    let found = DiscData::from_tracks(vec![track_of(&a), track_of(&b)]).unwrap();
    assert_eq!(found.tracks.iter().map(|track| track.sectors).collect::<Vec<_>>(), [32, 36]);
    found
}

#[cfg(feature = "chd")]
fn fixture(name: &str) -> io::BufReader<std::fs::File> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(name);
    io::BufReader::new(std::fs::File::open(path).unwrap())
}

/// A file per track, one image of them all with its pregaps marked, the same
/// image with none marked, and CloneCD's sheet: one disc, one value, and only
/// the files that hold its programs are opened.
#[test]
fn every_sheet_of_one_disc_gives_its_programs_value() {
    let files = files(Variant::default());
    let expected = expected(Variant::default());

    let (found, asked) = from_sheet(&split_sheet(), &files);
    assert_eq!(found.as_ref(), Some(&expected), "a file per track");
    assert_eq!(asked, ["Two (Track 2).bin", "Two (Track 4).bin"], "the music is never opened");

    let (found, asked) = from_sheet(&single_sheet(), &files);
    assert_eq!(found.as_ref(), Some(&expected), "one image");
    assert_eq!(asked, ["Two.bin"], "and it is read through once");

    assert_eq!(from_sheet(&loose_sheet(), &files).0.as_ref(), Some(&expected), "no pregaps marked");
    assert_eq!(from_sheet(&clonecd_sheet(), &files).0.as_ref(), Some(&expected), "CloneCD");
}

/// An `.iso` of each program's user data says the same as its raw sectors.
#[test]
fn cooked_tracks_give_the_raw_ones_value() {
    let files = files(Variant::default());
    let (found, asked) = from_sheet(&cooked_sheet(), &files);
    assert_eq!(found, Some(expected(Variant::default())));
    assert_eq!(asked, ["Two (Track 2).iso", "Two (Track 4).iso"]);
}

/// The CHDs chdman made of the disc, from its raw tracks and from its cooked
/// ones, give the value its sheets do.
#[cfg(feature = "chd")]
#[test]
fn a_chd_raw_or_cooked_gives_the_sheets_value() {
    let expected = Some(expected(Variant::default()));
    assert_eq!(disc_id::chd::disc_data(fixture("two.chd")).unwrap(), expected, "raw");
    assert_eq!(disc_id::chd::disc_data(fixture("two-cooked.chd")).unwrap(), expected, "cooked");
}

/// Saves follow the program, not its music.
#[test]
fn the_music_is_not_the_program() {
    let other = Variant { tune: 99, ..Variant::default() };
    assert_ne!(tracks(other)[0], tracks(Variant::default())[0], "the music does differ");
    assert_eq!(from_sheet(&split_sheet(), &files(other)).0, Some(expected(Variant::default())));
}

/// One byte of the second program changed, as a translation patches the copy
/// near the end of a disc while its first program reads the same: another
/// disc, so another value. A fingerprint of the first data track alone would
/// not see it.
#[test]
fn a_patched_second_program_is_another_disc() {
    let patched = Variant { patched: true, ..Variant::default() };
    let found = from_sheet(&split_sheet(), &files(patched)).0.unwrap();
    let original = expected(Variant::default());
    assert_eq!(found.tracks[0], original.tracks[0], "the first program is untouched");
    assert_ne!(found.digest, original.digest);
    assert_eq!(found, expected(patched));
    assert_eq!(from_sheet(&single_sheet(), &files(patched)).0, Some(found), "in any container");
}

/// A disc of music has no program to name, and a sheet whose program's file
/// ends before the program starts is a dump with a track missing.
#[test]
fn no_program_no_fingerprint() {
    let files = files(Variant::default());
    let music = "FILE \"Two (Track 1).bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n";
    assert_eq!(from_sheet(music, &files), (None, Vec::new()));

    let mut short = files.clone();
    short.get_mut("Two.bin").unwrap().truncate(starts()[3] * RAW);
    assert_eq!(from_sheet(&single_sheet(), &short).0, None, "track four is not there");
}

/// The sheet's own words decide how wide a track's sectors are, and a width no
/// data track has is not guessed at.
#[test]
fn a_width_no_data_track_has_is_not_guessed_at() {
    let sheet = "FILE \"x.bin\" BINARY\n  TRACK 01 MODE1/2340\n    INDEX 01 00:00:00\n";
    let files = HashMap::from([("x.bin", vec![1u8; 2340 * 4])]);
    assert_eq!(from_sheet(sheet, &files).0, None);
}

/// A file that cannot be opened is the caller's error, not an absence.
#[test]
fn a_file_that_will_not_open_is_an_error() {
    let error =
        disc_data::sheet(&split_sheet(), |_| Err::<&[u8], _>(io::Error::from(io::ErrorKind::NotFound)));
    assert_eq!(error.unwrap_err().kind(), io::ErrorKind::NotFound);
}

/// The Redump tests' cooked CHD holds program A, cooked, and nothing else.
#[cfg(feature = "chd")]
#[test]
fn a_cooked_chd_of_one_track_gives_that_tracks_value() {
    let [a, _] = programs(Variant::default());
    let found = disc_id::chd::disc_data(fixture("cooked.chd")).unwrap();
    assert_eq!(found, DiscData::from_tracks(vec![track_of(&a)]));
}

/// A DVD is one track, its whole image, as an `.iso` and as a CHD whose last
/// hunk is padded past it.
#[cfg(feature = "chd")]
#[test]
fn a_dvd_as_an_image_and_as_a_chd_agree() {
    let iso = dvd();
    let from_image = disc_data::image(Member(&iso)).unwrap().expect("a DVD");
    assert_eq!(from_image.tracks[0].sectors, (iso.len() / USER_DATA) as u64);
    assert_eq!(disc_id::chd::disc_data(fixture("dvd.chd")).unwrap(), Some(from_image));
}

/// The opening of a PlayStation 2 DVD, as `tests/redump.rs` makes the image
/// `dvd.chd` holds.
#[cfg(feature = "chd")]
fn dvd() -> Vec<u8> {
    let mut iso: Vec<u8> = (0..41).flat_map(|n| program(n + 2)).collect();
    for (sector, kind, id) in
        [(16, 1, b"CD001"), (17, 255, b"CD001"), (18, 0, b"BEA01"), (19, 0, b"NSR02"), (20, 0, b"TEA01")]
    {
        let at = sector * USER_DATA;
        iso[at..at + USER_DATA].fill(0);
        iso[at] = kind;
        iso[at + 1..at + 6].copy_from_slice(id);
        iso[at + 6] = 1;
    }
    iso
}

/// A GD-ROM: data, music, the high-density area's data at sector 45000, music
/// and data, each track a file of raw sectors as a GDI lists them.
fn gd_tracks() -> [(usize, bool, Vec<u8>); 5] {
    let data_track = |lba: usize, sectors: usize, from: usize| -> Vec<u8> {
        (0..sectors).flat_map(|n| data(lba + n, &program(from + n))).collect()
    };
    let music_track =
        |lba: usize, sectors: usize| -> Vec<u8> { (0..sectors).flat_map(|n| music(7, lba + n)).collect() };
    [
        (0, true, data_track(0, 10, 200)),
        (160, false, music_track(160, 7)),
        (45_000, true, data_track(45_000, 12, 300)),
        (45_162, false, music_track(45_162, 5)),
        (45_317, true, data_track(45_317, 9, 400)),
    ]
}

/// chdman stores a GD-ROM's tracks as it stores a CD's, and its `PAD` is the
/// gap on the disc: read past the stated padding, tracks three and five would
/// be music or nothing.
#[cfg(feature = "chd")]
#[test]
fn a_gd_rom_chd_gives_every_data_tracks_value() {
    let tracks: Vec<TrackData> = gd_tracks()
        .into_iter()
        .filter(|(_, data, _)| *data)
        .map(|(_, _, bytes)| disc_data::image(Member(&bytes)).unwrap().unwrap().tracks[0])
        .collect();
    assert_eq!(tracks.len(), 3);
    assert_eq!(disc_id::chd::disc_data(fixture("gd.chd")).unwrap(), DiscData::from_tracks(tracks));
}

/// Writes what the CHDs under `tests/data` were made from, into the folder
/// `DISC_ID_FIXTURE` names. See the top of this file.
#[test]
#[ignore = "writes the inputs the CHD fixtures were made from"]
fn write_the_fixture_inputs() {
    use std::fmt::Write as _;

    let dir = std::path::PathBuf::from(
        std::env::var_os("DISC_ID_FIXTURE").expect("DISC_ID_FIXTURE names a folder"),
    );
    for (name, bytes) in files(Variant::default()) {
        std::fs::write(dir.join(name), bytes).unwrap();
    }
    std::fs::write(dir.join("Two (split).cue"), split_sheet()).unwrap();
    std::fs::write(dir.join("Two (cooked).cue"), cooked_sheet()).unwrap();

    let mut gdi = String::from("5\n");
    for (number, (lba, data, bytes)) in gd_tracks().into_iter().enumerate() {
        let name = format!("gd (Track {}).{}", number + 1, if data { "bin" } else { "raw" });
        writeln!(gdi, "{} {lba} {} 2352 \"{name}\" 0", number + 1, if data { 4 } else { 0 }).unwrap();
        std::fs::write(dir.join(name), bytes).unwrap();
    }
    std::fs::write(dir.join("gd.gdi"), gdi).unwrap();
}
