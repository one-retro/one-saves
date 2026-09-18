//! What can go wrong converting a save.

use core::fmt;

/// A conversion that could not be done.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The input is not the length this format comes in.
    WrongLength {
        /// What the file is being read as.
        format: &'static str,
        /// The lengths that would have been acceptable.
        expected: &'static str,
        /// The length the file actually is.
        found: usize,
    },
    /// The input does not carry the signature this format opens with.
    NotThisFormat {
        /// What the file was being read as.
        format: &'static str,
        /// Why it was rejected.
        why: String,
    },
    /// The input is structurally broken in a way that is not recoverable by guessing.
    Corrupt {
        /// What the file is being read as.
        format: &'static str,
        /// Where and how it is broken.
        why: String,
    },
    /// The format could not be worked out from the extension or the bytes.
    Undetected,
    /// A format this build knows the name of but cannot write.
    ///
    /// A consumer that does not know a card format **must not** attempt the write: rebuilding a
    /// card it cannot lay out would corrupt it.
    CannotWrite(String),
    /// A card format this build was not compiled with.
    ///
    /// Every card format is behind a feature. One that is off leaves the format nameable — so a
    /// file is still identified rather than misread — and takes the reader and writer with it.
    Unsupported {
        /// What the format is called.
        format: &'static str,
        /// The feature that would have supplied it.
        feature: &'static str,
    },
    /// The bundle does not hold what this format needs to be rebuilt from.
    NotConvertible(String),
    /// The bundle itself is malformed.
    Bundle(one_saves::Error),
    /// An archive held nothing this could read, or more than one thing it could.
    Archive(String),
    /// Reading or writing a file failed.
    Io(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::WrongLength { format, expected, found } => {
                write!(f, "a {format} is {expected}, but this file is {found} bytes")
            }
            Error::NotThisFormat { format, why } => write!(f, "not a {format}: {why}"),
            Error::Corrupt { format, why } => write!(f, "this {format} is corrupt: {why}"),
            Error::Undetected => f.write_str("could not tell what this file is; name the format with --from"),
            Error::Archive(why) => write!(f, "this archive {why}"),
            Error::CannotWrite(format) => write!(
                f,
                "this build cannot write a {format}, and writing a card format it does not know \
                 would corrupt it"
            ),
            Error::Unsupported { format, feature } => write!(
                f,
                "this is a {format}, and this build has the `{feature}` feature off, so it can \
                 name the format and not read or write it"
            ),
            Error::NotConvertible(why) => write!(f, "cannot convert this bundle: {why}"),
            Error::Bundle(source) => write!(f, "{source}"),
            Error::Io(source) => write!(f, "{source}"),
        }
    }
}

impl core::error::Error for Error {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Error::Bundle(source) => Some(source),
            Error::Io(source) => Some(source),
            _ => None,
        }
    }
}

impl From<one_saves::Error> for Error {
    fn from(source: one_saves::Error) -> Self {
        Error::Bundle(source)
    }
}

impl From<std::io::Error> for Error {
    fn from(source: std::io::Error) -> Self {
        Error::Io(source)
    }
}

/// The result of a conversion.
pub type Result<T> = core::result::Result<T, Error>;
