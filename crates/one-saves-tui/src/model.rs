//! What the interface is looking at, with no interface in it.
//!
//! A card is a [`Bundle`] whose shape is `card` and whose parts are one nested bundle per save, so
//! every operation here is a part operation: deleting a save removes a part, moving one between
//! cards moves a part, and writing back rebuilds the card's filesystem from what is left. None of
//! that is format-specific, which is the point — the same code moves a save between two VMUs and
//! between two PS2 cards, because the format already made them the same shape.
//!
//! Keeping it here rather than in the drawing means the rules can be tested without a terminal.

use std::path::{Path, PathBuf};

use one_saves::{Bundle, PartKind, ReverseDnsName};
use one_saves_convert::{CardOptions, Format, card, detect};

/// What went wrong, in terms a person reading a status line can act on.
#[derive(Debug)]
pub enum Error {
    /// The file could not be read or written.
    Io(std::io::Error),
    /// The bytes are not a card this tool knows.
    NotACard(String),
    /// A card could not be read, or could not be written back.
    Convert(one_saves_convert::Error),
    /// A save cannot go on this card, and why.
    Refused(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::NotACard(what) => write!(f, "not a memory card this tool reads: {what}"),
            Error::Convert(e) => write!(f, "{e}"),
            Error::Refused(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for Error {}

type Result<T> = std::result::Result<T, Error>;

/// One save on a card, as a list needs it.
pub struct Entry {
    /// Where it sits among the card's parts, which is what every operation names it by.
    pub index: usize,
    /// What the console calls it, where the card said. Falls back to the filename.
    pub title: String,
    /// What the console says about it beneath the title, where there is one.
    pub detail: Option<String>,
    /// How many of the card's blocks it occupies.
    pub blocks: u64,
    /// The save's icon, in display order. A still icon is one frame.
    pub icon: Vec<IconFrame>,
}

/// One frame of a save's icon.
pub struct IconFrame {
    /// The frame, as the PNG `x.1sav.icon` carries.
    pub png: Vec<u8>,
    /// How long it shows before the next, where the format says.
    ///
    /// A GameCube varies this per frame; a PlayStation keeps no timing of its own and leaves the
    /// rate to the console, which is why this is optional rather than defaulted at the source.
    pub hold_ms: Option<u64>,
}

/// A card, open and possibly edited.
pub struct Card {
    /// Where it came from, and where saving writes back to.
    pub path: PathBuf,
    /// Which layout it is, which decides how it is written back.
    pub format: Format,
    /// The card as a bundle: parts are saves.
    bundle: Bundle,
    /// Whether it has been edited since it was opened or last written.
    dirty: bool,
}

impl Card {
    /// Opens a card, detecting its layout from the bytes and the file's extension.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let bytes = std::fs::read(&path).map_err(Error::Io)?;
        let extension = path.extension().and_then(|e| e.to_str()).unwrap_or_default();
        let format = detect(&bytes, extension).map_err(|_| Error::NotACard(path.display().to_string()))?;
        let bundle = card::read(format, &bytes, &CardOptions::default()).map_err(Error::Convert)?;
        Ok(Self { path, format, bundle, dirty: false })
    }

    /// Whether the card has been edited since it was opened or last written.
    #[must_use]
    pub fn dirty(&self) -> bool {
        self.dirty
    }

    /// The card's data area in bytes, which is not the length of a dump of it.
    #[must_use]
    pub fn capacity(&self) -> u64 {
        self.bundle.header.card.as_ref().map_or(0, |card| card.capacity)
    }

    /// The size of the unit this card allocates a save in.
    ///
    /// Registry data rather than arithmetic here. `saturn-bup` is the one format whose number does
    /// not settle the question, and no Saturn card is read by this tool.
    #[must_use]
    pub fn block_size(&self) -> u64 {
        self.format
            .card_format()
            .and_then(one_saves_registry::card_format)
            .map_or(1, |format| format.block_size as u64)
    }

    /// How many blocks the card holds in total.
    #[must_use]
    pub fn blocks(&self) -> u64 {
        self.capacity() / self.block_size().max(1)
    }

    /// How many blocks the saves on it occupy.
    #[must_use]
    pub fn used_blocks(&self) -> u64 {
        self.entries().iter().map(|entry| entry.blocks).sum()
    }

    /// How many blocks are left.
    #[must_use]
    pub fn free_blocks(&self) -> u64 {
        self.blocks().saturating_sub(self.used_blocks())
    }

    /// The saves on the card, in the order the directory lists them.
    #[must_use]
    pub fn entries(&self) -> Vec<Entry> {
        let block = self.block_size().max(1);
        self.bundle
            .parts
            .iter()
            .enumerate()
            .filter(|(_, part)| part.kind == PartKind::Bundle)
            .map(|(index, part)| {
                let inner = part.bytes().ok().and_then(|b| Bundle::from_slice(&b).ok());
                let label = inner.as_ref().and_then(read_label);
                Entry {
                    index,
                    // What to call it, best first. A save need not have a title: a Neo Geo
                    // game writes one only if it feels like it, and Fatal Fury 2 writes a
                    // binary header instead. The serial still says which game it belongs to,
                    // which beats calling it by its position on the card.
                    title: label
                        .as_ref()
                        .map(|(title, _)| title.clone())
                        .or_else(|| part.path.clone())
                        .or_else(|| inner.as_ref().and_then(|save| save.header.game.as_ref()?.serial.clone()))
                        .unwrap_or_else(|| format!("save {index}")),
                    detail: label.and_then(|(_, detail)| detail),
                    blocks: inner.as_ref().map_or(0, |save| blocks_of(save, block)),
                    icon: inner.as_ref().map(read_icon).unwrap_or_default(),
                }
            })
            .collect()
    }

    /// Removes a save.
    pub fn remove(&mut self, index: usize) -> Result<()> {
        if index >= self.bundle.parts.len() {
            return Err(Error::Refused("no such save".into()));
        }
        self.bundle.parts.remove(index);
        self.edited();
        Ok(())
    }

    /// Marks the card changed, and drops anything the change made untrue.
    ///
    /// A `card-image` part is the card's bytes as they were — what an empty one carries instead of
    /// saves, and what a producer may keep beside them. The moment a save is added or taken away
    /// that picture is of a card that no longer exists, so carrying it on would be carrying a
    /// lie, and rebuilding from it would undo the edit.
    fn edited(&mut self) {
        self.bundle.parts.retain(|part| part.kind != PartKind::CardImage);
        self.renumber();
        self.dirty = true;
    }

    /// Copies a save from another card onto this one.
    ///
    /// Refused where it would not fit, and where the two cards are not for the same system: a
    /// PlayStation save on a VMU is not a thing a console could read, whatever the bytes say.
    pub fn copy_from(&mut self, source: &Card, index: usize) -> Result<()> {
        let part = source.bundle.parts.get(index).ok_or_else(|| Error::Refused("no such save".into()))?;

        if self.format.system() != source.format.system() {
            return Err(Error::Refused(format!(
                "a {} save does not go on a {}",
                source.format.label(),
                self.format.label()
            )));
        }
        let blocks = part.payload.len().div_ceil(self.block_size().max(1));
        if blocks > self.free_blocks() {
            return Err(Error::Refused(format!("needs {blocks} blocks and {} are free", self.free_blocks())));
        }

        self.bundle.parts.push(part.clone());
        self.edited();
        Ok(())
    }

    /// Rebuilds the card and writes it back, leaving the original in place until it succeeds.
    pub fn save(&mut self) -> Result<()> {
        let bytes = card::write(self.format, &self.bundle).map_err(Error::Convert)?;

        // Read the rebuilt card before it replaces anything. This catches a card that came out
        // corrupt; it cannot catch a writer and a reader sharing a wrong assumption, which is what
        // the vendored cards in the test suite are for.
        let check = card::read(self.format, &bytes, &CardOptions::default())
            .map_err(|e| Error::Refused(format!("the rebuilt card does not read back: {e}")))?;
        // Saves, not parts: a card may carry an image of itself beside them, and whether the
        // writer reproduces one is its business rather than evidence that a save went missing.
        let saves =
            |bundle: &Bundle| bundle.parts.iter().filter(|part| part.kind == PartKind::Bundle).count();
        if saves(&check) != saves(&self.bundle) {
            return Err(Error::Refused(format!(
                "the rebuilt card holds {} of {} saves",
                saves(&check),
                saves(&self.bundle)
            )));
        }

        let temporary = self.path.with_extension("1cards-new");
        std::fs::write(&temporary, &bytes).map_err(Error::Io)?;
        std::fs::rename(&temporary, &self.path).map_err(Error::Io)?;
        self.dirty = false;
        Ok(())
    }

    /// Parts address themselves by `id`, so removing one renumbers the rest.
    fn renumber(&mut self) {
        for (index, part) in self.bundle.parts.iter_mut().enumerate() {
            part.id = index as u64;
        }
    }
}

/// How many of a card's blocks a save occupies.
///
/// Counted over what the save actually holds rather than over the part carrying it: a `bundle`
/// part's payload is the save *encoded*, header and all, which rounds up to one block more than
/// the save takes. A card allocates each of a save's files its own whole block, so the count is
/// per file rather than over their total.
///
/// A PS2 save also spends a block on the directory naming it, which this does not add: it is the
/// card's overhead rather than the save's, and a consumer comparing a save against the space it
/// would need somewhere else wants what the save is, not what a particular card spends on it.
fn blocks_of(save: &Bundle, block: u64) -> u64 {
    save.parts.iter().map(|part| part.payload.len().div_ceil(block)).sum()
}

/// The title and detail a save calls itself, from `x.1sav.label`.
fn read_label(save: &Bundle) -> Option<(String, Option<String>)> {
    let key = ReverseDnsName::parse("x.1sav.label").ok()?;
    let map = save.header.extensions.get(&key)?.as_map()?;
    let title = map.get::<u64, one_saves::dcbor::CBOR>(0)?.as_text()?.to_owned();
    let detail =
        map.get::<u64, one_saves::dcbor::CBOR>(1).and_then(|value| value.as_text().map(ToOwned::to_owned));
    Some((title, detail))
}

/// The save's icon frames, from `x.1sav.icon`, in display order.
fn read_icon(save: &Bundle) -> Vec<IconFrame> {
    let Ok(key) = ReverseDnsName::parse("x.1sav.icon") else { return Vec::new() };
    let Some(map) = save.header.extensions.get(&key).and_then(|value| value.as_map()) else {
        return Vec::new();
    };
    let Some(frames) = map.get::<u64, one_saves::dcbor::CBOR>(0) else { return Vec::new() };
    let Some(frames) = frames.as_array() else { return Vec::new() };
    frames
        .iter()
        .filter_map(|frame| {
            let frame = frame.as_array()?;
            Some(IconFrame {
                png: frame.first()?.as_byte_string()?.to_vec(),
                hold_ms: frame.get(1).and_then(|hold| u64::try_from(hold.clone()).ok()),
            })
        })
        .collect()
}
