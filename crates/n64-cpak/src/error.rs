//! What can be wrong with a pak image.

use core::fmt;

/// A pak image that could not be read, or could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The image is not the length a pak comes in.
    WrongLength(usize),
    /// Neither the index table nor its backup is readable, so nothing can be followed.
    NotAControllerPak,
    /// The image is structurally broken in a way that is not recoverable by guessing.
    Corrupt(String),
    /// The notes do not fit on a pak.
    Full {
        /// How many data pages they need.
        needed: usize,
        /// How many data pages a pak has.
        available: usize,
    },
    /// More notes than the note table has entries.
    TooManyNotes {
        /// How many were offered.
        given: usize,
        /// How many a pak holds.
        available: usize,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::WrongLength(len) => {
                write!(f, "a Controller Pak is {} bytes and this is {len}", crate::CAPACITY)
            }
            Error::NotAControllerPak => {
                f.write_str("neither the index table nor its backup reads as a chain")
            }
            Error::Corrupt(why) => write!(f, "this pak is corrupt: {why}"),
            Error::Full { needed, available } => {
                write!(f, "these notes need {needed} pages and a pak has {available}")
            }
            Error::TooManyNotes { given, available } => {
                write!(f, "a pak holds {available} notes and this is {given}")
            }
        }
    }
}

impl core::error::Error for Error {}

/// The result of reading or building a pak.
pub type Result<T> = core::result::Result<T, Error>;
