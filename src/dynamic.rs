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
use crate::format::dynamic_offsets as at;
use crate::format::{ones_complement_checksum, read_u32, read_u64, SECTOR_SIZE};

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
        if &bytes[at::COOKIE] != DYN_HEADER_COOKIE {
            return Err(Error::Corrupt("dynamic header bad cookie"));
        }

        let stored_checksum = read_u32(bytes, at::CHECKSUM.start);
        let computed_checksum = compute_checksum(bytes);
        if stored_checksum != computed_checksum {
            return Err(Error::BadChecksum {
                expected: stored_checksum,
                found: computed_checksum,
                what: "dynamic-header",
            });
        }

        let data_offset = read_u64(bytes, at::DATA_OFFSET);
        let table_offset = read_u64(bytes, at::TABLE_OFFSET);
        let header_version = read_u32(bytes, at::HEADER_VERSION);
        let max_table_entries = read_u32(bytes, at::MAX_TABLE_ENTRIES);
        let block_size = read_u32(bytes, at::BLOCK_SIZE);

        if !block_size.is_power_of_two() {
            return Err(Error::Corrupt("block_size is not a power of two"));
        }
        if (block_size as u64) < SECTOR_SIZE {
            return Err(Error::Corrupt("block_size < 512"));
        }

        let mut parent_unique_id = [0u8; 16];
        parent_unique_id.copy_from_slice(&bytes[at::PARENT_UNIQUE_ID]);
        let parent_timestamp = read_u32(bytes, at::PARENT_TIMESTAMP);

        let parent_name = decode_utf16_be(&bytes[at::PARENT_UNICODE_NAME]);

        let mut parent_locators = [ParentLocator::default(); at::PARENT_LOCATOR_COUNT];
        for (i, slot) in parent_locators.iter_mut().enumerate() {
            let off = at::PARENT_LOCATORS + i * at::PARENT_LOCATOR_STRIDE;
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
        let sectors = self.block_size as u64 / SECTOR_SIZE;
        let bits = sectors;
        let bytes = bits.div_ceil(8);
        // Round up to a 512-byte sector.
        bytes.div_ceil(SECTOR_SIZE) * SECTOR_SIZE
    }
}

/// One's complement of the u32 sum, with the checksum field (36..40)
/// zeroed during compute. Same algorithm as the footer.
pub fn compute_checksum(header_bytes: &[u8]) -> u32 {
    ones_complement_checksum(header_bytes, DYN_HEADER_SIZE, at::CHECKSUM)
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

    /// Build a valid dynamic header with the given block size and a
    /// correct checksum.
    fn valid_header(block_size: u32) -> [u8; 1024] {
        let mut h = [0u8; 1024];
        h[0..8].copy_from_slice(DYN_HEADER_COOKIE);
        h[8..16].copy_from_slice(&u64::MAX.to_be_bytes());
        h[16..24].copy_from_slice(&3072u64.to_be_bytes());
        h[24..28].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        h[28..32].copy_from_slice(&4u32.to_be_bytes());
        h[32..36].copy_from_slice(&block_size.to_be_bytes());
        let cs = compute_checksum(&h);
        h[36..40].copy_from_slice(&cs.to_be_bytes());
        h
    }

    #[test]
    fn rejects_header_shorter_than_1024_bytes() {
        let err = DynamicHeader::parse(&[0u8; 512]).unwrap_err();
        assert!(matches!(err, Error::Corrupt(_)), "got {err:?}");
    }

    #[test]
    fn rejects_bad_cookie() {
        // parse() validates the cookie before the checksum, so corrupting
        // the cookie alone yields Corrupt — no checksum repair needed.
        let mut h = valid_header(0x0010_0000);
        h[0..8].copy_from_slice(b"NOTcxspr");
        let err = DynamicHeader::parse(&h).unwrap_err();
        assert!(matches!(err, Error::Corrupt(_)), "got {err:?}");
    }

    #[test]
    fn rejects_checksum_mismatch() {
        let mut h = valid_header(0x0010_0000);
        h[28] ^= 0xFF; // perturb max_table_entries without fixing checksum
        match DynamicHeader::parse(&h).unwrap_err() {
            Error::BadChecksum { what, .. } => assert_eq!(what, "dynamic-header"),
            other => panic!("expected BadChecksum, got {other:?}"),
        }
    }

    #[test]
    fn rejects_block_size_not_power_of_two() {
        let h = valid_header(0x0010_0001); // 1 MiB + 1
        let err = DynamicHeader::parse(&h).unwrap_err();
        assert!(matches!(err, Error::Corrupt(_)), "got {err:?}");
    }

    #[test]
    fn rejects_block_size_below_512() {
        let h = valid_header(256);
        let err = DynamicHeader::parse(&h).unwrap_err();
        assert!(matches!(err, Error::Corrupt(_)), "got {err:?}");
    }

    #[test]
    fn bitmap_size_rounds_up_to_a_sector() {
        // 512-byte block -> 1 sector -> 1 bit -> 1 byte -> padded to 512.
        assert_eq!(valid_parsed(512).bitmap_size_bytes(), 512);
        // 2 MiB block -> 4096 sectors -> 512 bytes -> exactly 1 sector.
        assert_eq!(valid_parsed(0x0020_0000).bitmap_size_bytes(), 512);
        // 8 MiB block -> 16384 sectors -> 2048 bytes -> 4 sectors.
        assert_eq!(valid_parsed(0x0080_0000).bitmap_size_bytes(), 2048);
    }

    fn valid_parsed(block_size: u32) -> DynamicHeader {
        DynamicHeader::parse(&valid_header(block_size)).unwrap()
    }

    #[test]
    fn decodes_parent_name_from_utf16_be() {
        let mut h = valid_header(0x0020_0000);
        // Write "p.vhd" as UTF-16 BE into the parent name field at 64.
        let name: Vec<u8> = "p.vhd"
            .encode_utf16()
            .flat_map(|u| u.to_be_bytes())
            .collect();
        h[64..64 + name.len()].copy_from_slice(&name);
        let cs = compute_checksum(&h);
        h[36..40].copy_from_slice(&cs.to_be_bytes());
        let parsed = DynamicHeader::parse(&h).unwrap();
        assert_eq!(parsed.parent_name, "p.vhd");
    }
}
