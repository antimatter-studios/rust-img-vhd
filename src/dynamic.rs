//! VHD dynamic header (1024 bytes; located at footer.data_offset, which
//! is typically 512). Present only for dynamic / differencing VHDs.
//!
//! Layout (offsets within the 1024-byte header):
//!
//! ```text
//!    0   8   cookie ("cxsparse")
//!    8   8   data_offset           (next header in chain; 0xFFFF... if none)
//!   16   8   table_offset          (host offset of the BAT)
//!   24   4   header_version        (0x00010000 = 1.0)
//!   28   4   max_table_entries     (BAT length, in u32 entries)
//!   32   4   block_size            (typically 0x200000 = 2 MiB)
//!   36   4   checksum
//!   40  16   parent_unique_id      (only for differencing)
//!   56   4   parent_timestamp
//!   60   4   reserved
//!   64 512   parent_unicode_name   (UTF-16 BE, zero-padded)
//!  576 192   parent_locator_entries  (eight 24-byte structs)
//!  768 256   reserved (zero)
//! ```
//!
//! All multi-byte integers are big-endian.

use crate::error::{Error, Result};

pub const DYN_HEADER_SIZE: usize = 1024;
pub const DYN_HEADER_COOKIE: &[u8; 8] = b"cxsparse";
pub const BAT_UNALLOCATED: u32 = 0xFFFF_FFFF;

#[derive(Debug, Clone)]
pub struct DynamicHeader {
    pub data_offset: u64,
    pub table_offset: u64,
    pub header_version: u32,
    pub max_table_entries: u32,
    pub block_size: u32,
    pub parent_unique_id: [u8; 16],
    pub parent_timestamp: u32,
    /// Decoded parent name from the UTF-16 BE field; trailing zeros
    /// stripped. Empty for non-differencing VHDs.
    pub parent_name: String,
    /// Eight raw locator entries. Decoding the locator data into a
    /// usable path goes through `ParentLocator::resolve`.
    pub parent_locators: [ParentLocator; 8],
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ParentLocator {
    pub platform_code: [u8; 4],
    pub platform_data_space: u32,
    pub platform_data_length: u32,
    pub reserved: u32,
    pub platform_data_offset: u64,
}

impl DynamicHeader {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < DYN_HEADER_SIZE {
            return Err(Error::Corrupt("dynamic header shorter than 1024 bytes"));
        }
        if &bytes[0..8] != DYN_HEADER_COOKIE {
            return Err(Error::Corrupt("dynamic header bad cookie"));
        }

        let stored_checksum = read_u32(bytes, 36);
        let computed_checksum = compute_checksum(bytes);
        if stored_checksum != computed_checksum {
            return Err(Error::BadChecksum {
                expected: stored_checksum,
                found: computed_checksum,
                what: "dynamic-header",
            });
        }

        let data_offset = read_u64(bytes, 8);
        let table_offset = read_u64(bytes, 16);
        let header_version = read_u32(bytes, 24);
        let max_table_entries = read_u32(bytes, 28);
        let block_size = read_u32(bytes, 32);

        if !block_size.is_power_of_two() {
            return Err(Error::Corrupt("block_size is not a power of two"));
        }
        if block_size < 512 {
            return Err(Error::Corrupt("block_size < 512"));
        }

        let mut parent_unique_id = [0u8; 16];
        parent_unique_id.copy_from_slice(&bytes[40..56]);
        let parent_timestamp = read_u32(bytes, 56);

        let parent_name = decode_utf16_be(&bytes[64..64 + 512]);

        let mut parent_locators = [ParentLocator::default(); 8];
        for (i, slot) in parent_locators.iter_mut().enumerate() {
            let off = 576 + i * 24;
            slot.platform_code.copy_from_slice(&bytes[off..off + 4]);
            slot.platform_data_space = read_u32(bytes, off + 4);
            slot.platform_data_length = read_u32(bytes, off + 8);
            slot.reserved = read_u32(bytes, off + 12);
            slot.platform_data_offset = read_u64(bytes, off + 16);
        }

        Ok(DynamicHeader {
            data_offset,
            table_offset,
            header_version,
            max_table_entries,
            block_size,
            parent_unique_id,
            parent_timestamp,
            parent_name,
            parent_locators,
        })
    }

    /// Bytes per block bitmap, padded up to a sector multiple. With
    /// 512-byte sectors and the typical 2-MiB block size: 4096
    /// sectors / block → 4096 / 8 = 512 bytes = 1 sector.
    pub fn bitmap_size_bytes(&self) -> u64 {
        let sectors = self.block_size as u64 / 512;
        let bits = sectors;
        let bytes = bits.div_ceil(8);
        // Round up to a 512-byte sector.
        bytes.div_ceil(512) * 512
    }
}

/// One's complement of the u32 sum, with the checksum field (36..40)
/// zeroed during compute. Same algorithm as the footer.
pub fn compute_checksum(header_bytes: &[u8]) -> u32 {
    let mut sum: u32 = 0;
    for (i, b) in header_bytes.iter().enumerate().take(DYN_HEADER_SIZE) {
        if (36..40).contains(&i) {
            continue;
        }
        sum = sum.wrapping_add(*b as u32);
    }
    !sum
}

fn decode_utf16_be(bytes: &[u8]) -> String {
    let mut units = Vec::with_capacity(bytes.len() / 2);
    for c in bytes.chunks_exact(2) {
        let u = u16::from_be_bytes([c[0], c[1]]);
        if u == 0 {
            break;
        }
        units.push(u);
    }
    String::from_utf16(&units).unwrap_or_default()
}

fn read_u32(b: &[u8], off: usize) -> u32 {
    u32::from_be_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

fn read_u64(b: &[u8], off: usize) -> u64 {
    u64::from_be_bytes([
        b[off],
        b[off + 1],
        b[off + 2],
        b[off + 3],
        b[off + 4],
        b[off + 5],
        b[off + 6],
        b[off + 7],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_round_trip_for_minimal_dynamic_header() {
        let mut h = [0u8; 1024];
        h[0..8].copy_from_slice(DYN_HEADER_COOKIE);
        h[8..16].copy_from_slice(&u64::MAX.to_be_bytes()); // no next
        h[16..24].copy_from_slice(&3072u64.to_be_bytes()); // BAT @ 6 sectors
        h[24..28].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        h[28..32].copy_from_slice(&4u32.to_be_bytes()); // max table entries
        h[32..36].copy_from_slice(&0x0010_0000u32.to_be_bytes()); // block_size = 1 MB
        let cs = compute_checksum(&h);
        h[36..40].copy_from_slice(&cs.to_be_bytes());

        let parsed = DynamicHeader::parse(&h).unwrap();
        assert_eq!(parsed.block_size, 0x0010_0000);
        assert_eq!(parsed.max_table_entries, 4);
        assert_eq!(parsed.bitmap_size_bytes(), 512);
    }
}
