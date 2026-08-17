//! What can be wrong with a VMU image.

use core::fmt;

/// A VMU image that could not be read, or could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The image is not the length a VMU comes in.
    WrongLength(usize),
    /// The root block does not carry the format mark, so the VMU is not formatted.
    NotFormatted,
    /// The image is structurally broken in a way that is not recoverable by guessing.
    Corrupt(String),
    /// The saves do not fit on a VMU.
    Full {
        /// How many blocks they need.
        needed: usize,
        /// How many blocks a VMU has for saves.
        available: usize,
    },
    /// More saves than the directory has entries.
    TooManySaves {
        /// How many were offered.
        given: usize,
        /// How many a VMU holds.
        available: usize,
    },
    /// A save with no bytes, which occupies no blocks and so cannot be filed.
    EmptySave(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::WrongLength(len) => {
                write!(f, "a VMU image is {} bytes and this is {len}", crate::IMAGE_LEN)
            }
            Error::NotFormatted => f.write_str(
                "the root block does not open with sixteen 0x55 bytes, so this VMU is unformatted",
            ),
            Error::Corrupt(why) => write!(f, "this VMU is corrupt: {why}"),
            Error::Full { needed, available } => {
                write!(f, "these saves need {needed} blocks and a VMU has {available}")
            }
            Error::TooManySaves { given, available } => {
                write!(f, "a VMU holds {available} saves and this is {given}")
            }
            Error::EmptySave(name) => write!(f, "the save {name:?} has no bytes"),
        }
    }
}

impl core::error::Error for Error {}

/// The result of reading or building a VMU.
pub type Result<T> = core::result::Result<T, Error>;
