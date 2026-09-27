//! Narrow error type — one variant per failure shape.

use std::fmt;
use std::io;

/// Everything that can go wrong reading or writing a VHD.
///
/// `#[non_exhaustive]`, SO THE NEXT VARIANT IS NOT A BREAKING CHANGE. A caller
/// must carry a wildcard arm, and gains variants as this grows rather than
/// failing to compile. Adding the attribute is itself breaking, which is why it
/// lands in the 0.4.0 bump alongside `ReadOnly` gaining its payload rather than
/// later as a patch: doing it later would repeat the problem it removes.
///
/// The sibling `rust-img-vhdx` did the same for the same reason
/// (rust-img-vhdx#63), after three variants were added across one minor line
/// and each was a break the changelog had to be corrected for.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    Io(io::Error),
    /// File is missing the VHD footer or the cookie doesn't match.
    NotVhd,
    /// Footer or dynamic-header checksum mismatch.
    BadChecksum {
        expected: u32,
        found: u32,
        what: &'static str,
    },
    /// Disk type byte outside the known {2 fixed, 3 dynamic, 4 differencing} set.
    UnsupportedDiskType(u32),
    /// Header field combination is internally inconsistent.
    Corrupt(&'static str),
    /// A feature the reader doesn't yet handle (e.g. parent-locator paths
    /// pointing at non-file-relative sources).
    Unsupported(&'static str),
    /// Read past the end of the virtual disk.
    OutOfBounds {
        offset: u64,
        len: u64,
        size: u64,
    },
    /// Differencing-chain depth exceeded.
    ParentTooDeep,
    /// Differencing parent could not be located via any locator.
    ParentNotFound(String),
    /// A write, or a read-write open, that cannot be honoured. The payload
    /// names which of the causes it was, because they point at different
    /// objects: the reader was opened read-only, the image's subtype has
    /// no write path yet (differencing), or the caller's backing device is
    /// not writable -- the only one of the three a caller can fix, and the
    /// only one `open_rw_on_device` refuses at open time.
    ReadOnly(&'static str),
    /// Backing-device error not otherwise classified (e.g. a custom
    /// `fs_core::Error::Custom` from a callback-backed device).
    Custom(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "io: {e}"),
            Error::NotVhd => write!(f, "not a VHD image (missing or invalid footer)"),
            Error::BadChecksum {
                expected,
                found,
                what,
            } => {
                write!(
                    f,
                    "{what} checksum mismatch: expected {expected:#x}, found {found:#x}"
                )
            }
            Error::UnsupportedDiskType(t) => write!(f, "unsupported VHD disk type: {t}"),
            Error::Corrupt(s) => write!(f, "corrupt VHD: {s}"),
            Error::Unsupported(s) => write!(f, "unsupported VHD feature: {s}"),
            Error::OutOfBounds { offset, len, size } => {
                write!(
                    f,
                    "read [{offset}, {offset}+{len}) past virtual size {size}"
                )
            }
            Error::ParentTooDeep => write!(f, "differencing chain too deep (cycle?)"),
            Error::ParentNotFound(s) => write!(f, "differencing parent not found: {s}"),
            Error::ReadOnly(why) => write!(f, "VHD is read-only: {why}"),
            Error::Custom(s) => write!(f, "{s}"),
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
