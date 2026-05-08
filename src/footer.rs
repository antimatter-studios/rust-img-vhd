//! VHD footer (512 bytes at end of file; for dynamic/differencing, also
//! mirrored at offset 0).
//!
//! Layout (offsets within the 512-byte footer):
//!
//! ```text
//!   0   8   cookie ("conectix")
//!   8   4   features
//!  12   4   file_format_version  (0x00010000 = 1.0)
//!  16   8   data_offset          (0xFFFF... for fixed; dynamic header
//!                                 host offset for dynamic/differencing)
//!  24   4   timestamp            (seconds since 2000-01-01T00:00:00)
//!  28   4   creator_application
//!  32   4   creator_version
//!  36   4   creator_host_os
//!  40   8   original_size        (virtual disk size in bytes)
//!  48   8   current_size         (virtual disk size in bytes)
//!  56   4   disk_geometry        (CHS encoded: u16 cyls, u8 heads, u8 spt)
//!  60   4   disk_type            (2=fixed, 3=dynamic, 4=differencing)
//!  64   4   checksum             (one's complement sum, with checksum
//!                                 field zeroed during compute)
//!  68  16   unique_id
//!  84   1   saved_state
//!  85 427   reserved (zero)
//! ```
//!
//! All multi-byte integers are big-endian.

use crate::error::{Error, Result};

pub const FOOTER_SIZE: usize = 512;
pub const FOOTER_COOKIE: &[u8; 8] = b"conectix";

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskType {
    Fixed = 2,
    Dynamic = 3,
    Differencing = 4,
}

impl DiskType {
    pub fn from_u32(v: u32) -> Result<Self> {
        match v {
            2 => Ok(DiskType::Fixed),
            3 => Ok(DiskType::Dynamic),
            4 => Ok(DiskType::Differencing),
            other => Err(Error::UnsupportedDiskType(other)),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Footer {
    pub features: u32,
    pub file_format_version: u32,
    pub data_offset: u64,
    pub original_size: u64,
    pub current_size: u64,
    pub disk_type: DiskType,
    pub unique_id: [u8; 16],
}

impl Footer {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < FOOTER_SIZE {
            return Err(Error::Corrupt("footer shorter than 512 bytes"));
        }
        if &bytes[0..8] != FOOTER_COOKIE {
            return Err(Error::NotVhd);
        }

        let stored_checksum = read_u32(bytes, 64);
        let computed_checksum = compute_checksum(bytes);
        if stored_checksum != computed_checksum {
            return Err(Error::BadChecksum {
                expected: stored_checksum,
                found: computed_checksum,
                what: "footer",
            });
        }

        let features = read_u32(bytes, 8);
        let file_format_version = read_u32(bytes, 12);
        let data_offset = read_u64(bytes, 16);
        let original_size = read_u64(bytes, 40);
        let current_size = read_u64(bytes, 48);
        let disk_type = DiskType::from_u32(read_u32(bytes, 60))?;

        let mut unique_id = [0u8; 16];
        unique_id.copy_from_slice(&bytes[68..84]);

        Ok(Footer {
            features,
            file_format_version,
            data_offset,
            original_size,
            current_size,
            disk_type,
            unique_id,
        })
    }
}

/// One's complement of the u32 sum of all bytes treated as u8, with the
/// checksum field (bytes 64..68) zeroed during computation.
pub fn compute_checksum(footer_bytes: &[u8]) -> u32 {
    let mut sum: u32 = 0;
    for (i, b) in footer_bytes.iter().enumerate().take(FOOTER_SIZE) {
        if (64..68).contains(&i) {
            continue;
        }
        sum = sum.wrapping_add(*b as u32);
    }
    !sum
}

fn read_u32(b: &[u8], off: usize) -> u32 {
    u32::from_be_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

fn read_u64(b: &[u8], off: usize) -> u64 {
    u64::from_be_bytes([
        b[off], b[off + 1], b[off + 2], b[off + 3],
        b[off + 4], b[off + 5], b[off + 6], b[off + 7],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_cookie() {
        let bytes = [0u8; 512];
        assert!(matches!(Footer::parse(&bytes), Err(Error::NotVhd)));
    }

    #[test]
    fn checksum_round_trip_for_minimal_fixed_footer() {
        let mut f = [0u8; 512];
        f[0..8].copy_from_slice(FOOTER_COOKIE);
        f[8..12].copy_from_slice(&0x0000_0002u32.to_be_bytes());
        f[12..16].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        f[16..24].copy_from_slice(&u64::MAX.to_be_bytes()); // fixed
        f[40..48].copy_from_slice(&(8u64 * 1024 * 1024).to_be_bytes());
        f[48..56].copy_from_slice(&(8u64 * 1024 * 1024).to_be_bytes());
        f[60..64].copy_from_slice(&2u32.to_be_bytes()); // disk_type fixed
        let cs = compute_checksum(&f);
        f[64..68].copy_from_slice(&cs.to_be_bytes());

        let parsed = Footer::parse(&f).unwrap();
        assert_eq!(parsed.disk_type, DiskType::Fixed);
        assert_eq!(parsed.current_size, 8 * 1024 * 1024);
    }
}
