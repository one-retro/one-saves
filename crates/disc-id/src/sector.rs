//! Disc sector geometry, and reading sectors out of a stream.
//!
//! A dump is either **cooked** (2048-byte sectors, user data and nothing else)
//! or **raw** (2352-byte sectors carrying the sync pattern, an address header
//! and error correction around the same 2048 bytes). Nothing in a filename says
//! which, and every signature is stated as an offset into *user data*, so this
//! is the layer that has to get it right before anything else can look.
//!
//! Reading is **forward-only**. A caller may be holding an SFTP stream, a
//! decompressing zip entry, or both at once, none of which seek cheaply, so
//! this buffers what it has read and never asks to go backwards. What it will
//! not do is read the whole disc: [`MAX_PREFIX`] caps the buffer, and a sector
//! past that is simply not available.

use std::cell::OnceCell;
use std::ops::ControlFlow;

/// The twelve bytes every raw sector opens with.
const SYNC: [u8; 12] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];

/// Bytes of user data in a sector, whatever the container around it.
pub const USER_DATA: usize = 2048;

/// A raw sector: sync, header, user data, error correction.
pub const RAW: usize = 2352;

/// How much of a disc this will ever buffer.
///
/// Everything identification needs lives near the front: the volume descriptor
/// is sector 16, and a root directory sits within a few hundred sectors of it.
/// Four mebibytes clears that with room to spare while keeping the promise that
/// identifying a disc never reads a disc's worth of bytes.
pub const MAX_PREFIX: usize = 4 * 1024 * 1024;

/// How a dump lays its sectors out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Geometry {
    /// 2048-byte sectors: user data only. `.iso`, and most DVD-era dumps.
    Cooked,
    /// 2352-byte sectors with user data 16 bytes in. CD-ROM Mode 1.
    RawMode1,
    /// 2352-byte sectors with an 8-byte subheader, so user data 24 bytes in.
    /// CD-ROM Mode 2 Form 1, which is what most PlayStation and PC Engine
    /// dumps are.
    RawMode2,
}

impl Geometry {
    /// Bytes per sector in the file.
    #[must_use]
    pub fn stride(self) -> usize {
        match self {
            Geometry::Cooked => USER_DATA,
            Geometry::RawMode1 | Geometry::RawMode2 => RAW,
        }
    }

    /// Where user data starts inside a sector.
    #[must_use]
    pub fn offset(self) -> usize {
        match self {
            Geometry::Cooked => 0,
            // 12 sync + 4 address/mode.
            Geometry::RawMode1 => 16,
            // ...plus the 8-byte Mode 2 subheader.
            Geometry::RawMode2 => 24,
        }
    }

    /// Work out the geometry from the opening bytes.
    ///
    /// The sync pattern is the whole test: it cannot open user data in a cooked
    /// dump, because a cooked dump has no sync pattern anywhere. The mode byte
    /// follows the address, and anything but 1 reads as Mode 2, which is what
    /// the discs using it are.
    #[must_use]
    pub fn detect(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < USER_DATA {
            return None;
        }
        if !bytes.starts_with(&SYNC) {
            return Some(Geometry::Cooked);
        }
        if bytes.len() < RAW {
            return None;
        }
        Some(if bytes[15] == 1 { Geometry::RawMode1 } else { Geometry::RawMode2 })
    }
}

/// A prefix of a disc image, and a note of how much more would be needed.
///
/// This crate performs no I/O. A caller hands over whatever bytes it has from
/// the start of the image; when a lookup runs past them, the shortfall is
/// recorded rather than read, and surfaces as a
/// [`ControlFlow::Continue`]. That keeps the crate
/// out of the choice between blocking reads, `tokio`, `futures` and an inflating
/// zip entry, all of which a caller may be holding.
pub struct Prefix<'a> {
    bytes: &'a [u8],
    /// Detected once, on the first lookup that needs it; empty until then.
    geometry: OnceCell<Option<Geometry>>,
    needed: Option<usize>,
}

impl<'a> Prefix<'a> {
    /// Read from the bytes available so far.
    #[must_use]
    pub fn new(bytes: &'a [u8]) -> Self {
        Prefix { bytes, geometry: OnceCell::new(), needed: None }
    }

    /// How many bytes from the start would have satisfied every lookup so far.
    ///
    /// `None` once nothing has come up short. Capped at [`MAX_PREFIX`]: a
    /// lookup beyond that is refused rather than requested, so identification
    /// can never ask for a disc's worth of bytes.
    #[must_use]
    pub fn needed(&self) -> Option<usize> {
        self.needed
    }

