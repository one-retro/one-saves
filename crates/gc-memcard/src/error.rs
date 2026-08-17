//! What can be wrong with a card image.

use core::fmt;

/// A card image that could not be read, or could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The image is not a whole number of blocks, or not a size the console formats.
    WrongLength(usize),
    /// Neither the directory nor its mirror checksums, so the card cannot be read.
    NotAMemoryCard,
    /// The image is structurally broken in a way that is not recoverable by guessing.
    Corrupt(String),
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
        /// How many a card holds.
        available: usize,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::WrongLength(len) => write!(
                f,
                "a card is a whole number of {}-byte blocks in a size the console formats, and this is {len}",
                crate::BLOCK
            ),
            Error::NotAMemoryCard => f.write_str("neither the directory nor its mirror checksums"),
            Error::Corrupt(why) => write!(f, "this card is corrupt: {why}"),
            Error::Full { needed, available } => {
                write!(f, "these saves need {needed} blocks and the card has {available}")
            }
            Error::TooManySaves { given, available } => {
                write!(f, "a card holds {available} saves and this is {given}")
            }
        }
    }
}

impl core::error::Error for Error {}

/// The result of reading or building a card.
pub type Result<T> = core::result::Result<T, Error>;
