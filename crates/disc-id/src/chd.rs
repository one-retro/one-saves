//! Reading a disc image out of a CHD.
//!
//! CHD stores the image in compressed hunks, so unlike every other container
//! here the bytes cannot simply be looked at. That is why this is behind a
//! feature: it brings four decompressors — zlib, LZMA, zstd, and FLAC for audio
//! tracks — and without it the crate depends on nothing outside this workspace.
//!
//! Two things make a CHD not quite the image it holds. Hunks are compressed, so
//! reaching sector 16 means inflating the two or three that precede it. And a
//! **CD** CHD stores each sector as a 2352-byte frame followed by 96 bytes of
//! subcode, so the image only reappears once the subcode is dropped. A CHD of
//! anything else — a DVD, a hard disk — has no subcode and passes through.
//!
//! Bounded reading survives all of it: identification still stops after a
//! handful of hunks rather than inflating a disc.

use std::io::{self, Read, Seek, SeekFrom};
use std::ops::ControlFlow;

use chd::Chd;

use crate::DiscId;

/// Bytes of a CD frame, sector data and subcode together.
const CD_FRAME: usize = 2448;

/// Bytes of sector data in a CD frame; the remainder is subcode.
const CD_SECTOR: usize = 2352;

/// Tracks are stored padded to a multiple of this many frames.
const TRACK_PADDING: usize = 4;

/// Bytes of user data in a cooked sector, which is what a non-`_RAW` track
/// stores at the head of its frame.
const COOKED_SECTOR: usize = 2048;

/// The largest hunk `chdman` writes, and so the largest read here.
const MAX_HUNK: u64 = 1024 * 1024;

/// The most hunks a CHD may hold.
///
/// What this bounds is the map. The `chd` crate allocates twelve bytes a hunk
/// for a compressed one before it reads a bit of it, so a header of a hundred
/// bytes claiming four billion hunks is a 48 GiB allocation. A dual-layer DVD,
/// the largest disc any system here ships on, is a little over four million
/// hunks at one 2048-byte sector a hunk, the smallest a DVD's can be; this is
/// four times that, and 192 MiB of map. It also keeps every size computed from
/// the header a long way from overflowing a `u64`, which the crate does not
/// check either.
const MAX_HUNKS: u64 = 1 << 24;

/// The most metadata entries a CHD may chain together.
///
/// chdman writes one a track, and a CD holds at most 99, so this is far past
/// any real disc. What it stops is a chain that never ends: see
/// [`metadata_chain`].
const MAX_METADATA: usize = 1024;

/// The longest metadata entry read, of the ones that are: a track table line,
/// and a hard disk's geometry, which the `chd` crate reads when it opens an
/// older CHD. chdman writes about a hundred bytes for either.
const MAX_METADATA_ENTRY: u32 = 4096;

/// The tags of the entries that carry a track table. Three carry the same one
/// in the same format, and which a file has depends on what wrote it: `CHT2` is
/// current chdman, `CHTR` is what older versions wrote, and `CHGD` is a GD-ROM,
/// which is every Dreamcast dump.
const TRACK_TAGS: [&[u8; 4]; 3] = [b"CHT2", b"CHTR", b"CHGD"];

/// The tag of a hard disk's geometry.
const GEOMETRY_TAG: &[u8; 4] = b"GDDD";

/// The tag chdman's `createdvd` marks a DVD with, checked against chdman 0.289.
#[cfg(any(feature = "redump", feature = "disc-data"))]
const DVD_TAG: &[u8; 4] = b"DVD ";

/// The A/V Huffman codec, which compresses LaserDisc video: `avhu` in a version
/// 5 header, and 3 in an older one.
const AV_CODECS: [u32; 2] = [u32::from_be_bytes(*b"avhu"), 3];

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// What a CHD's header says about itself, as far as vetting it goes.
struct Declared {
    /// The header's version, 1 to 5.
    version: u32,
    /// Bytes in a hunk.
    hunk: u64,
    /// Bytes of image the hunks hold, or `None` where working it out from a
    /// version 1 or 2 header's disk geometry overflows.
    logical: Option<u64>,
    /// The codecs it names: one for versions 1 to 4, four for version 5.
    codecs: Vec<u32>,
    /// Where a version 5 map starts.
    map: Option<u64>,
    /// Where the metadata chain starts, or zero for none.
    metadata: u64,
}

impl Declared {
    /// Reads what the header states, or `None` for one too short or of a
    /// version this does not know, which the `chd` crate refuses on its own.
    fn read(header: &[u8]) -> Option<Self> {
        let u32_at = |at: usize| Some(u32::from_be_bytes(header.get(at..at + 4)?.try_into().ok()?));
        let u64_at = |at: usize| Some(u64::from_be_bytes(header.get(at..at + 8)?.try_into().ok()?));
        let version = u32_at(12)?;
        Some(match version {
            // Versions 1 and 2 describe a hard disk by its geometry, so the
            // image's size is the product of four of the header's numbers
            // rather than one of them, and a hunk is counted in sectors: of
            // 512 bytes, or of the width version 2 states.
            1 | 2 => {
                let sector = u64::from(if version == 1 { 512 } else { u32_at(76)? });
                let [cylinders, heads, sectors] = [u32_at(32)?, u32_at(36)?, u32_at(40)?].map(u64::from);
                Self {
                    version,
                    hunk: u64::from(u32_at(24)?) * sector,
                    logical: cylinders
                        .checked_mul(heads)
                        .and_then(|n| n.checked_mul(sectors))
                        .and_then(|n| n.checked_mul(sector)),
                    codecs: vec![u32_at(20)?],
                    map: None,
                    metadata: 0,
                }
            }
            3 | 4 => Self {
                version,
                hunk: u64::from(u32_at(if version == 3 { 76 } else { 44 })?),
                logical: Some(u64_at(28)?),
                codecs: vec![u32_at(20)?],
                map: None,
                metadata: u64_at(36)?,
            },
            5 => Self {
                version,
                hunk: u64::from(u32_at(56)?),
                logical: Some(u64_at(32)?),
                codecs: vec![u32_at(16)?, u32_at(20)?, u32_at(24)?, u32_at(28)?],
                map: Some(u64_at(40)?),
                metadata: u64_at(48)?,
            },
            _ => return None,
        })
    }
}