    fn short(&mut self, want: usize) {
        if want <= MAX_PREFIX {
            self.needed = Some(self.needed.map_or(want, |n| n.max(want)));
        }
    }

    /// The opening bytes, up to `want` of them.
    #[must_use]
    pub fn head(&mut self, want: usize) -> &'a [u8] {
        if self.bytes.len() < want {
            self.short(want);
        }
        &self.bytes[..self.bytes.len().min(want)]
    }

    /// How this dump is laid out, or `None` if there is not yet enough to say.
    pub fn geometry(&mut self) -> Option<Geometry> {
        if let Some(&known) = self.geometry.get() {
            return known;
        }
        if self.bytes.len() < RAW {
            self.short(RAW);
        }
        let detected = Geometry::detect(self.bytes);
        let _ = self.geometry.set(detected);
        detected
    }

    /// The user data of one sector, or `None` when it lies past what is held.
    pub fn sector(&mut self, index: usize) -> Option<&'a [u8]> {
        let geometry = self.geometry()?;
        let start = index.checked_mul(geometry.stride()).and_then(|s| s.checked_add(geometry.offset()))?;
        let end = start.checked_add(USER_DATA)?;
        let data = self.bytes.get(start..end);
        if data.is_none() {
            self.short(end);
        }
        data
    }
}

/// The user data of the first `sectors` sectors, concatenated.
///
/// This is the one representation of a disc that every container agrees on.
/// A `.iso` stores 2048-byte sectors and nothing else; a `.bin` from a cue
/// wraps the same 2048 bytes in a sync pattern, an address header and error
/// correction; a CHD stores 2352-byte sectors followed by 96 bytes of subcode,
/// or 2048 alone for a cooked track; CloneCD keeps subcode in a separate file
/// entirely. Strip all of that and the same disc yields the same bytes.
///
/// So this is what a fingerprint should be taken over. Hashing a container
/// instead would give one game as many identities as it has dump formats, and
/// would change when someone recompressed it. Hashing this does not.
///
/// What the fingerprint is *for* shapes how much to take. It is not naming a
/// dump, it is naming a set of dumps whose saves are interchangeable. Two discs
/// that open with the same program produce the same save format, so they should
/// answer alike even when their later sectors differ: a fan translation patches
/// text well past the boot area and its saves still load in the original. A
/// bounded prefix gets that equivalence for free, where a whole-disc hash would
/// split one save into two games.
///
/// Deliberately returns the bytes rather than a hash: this crate depends on
/// nothing, and picking a digest for a caller is not its job.
///
/// `Continue` carries how many bytes from the start would have been enough,
/// the same contract as [`identify_prefix`](crate::identify_prefix).
pub fn user_data_prefix(bytes: &[u8], sectors: usize) -> ControlFlow<Vec<u8>, usize> {
    let mut prefix = Prefix::new(bytes);
    let mut out = Vec::with_capacity(sectors.saturating_mul(USER_DATA));
    for index in 0..sectors {
        match prefix.sector(index) {
            Some(data) => out.extend_from_slice(data),
            None => return ControlFlow::Continue(prefix.needed().unwrap_or(RAW)),
        }
    }
    ControlFlow::Break(out)
}

