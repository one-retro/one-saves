//! What can be wrong with a card image.

use core::fmt;

/// A card image that could not be read, or could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The image is not a card, in any of the containers one ships in.
    WrongLength(usize),
    /// The image does not carry the `NEO-GEO` signature.
    NotAMemoryCard,
    /// The image is structurally broken in a way that is not recoverable by guessing.
    Corrupt(String),
    /// The card states a size this format does not come in.
    ///
    /// Cards run 2 KiB to 16 KiB in 2 KiB steps, and the size is the card's own word rather than
    /// the length of the file it arrived in.
    UnknownSize(usize),
    /// The card's geometry does not agree with its own allocation table.
    ///
    /// How many blocks the header, directory and both FATs take is worked out from the card's
    /// size, and the table marks exactly those blocks reserved. A card where the two disagree is
    /// one this crate has the geometry wrong for, so it is refused rather than read through the
    /// wrong offsets.
    UnexpectedGeometry {
        /// How many blocks the computed layout reserves.
        computed: usize,
        /// How many the card's own table marks reserved.
        stated: usize,
    },
    /// The saves do not fit on a card.
    Full {
        /// How many blocks they need.
        needed: usize,
        /// How many blocks are free for saves.
        available: usize,
    },
    /// More saves than the directory has entries.
    TooManySaves {
        /// How many were offered.
        given: usize,
        /// How many the directory holds.
        available: usize,
    },
    /// A save with no bytes, which occupies no blocks and so cannot be filed.
    EmptySave(u16),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::WrongLength(len) => {
                write!(f, "{len} bytes is not a card image, bare or in a MiSTer save")
            }
            Error::NotAMemoryCard => f.write_str("this image does not carry the `NEO-GEO` signature"),
            Error::Corrupt(why) => write!(f, "this card is corrupt: {why}"),
            Error::UnknownSize(size) => {
                write!(f, "the card states a size of {size} bytes, which is not one cards come in")
            }
            Error::UnexpectedGeometry { computed, stated } => write!(
                f,
                "this card reserves {stated} blocks where a card its size reserves {computed}, so \
                 its layout is not the one this crate knows"
            ),
            Error::Full { needed, available } => {
                write!(f, "these saves need {needed} blocks and the card has {available}")
            }
            Error::TooManySaves { given, available } => {
                write!(f, "the directory holds {available} saves and this is {given}")
            }
            Error::EmptySave(ngh) => write!(f, "the save for NGH {ngh:04X} has no bytes"),
        }
    }
}

impl core::error::Error for Error {}

/// The result of reading or building a card.
pub type Result<T> = core::result::Result<T, Error>;
