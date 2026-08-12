//! What can be wrong with a card image.

use core::fmt;

/// A card image that could not be read, or could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The image is shorter than a superblock, or is not a whole number of pages.
    TooShort(usize),
    /// The image does not open with the memory card magic.
    NotAMemoryCard,
    /// The image is structurally broken in a way that is not recoverable by guessing.
    Corrupt(String),
    /// The saves do not fit on a card this size.
    Full {
        /// How many clusters they need.
        needed: usize,
        /// How many the card has.
        available: usize,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::TooShort(len) => write!(f, "{len} bytes is not a whole memory card image"),
            Error::NotAMemoryCard => write!(
                f,
                "this image does not open with {:?}",
                core::str::from_utf8(crate::MAGIC).unwrap_or("the card magic")
            ),
            Error::Corrupt(why) => write!(f, "this card is corrupt: {why}"),
            Error::Full { needed, available } => {
                write!(f, "these saves need {needed} clusters and the card has {available}")
            }
        }
    }
}

impl core::error::Error for Error {}

/// The result of reading or building a card.
pub type Result<T> = core::result::Result<T, Error>;