/// Refuse a CHD the `chd` crate would mishandle, before it sees it, and hand
/// back its metadata chain.
///
/// The crate trusts its header. It sizes a buffer per codec by the hunk, which
/// it checks on every header version but the fifth, the one chdman writes. It
/// adds and multiplies the sizes the header states without checking them, and
/// divides by a hunk size it never checked was there. It sizes the map by the
/// hunk count before reading any of it, and trusts the map's own header: see
/// [`vet_map`]. And it follows the metadata chain wherever it leads: see
/// [`metadata_chain`]. Each of those is a panic, a hang or an allocation of
/// gigabytes from a file of a few hundred bytes, so each is checked here first.
///
/// The A/V codec is refused outright. It compresses LaserDisc video, never a
/// CD or a DVD, and the crate reads its Huffman trees as unguardedly as it
/// reads the map's.
///
/// A header this cannot read is left for the crate to refuse.
fn vet<F: Read + Seek>(file: &mut F) -> io::Result<Vec<Metadata>> {
    let mut header = [0u8; 124];
    file.seek(SeekFrom::Start(0))?;
    let read = crate::read_upto(file, &mut header)?;
    let header = &header[..read];
    let declared = header.starts_with(b"MComprHD").then(|| Declared::read(header)).flatten();
    let Some(declared) = declared else {
        file.seek(SeekFrom::Start(0))?;
        return Ok(Vec::new());
    };

    let hunk = declared.hunk;
    if hunk > MAX_HUNK {
        return Err(invalid(format!("a CHD hunk of {hunk} bytes, where chdman writes at most {MAX_HUNK}")));
    }
    // An older header with no hunk size has the crate divide by it.
    if hunk == 0 {
        return Err(invalid("a CHD whose hunks hold nothing".into()));
    }
    if declared.codecs.iter().any(|codec| AV_CODECS.contains(codec)) {
        return Err(invalid("a CHD of LaserDisc video, compressed with the A/V codec".into()));
    }
    let hunks = match declared.logical {
        Some(logical) if logical.div_ceil(hunk) <= MAX_HUNKS => logical.div_ceil(hunk),
        Some(logical) => {
            return Err(invalid(format!(
                "a CHD of {logical} bytes, which is more than {MAX_HUNKS} hunks of {hunk}"
            )));
        }
        None => return Err(invalid("a CHD whose disk geometry multiplies past any disk".into())),
    };

    if let Some(map) = declared.map {
        vet_map(file, &declared, map, hunks)?;
    }

    let chain = metadata_chain(file, declared.metadata)?;
    for entry in &chain {
        if (TRACK_TAGS.contains(&&entry.tag) || &entry.tag == GEOMETRY_TAG)
            && entry.length > MAX_METADATA_ENTRY
        {
            return Err(invalid(format!(
                "a CHD metadata entry of {} bytes, where chdman writes about a hundred",
                entry.length
            )));
        }
    }
    // An older CHD states no unit, so the crate takes a hard disk's sector
    // size from its geometry and divides by it: the first entry, as it does.
    if matches!(declared.version, 3 | 4)
        && let Some(geometry) = chain.iter().find(|entry| &entry.tag == GEOMETRY_TAG)
        && let Ok(value) = geometry.read(file)
        && sector_size(&value) == Some(0)
    {
        return Err(invalid("a CHD of a hard disk whose sectors hold nothing".into()));
    }

    file.seek(SeekFrom::Start(0))?;
    Ok(chain)
}

/// Refuse a version 5 map the `chd` crate would mishandle.
///
/// An uncompressed map is four bytes a hunk, read whole, and has to be in the
/// file. A compressed one opens with sixteen bytes: how much compressed map
/// follows, which the crate allocates before reading, where the first hunk
/// sits, and how many bits each entry's fields take. Then comes a Huffman tree.
/// The crate expands every entry back into twelve bytes, a length in three of
/// them and an offset in six, through a byte writer that asserts rather than
/// fails when a value does not fit; so fields wide enough that a length or an
/// offset could outgrow its bytes are refused too. chdman sizes each field to
/// the largest value it holds, which for a hunk of at most a MiB and at most
/// [`MAX_HUNKS`] of them is far inside both.
fn vet_map<F: Read + Seek>(file: &mut F, declared: &Declared, map: u64, hunks: u64) -> io::Result<()> {
    let length = file.seek(SeekFrom::End(0))?;
    let past = || invalid("a CHD whose map runs past the end of the file".into());
    if declared.codecs.first() == Some(&0) {
        return if map.checked_add(hunks * 4).is_some_and(|end| end <= length) {
            Ok(())
        } else {
            Err(past())
        };
    }

    // The tree is sixteen codes of at most three nibbles each.
    let mut head = [0u8; 16 + 24];
    file.seek(SeekFrom::Start(map))?;
    let read = crate::read_upto(file, &mut head)?;
    if read < 16 {
        // Not even the map's own header, which the crate fails to read.
        return Ok(());
    }
    let compressed = u32::from_be_bytes([head[0], head[1], head[2], head[3]]);
    if map.checked_add(16 + u64::from(compressed)).is_none_or(|end| end > length) {
        return Err(past());
    }
    let first = u64::from_be_bytes([0, 0, head[4], head[5], head[6], head[7], head[8], head[9]]);
    let [length_bits, self_bits, parent_bits] = [head[12], head[13], head[14]];
    // A self or parent reference is read in its own bits and then stepped
    // once a hunk, by at most a hunk's worth of units, so 47 bits leaves room
    // for every step. A stored hunk's offset is the first one's plus the
    // lengths before it, none longer than its bits say or than a hunk.
    let longest = (1u64 << length_bits.min(32)).saturating_sub(1).max(declared.hunk);
    if length_bits > 24 || self_bits > 47 || parent_bits > 47 || first + hunks * longest >= 1 << 48 {
        return Err(invalid(
            "a CHD map whose entries hold more than the crate can write them back into".into(),
        ));
    }
    let tree = &head[16..read.min(16 + compressed as usize)];
    if map_tree_overflows(tree) {
        return Err(invalid("a CHD map whose Huffman tree does not fit its sixteen codes".into()));
    }
    Ok(())
}

/// Whether the `chd` crate indexes past an array reading a compressed map's
/// Huffman tree.
///
/// The tree is sixteen code lengths of four bits each, where a 1 escapes
/// either a literal 1 or a run: a length, then how many of it less three. The
/// crate writes a run into its sixteen-entry array without checking it fits.
/// It then builds a lookup table of 256 entries from the lengths without
/// checking that they describe a code at all, so lengths asking for more codes
/// than eight bits have room for write past the table's end. Either is an
/// index out of bounds rather than an error. A tree that ends early, or states
/// a length past eight bits, the crate refuses on its own.
fn map_tree_overflows(bits: &[u8]) -> bool {
    let mut nibbles = bits.iter().flat_map(|byte| [byte >> 4, byte & 0x0f]);
    let mut lengths: Vec<u8> = Vec::new();
    while lengths.len() < 16 {
        let Some(length) = nibbles.next() else { return false };
        if length != 1 {
            lengths.push(length);
            continue;
        }
        match nibbles.next() {
            None => return false,
            Some(1) => lengths.push(1),
            Some(length) => {
                let Some(run) = nibbles.next() else { return false };
                lengths.extend(std::iter::repeat_n(length, usize::from(run) + 3));
            }
        }
    }
    if lengths.len() > 16 {
        return true;
    }
    // A code of `n` bits takes 2^(8 - n) of the table's entries.
    lengths.iter().all(|&n| n <= 8)
        && lengths.iter().filter(|&&n| n > 0).map(|&n| 1u32 << (8 - n)).sum::<u32>() > 256
}