/// The user data of however many of the first `max_sectors` the bytes hold.
///
/// [`user_data_prefix`] is all-or-nothing, which is right when the caller can
/// fetch more and wrong when there is no more to fetch. A disc whose data track
/// is shorter than the window asked for is a real disc, not a failure: PC
/// Engine CD titles exist with a 536-sector program track, and refusing to
/// look at them means they cannot be fingerprinted at all and fall back to
/// hashing the container, which is the very thing a fingerprint is for.
///
/// Returns whole sectors only, and an empty vector when not even one is there.
#[must_use]
pub fn user_data_upto(bytes: &[u8], max_sectors: usize) -> Vec<u8> {
    let mut prefix = Prefix::new(bytes);
    let mut out = Vec::new();
    for index in 0..max_sectors {
        match prefix.sector(index) {
            Some(data) => out.extend_from_slice(data),
            None => break,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_sector(mode: u8, data: &[u8]) -> Vec<u8> {
        let mut s = SYNC.to_vec();
        s.extend_from_slice(&[0, 0, 0, mode]);
        if mode == 2 {
            s.extend_from_slice(&[0u8; 8]);
        }
        let mut payload = data.to_vec();
        payload.resize(USER_DATA, 0);
        s.extend_from_slice(&payload);
        s.resize(RAW, 0);
        s
    }

    /// The property the fingerprint rests on: the same sectors packed raw and
    /// packed cooked come out as the same bytes. Verified on a real disc too,
    /// where a PlayStation bin and an ISO cooked from it agreed exactly.
    #[test]
    fn the_same_sectors_yield_the_same_user_data_whatever_the_container() {
        let mut raw = Vec::new();
        let mut cooked = Vec::new();
        for i in 0..4u8 {
            let payload = [i; 32];
            raw.extend_from_slice(&raw_sector(2, &payload));
            let mut flat = payload.to_vec();
            flat.resize(USER_DATA, 0);
            cooked.extend_from_slice(&flat);
        }

        let ControlFlow::Break(from_raw) = user_data_prefix(&raw, 4) else {
            panic!("raw dump came up short");
        };
        let ControlFlow::Break(from_cooked) = user_data_prefix(&cooked, 4) else {
            panic!("cooked dump came up short");
        };
        assert_eq!(from_raw, from_cooked);
        assert_eq!(from_raw.len(), 4 * USER_DATA);
    }

    /// A disc shorter than the fingerprint asks for says how much it wanted
    /// rather than hashing a partial read, which would invent an identity.
    #[test]
    fn a_short_dump_reports_the_shortfall_instead_of_hashing_less() {
        let one = raw_sector(1, b"only one sector here");
        match user_data_prefix(&one, 4) {
            ControlFlow::Continue(needed) => {
                assert!(needed > one.len(), "asked for more than it was given");
            }
            ControlFlow::Break(_) => panic!("one sector cannot satisfy four"),
        }
    }

    #[test]
    fn a_dump_without_the_sync_pattern_is_cooked() {
        let bytes = vec![0u8; USER_DATA * 2];
        assert_eq!(Geometry::detect(&bytes), Some(Geometry::Cooked));
    }

    #[test]
    fn the_mode_byte_picks_the_user_data_offset() {
        assert_eq!(Geometry::detect(&raw_sector(1, b"one")), Some(Geometry::RawMode1));
        assert_eq!(Geometry::detect(&raw_sector(2, b"two")), Some(Geometry::RawMode2));
        assert_eq!(Geometry::RawMode1.offset(), 16);
        assert_eq!(Geometry::RawMode2.offset(), 24);
    }

    #[test]
    fn user_data_reads_the_same_whatever_the_packing() {
        for mode in [1u8, 2] {
            let dump = raw_sector(mode, b"SEGADISCSYSTEM");
            let mut prefix = Prefix::new(&dump);
            assert!(prefix.sector(0).unwrap().starts_with(b"SEGADISCSYSTEM"));
            assert_eq!(prefix.needed(), None, "nothing came up short");
        }
        let mut cooked = b"SEGADISCSYSTEM".to_vec();
        cooked.resize(USER_DATA, 0);
        let mut prefix = Prefix::new(&cooked);
        assert!(prefix.sector(0).unwrap().starts_with(b"SEGADISCSYSTEM"));
    }

    /// A short prefix must say how much would have done, so a caller can fetch
    /// exactly that and no more.
    #[test]
    fn a_lookup_past_the_prefix_records_what_would_satisfy_it() {
        let dump = raw_sector(1, b"x");
        let mut prefix = Prefix::new(&dump);
        assert!(prefix.sector(4).is_none());
        assert_eq!(prefix.needed(), Some(4 * RAW + 16 + USER_DATA));
    }

    #[test]
    fn a_prefix_too_short_to_type_asks_for_one_sector() {
        let mut prefix = Prefix::new(&[0u8; 10]);
        assert!(prefix.geometry().is_none());
        assert_eq!(prefix.needed(), Some(RAW));
    }

    /// Beyond the cap the answer is "no", not "fetch more": identification is
    /// never allowed to walk a whole disc.
    #[test]
    fn nothing_beyond_the_cap_is_ever_requested() {
        let mut prefix = Prefix::new(&[0u8; USER_DATA]);
        assert!(prefix.sector(usize::MAX).is_none());
        assert!(prefix.sector(MAX_PREFIX).is_none());
        assert!(prefix.needed().is_none_or(|n| n <= MAX_PREFIX));
    }
}
