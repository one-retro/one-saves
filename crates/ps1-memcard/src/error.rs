//! What can be wrong with a card image.

use core::fmt;

/// A card image that could not be read, or could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The image is not a card, in any of the containers a card ships in.
    WrongLength(usize),
    /// The image is the right length and does not open with `MC`.
    NotAMemoryCard,
    /// The image is structurally broken in a way that is not recoverable by guessing.
    Corrupt(String),
    /// The saves do not fit on a card.
    Full {
        /// How many blocks they need.
        needed: usize,
        /// How many blocks a card has for saves.
        available: usize,
    },
    /// A save with no bytes, which occupies no blocks and so cannot be filed.
    EmptySave(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::WrongLength(len) => write!(
                f,
                "a card is {} bytes, or that behind a DexDrive, VGS or PSP header, and this is {len}",
                crate::CAPACITY
            ),
            Error::NotAMemoryCard => f.write_str("the header block does not open with \"MC\""),
            Error::Corrupt(why) => write!(f, "this card is corrupt: {why}"),
            Error::Full { needed, available } => {
                write!(f, "these saves need {needed} blocks and a card has {available}")
            }
            Error::EmptySave(name) => write!(f, "the save {name:?} has no bytes"),
        }
    }
}

impl core::error::Error for Error {}

/// The result of reading or building a card.
pub type Result<T> = core::result::Result<T, Error>;