/// The sector size a hard disk's geometry states, as the `chd` crate reads it:
/// whatever follows `BPS:`, up to the terminating NUL.
fn sector_size(geometry: &[u8]) -> Option<u32> {
    let text = String::from_utf8_lossy(geometry);
    let (_, rest) = text.split_once("BPS:")?;
    rest.split('\0').next()?.parse().ok()
}

/// One entry in a CHD's metadata chain, as its own header states it.
struct Metadata {
    tag: [u8; 4],
    /// Where the entry's sixteen-byte header starts; its value follows it.
    offset: u64,
    /// How long its value is.
    length: u32,
}

impl Metadata {
    /// The entry's value. Only ever asked of an entry [`vet`] has checked the
    /// length of.
    fn read<F: Read + Seek>(&self, file: &mut F) -> io::Result<Vec<u8>> {
        let at = self.offset.checked_add(16).ok_or_else(|| invalid("CHD metadata past the end".into()))?;
        let mut value = vec![0u8; self.length as usize];
        file.seek(SeekFrom::Start(at))?;
        file.read_exact(&mut value)?;
        Ok(value)
    }
}

/// A CHD's metadata chain, walked here rather than by the `chd` crate.
///
/// Each entry names the next by its offset, and the crate follows them with
/// nothing to stop it, so a chain that leads back to an entry it has already
/// read keeps it busy, and growing, forever. It walks the chain to open a
/// version 3 or 4 CHD, and again when asked for the track table. Here an entry
/// visited twice, or more of them than [`MAX_METADATA`], is refused. Like the
/// crate, a header that cannot be read in full ends the chain rather than
/// failing it.
fn metadata_chain<F: Read + Seek>(file: &mut F, first: u64) -> io::Result<Vec<Metadata>> {
    let mut chain: Vec<Metadata> = Vec::new();
    let mut at = first;
    while at != 0 {
        if chain.iter().any(|entry| entry.offset == at) {
            return Err(invalid(format!("CHD metadata that leads back to itself, at offset {at}")));
        }
        if chain.len() == MAX_METADATA {
            return Err(invalid(format!(
                "more than {MAX_METADATA} CHD metadata entries, where chdman writes one a track"
            )));
        }
        let mut header = [0u8; 16];
        file.seek(SeekFrom::Start(at))?;
        if crate::read_upto(file, &mut header)? < header.len() {
            break;
        }
        let [t0, t1, t2, t3, l0, l1, l2, l3, next @ ..] = header;
        // The top byte of the length is the entry's flags.
        let length = u32::from_be_bytes([l0, l1, l2, l3]) & 0x00ff_ffff;
        chain.push(Metadata { tag: [t0, t1, t2, t3], offset: at, length });
        at = u64::from_be_bytes(next);
    }
    Ok(chain)
}

/// Where a CHD stores one track, as its track table states it.
struct Stored {
    /// Frames stored ahead of it: every earlier track, and its padding.
    before: usize,
    /// Frames it stores. chdman's `FRAMES` counts a pregap the file holds, so
    /// this is the track as Redump cuts it.
    #[cfg_attr(not(any(feature = "redump", feature = "disc-data")), allow(dead_code))]
    frames: usize,
    /// Of those, how many are that pregap, ahead of the track's content.
    pregap: usize,
    /// Its `TYPE`: `AUDIO`, or how a data track keeps its sectors.
    kind: String,
}

impl Stored {
    /// Whether it is a data track: `MODE1` or `MODE2`, however stored.
    fn data(&self) -> bool {
        self.kind.starts_with("MODE")
    }

    /// Whether it keeps whole 2352-byte sectors, sync header and all, where a
    /// cooked one stores only user data at the head of each frame.
    fn raw(&self) -> bool {
        self.kind.ends_with("_RAW")
    }

    /// How a data track keeps its sectors, as chdman names them, or `None` for
    /// audio and for a type this does not know.
    #[cfg(feature = "disc-data")]
    fn layout(&self) -> Option<crate::disc_data::Layout> {
        use crate::disc_data::Layout;
        match self.kind.as_str() {
            "MODE1_RAW" | "MODE2_RAW" => Some(Layout::Raw),
            "MODE1" | "MODE2_FORM1" => Some(Layout::Cooked),
            "MODE2" | "MODE2_FORM_MIX" => Some(Layout::Mode2),
            "MODE2_FORM2" => Some(Layout::Form2),
            _ => None,
        }
    }
}

/// A track table whose frames add up past a `usize`: the numbers are the
/// file's say, and a disc that long is not one.
struct PastAnyDisc;

/// Every track in a CHD's track table, in order, and where each is stored.
///
/// chdman stores each track's frames padded out to a whole number of
/// [`TRACK_PADDING`]: a CD's and a GD-ROM's alike. A GD-ROM's table carries a
/// `PAD` field as well, and it is not that padding but the gap between the
/// track and the next on the disc: Mat Hoffman's Pro BMX opens
/// `FRAMES:450 PAD:150` then `FRAMES:44550 PAD:44024`, and its file holds
/// 549,160 frames, which is its five tracks rounded up to fours and 44,464
/// fewer than the frames and `PAD`s add up to. All 27 GD-ROM CHDs checked hold
/// their tracks this way.
///
/// A track that ends past any disc is an error, and so is every one after it.
/// Lazy, so a walk that stops at the track it wants is not failed by a later
/// one.
fn stored(entries: &[String]) -> impl Iterator<Item = Result<Stored, PastAnyDisc>> + '_ {
    let mut before = Some(0usize);
    entries.iter().map(move |entry| {
        let at = before.ok_or(PastAnyDisc)?;
        let field =
            |name: &str| -> Option<&str> { entry.split_whitespace().find_map(|f| f.strip_prefix(name)) };
        let kind = field("TYPE:").unwrap_or("").to_string();
        let frames: usize = field("FRAMES:").and_then(|v| v.parse().ok()).unwrap_or(0);
        // A `PGTYPE` beginning with `V` means the pregap is *stored* ahead of
        // the track rather than synthesised. Verified against two PC Engine
        // discs: one `MODE1` with `PGTYPE:MODE1` and one `MODE1_RAW` with
        // `PGTYPE:VMODE1_RAW`, and only the second carries its 225 frames in
        // the file. Its `FRAMES` counts them: five more PC Engine CHDs store
        // exactly as many frames as Redump's track file holds sectors, pregap
        // and all.
        let pregap = if field("PGTYPE:").is_some_and(|t| t.starts_with('V')) {
            field("PREGAP:").and_then(|v| v.parse::<usize>().ok()).unwrap_or(0)
        } else {
            0
        };
        before = frames
            .div_ceil(TRACK_PADDING)
            .checked_mul(TRACK_PADDING)
            .and_then(|padded| at.checked_add(padded));
        if before.is_none() {
            return Err(PastAnyDisc);
        }
        Ok(Stored { before: at, frames, pregap, kind })
    })
}

