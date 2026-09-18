//! Reading a card, or the loose saves off one, out of an archive.
//!
//! Saves are shared as archives. Sometimes that is an archive of a card, and taking it out is all
//! there is to do. More often it is the saves themselves, one file each, because that is what a
//! site hosting them has: a card is one person's, and a save is the thing worth passing on.
//!
//! Both come back as a card, so nothing downstream has to know an archive was involved. The second
//! kind is **assembled** here — a card is built and the saves are filed onto it — which is worth
//! knowing about, because a card that was assembled never existed until now: it has the saves the
//! archive held and the free space is free because nothing has used it, not because a console
//! cleared it.
//!
//! Reading only. An archive is a way a save arrived, not a format this crate writes back.

use std::io::Read;

use crate::detect::detect;
use crate::error::{Error, Result};

/// What a zip opens with. Two more magics exist for the empty and spanned cases, and neither is an
/// archive with a card in it.
const MAGIC: &[u8] = b"PK\x03\x04";

/// How many members are looked at. An archive of saves holds tens, not thousands.
const MAX_MEMBERS: usize = 512;

/// The most any one member is read to, and the most they may come to together.
///
/// A PS2 card is 8 MiB, which is the largest thing expected in here by a wide margin. The cap is
/// what keeps a small archive claiming an enormous member from being decompressed on trust.
const MAX_MEMBER: u64 = 32 * 1024 * 1024;
const MAX_TOTAL: u64 = 64 * 1024 * 1024;

/// A card taken out of an archive, ready to be read as if the archive were not there.
#[derive(Debug, Clone)]
pub struct Unpacked {
    /// The card image.
    pub bytes: Vec<u8>,
    /// The extension to detect against, which for an assembled card is the format's own.
    pub extension: String,
    /// What inside the archive this came from, for a message that has to say.
    pub source: String,
    /// Whether the card was built here out of loose saves rather than found whole.
    pub assembled: bool,
}

/// Whether these bytes are an archive this can open.
#[must_use]
pub fn is_archive(bytes: &[u8]) -> bool {
    bytes.starts_with(MAGIC)
}

/// One file out of the archive.
struct Member {
    name: String,
    bytes: Vec<u8>,
}

impl Member {
    /// The part of the name after the last dot, which is what detection wants.
    fn extension(&self) -> &str {
        self.name.rsplit_once('.').map_or("", |(_, ext)| ext)
    }
}

/// Everything in the archive worth looking at.
fn members(bytes: &[u8]) -> Result<Vec<Member>> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| Error::Archive(format!("could not be opened: {e}")))?;

    let mut out = Vec::new();
    let mut total = 0u64;
    for index in 0..zip.len().min(MAX_MEMBERS) {
        let mut file = zip.by_index(index).map_err(|e| Error::Archive(format!("is broken: {e}")))?;
        if file.is_dir() {
            continue;
        }
        // `name` is the path as stored. Nothing here writes a file, so a member that climbs out of
        // the archive with `..` cannot reach anything; it is skipped anyway, because a name like
        // that is a sign of an archive built to be extracted carelessly rather than read.
        let name = file.name().to_owned();
        if name.split('/').any(|part| part == "..") || name.starts_with("__MACOSX/") {
            continue;
        }
        // The junk an archiver leaves behind, which is never a save.
        let bare = name.rsplit('/').next().unwrap_or(&name);
        if bare.starts_with('.') {
            continue;
        }

        let mut buffer = Vec::new();
        file.by_ref()
            .take(MAX_MEMBER.min(MAX_TOTAL - total))
            .read_to_end(&mut buffer)
            .map_err(|e| Error::Archive(format!("could not be read: {e}")))?;
        total += buffer.len() as u64;
        out.push(Member { name, bytes: buffer });
        if total >= MAX_TOTAL {
            break;
        }
    }
    Ok(out)
}

/// Reads a card out of an archive.
///
/// The archive holding one card image is the simple case. Holding loose saves instead, it is the
/// saves that come back, on a card built to carry them. Holding several cards, or nothing that
/// reads, it is an error that says which — guessing between two cards is not this function's
/// decision to make.
pub fn unpack(bytes: &[u8]) -> Result<Unpacked> {
    let members = members(bytes)?;
    if members.is_empty() {
        return Err(Error::Archive("holds nothing".into()));
    }

    // A card, found whole. Only a *card* counts: detection falls back to the extension and its
    // catch-all is a flat save, so a readme in the archive would otherwise look like a candidate.
    let cards: Vec<&Member> = members
        .iter()
        .filter(|member| {
            detect(&member.bytes, member.extension()).is_ok_and(|format| format.card_format().is_some())
        })
        .collect();
    if cards.len() > 1 {
        let names: Vec<&str> = cards.iter().map(|member| member.name.as_str()).collect();
        return Err(Error::Archive(format!("holds {} cards: {}", cards.len(), names.join(", "))));
    }
    if let Some(card) = cards.first() {
        return Ok(Unpacked {
            bytes: card.bytes.clone(),
            extension: card.extension().to_owned(),
            source: card.name.clone(),
            assembled: false,
        });
    }

    assemble(&members)
}

/// Builds a card out of the loose saves an archive held.
#[cfg(feature = "ps1")]
fn assemble(members: &[Member]) -> Result<Unpacked> {
    let saves: Vec<(&str, ps1_memcard::Save)> = members
        .iter()
        .filter_map(|member| Some((member.name.as_str(), ps1_memcard::read_single(&member.bytes)?)))
        .collect();
    if saves.is_empty() {
        return Err(Error::Archive(format!("holds no card and no save this reads: {}", listing(members))));
    }

    let mut builder = ps1_memcard::CardBuilder::new();
    for (_, save) in &saves {
        builder.add(save.clone());
    }
    let image = builder.build().map_err(|e| {
        Error::Archive(format!("holds {} saves, which do not fit on one card: {e}", saves.len()))
    })?;

    let names: Vec<&str> = saves.iter().map(|(name, _)| *name).collect();
    Ok(Unpacked {
        bytes: image,
        extension: crate::Format::Ps1Card.extension().to_owned(),
        source: names.join(", "),
        assembled: true,
    })
}

/// Without a format whose single saves this reads, an archive of them cannot be assembled.
#[cfg(not(feature = "ps1"))]
fn assemble(members: &[Member]) -> Result<Unpacked> {
    Err(Error::Archive(format!("holds no card: {}", listing(members))))
}

/// The member names, for an error that has to say what was in there.
fn listing(members: &[Member]) -> String {
    let mut names: Vec<&str> = members.iter().take(8).map(|member| member.name.as_str()).collect();
    if members.len() > names.len() {
        names.push("...");
    }
    names.join(", ")
}
