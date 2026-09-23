//! What can be wrong with a backup RAM image.

use core::fmt;

/// A backup RAM that could not be read, or could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The image is not one of the lengths a backup RAM comes in.
    WrongLength(usize),
    /// The image does not open with `BackUpRam Format`.
    NotBackupRam,
    /// The image is structurally broken in a way that is not recoverable by guessing.
    Corrupt(String),
    /// The saves do not fit.
    Full {
        /// How many blocks they need.
        needed: usize,
        /// How many blocks are free for saves.
        available: usize,
    },
    /// A save with no bytes, which occupies no blocks and so cannot be filed.
    EmptySave(String),
    /// A save whose name is not one the console can hold.
    ///
    /// The field is eleven bytes of the console's own character set, and a name that does not fit
    /// is refused rather than truncated: a truncated name is a different save.
    BadName(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::WrongLength(len) => {
                write!(f, "{len} bytes is not a length backup RAM comes in")
            }
            Error::NotBackupRam => f.write_str("this image does not open with `BackUpRam Format`"),
            Error::Corrupt(why) => write!(f, "this backup RAM is corrupt: {why}"),
            Error::Full { needed, available } => {
                write!(f, "these saves need {needed} blocks and the volume has {available}")
            }
            Error::EmptySave(name) => write!(f, "the save `{name}` has no bytes"),
            Error::BadName(name) => {
                write!(f, "`{name}` is not a name a save can carry: eleven bytes at the most")
            }
        }
    }
}

impl core::error::Error for Error {}

/// The result of reading or building a backup RAM.
pub type Result<T> = core::result::Result<T, Error>;