/// The first data track in a CHD's track table, or `None` for a table that
/// names none.
///
/// **Track one is not always the data track.** A PC Engine CD commonly opens
/// with audio and puts the program on track two, so starting at frame zero
/// hands the identifier music. The `CHT2` metadata names each track's type, so
/// this walks it to the first `MODE1` or `MODE2` track.
fn first_data(entries: &[String]) -> Result<Option<Stored>, PastAnyDisc> {
    for track in stored(entries) {
        let track = track?;
        if track.data() {
            return Ok(Some(track));
        }
    }
    Ok(None)
}

/// Where the data track's content starts, and how much of each frame is its
/// data.
///
/// Zero for the ordinary case — Sega CD, PlayStation, Saturn all put data on
/// track one — so following the table changes nothing for them. A pregap the
/// file stores is stepped over, since a header is read from the content.
///
/// A `_RAW` track is read at its full width and a cooked one at 2048 bytes.
/// Taking the wrong width leaves the image with a stride the sector layer
/// cannot infer, so every lookup past the first lands in the wrong place.
///
/// `None` for a table whose frames add up past a `usize`.
fn data_track(entries: &[String]) -> Option<(usize, usize)> {
    let Ok(found) = first_data(entries) else { return None };
    // No data track named: read from the start rather than refusing outright.
    let Some(stored) = found else { return Some((0, CD_SECTOR)) };
    let width = if stored.raw() { CD_SECTOR } else { COOKED_SECTOR };
    Some((stored.before.checked_add(stored.pregap)?, width))
}

fn other(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}

fn past_any_disc() -> io::Error {
    invalid("a CHD track table whose frames add up past any disc".into())
}

/// A CHD vetted and opened, with what it says about itself.
struct Opened<F: Read + Seek> {
    chd: Chd<F>,
    /// Its metadata chain, as [`vet`] walked it.
    #[cfg_attr(not(any(feature = "redump", feature = "disc-data")), allow(dead_code))]
    metadata: Vec<Metadata>,
    /// Its track table, an entry a track, where it is a CD.
    tracks: Vec<String>,
    /// Whether frames carry subcode to drop, which a DVD or hard disk CHD
    /// does not.
    cd: bool,
}

/// Vet a CHD, open it, and read its track table.
fn open<F: Read + Seek>(mut file: F) -> io::Result<Opened<F>> {
    let metadata = vet(&mut file)?;
    let mut chd = Chd::open(file, None).map_err(other)?;
    let cd = chd.header().unit_bytes() as usize == CD_FRAME;

    // Where the program lives, which is not always where the disc starts.
    //
    // Every tag that carries the table is read. Reading only `CHT2` left
    // older dumps and every Dreamcast one with no track table at all, so
    // the reader fell back to frame zero and handed the identifier
    // whatever track one happened to be -- music, on a disc that opens
    // with audio. The entries are the ones `vet` walked, rather than the
    // crate's own walk of them, which would follow a chain that loops.
    let tracks: Vec<String> = metadata
        .iter()
        .filter(|entry| TRACK_TAGS.contains(&&entry.tag))
        .filter_map(|entry| entry.read(chd.inner()).ok())
        .map(|value| String::from_utf8_lossy(&value).trim_end_matches('\0').to_string())
        .collect();
    Ok(Opened { chd, metadata, tracks, cd })
}

/// A CHD's image as a stream, inflated a hunk at a time.
///
/// Hunks hold several frames each and are compressed, so nothing can be looked
/// at until the hunk holding it is inflated. What comes out is the **image**
/// rather than the file: it starts `skip` frames in, drops each frame's
/// subcode, cuts each to `width`, and ends after `limit` bytes or the last
/// hunk, so it reads like a `.bin` of the same track. Only one hunk is ever
/// held, which is what lets a whole track be hashed out of a disc.
struct Frames<F: Read + Seek> {
    chd: Chd<F>,
    hunk_count: u32,
    next: u32,
    cd: bool,
    width: usize,
    /// Frames of the next hunk still to drop: the ones before `skip`.
    drop: usize,
    /// Bytes still to hand out.
    left: u64,
    compressed: Vec<u8>,
    hunk: Vec<u8>,
    /// The current hunk's share of the image, read up to `at`.
    out: Vec<u8>,
    at: usize,
}

impl<F: Read + Seek> Frames<F> {
    fn new(chd: Chd<F>, skip: usize, width: usize, limit: u64) -> Self {
        let hunk_size = chd.header().hunk_size() as usize;
        let hunk_count = chd.header().hunk_count();
        let cd = chd.header().unit_bytes() as usize == CD_FRAME;
        // Start at the hunk holding the first frame wanted, then drop the
        // frames of it that still belong to whatever came before.
        let frames_per_hunk = hunk_size / CD_FRAME;
        let (next, drop) = if cd && frames_per_hunk > 0 {
            (
                // A hunk past `u32` is past every hunk a CHD can hold, so reading from it finds
                // nothing and the disc goes unidentified, as one that short would.
                u32::try_from(skip / frames_per_hunk).unwrap_or(u32::MAX),
                skip % frames_per_hunk,
            )
        } else {
            (0, 0)
        };
        Frames {
            chd,
            hunk_count,
            next,
            cd,
            width,
            drop,
            left: limit,
            compressed: Vec::new(),
            hunk: vec![0u8; hunk_size],
            out: Vec::new(),
            at: 0,
        }
    }

    /// Whether everything there is has been read.
    fn exhausted(&self) -> bool {
        self.at == self.out.len() && (self.left == 0 || self.next >= self.hunk_count)
    }

    /// The CHD back, to read another track out of.
    #[cfg(feature = "disc-data")]
    fn into_chd(self) -> Chd<F> {
        self.chd
    }

    /// Inflate the next hunk into `out`.
    fn fill(&mut self) -> io::Result<()> {
        self.chd
            .hunk(self.next)
            .map_err(other)?
            .read_hunk_in(&mut self.compressed, &mut self.hunk)
            .map_err(other)?;
        self.next += 1;
        self.out.clear();
        self.at = 0;
        if self.cd {
            // Keep the sector, drop the subcode that follows it.
            for frame in self.hunk.chunks(CD_FRAME) {
                if self.drop > 0 {
                    self.drop -= 1;
                    continue;
                }
                let take = self.width.min(frame.len());
                self.out.extend_from_slice(&frame[..take]);
            }
        } else {
            self.out.extend_from_slice(&self.hunk);
        }
        let keep = usize::try_from(self.left).unwrap_or(usize::MAX).min(self.out.len());
        self.out.truncate(keep);
        self.left -= keep as u64;
        Ok(())
    }

    /// The next stretch of image, at most a hunk of it, or `None` once there is
    /// no more.
    fn chunk(&mut self) -> io::Result<Option<&[u8]>> {
        while self.at == self.out.len() {
            if self.exhausted() {
                return Ok(None);
            }
            self.fill()?;
        }
        let from = self.at;
        self.at = self.out.len();
        Ok(Some(&self.out[from..]))
    }
}

