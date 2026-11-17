//! The Redump hash of one disc, in every container it comes in.
//!
//! The disc is made here, shaped like a PC Engine CD: music on track one, the
//! program on track two behind a pregap that opens as silence and turns into
//! data sectors, and music again on track three behind a pregap of its own.
//! Redump would publish the hash of `(Track 2).bin` for it, so that is the
//! value every container has to give.
//!
//! The CHDs under `tests/data` were made from it by chdman 0.289, so they are
//! laid out by the tool everyone uses rather than by this crate's idea of it:
//!
//! ```console
//! DISC_ID_FIXTURE=dir cargo test -p disc-id --all-features --test redump -- --ignored
//! cd dir
//! chdman createcd -i "Disc (split).cue" -o disc.chd
//! chdman createcd -i cooked.cue -o cooked.chd
//! chdman createdvd -i dvd.iso -o dvd.chd
//! ```

#![cfg(feature = "redump")]

use std::io::{self, Read};

use disc_id::cue::{self, DataTrack};
use disc_id::redump::{self, RedumpHash};
use disc_id::sector::{RAW, USER_DATA};

const SYNC: [u8; 12] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];

/// Frames of each stretch of the disc, in order.
const TRACK_1: usize = 10;
/// Track two's pregap: silence written as audio, then empty data sectors.
const PREGAP_SILENT: usize = 3;
const PREGAP_DATA: usize = 3;
const PROGRAM: usize = 20;
const PREGAP_3: usize = 2;
const TRACK_3: usize = 8;

/// What Redump's `(Track 2).bin` would hash to, from Python's `hashlib`.
const TRACK_2_SHA1: &str = "0f60410dc6b0fbcdd904ef90a514a7ee26f3d6a0";
const TRACK_2_MD5: &str = "370331e5e8f6f9cf30f92ab81451bae5";

fn bcd(n: usize) -> u8 {
    u8::try_from(n / 10 * 16 + n % 10).unwrap()
}

