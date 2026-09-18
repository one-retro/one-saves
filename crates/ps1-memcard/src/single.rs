//! Reading one save on its own, outside a card.

use crate::{BLOCK, FRAME, Save, state};

/// The `.psv` header, which is the same shape for a PS1 save and a PS2 one.
mod psv {
    /// Byte 0 is zero and the rest spells the format, so the magic is checked from byte 1.
    pub(super) const MAGIC: &[u8] = b"VSP";
    /// Which console wrote it: 1 for PS1, 2 for PS2.
    pub(super) const KIND: usize = 0x3C;
    pub(super) const PS1: u32 = 1;
    /// How many bytes of save follow, and how far in they start.
    pub(super) const LEN: usize = 0x40;
    pub(super) const OFFSET: usize = 0x44;
    /// The directory name, NUL-padded. This is the save's real name, which the *filename* an
    /// exporter chose is not: those hex-escape anything outside `[A-Za-z0-9]`, so the save the
    /// console calls `BASLUS-01363-00002` arrives as `BASLUS-013632D3030303032.PSV`.
    pub(super) const NAME: usize = 0x64;
    pub(super) const NAME_LEN: usize = 20;
}

/// Reads a save that arrived on its own rather than on a card.
///
/// Two containers, both of them one save and its name:
///
/// - `.psv`, what a PS3 or a Vita exports. A 132-byte header, of which what matters is the kind,
///   the length, the offset and the name; the rest is a signature over the save, which is checked
///   by the console it is going back to and not by this crate.
/// - `.mcs`, what most PC tools write. The save's 128-byte directory entry verbatim, then its
///   blocks — so unlike a `.psv` it carries the whole entry, and this keeps it.
///
/// `None` for anything else, including a `.psv` holding a PS2 save: that one is a directory of
/// files rather than a run of blocks, and reading it as blocks would produce a plausible-looking
/// save made of the wrong bytes.
#[must_use]
pub fn read_single(bytes: &[u8]) -> Option<Save> {
    read_psv(bytes).or_else(|| read_mcs(bytes))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// A name out of a fixed field, cut at the first NUL.
fn name_at(bytes: &[u8], at: usize, len: usize) -> Option<String> {
    let field = bytes.get(at..at + len)?;
    let text = &field[..field.iter().position(|&b| b == 0).unwrap_or(field.len())];
    // A name is ASCII; anything else means this is not the field it was taken for.
    text.is_ascii().then(|| String::from_utf8_lossy(text).into_owned())
}

fn read_psv(bytes: &[u8]) -> Option<Save> {
    if bytes.get(1..4)? != psv::MAGIC || u32_at(bytes, psv::KIND)? != psv::PS1 {
        return None;
    }
    let len = usize::try_from(u32_at(bytes, psv::LEN)?).ok()?;
    let offset = usize::try_from(u32_at(bytes, psv::OFFSET)?).ok()?;
    let data = bytes.get(offset..offset.checked_add(len)?)?;
    if data.is_empty() || !data.len().is_multiple_of(BLOCK) {
        return None;
    }
    // A `.psv` carries the name and no other part of the directory entry, so the entry stays
    // empty and a builder writes one from the name.
    Some(Save {
        slot: 0,
        name: name_at(bytes, psv::NAME, psv::NAME_LEN)?,
        dirent: Vec::new(),
        data: data.to_vec(),
    })
}

fn read_mcs(bytes: &[u8]) -> Option<Save> {
    if bytes.len() <= FRAME || !(bytes.len() - FRAME).is_multiple_of(BLOCK) {
        return None;
    }
    let (dirent, data) = bytes.split_at(FRAME);
    // The entry is the save's first, which is the only one a single-save file can hold.
    if u32_at(dirent, 0)? != state::FIRST {
        return None;
    }
    Some(Save {
        slot: 0,
        name: name_at(dirent, 10, psv::NAME_LEN)?,
        dirent: dirent.to_vec(),
        data: data.to_vec(),
    })
}