impl<F: Read + Seek> Read for Frames<F> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.at == self.out.len() {
            if self.exhausted() {
                return Ok(0);
            }
            self.fill()?;
        }
        let n = buf.len().min(self.out.len() - self.at);
        buf[..n].copy_from_slice(&self.out[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

/// A CHD's first data track, inflated into a buffer on demand.
///
/// For identification, which reads a few hunks: the sector layer looks at the
/// buffer without knowing a CHD was involved.
struct Image<F: Read + Seek> {
    frames: Frames<F>,
    bytes: Vec<u8>,
}

impl<F: Read + Seek> Image<F> {
    /// Open a CHD and position on the first data track's content.
    fn open(file: F) -> io::Result<Self> {
        let Opened { chd, tracks, cd, .. } = open(file)?;
        let (skip, width) = if cd { data_track(&tracks).ok_or_else(past_any_disc)? } else { (0, CD_SECTOR) };
        Ok(Image { frames: Frames::new(chd, skip, width, u64::MAX), bytes: Vec::new() })
    }

    /// Whether every hunk has been read.
    fn exhausted(&self) -> bool {
        self.frames.exhausted()
    }

    /// The image decompressed so far.
    fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Inflate until the image holds `want` bytes, or the CHD runs out.
    fn grow_to(&mut self, want: usize) -> io::Result<()> {
        while self.bytes.len() < want {
            let Some(chunk) = self.frames.chunk()? else { break };
            self.bytes.extend_from_slice(chunk);
        }
        Ok(())
    }
}

/// Identify the disc inside a CHD.
///
/// Inflates only as far as identification asks, which for every system here is
/// a few hunks. `Ok(None)` means the image matches nothing recognised; an error
/// means the CHD itself could not be read.
pub fn identify<F: Read + Seek>(file: F) -> io::Result<Option<DiscId>> {
    let mut image = Image::open(file)?;
    let mut want = 64 * 1024;
    loop {
        image.grow_to(want)?;
        match crate::identify_prefix(image.bytes()) {
            ControlFlow::Break(found) => return Ok(found),
            // More was asked for than the hunks can supply: this is the disc.
            ControlFlow::Continue(more) if more > image.bytes().len() && !image.exhausted() => {
                want = more;
            }
            ControlFlow::Continue(_) => return Ok(None),
        }
    }
}

/// The user data of however many of the first `max_sectors` the disc holds.
///
/// The CHD counterpart of [`crate::sector::user_data_upto`], for the same
/// reason: a data track shorter than the window is a real disc and should be
/// read, not refused.
pub fn user_data_upto<F: Read + Seek>(file: F, max_sectors: usize) -> io::Result<Vec<u8>> {
    let mut image = Image::open(file)?;
    // An upper bound: raw sectors are the widest a track can be.
    let want = max_sectors.saturating_mul(CD_SECTOR);
    // Grow once to the bound; a short disc simply exhausts before reaching it.
    image.grow_to(want)?;
    while !image.exhausted() && image.bytes().len() < want {
        image.grow_to(want)?;
    }
    Ok(crate::sector::user_data_upto(image.bytes(), max_sectors))
}

/// The user data of the first `sectors` sectors of the disc inside a CHD.
///
/// The CHD counterpart of [`crate::sector::user_data_prefix`], and it returns
/// the identical bytes: a game stored as a CHD and the same game stored as a
/// cue and bin both come out as plain 2048-byte sectors. That is what makes a
/// fingerprint taken over these bytes survive a change of container.
///
/// `Ok(None)` when the disc is shorter than the sectors asked for.
pub fn user_data_prefix<F: Read + Seek>(file: F, sectors: usize) -> io::Result<Option<Vec<u8>>> {
    let mut image = Image::open(file)?;
    // An upper bound: raw sectors are the widest a track can be.
    let mut want = sectors.saturating_mul(CD_SECTOR);
    loop {
        image.grow_to(want)?;
        match crate::sector::user_data_prefix(image.bytes(), sectors) {
            ControlFlow::Break(bytes) => return Ok(Some(bytes)),
            ControlFlow::Continue(more) if more > image.bytes().len() && !image.exhausted() => {
                want = more;
            }
            ControlFlow::Continue(_) => return Ok(None),
        }
    }
}

/// The Redump hash of the disc inside a CHD.
///
/// The CHD counterpart of [`redump::track`](crate::redump::track), and it
/// gives the same value as the cue sheet and track files the CHD was made from:
/// the first data track's frames, subcode dropped, from its stored pregap to
/// its last frame, which is the bytes of Redump's file for that track. A CD
/// track is inflated a hunk at a time and hashed as it goes, never held.
///
/// A DVD, which chdman's `createdvd` marks as one, is hashed whole, as Redump
/// hashes a DVD's image.
///
/// `Ok(None)` for a CD whose first data track is stored cooked (`MODE1`,
/// `MODE2_FORM1` and the rest without `_RAW`), since the bytes Redump hashed
/// are not in it; for a disc of only audio; for a CHD that is neither a CD nor
/// a DVD; and for one whose hunks end before its track does. An error means
/// the CHD itself could not be read.
#[cfg(feature = "redump")]
pub fn redump<F: Read + Seek>(file: F) -> io::Result<Option<crate::redump::RedumpHash>> {
    use crate::redump::{Framed, raw_track, whole};

    let Opened { chd, metadata, tracks, cd } = open(file)?;
    if !cd {
        if !metadata.iter().any(|entry| &entry.tag == DVD_TAG) {
            return Ok(None);
        }
        // The last hunk is padded past the image; the header says where it ends.
        let logical = chd.header().logical_bytes();
        let found = whole(Frames::new(chd, 0, CD_SECTOR, logical))?;
        return Ok((found.size == logical).then_some(found));
    }
    let Some(stored) = first_data(&tracks).map_err(|PastAnyDisc| past_any_disc())? else {
        return Ok(None);
    };
    if !stored.raw() {
        return Ok(None);
    }
    let sector = CD_SECTOR as u64;
    let (Some(span), Some(content)) =
        ((stored.frames as u64).checked_mul(sector), (stored.pregap as u64).checked_mul(sector))
    else {
        return Ok(None);
    };
    raw_track(Frames::new(chd, stored.before, CD_SECTOR, span), &Framed::At(content), Some(span))
}

/// The [`disc-data`](crate::disc_data) fingerprint of the disc inside a CHD.
///
/// The CHD counterpart of [`disc_data::sheet`](crate::disc_data::sheet), and
/// it gives the same value as the cue sheet and files the CHD was made from,
/// stored raw or cooked: each data track's frames from its content to its end,
/// a stored pregap stepped over, inflated a hunk at a time and never held. The
/// hunks of audio tracks are not inflated at all.
///
/// A DVD, which chdman's `createdvd` marks as one, is one track, its whole
/// image.
///
/// `Ok(None)` for a disc whose data tracks hold nothing, or of only audio; for a
/// data track of a type this does not know; for a CHD that is neither a CD nor
/// a DVD, or a CD with no track table; and for one whose hunks end before its
/// tracks do. An error means the CHD itself could not be read.
#[cfg(feature = "disc-data")]
pub fn disc_data<F: Read + Seek>(file: F) -> io::Result<Option<crate::disc_data::DiscData>> {
    use crate::disc_data::{DiscData, Layout, read_track};

    let Opened { mut chd, metadata, tracks, cd } = open(file)?;
    if !cd {
        if !metadata.iter().any(|entry| &entry.tag == DVD_TAG) {
            return Ok(None);
        }
        // The last hunk is padded past the image; the header says where it ends.
        let logical = chd.header().logical_bytes();
        let read = read_track(Frames::new(chd, 0, CD_SECTOR, logical), Layout::Cooked)?;
        if read.bytes < logical {
            return Ok(None);
        }
        return Ok(read.track.and_then(|track| DiscData::from_tracks(vec![track])));
    }
    if tracks.is_empty() {
        return Ok(None);
    }
    let mut found = Vec::new();
    for stored in stored(&tracks) {
        let stored = stored.map_err(|PastAnyDisc| past_any_disc())?;
        if !stored.data() {
            continue;
        }
        let Some(layout) = stored.layout() else { return Ok(None) };
        let width = layout.width();
        let (Some(skip), Some(bytes)) = (
            stored.before.checked_add(stored.pregap),
            stored
                .frames
                .checked_sub(stored.pregap)
                .and_then(|count| (count as u64).checked_mul(width as u64)),
        ) else {
            return Ok(None);
        };
        let mut frames = Frames::new(chd, skip, width, bytes);
        let read = read_track(&mut frames, layout)?;
        if read.bytes < bytes {
            return Ok(None);
        }
        found.extend(read.track);
        chd = frames.into_chd();
    }
    Ok(DiscData::from_tracks(found))
}

#[cfg(test)]
mod tests {
    use super::{CD_SECTOR, COOKED_SECTOR, MAX_HUNK, MAX_HUNKS, MAX_METADATA, data_track, identify, stored};

    fn entries(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|s| (*s).to_string()).collect()
    }

    /// The ordinary case: data on track one, nothing skipped.
    #[test]
    fn a_disc_whose_first_track_is_data_starts_at_zero() {
        let tracks = entries(&[
            "TRACK:1 TYPE:MODE1 SUBTYPE:NONE FRAMES:10149 PREGAP:0",
            "TRACK:2 TYPE:AUDIO SUBTYPE:NONE FRAMES:7984 PREGAP:150",
        ]);
        assert_eq!(data_track(&tracks), Some((0, COOKED_SECTOR)));
    }

    /// Read from a real PC Engine CD: audio first, program on track two. Its
    /// 3665 frames pad up to a multiple of four.
    #[test]
    fn audio_before_the_program_is_skipped() {
        let tracks = entries(&[
            "TRACK:1 TYPE:AUDIO SUBTYPE:NONE FRAMES:3665 PREGAP:0",
            "TRACK:2 TYPE:MODE1 SUBTYPE:NONE FRAMES:10149 PREGAP:225 PGTYPE:MODE1",
        ]);
        assert_eq!(data_track(&tracks), Some((3668, COOKED_SECTOR)), "3665 padded to 3668");
    }

    /// A GD-ROM's `PAD` is the gap to the next track on the disc, not padding
    /// in the file, which is the rounding a CD's is. The track table of Mat
    /// Hoffman's Pro BMX (USA), whose CHD holds 549,160 frames: rounding each
    /// track to four accounts for every one of them, and adding the `PAD`s
    /// would put its program 44,170 frames past where it is.
    #[test]
    fn a_gd_rom_pads_its_tracks_as_a_cd_does() {
        let tracks = entries(&[
            "TRACK:1 TYPE:MODE1_RAW SUBTYPE:NONE FRAMES:450 PAD:150 PREGAP:0 PGTYPE:MODE1",
            "TRACK:2 TYPE:AUDIO SUBTYPE:NONE FRAMES:44550 PAD:44024 PREGAP:0 PGTYPE:MODE1",
            "TRACK:3 TYPE:MODE1_RAW SUBTYPE:NONE FRAMES:502967 PAD:150 PREGAP:0 PGTYPE:MODE1",
            "TRACK:4 TYPE:AUDIO SUBTYPE:NONE FRAMES:601 PAD:150 PREGAP:0 PGTYPE:MODE1",
            "TRACK:5 TYPE:MODE1_RAW SUBTYPE:NONE FRAMES:582 PAD:0 PREGAP:0 PGTYPE:MODE1",
        ]);
        let starts: Vec<usize> = stored(&tracks).map(|track| track.ok().unwrap().before).collect();
        assert_eq!(starts, [0, 452, 45_004, 547_972, 548_576]);
        assert_eq!(548_576 + 584, 549_160, "and the last, rounded, ends where the file does");

        let tracks = entries(&[
            "TRACK:1 TYPE:AUDIO SUBTYPE:NONE FRAMES:450 PAD:150 PREGAP:0",
            "TRACK:2 TYPE:MODE1_RAW SUBTYPE:NONE FRAMES:1000 PAD:0 PREGAP:0",
        ]);
        assert_eq!(data_track(&tracks), Some((452, CD_SECTOR)), "450 rounded to 452");
    }

    /// A CD's table states no `PAD`, and its tracks are rounded the same way.
    #[test]
    fn a_cd_pads_each_track_to_four_frames() {
        let tracks = entries(&[
            "TRACK:1 TYPE:AUDIO SUBTYPE:NONE FRAMES:450 PREGAP:0",
            "TRACK:2 TYPE:MODE1 SUBTYPE:NONE FRAMES:1000 PREGAP:0",
        ]);
        assert_eq!(data_track(&tracks), Some((452, COOKED_SECTOR)), "450 rounded to 452");
    }

    /// The other real disc: its pregap *is* in the file, so the program starts
    /// 225 frames later than the track table alone would suggest. Getting this
    /// wrong reads 225 frames of silence as the boot header.
    #[test]
    fn a_stored_pregap_is_stepped_over_too() {
        let tracks = entries(&[
            "TRACK:1 TYPE:AUDIO SUBTYPE:NONE FRAMES:3009 PREGAP:0 PGTYPE:MODE1",
            "TRACK:2 TYPE:MODE1_RAW SUBTYPE:NONE FRAMES:3446 PREGAP:225 PGTYPE:VMODE1_RAW",
        ]);
        assert_eq!(data_track(&tracks), Some((3012 + 225, CD_SECTOR)), "a _RAW track keeps its sync header");
    }

    /// An all-audio disc has no program to find; reading from the start is a
    /// better answer than refusing, and identification will simply say no.
    #[test]
    fn a_disc_with_no_data_track_reads_from_the_start() {
        let tracks = entries(&["TRACK:1 TYPE:AUDIO SUBTYPE:NONE FRAMES:100 PREGAP:0"]);
        assert_eq!(data_track(&tracks).map(|(frames, _)| frames), Some(0));
    }

    /// A version 5 header, the one the `chd` crate does not check, declaring
    /// a hunk of `hunk` bytes.
    fn v5_header(hunk: u32) -> Vec<u8> {
        let mut header = vec![0u8; 124];
        header[..8].copy_from_slice(b"MComprHD");
        header[8..12].copy_from_slice(&124u32.to_be_bytes());
        header[12..16].copy_from_slice(&5u32.to_be_bytes());
        header[32..40].copy_from_slice(&u64::from(hunk).to_be_bytes());
        header[40..48].copy_from_slice(&124u64.to_be_bytes());
        header[56..60].copy_from_slice(&hunk.to_be_bytes());
        header[60..64].copy_from_slice(&2448u32.to_be_bytes());
        header
    }

    #[test]
    fn a_hunk_larger_than_chdman_writes_is_refused_before_opening() {
        for hunk in [u32::try_from(MAX_HUNK).unwrap() + 1, u32::MAX] {
            let error = identify(std::io::Cursor::new(v5_header(hunk))).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
            assert!(error.to_string().contains("chdman writes at most"), "{error}");
        }
        // At the limit the header is the crate's to judge, and this one is not a whole CHD.
        let error = identify(std::io::Cursor::new(v5_header(19_584))).unwrap_err();
        assert!(!error.to_string().contains("chdman writes at most"), "{error}");
    }

    /// Eight CD frames, the hunk chdman writes a CD in.
    const HUNK: u32 = 2448 * 8;

    /// Where [`cd`] puts the metadata: after the header and a map of one hunk.
    const METADATA: u64 = 128;

    /// One metadata entry: its tag, where the next one is, and its value.
    fn entry(tag: [u8; 4], next: u64, value: &[u8]) -> Vec<u8> {
        let mut entry = tag.to_vec();
        entry.extend_from_slice(&u32::try_from(value.len()).unwrap().to_be_bytes());
        entry.extend_from_slice(&next.to_be_bytes());
        entry.extend_from_slice(value);
        entry
    }

    /// A version 5 CHD of one uncompressed CD hunk, with `metadata` at
    /// [`METADATA`] and the hunk after it. Offsets in the metadata count from
    /// the start of the file.
    fn cd(metadata: &[u8]) -> Vec<u8> {
        let mut file = v5_header(HUNK);
        let first = if metadata.is_empty() { 0 } else { METADATA };
        file[48..56].copy_from_slice(&first.to_be_bytes());
        file.extend_from_slice(&[0u8; 4]);
        file.extend_from_slice(metadata);
        file.resize(file.len() + HUNK as usize, 0);
        file
    }

    /// Why a file was refused here, before the crate saw it.
    fn refusal(file: Vec<u8>) -> String {
        let error = identify(std::io::Cursor::new(file)).expect_err("refused");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData, "{error}");
        error.to_string()
    }

    /// Why a file failed, here or in the crate.
    fn failure(file: Vec<u8>) -> String {
        identify(std::io::Cursor::new(file)).expect_err("fails").to_string()
    }

    /// What every check here has to leave alone: the layout the tests below
    /// break, whole, opens and reads as the blank disc it is.
    #[test]
    fn a_well_formed_chd_still_opens() {
        let table = [
            entry(*b"CHT2", METADATA + 16 + 41, b"TRACK:1 TYPE:AUDIO SUBTYPE:NONE FRAMES:4\0"),
            entry(*b"CHT2", 0, b"TRACK:2 TYPE:MODE1 SUBTYPE:NONE FRAMES:4\0"),
        ]
        .concat();
        assert_eq!(identify(std::io::Cursor::new(cd(&table))).expect("opens"), None);
        assert_eq!(identify(std::io::Cursor::new(cd(&[]))).expect("opens"), None);
    }

    /// A metadata chain is walked by following each entry's `next`, and the
    /// `chd` crate follows it forever where it leads back: collecting the
    /// track table spun, growing, on a file of twenty kilobytes. A version 4
    /// header is walked by the crate as it opens, before anything here would
    /// get a look at it.
    #[test]
    fn a_metadata_chain_that_leads_back_on_itself_is_refused() {
        let track = b"TRACK:1 TYPE:MODE1 SUBTYPE:NONE FRAMES:1\0";
        let itself = entry(*b"CHT2", METADATA, track);
        assert!(refusal(cd(&itself)).contains("leads back"));

        let second = METADATA + 16 + track.len() as u64;
        let pair = [entry(*b"CHT2", second, track), entry(*b"CHT2", METADATA, track)].concat();
        assert!(refusal(cd(&pair)).contains("leads back"));

        let mut v4 = vec![0u8; 108];
        v4[..8].copy_from_slice(b"MComprHD");
        v4[8..12].copy_from_slice(&108u32.to_be_bytes());
        v4[12..16].copy_from_slice(&4u32.to_be_bytes());
        v4[24..28].copy_from_slice(&1u32.to_be_bytes());
        v4[28..36].copy_from_slice(&u64::from(HUNK).to_be_bytes());
        v4[36..44].copy_from_slice(&108u64.to_be_bytes());
        v4[44..48].copy_from_slice(&HUNK.to_be_bytes());
        v4.extend_from_slice(&entry(*b"CHT2", 108, track));
        assert!(refusal(v4).contains("leads back"));

        // One that never comes back but never ends either, as near as matters.
        let long: Vec<u8> =
            (0..=MAX_METADATA as u64).flat_map(|at| entry(*b"CHT2", METADATA + (at + 1) * 16, b"")).collect();
        assert!(refusal(cd(&long)).contains("one a track"));
    }

    /// The track table's numbers are the file's say. Added up unchecked, a
    /// track of `usize::MAX` frames overflowed: a panic in a debug build and a
    /// wrapped, wrong starting frame in a release one.
    #[test]
    fn a_track_table_past_any_disc_is_refused_rather_than_wrapped() {
        let max = usize::MAX;
        for table in [
            vec![format!("TRACK:1 TYPE:AUDIO SUBTYPE:NONE FRAMES:{max}"), "TRACK:2 TYPE:MODE1".into()],
            vec![format!("TRACK:1 TYPE:AUDIO SUBTYPE:NONE FRAMES:{max} PAD:1"), "TRACK:2 TYPE:MODE1".into()],
            vec![
                format!("TRACK:1 TYPE:AUDIO SUBTYPE:NONE FRAMES:{} PAD:0", max / 2 + 1),
                format!("TRACK:2 TYPE:AUDIO SUBTYPE:NONE FRAMES:{} PAD:0", max / 2 + 1),
                "TRACK:3 TYPE:MODE1".into(),
            ],
            vec![
                "TRACK:1 TYPE:AUDIO SUBTYPE:NONE FRAMES:4".into(),
                format!("TRACK:2 TYPE:MODE1 SUBTYPE:NONE FRAMES:4 PREGAP:{max} PGTYPE:VMODE1"),
            ],
        ] {
            assert_eq!(data_track(&table), None, "{table:?}");
        }

        let table = format!("TRACK:1 TYPE:AUDIO SUBTYPE:NONE FRAMES:{max}\0");
        assert!(refusal(cd(&entry(*b"CHT2", 0, table.as_bytes()))).contains("past any disc"));
    }

    /// A header stating more image than a CHD's hunks can hold. The `chd`
    /// crate adds a hunk to the size before dividing, which for `u64::MAX`
    /// overflowed, and sizes its map by the quotient, which for four billion
    /// hunks is an allocation of 16 GiB from a header and nothing else.
    #[test]
    fn a_size_past_what_a_chd_holds_is_refused_before_opening() {
        for logical in [u64::MAX, u64::from(HUNK) * u64::from(u32::MAX), u64::from(HUNK) * (MAX_HUNKS + 1)] {
            let mut file = cd(&[]);
            file[32..40].copy_from_slice(&logical.to_be_bytes());
            assert!(refusal(file).contains("more than"), "{logical}");
        }
        // The most it does hold is a map the file has to have room for.
        let mut file = cd(&[]);
        file[32..40].copy_from_slice(&(u64::from(HUNK) * MAX_HUNKS).to_be_bytes());
        assert!(refusal(file).contains("past the end"));

        // A version 1 header states a hard disk's geometry instead, and the
        // crate multiplies it out unchecked.
        let mut v1 = vec![0u8; 76];
        v1[..8].copy_from_slice(b"MComprHD");
        v1[8..12].copy_from_slice(&76u32.to_be_bytes());
        v1[12..16].copy_from_slice(&1u32.to_be_bytes());
        v1[24..28].copy_from_slice(&1u32.to_be_bytes());
        v1[32..44].fill(0xff);
        assert!(refusal(v1).contains("geometry"));
    }

    /// An older header with no hunk size has the crate divide by zero as it
    /// opens, in a release build as much as a debug one, and so does a hard
    /// disk whose geometry gives its sectors no bytes.
    #[test]
    fn a_unit_of_nothing_is_refused_before_the_crate_divides_by_it() {
        let mut v3 = vec![0u8; 120];
        v3[..8].copy_from_slice(b"MComprHD");
        v3[8..12].copy_from_slice(&120u32.to_be_bytes());
        v3[12..16].copy_from_slice(&3u32.to_be_bytes());
        v3[24..28].copy_from_slice(&1u32.to_be_bytes());
        v3[28..36].copy_from_slice(&1u64.to_be_bytes());
        assert!(refusal(v3).contains("hold nothing"));

        let mut v4 = vec![0u8; 108];
        v4[..8].copy_from_slice(b"MComprHD");
        v4[8..12].copy_from_slice(&108u32.to_be_bytes());
        v4[12..16].copy_from_slice(&4u32.to_be_bytes());
        v4[24..28].copy_from_slice(&1u32.to_be_bytes());
        v4[28..36].copy_from_slice(&512u64.to_be_bytes());
        v4[36..44].copy_from_slice(&108u64.to_be_bytes());
        v4[44..48].copy_from_slice(&4096u32.to_be_bytes());
        v4.extend_from_slice(&entry(*b"GDDD", 0, b"CYLS:1,HEADS:1,SECS:1,BPS:0\0"));
        assert!(refusal(v4).contains("sectors hold nothing"));
    }

    /// A version 5 CHD of one hunk whose compressed map has the given field
    /// widths, first offset, and bitstream: the Huffman tree, then the map.
    fn compressed(bits: [u8; 3], first: u64, stream: &[u8]) -> Vec<u8> {
        let mut file = v5_header(HUNK);
        file[16..20].copy_from_slice(b"zlib");
        let mut stream = stream.to_vec();
        stream.resize(stream.len().max(16), 0);
        file.extend_from_slice(&u32::try_from(stream.len()).unwrap().to_be_bytes());
        file.extend_from_slice(&first.to_be_bytes()[2..]);
        file.extend_from_slice(&[0, 0, bits[0], bits[1], bits[2], 0]);
        file.extend_from_slice(&stream);
        file
    }

    /// A compressed map's Huffman tree that runs past its sixteen codes, or
    /// whose lengths ask for more codes than eight bits hold: the crate
    /// indexes past an array either way, a panic in any build. One that fills
    /// them exactly with a code that fits is the crate's to judge.
    #[test]
    fn a_map_tree_the_crate_would_index_past_is_refused_before_opening() {
        let tree = |tree: &[u8]| compressed([16, 0, 0], 0, tree);
        // An escape, a length of 2, and a run of 15 + 3: eighteen codes.
        assert!(refusal(tree(&[0x12, 0xf0])).contains("Huffman tree"));
        // Sixteen codes of two bits, where two bits make four.
        assert!(refusal(tree(&[0x22; 8])).contains("Huffman tree"));
        assert!(refusal(tree(&[0x12, 0xd0])).contains("Huffman tree"));
        // Sixteen of four bits is all of them, spelled out or as a run.
        assert!(!failure(tree(&[0x44; 8])).contains("Huffman tree"));
        assert!(!failure(tree(&[0x14, 0xd0])).contains("Huffman tree"));

        // And a map that says more of it follows than the file holds, which
        // the crate allocates before finding out: up to 4 GiB.
        let mut file = tree(&[0x44; 8]);
        file[124..128].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(refusal(file).contains("past the end"));
    }

    /// The crate expands each map entry into twelve bytes through a writer
    /// that asserts the value fits: a length of all 32 bits, read for a hunk
    /// of code `0000`, panicked in any build.
    #[test]
    fn a_map_whose_fields_outgrow_their_entries_is_refused_before_opening() {
        let stream = [[0x44; 8].as_slice(), &[0x0f, 0xff, 0xff, 0xff, 0xf0, 0, 0, 0]].concat();
        assert!(refusal(compressed([32, 0, 0], 0, &stream)).contains("write them back"));
        assert!(refusal(compressed([16, 48, 0], 0, &stream)).contains("write them back"));
        assert!(refusal(compressed([16, 0, 48], 0, &stream)).contains("write them back"));
        assert!(refusal(compressed([16, 0, 0], (1 << 48) - 1, &stream)).contains("write them back"));
        // Widths chdman would write are left alone.
        assert!(!failure(compressed([20, 24, 0], 124, &stream)).contains("write them back"));
    }

    /// LaserDisc video is never a CD or a DVD, and its codec reads Huffman
    /// trees as unguardedly as the map does.
    #[test]
    fn the_av_codec_is_refused_before_opening() {
        let mut file = cd(&[]);
        file[20..24].copy_from_slice(b"avhu");
        assert!(refusal(file).contains("LaserDisc"));
    }
}