fn music(lba: usize) -> Vec<u8> {
    (0..RAW).map(|i| u8::try_from((lba * 31 + i * 7) % 251).unwrap()).collect()
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

/// The program's user data: a PC Engine boot header in its second sector,
/// where the system card looks, and a pattern everywhere else.
fn program(n: usize) -> Vec<u8> {
    if n == 1 {
        let mut boot = vec![0u8; 0x20];
        boot.extend_from_slice(b"PC Engine CD-ROM SYSTEM\0Copyright HUDSON SOFT / NEC Home Electronics,Ltd.");
        return boot;
    }
    (0..USER_DATA).map(|i| u8::try_from((n * 13 + i) % 256).unwrap()).collect()
}

/// The disc's three tracks as Redump cuts them: each with its pregap.
fn tracks() -> [Vec<u8>; 3] {
    let mut lba = 0;
    let mut next = |make: &dyn Fn(usize) -> Vec<u8>| {
        let sector = make(lba);
        lba += 1;
        sector
    };
    let one: Vec<u8> = (0..TRACK_1).flat_map(|_| next(&music)).collect();
    let mut two: Vec<u8> = (0..PREGAP_SILENT).flat_map(|_| next(&|_| vec![0; RAW])).collect();
    two.extend((0..PREGAP_DATA).flat_map(|_| next(&|lba| data(lba, &[]))));
    two.extend((0..PROGRAM).flat_map(|n| next(&move |lba| data(lba, &program(n)))));
    let mut three: Vec<u8> = (0..PREGAP_3).flat_map(|_| next(&|_| vec![0; RAW])).collect();
    three.extend((0..TRACK_3).flat_map(|_| next(&music)));
    [one, two, three]
}

fn msf(frames: usize) -> String {
    format!("{:02}:{:02}:{:02}", frames / 4500, frames / 75 % 60, frames % 75)
}

/// Redump's shape: a file per track, each opening with its pregap.
fn split_sheet() -> String {
    format!(
        "FILE \"Disc (Track 1).bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n\
         FILE \"Disc (Track 2).bin\" BINARY\n  TRACK 02 MODE1/2352\n    INDEX 00 00:00:00\n    INDEX 01 {}\n\
         FILE \"Disc (Track 3).bin\" BINARY\n  TRACK 03 AUDIO\n    INDEX 00 00:00:00\n    INDEX 01 {}\n",
        msf(PREGAP_SILENT + PREGAP_DATA),
        msf(PREGAP_3),
    )
}

/// Every track in one image, as a merge of Redump's files writes it.
fn single_sheet() -> String {
    let two = TRACK_1;
    let three = two + PREGAP_SILENT + PREGAP_DATA + PROGRAM;
    format!(
        "FILE \"Disc.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n\
         \x20 TRACK 02 MODE1/2352\n    INDEX 00 {}\n    INDEX 01 {}\n\
         \x20 TRACK 03 AUDIO\n    INDEX 00 {}\n    INDEX 01 {}\n",
        msf(two),
        msf(two + PREGAP_SILENT + PREGAP_DATA),
        msf(three),
        msf(three + PREGAP_3),
    )
}

/// The same image as CloneCD names it, with the unpadded numbers its sheets
/// carry.
fn clonecd_sheet() -> String {
    let padded = single_sheet().replace("Disc.bin", "Disc.img");
    padded
        .lines()
        .map(|line| {
            let words: Vec<&str> = line.split_whitespace().collect();
            match words.as_slice() {
                ["TRACK", n, mode] => format!("   TRACK {} {mode}", n.trim_start_matches('0')),
                ["INDEX", n, at] => format!("   INDEX {} {at}", n.parse::<u32>().unwrap()),
                _ => line.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
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

/// The data track a sheet names, hashed out of `file`.
fn from_sheet(sheet: &str, file: &[u8]) -> Option<RedumpHash> {
    let track = cue::first_data_track(sheet).expect("a data track");
    redump::track(&track, Member(file)).unwrap()
}

#[cfg(feature = "chd")]
fn fixture(name: &str) -> std::fs::File {
    std::fs::File::open(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(name))
        .unwrap()
}

fn track_2() -> RedumpHash {
    let found = from_sheet(&split_sheet(), &tracks()[1]).expect("track two is raw data");
    assert_eq!((found.sha1_hex(), found.md5_hex()), (TRACK_2_SHA1.into(), TRACK_2_MD5.into()));
    assert_eq!(found.size, ((PREGAP_SILENT + PREGAP_DATA + PROGRAM) * RAW) as u64, "pregap and all");
    found
}

/// The value is the track's file, whatever holds it: a file per track, or the
/// CHD chdman made of them.
#[cfg(feature = "chd")]
#[test]
fn a_cue_with_its_bins_and_a_chd_of_the_disc_agree() {
    let from_chd = disc_id::chd::redump(io::BufReader::new(fixture("disc.chd"))).unwrap();
    assert_eq!(from_chd, Some(track_2()));
}

/// One image of every track, cut by its sheet. Hashed whole it would fold the
/// music in, and without its sheet nothing says where the program ends.
#[test]
fn a_single_bin_with_audio_tracks_hashes_only_the_data_track() {
    let image = tracks().concat();
    assert_eq!(from_sheet(&single_sheet(), &image), Some(track_2()));
    assert_eq!(from_sheet(&clonecd_sheet(), &image), Some(track_2()), "CloneCD's sheet over its .img");

    let whole = redump::image(Member(&image)).unwrap();
    assert_eq!(whole, None, "an image with music in it, and no sheet to say where");
}

/// The program is on track two, so the disc identifies by what is there and
/// its fingerprint is track two's, pregap included: starting at the program
/// would drop six sectors of it, and starting at the disc would hash music.
#[test]
fn a_pc_engine_cd_gives_its_second_tracks_value() {
    let sheet = single_sheet();
    let image = tracks().concat();
    let track = cue::first_data_track(&sheet).unwrap();
    let at = usize::try_from(track.offset).unwrap();
    let id = disc_id::identify_bytes(&image[at..]).expect("identified");
    assert_eq!(id.system.slug, "pcenginecd");

    assert_eq!(from_sheet(&sheet, &image), Some(track_2()));
    let from_the_program = DataTrack { pregap: 0, ..track.clone() };
    assert_ne!(redump::track(&from_the_program, Member(&image)).unwrap(), Some(track_2()));
}

/// The CHD layout this rests on: chdman stores the pregap in the track and
/// counts it in `FRAMES`. Identification still lands on the program.
#[cfg(feature = "chd")]
#[test]
fn the_chd_identifies_from_the_program_past_its_pregap() {
    let id = disc_id::chd::identify(io::BufReader::new(fixture("disc.chd"))).unwrap().expect("identified");
    assert_eq!(id.system.slug, "pcenginecd");
}

/// A track stored cooked has nothing left of what Redump hashed, in a sheet
/// or in a CHD.
#[cfg(feature = "chd")]
#[test]
fn a_cooked_track_has_no_fingerprint() {
    assert_eq!(disc_id::chd::redump(io::BufReader::new(fixture("cooked.chd"))).unwrap(), None);
    let sheet = "FILE \"cooked.iso\" BINARY\n  TRACK 01 MODE1/2048\n    INDEX 01 00:00:00\n";
    assert_eq!(from_sheet(sheet, &cooked()), None);
    let sheet = "FILE \"cooked.bin\" BINARY\n  TRACK 01 MODE2/2336\n    INDEX 01 00:00:00\n";
    assert_eq!(from_sheet(sheet, &vec![0u8; 2336 * 4]), None);
}

/// Audio has no fingerprint, alone or ahead of nothing.
#[test]
fn a_disc_of_music_has_no_data_track_to_hash() {
    let sheet = "FILE \"music.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n";
    assert_eq!(cue::first_data_track(sheet), None);
    assert_eq!(redump::image(Member(&tracks()[0])).unwrap(), None);
}

/// A DVD is hashed whole, as an image or as a CHD, and both are the image's
/// own bytes: the CHD's last hunk is padded past them.
#[cfg(feature = "chd")]
#[test]
fn a_dvd_as_an_image_and_as_a_chd_agree() {
    let iso = dvd();
    let from_image = redump::image(Member(&iso)).unwrap().expect("a DVD");
    assert_eq!(from_image.size, iso.len() as u64);
    let from_chd = disc_id::chd::redump(io::BufReader::new(fixture("dvd.chd"))).unwrap();
    assert_eq!(from_chd, Some(from_image));
}

/// Thirty-two cooked sectors of the program. chdman 0.289 writes a cooked track
/// of twenty or twenty-four sectors that it then cannot open itself.
fn cooked() -> Vec<u8> {
    (0..32)
        .flat_map(|n| {
            let mut sector = program(n);
            sector.resize(USER_DATA, 0);
            sector
        })
        .collect()
}

/// A PlayStation 2 DVD's opening, in an odd number of sectors so a CHD's
/// hunks of two do not end where it does.
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

/// Writes what the CHDs under `tests/data` were made from, into the folder
/// `DISC_ID_FIXTURE` names. See the top of this file.
#[test]
#[ignore = "writes the inputs the CHD fixtures were made from"]
fn write_the_fixture_inputs() {
    let dir = std::path::PathBuf::from(
        std::env::var_os("DISC_ID_FIXTURE").expect("DISC_ID_FIXTURE names a folder"),
    );
    let [one, two, three] = tracks();
    for (name, bytes) in
        [("Disc (Track 1).bin", one), ("Disc (Track 2).bin", two), ("Disc (Track 3).bin", three)]
    {
        std::fs::write(dir.join(name), bytes).unwrap();
    }
    std::fs::write(dir.join("Disc (split).cue"), split_sheet()).unwrap();
    std::fs::write(dir.join("cooked.iso"), cooked()).unwrap();
    std::fs::write(
        dir.join("cooked.cue"),
        "FILE \"cooked.iso\" BINARY\n  TRACK 01 MODE1/2048\n    INDEX 01 00:00:00\n",
    )
    .unwrap();
    std::fs::write(dir.join("dvd.iso"), dvd()).unwrap();
}
