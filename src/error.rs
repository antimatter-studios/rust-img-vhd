//! Narrow error type — one variant per failure shape.

use std::fmt;
use std::io;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    /// File is missing the VHD footer or the cookie doesn't match.
    NotVhd,
    /// Footer or dynamic-header checksum mismatch.
    BadChecksum { expected: u32, found: u32, what: &'static str },
    /// Disk type byte outside the known {2 fixed, 3 dynamic, 4 differencing} set.
    UnsupportedDiskType(u32),
    /// Header field combination is internally inconsistent.
    Corrupt(&'static str),
    /// A feature the reader doesn't yet handle (e.g. parent-locator paths
    /// pointing at non-file-relative sources).
    Unsupported(&'static str),
    /// Read past the end of the virtual disk.
    OutOfBounds { offset: u64, len: u64, size: u64 },
    /// Differencing-chain depth exceeded.
    ParentTooDeep,
    /// Differencing parent could not be located via any locator.
    ParentNotFound(String),
    /// Write attempted on a reader opened read-only, or on a subtype that
    /// hasn't grown a write path yet (dynamic / differencing).
    ReadOnly,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "io: {e}"),
            Error::NotVhd => write!(f, "not a VHD image (missing or invalid footer)"),
            Error::BadChecksum { expected, found, what } => {
                write!(f, "{what} checksum mismatch: expected {expected:#x}, found {found:#x}")
            }
            Error::UnsupportedDiskType(t) => write!(f, "unsupported VHD disk type: {t}"),
            Error::Corrupt(s) => write!(f, "corrupt VHD: {s}"),
            Error::Unsupported(s) => write!(f, "unsupported VHD feature: {s}"),
            Error::OutOfBounds { offset, len, size } => {
                write!(f, "read [{offset}, {offset}+{len}) past virtual size {size}")
            }
            Error::ParentTooDeep => write!(f, "differencing chain too deep (cycle?)"),
            Error::ParentNotFound(s) => write!(f, "differencing parent not found: {s}"),
            Error::ReadOnly => write!(f, "VHD is read-only (opened RO, or write path not yet implemented for this subtype)"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
