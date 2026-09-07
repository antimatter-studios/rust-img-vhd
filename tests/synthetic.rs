//! End-to-end tests built around hand-crafted VHD fixtures.
//!
//! Three image shapes:
//!
//! - **Fixed**: 16 KiB virtual, all data in place, footer trailing.
//! - **Dynamic**: 16 KiB virtual, block_size = 4 KiB, 4 BAT entries.
//!   Half allocated, half sparse. Sector bitmap exercised.
//! - **Differencing**: child with one block over a fixed parent, with
//!   the bitmap controlling per-sector defer-or-data.

use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::{Arc, Barrier};

use vhd::dynamic::{
    compute_checksum as dyn_cs, BAT_UNALLOCATED, DYN_HEADER_COOKIE, DYN_HEADER_SIZE,
};
use vhd::footer::{compute_checksum as footer_cs, FOOTER_COOKIE, FOOTER_SIZE};
mod common;
use common::{tmp_path_with, TempPath};
use vhd::{DiskType, VhdReader};

fn tmp_path(name: &str) -> TempPath {
    tmp_path_with("vhd_synth", name, "vhd")
}

trait WriteAt {
    fn write_all_at(&mut self, buf: &[u8], offset: u64) -> std::io::Result<()>;
}
impl WriteAt for File {
    fn write_all_at(&mut self, buf: &[u8], offset: u64) -> std::io::Result<()> {
        self.seek(SeekFrom::Start(offset))?;
        self.write_all(buf)
    }
}

fn build_footer(disk_type: DiskType, data_offset: u64, virt_size: u64) -> [u8; FOOTER_SIZE] {
    build_footer_with_id(disk_type, data_offset, virt_size, [0u8; 16])
}

/// As [`build_footer`], with the footer's `unique_id` — the value a
/// differencing child's `parent_unique_id` has to match.
fn build_footer_with_id(
    disk_type: DiskType,
    data_offset: u64,
    virt_size: u64,
    unique_id: [u8; 16],
) -> [u8; FOOTER_SIZE] {
    let mut f = [0u8; FOOTER_SIZE];
    f[68..84].copy_from_slice(&unique_id);
    f[0..8].copy_from_slice(FOOTER_COOKIE);
    f[8..12].copy_from_slice(&0x0000_0002u32.to_be_bytes()); // features
    f[12..16].copy_from_slice(&0x0001_0000u32.to_be_bytes()); // file_format_version
    f[16..24].copy_from_slice(&data_offset.to_be_bytes());
    f[40..48].copy_from_slice(&virt_size.to_be_bytes()); // original
    f[48..56].copy_from_slice(&virt_size.to_be_bytes()); // current
    let dt: u32 = match disk_type {
        DiskType::Fixed => 2,
        DiskType::Dynamic => 3,
        DiskType::Differencing => 4,
    };
    f[60..64].copy_from_slice(&dt.to_be_bytes());
    let cs = footer_cs(&f);
    f[64..68].copy_from_slice(&cs.to_be_bytes());
    f
}

// ---------------------------------------------------------------------------
// Fixed
// ---------------------------------------------------------------------------

#[test]
fn fixed_round_trip() {
    let path = tmp_path("fixed");
    let virt_size = 16u64 * 1024;

    // Lay down virt_size bytes of pattern + 512-byte footer.
    let mut f = File::create(&path).unwrap();
    let pattern: Vec<u8> = (0u8..=255u8).cycle().take(virt_size as usize).collect();
    f.write_all(&pattern).unwrap();

    let footer = build_footer(DiskType::Fixed, u64::MAX, virt_size);
    f.write_all(&footer).unwrap();
    drop(f);

    let r = VhdReader::open(&path).unwrap();
    assert_eq!(r.disk_type(), DiskType::Fixed);
    assert_eq!(r.virtual_size(), virt_size);
    assert_eq!(r.block_size(), 0);

    let mut buf = vec![0u8; 256];
    r.read_at(100, &mut buf).unwrap();
    assert_eq!(buf, pattern[100..356]);
}

#[test]
fn fixed_read_past_end_errors() {
    let path = tmp_path("fixed_oob");
    let virt_size = 4096u64;
    let mut f = File::create(&path).unwrap();
    f.write_all(&vec![0u8; virt_size as usize]).unwrap();
    let footer = build_footer(DiskType::Fixed, u64::MAX, virt_size);
    f.write_all(&footer).unwrap();
    drop(f);

    let r = VhdReader::open(&path).unwrap();
    let mut buf = [0u8; 16];
    let err = r.read_at(virt_size - 8, &mut buf).unwrap_err();
    matches!(err, vhd::Error::OutOfBounds { .. });
}

// ---------------------------------------------------------------------------
// Dynamic
// ---------------------------------------------------------------------------

/// Lay out a dynamic VHD with:
///   sector 0       footer (mirror)
///   sector 1..2    dynamic header (1024 bytes = 2 sectors)
///   sector 3..4    BAT (8 bytes — 2 entries — fits in 1 sector but we
///                  pad up to 2 sectors so block 0's bitmap+data start
///                  cleanly at sector 5)
///   sector 5       block 0 bitmap (1 sector)
///   sector 6..13   block 0 data (4096 bytes = 8 sectors)
///   ...trailer footer
///
/// virt_size = 8 KiB; block_size = 4 KiB; max_table_entries = 2.
/// Block 0 allocated; block 1 sparse.
fn build_dynamic_vhd(path: &Path, block0_data: &[u8], bitmap: u8) {
    const SECTOR: u64 = 512;
    const FOOTER_OFF: u64 = 0;
    const DYN_HEADER_OFF: u64 = SECTOR; // sector 1
    const BAT_OFF: u64 = SECTOR * 3; // sector 3 (after dyn header sectors 1+2)
    const BLOCK0_OFF: u64 = SECTOR * 5; // sector 5
    const BLOCK_SIZE: u32 = 4096;
    const VIRT_SIZE: u64 = 8 * 1024;

    assert_eq!(block0_data.len(), BLOCK_SIZE as usize);

    // Mirror footer at offset 0.
    let footer = build_footer(DiskType::Dynamic, DYN_HEADER_OFF, VIRT_SIZE);

    // Dynamic header.
    let mut hdr = [0u8; DYN_HEADER_SIZE];
    hdr[0..8].copy_from_slice(DYN_HEADER_COOKIE);
    hdr[8..16].copy_from_slice(&u64::MAX.to_be_bytes()); // no next
    hdr[16..24].copy_from_slice(&BAT_OFF.to_be_bytes());
    hdr[24..28].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    hdr[28..32].copy_from_slice(&2u32.to_be_bytes()); // 2 entries
    hdr[32..36].copy_from_slice(&BLOCK_SIZE.to_be_bytes());
    let cs = dyn_cs(&hdr);
    hdr[36..40].copy_from_slice(&cs.to_be_bytes());

    // BAT: entry 0 = sector index of block 0's bitmap; entry 1 = unallocated.
    let mut bat = [0u8; 512]; // pad to a sector
    let bat0 = (BLOCK0_OFF / SECTOR) as u32;
    bat[0..4].copy_from_slice(&bat0.to_be_bytes());
    bat[4..8].copy_from_slice(&BAT_UNALLOCATED.to_be_bytes());

    // Block 0 bitmap (1 sector). For block_size=4096, sectors_per_block=8 →
    // 8 bits = 1 byte → bitmap[0] is the only meaningful byte.
    let mut block0_bitmap = [0u8; 512];
    block0_bitmap[0] = bitmap;

    // Compose the file. End-of-data offset:
    //   BLOCK0_OFF (2560) + bitmap (512) + block_size (4096) = 7168
    let end_of_data = BLOCK0_OFF + 512 + BLOCK_SIZE as u64;
    let total_with_footer = end_of_data + FOOTER_SIZE as u64;
    let mut f = File::create(path).unwrap();
    f.set_len(total_with_footer).unwrap();
    f.write_all_at(&footer, FOOTER_OFF).unwrap();
    f.write_all_at(&hdr, DYN_HEADER_OFF).unwrap();
    f.write_all_at(&bat, BAT_OFF).unwrap();
    f.write_all_at(&block0_bitmap, BLOCK0_OFF).unwrap();
    f.write_all_at(block0_data, BLOCK0_OFF + 512).unwrap();
    // Trailing footer at end of file.
    f.write_all_at(&footer, end_of_data).unwrap();
}

#[test]
fn dynamic_allocated_block_reads_back_data() {
    let path = tmp_path("dynamic_alloc");
    let block: Vec<u8> = (0u8..=255u8).cycle().take(4096).collect();
    // bitmap: all 8 sectors of block 0 allocated → 0xFF.
    build_dynamic_vhd(&path, &block, 0xFF);

    let r = VhdReader::open(&path).unwrap();
    assert_eq!(r.disk_type(), DiskType::Dynamic);
    assert_eq!(r.virtual_size(), 8 * 1024);
    assert_eq!(r.block_size(), 4096);

    let mut buf = vec![0u8; 4096];
    r.read_at(0, &mut buf).unwrap();
    if buf != block {
        let first_diff = buf
            .iter()
            .zip(block.iter())
            .position(|(a, b)| a != b)
            .unwrap_or(usize::MAX);
        panic!(
            "buf and block differ: first diff at idx {first_diff}, \
             got 0x{:02X}, expected 0x{:02X}; buf.len()={}, block.len()={}",
            buf[first_diff],
            block[first_diff],
            buf.len(),
            block.len()
        );
    }
}

#[test]
fn dynamic_unallocated_block_reads_zeros() {
    let path = tmp_path("dynamic_unalloc");
    let block = vec![0u8; 4096];
    build_dynamic_vhd(&path, &block, 0xFF);

    let r = VhdReader::open(&path).unwrap();
    let mut buf = vec![0xAAu8; 4096];
    r.read_at(4096, &mut buf).unwrap(); // virt block 1 — unallocated
    assert!(buf.iter().all(|&b| b == 0));
}

#[test]
fn dynamic_partial_bitmap_zero_fills_unset_sectors() {
    let path = tmp_path("dynamic_partial");
    let block: Vec<u8> = vec![0xCC; 4096];
    // Bitmap 0b1010_1010 → only every other sector is allocated.
    // The MSB (bit 7) corresponds to sector 0; LSB (bit 0) is sector 7.
    // 0xAA = 1010_1010 → sectors 0, 2, 4, 6 allocated; 1, 3, 5, 7 not.
    build_dynamic_vhd(&path, &block, 0xAA);

    let r = VhdReader::open(&path).unwrap();
    let mut buf = vec![0u8; 4096];
    r.read_at(0, &mut buf).unwrap();

    for sector in 0..8u64 {
        let start = (sector * 512) as usize;
        let end = start + 512;
        let allocated = sector % 2 == 0;
        let expected = if allocated { 0xCC } else { 0 };
        assert!(
            buf[start..end].iter().all(|&b| b == expected),
            "sector {sector}: allocated={allocated}"
        );
    }
}

// ---------------------------------------------------------------------------
// Differencing — child with one block over a fixed parent.
// ---------------------------------------------------------------------------

#[test]
fn differencing_falls_through_to_parent_for_unallocated() {
    let parent_path = tmp_path("diff_parent");
    let virt_size = 8u64 * 1024;
    let parent_data: Vec<u8> = vec![0xAB; virt_size as usize];

    // Parent: fixed, 8 KiB of 0xAB, footer.
    let mut p = File::create(&parent_path).unwrap();
    p.write_all(&parent_data).unwrap();
    let parent_footer = build_footer(DiskType::Fixed, u64::MAX, virt_size);
    p.write_all(&parent_footer).unwrap();
    drop(p);

    // Child: differencing dynamic with parent_unicode_name = parent_path's filename.
    let child_path = tmp_path("diff_child");
    build_differencing_vhd(&child_path, &parent_path, 0xAA, 0xCD);
    // bitmap 0xAA → sectors 0,2,4,6 from child (0xCD), 1,3,5,7 from parent (0xAB).

    let r = VhdReader::open(&child_path).unwrap();
    assert_eq!(r.disk_type(), DiskType::Differencing);
    assert!(r.has_parent());

    let mut buf = vec![0u8; 4096];
    r.read_at(0, &mut buf).unwrap();
    for sector in 0..8u64 {
        let start = (sector * 512) as usize;
        let end = start + 512;
        let from_child = sector % 2 == 0;
        let expected = if from_child { 0xCD } else { 0xAB };
        assert!(
            buf[start..end].iter().all(|&b| b == expected),
            "sector {sector}: from_child={from_child}"
        );
    }

    // Block 1 is unallocated entirely → all bytes from parent (0xAB).
    let mut buf2 = vec![0u8; 4096];
    r.read_at(4096, &mut buf2).unwrap();
    assert!(buf2.iter().all(|&b| b == 0xAB));

    let _ = std::fs::remove_file(&child_path);
    let _ = std::fs::remove_file(&parent_path);
}

fn build_differencing_vhd(child_path: &Path, parent_path: &Path, bitmap: u8, child_block_byte: u8) {
    const SECTOR: u64 = 512;
    const DYN_HEADER_OFF: u64 = SECTOR;
    const BAT_OFF: u64 = SECTOR * 3;
    const BLOCK0_OFF: u64 = SECTOR * 5;
    const BLOCK_SIZE: u32 = 4096;
    const VIRT_SIZE: u64 = 8 * 1024;

    let footer = build_footer(DiskType::Differencing, DYN_HEADER_OFF, VIRT_SIZE);

    // Dynamic header with parent name.
    let mut hdr = [0u8; DYN_HEADER_SIZE];
    hdr[0..8].copy_from_slice(DYN_HEADER_COOKIE);
    hdr[8..16].copy_from_slice(&u64::MAX.to_be_bytes());
    hdr[16..24].copy_from_slice(&BAT_OFF.to_be_bytes());
    hdr[24..28].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    hdr[28..32].copy_from_slice(&2u32.to_be_bytes()); // 2 entries
    hdr[32..36].copy_from_slice(&BLOCK_SIZE.to_be_bytes());

    // parent_unicode_name (UTF-16 BE, 512 bytes).
    let parent_name = parent_path.file_name().unwrap().to_string_lossy();
    for (i, c) in parent_name.encode_utf16().enumerate() {
        if i * 2 + 2 > 512 {
            break;
        }
        hdr[64 + i * 2..64 + i * 2 + 2].copy_from_slice(&c.to_be_bytes());
    }

    let cs = dyn_cs(&hdr);
    hdr[36..40].copy_from_slice(&cs.to_be_bytes());

    let mut bat = [0u8; 512];
    let bat0 = (BLOCK0_OFF / SECTOR) as u32;
    bat[0..4].copy_from_slice(&bat0.to_be_bytes());
    bat[4..8].copy_from_slice(&BAT_UNALLOCATED.to_be_bytes());

    let mut block0_bitmap = [0u8; 512];
    block0_bitmap[0] = bitmap;

    let block0_data = vec![child_block_byte; BLOCK_SIZE as usize];

    let end_of_data = BLOCK0_OFF + 512 + BLOCK_SIZE as u64;
    let total_with_footer = end_of_data + FOOTER_SIZE as u64;
    let mut f = File::create(child_path).unwrap();
    f.set_len(total_with_footer).unwrap();
    f.write_all_at(&footer, 0).unwrap();
    f.write_all_at(&hdr, DYN_HEADER_OFF).unwrap();
    f.write_all_at(&bat, BAT_OFF).unwrap();
    f.write_all_at(&block0_bitmap, BLOCK0_OFF).unwrap();
    f.write_all_at(&block0_data, BLOCK0_OFF + 512).unwrap();
    f.write_all_at(&footer, end_of_data).unwrap();
}

// ---------------------------------------------------------------------------
// Fixed write path
// ---------------------------------------------------------------------------

#[test]
fn create_fixed_round_trip_pattern() {
    let path = tmp_path("create_rt");
    let virt_size = 64u64 * 1024;
    let r = VhdReader::create_fixed(&path, virt_size).unwrap();
    assert_eq!(r.disk_type(), DiskType::Fixed);
    assert_eq!(r.virtual_size(), virt_size);
    assert!(r.writable());

    // Write a pattern then read it back through a freshly opened RO reader.
    let pattern: Vec<u8> = (0u8..=255u8).cycle().take(virt_size as usize).collect();
    r.write_at(0, &pattern).unwrap();
    r.flush_writes().unwrap();
    drop(r);

    let r2 = VhdReader::open(&path).unwrap();
    let mut buf = vec![0u8; virt_size as usize];
    r2.read_at(0, &mut buf).unwrap();
    assert_eq!(buf, pattern);
    assert_eq!(r2.virtual_size(), virt_size);
}

#[test]
fn create_fixed_partial_write_within_bounds() {
    let path = tmp_path("create_partial");
    let virt_size = 32u64 * 1024;
    let r = VhdReader::create_fixed(&path, virt_size).unwrap();

    // Write 4 KiB starting at offset 8 KiB.
    let chunk: Vec<u8> = vec![0xA5; 4096];
    r.write_at(8 * 1024, &chunk).unwrap();
    r.flush_writes().unwrap();

    let mut buf = vec![0u8; 4096];
    r.read_at(8 * 1024, &mut buf).unwrap();
    assert_eq!(buf, chunk);

    // Surrounding region should still read as zero (sparse data area).
    let mut zero = vec![0xFFu8; 1024];
    r.read_at(0, &mut zero).unwrap();
    assert!(zero.iter().all(|&b| b == 0));
}

#[test]
fn write_past_virtual_size_returns_out_of_bounds() {
    let path = tmp_path("create_oob");
    let virt_size = 8u64 * 1024;
    let r = VhdReader::create_fixed(&path, virt_size).unwrap();
    let buf = vec![0u8; 16];
    let err = r.write_at(virt_size - 8, &buf).unwrap_err();
    assert!(matches!(err, vhd::Error::OutOfBounds { .. }), "got {err:?}");
}

#[test]
fn write_into_footer_region_returns_out_of_bounds() {
    let path = tmp_path("create_footer_oob");
    let virt_size = 8u64 * 1024;
    let r = VhdReader::create_fixed(&path, virt_size).unwrap();
    // Trying to write a single byte at virtual_size (== footer host offset)
    // must be rejected: that's footer territory.
    let buf = [0u8; 1];
    let err = r.write_at(virt_size, &buf).unwrap_err();
    assert!(matches!(err, vhd::Error::OutOfBounds { .. }), "got {err:?}");
    // After a rejected write, the footer must still parse cleanly: reopen.
    drop(r);
    let _r2 = VhdReader::open(&path).expect("footer survives rejected write");
}

#[test]
fn fixed_opened_read_only_is_not_writable() {
    let path = tmp_path("ro_fixed");
    let virt_size = 4u64 * 1024;
    {
        let r = VhdReader::create_fixed(&path, virt_size).unwrap();
        assert!(r.writable());
    }
    // Reopen RO.
    let r = VhdReader::open(&path).unwrap();
    assert!(!r.writable());
    assert!(!<VhdReader as fs_core::BlockDevice>::is_writable(&r));
    let buf = [0u8; 16];
    let err = r.write_at(0, &buf).unwrap_err();
    assert!(matches!(err, vhd::Error::ReadOnly), "got {err:?}");
}

#[test]
fn fixed_opened_read_write_is_writable() {
    let path = tmp_path("rw_fixed");
    let virt_size = 4u64 * 1024;
    {
        let _r = VhdReader::create_fixed(&path, virt_size).unwrap();
    }
    let r = VhdReader::open_rw(&path).unwrap();
    assert!(r.writable());
    let chunk = vec![0x42u8; 512];
    r.write_at(1024, &chunk).unwrap();
    r.flush_writes().unwrap();
    let mut buf = vec![0u8; 512];
    r.read_at(1024, &mut buf).unwrap();
    assert_eq!(buf, chunk);
}

#[test]
fn dynamic_opened_read_only_rejects_writes() {
    let path = tmp_path("dynamic_ro_writes_rejected");
    let block: Vec<u8> = vec![0u8; 4096];
    build_dynamic_vhd(&path, &block, 0xFF);

    let r = VhdReader::open(&path).unwrap();
    assert_eq!(r.disk_type(), DiskType::Dynamic);
    assert!(!r.writable());
    let buf = [0u8; 16];
    let err = r.write_at(0, &buf).unwrap_err();
    assert!(matches!(err, vhd::Error::ReadOnly), "got {err:?}");
}

#[test]
fn dynamic_opened_rw_is_writable() {
    let path = tmp_path("dynamic_rw_writable");
    let block: Vec<u8> = vec![0u8; 4096];
    build_dynamic_vhd(&path, &block, 0xFF);

    let r = VhdReader::open_rw(&path).unwrap();
    assert_eq!(r.disk_type(), DiskType::Dynamic);
    assert!(r.writable());
    assert!(<VhdReader as fs_core::BlockDevice>::is_writable(&r));
}

#[test]
fn differencing_is_not_writable() {
    let parent_path = tmp_path("diff_parent_nw");
    let virt_size = 8u64 * 1024;
    let parent_data: Vec<u8> = vec![0xAB; virt_size as usize];
    let mut p = File::create(&parent_path).unwrap();
    p.write_all(&parent_data).unwrap();
    let parent_footer = build_footer(DiskType::Fixed, u64::MAX, virt_size);
    p.write_all(&parent_footer).unwrap();
    drop(p);

    let child_path = tmp_path("diff_child_nw");
    build_differencing_vhd(&child_path, &parent_path, 0xFF, 0x11);

    let r = VhdReader::open_rw(&child_path).unwrap();
    assert_eq!(r.disk_type(), DiskType::Differencing);
    assert!(!r.writable());
    let buf = [0u8; 16];
    let err = r.write_at(0, &buf).unwrap_err();
    assert!(matches!(err, vhd::Error::ReadOnly), "got {err:?}");

    let _ = std::fs::remove_file(&child_path);
    let _ = std::fs::remove_file(&parent_path);
}

#[test]
fn fs_core_blockdevice_write_passes_through_for_fixed_rw() {
    use fs_core::BlockDevice;
    let path = tmp_path("fs_core_rw");
    let virt_size = 4u64 * 1024;
    let r = VhdReader::create_fixed(&path, virt_size).unwrap();
    let payload = vec![0x7Eu8; 256];
    BlockDevice::write_at(&r, 512, &payload).unwrap();
    BlockDevice::flush(&r).unwrap();
    assert!(BlockDevice::is_writable(&r));

    let mut buf = vec![0u8; 256];
    r.read_at(512, &mut buf).unwrap();
    assert_eq!(buf, payload);
}

#[test]
fn create_fixed_rejects_unaligned_size() {
    let path = tmp_path("create_unaligned");
    match VhdReader::create_fixed(&path, 1024 + 1) {
        Ok(_) => panic!("expected error for unaligned size"),
        Err(e) => assert!(matches!(e, vhd::Error::Corrupt(_)), "got {e:?}"),
    }
}

// ---------------------------------------------------------------------------
// fs_core::BlockRead bridge sanity
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// On-device entry points (FileDevice round-trip + parent-on-device rejection)
// ---------------------------------------------------------------------------

#[test]
fn open_on_device_round_trips_fixed_image() {
    use fs_core::FileDevice;
    use std::sync::Arc;
    let path = tmp_path("on_device_fixed");
    let virt_size = 8u64 * 1024;
    let pattern: Vec<u8> = (0u8..=255u8).cycle().take(virt_size as usize).collect();
    let mut f = File::create(&path).unwrap();
    f.write_all(&pattern).unwrap();
    let footer = build_footer(DiskType::Fixed, u64::MAX, virt_size);
    f.write_all(&footer).unwrap();
    drop(f);

    let dev = Arc::new(FileDevice::open(&path).unwrap()) as Arc<dyn fs_core::BlockDevice>;
    let r = VhdReader::open_on_device(dev).unwrap();
    assert_eq!(r.disk_type(), DiskType::Fixed);
    assert_eq!(r.virtual_size(), virt_size);

    let mut buf = vec![0u8; 256];
    r.read_at(64, &mut buf).unwrap();
    assert_eq!(buf, pattern[64..64 + 256]);
}

#[test]
fn open_rw_on_device_supports_fixed_writes() {
    use fs_core::FileDevice;
    use std::sync::Arc;
    let path = tmp_path("on_device_rw_fixed");
    let virt_size = 4u64 * 1024;
    {
        let _r = VhdReader::create_fixed(&path, virt_size).unwrap();
    }

    let dev = Arc::new(FileDevice::open_rw(&path).unwrap()) as Arc<dyn fs_core::BlockDevice>;
    let r = VhdReader::open_rw_on_device(dev).unwrap();
    assert!(r.writable());

    let payload = vec![0xCDu8; 256];
    r.write_at(512, &payload).unwrap();
    r.flush_writes().unwrap();
    let mut buf = vec![0u8; 256];
    r.read_at(512, &mut buf).unwrap();
    assert_eq!(buf, payload);
}

#[test]
fn open_rw_on_device_rejects_readonly_inner() {
    use fs_core::FileDevice;
    use std::sync::Arc;
    let path = tmp_path("on_device_ro_inner");
    let virt_size = 4u64 * 1024;
    {
        let _r = VhdReader::create_fixed(&path, virt_size).unwrap();
    }
    let dev = Arc::new(FileDevice::open(&path).unwrap()) as Arc<dyn fs_core::BlockDevice>;
    match VhdReader::open_rw_on_device(dev) {
        Err(vhd::Error::ReadOnly) => {}
        Err(e) => panic!("expected ReadOnly, got error {e:?}"),
        Ok(_) => panic!("expected ReadOnly, got Ok"),
    }
}

#[test]
fn differencing_on_device_rejected() {
    use fs_core::FileDevice;
    use std::sync::Arc;
    let parent_path = tmp_path("diff_on_dev_parent");
    let virt_size = 8u64 * 1024;
    let parent_data: Vec<u8> = vec![0xAB; virt_size as usize];
    let mut p = File::create(&parent_path).unwrap();
    p.write_all(&parent_data).unwrap();
    let parent_footer = build_footer(DiskType::Fixed, u64::MAX, virt_size);
    p.write_all(&parent_footer).unwrap();
    drop(p);

    let child_path = tmp_path("diff_on_dev_child");
    build_differencing_vhd(&child_path, &parent_path, 0xFF, 0x11);

    let dev = Arc::new(FileDevice::open(&child_path).unwrap()) as Arc<dyn fs_core::BlockDevice>;
    match VhdReader::open_on_device(dev) {
        Err(vhd::Error::Unsupported(_)) => {}
        Err(e) => panic!("expected Unsupported, got error {e:?}"),
        Ok(_) => panic!("expected Unsupported, got Ok"),
    }
    let _ = std::fs::remove_file(&child_path);
    let _ = std::fs::remove_file(&parent_path);
}

// ---------------------------------------------------------------------------
// Dynamic write path
// ---------------------------------------------------------------------------

#[test]
fn dynamic_write_into_existing_block_round_trips() {
    let path = tmp_path("dynamic_write_existing");
    // Block 0 starts with pattern; bitmap fully set so the pre-write
    // read sees the original bytes.
    let original: Vec<u8> = (0u8..=255u8).cycle().take(4096).collect();
    build_dynamic_vhd(&path, &original, 0xFF);

    let r = VhdReader::open_rw(&path).unwrap();
    assert!(r.writable());

    // Overwrite 256 bytes inside block 0.
    let payload = vec![0x5Au8; 256];
    r.write_at(100, &payload).unwrap();
    r.flush_writes().unwrap();

    // Read it back through the same handle.
    let mut buf = vec![0u8; 256];
    r.read_at(100, &mut buf).unwrap();
    assert_eq!(buf, payload);

    // Bytes outside the written window are untouched.
    let mut head = vec![0u8; 100];
    r.read_at(0, &mut head).unwrap();
    assert_eq!(head, original[..100]);

    // Re-open RO; the on-disk image must reflect the write.
    drop(r);
    let r2 = VhdReader::open(&path).unwrap();
    let mut buf2 = vec![0u8; 256];
    r2.read_at(100, &mut buf2).unwrap();
    assert_eq!(buf2, payload);
}

#[test]
fn dynamic_write_allocates_fresh_block() {
    // Block 0 allocated, block 1 sparse. Write into block 1 to force
    // a fresh allocation, then verify both the new content and that
    // the trailing footer mirror still parses on re-open.
    let path = tmp_path("dynamic_write_alloc");
    let block0: Vec<u8> = vec![0u8; 4096];
    build_dynamic_vhd(&path, &block0, 0xFF);

    let r = VhdReader::open_rw(&path).unwrap();
    assert!(r.writable());

    // Block 1 is virt offset [4096, 8192).
    let payload = vec![0xE3u8; 1024];
    r.write_at(4096 + 200, &payload).unwrap();
    r.flush_writes().unwrap();

    // Read back through the same handle.
    let mut buf = vec![0u8; 1024];
    r.read_at(4096 + 200, &mut buf).unwrap();
    assert_eq!(buf, payload);

    // Untouched parts of block 1 still read as zeros.
    let mut head = vec![0xFFu8; 200];
    r.read_at(4096, &mut head).unwrap();
    assert!(head.iter().all(|&b| b == 0), "block 1 head should be zero");

    // Trailing parts of block 1 also zero.
    let mut tail = vec![0xFFu8; 4096 - 200 - 1024];
    r.read_at(4096 + 200 + 1024, &mut tail).unwrap();
    assert!(tail.iter().all(|&b| b == 0), "block 1 tail should be zero");

    // Re-open RO — exercises footer-mirror rewrite + BAT durability.
    drop(r);
    let r2 = VhdReader::open(&path).unwrap();
    let mut buf2 = vec![0u8; 1024];
    r2.read_at(4096 + 200, &mut buf2).unwrap();
    assert_eq!(buf2, payload);
    assert_eq!(r2.virtual_size(), 8 * 1024);
}

#[test]
fn dynamic_write_spanning_block_boundary_allocates_both() {
    // Both blocks start sparse. A single write that straddles the
    // boundary must allocate two fresh blocks and splice the payload
    // correctly across them.
    //
    // Build a dynamic VHD with both BAT entries unallocated.
    let path = tmp_path("dynamic_write_span");
    build_dynamic_vhd_all_sparse(&path);

    let r = VhdReader::open_rw(&path).unwrap();
    assert!(r.writable());

    // Write 2 KiB straddling block boundary at virt offset 4096-512:
    //   first 512 bytes land at end of block 0, next 1536 bytes at
    //   start of block 1.
    let payload: Vec<u8> = (0u8..=255u8).cycle().take(2048).collect();
    let start = 4096u64 - 512;
    r.write_at(start, &payload).unwrap();
    r.flush_writes().unwrap();

    // Same-handle read.
    let mut buf = vec![0u8; 2048];
    r.read_at(start, &mut buf).unwrap();
    assert_eq!(buf, payload);

    // Untouched preceding bytes of block 0 still zero.
    let mut head = vec![0xFFu8; 512];
    r.read_at(0, &mut head).unwrap();
    assert!(head.iter().all(|&b| b == 0));

    // Untouched trailing bytes of block 1 still zero.
    let mut tail = vec![0xFFu8; 4096 - 1536];
    r.read_at(start + 2048, &mut tail).unwrap();
    assert!(tail.iter().all(|&b| b == 0));

    // Reopen RO; the image must be coherent.
    drop(r);
    let r2 = VhdReader::open(&path).unwrap();
    let mut buf2 = vec![0u8; 2048];
    r2.read_at(start, &mut buf2).unwrap();
    assert_eq!(buf2, payload);
}

/// Like `build_dynamic_vhd` but with both BAT entries marked unallocated.
/// Used for tests that exercise the allocation path on first write.
fn build_dynamic_vhd_all_sparse(path: &Path) {
    const SECTOR: u64 = 512;
    const FOOTER_OFF: u64 = 0;
    const DYN_HEADER_OFF: u64 = SECTOR;
    const BAT_OFF: u64 = SECTOR * 3;
    // No allocated blocks: end-of-data == BAT_OFF + bat sector.
    const END_OF_DATA: u64 = SECTOR * 5;
    const BLOCK_SIZE: u32 = 4096;
    const VIRT_SIZE: u64 = 8 * 1024;

    let footer = build_footer(DiskType::Dynamic, DYN_HEADER_OFF, VIRT_SIZE);

    let mut hdr = [0u8; DYN_HEADER_SIZE];
    hdr[0..8].copy_from_slice(DYN_HEADER_COOKIE);
    hdr[8..16].copy_from_slice(&u64::MAX.to_be_bytes());
    hdr[16..24].copy_from_slice(&BAT_OFF.to_be_bytes());
    hdr[24..28].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    hdr[28..32].copy_from_slice(&2u32.to_be_bytes());
    hdr[32..36].copy_from_slice(&BLOCK_SIZE.to_be_bytes());
    let cs = dyn_cs(&hdr);
    hdr[36..40].copy_from_slice(&cs.to_be_bytes());

    let mut bat = [0u8; 512];
    bat[0..4].copy_from_slice(&BAT_UNALLOCATED.to_be_bytes());
    bat[4..8].copy_from_slice(&BAT_UNALLOCATED.to_be_bytes());

    let total_with_footer = END_OF_DATA + FOOTER_SIZE as u64;
    let mut f = File::create(path).unwrap();
    f.set_len(total_with_footer).unwrap();
    f.write_all_at(&footer, FOOTER_OFF).unwrap();
    f.write_all_at(&hdr, DYN_HEADER_OFF).unwrap();
    f.write_all_at(&bat, BAT_OFF).unwrap();
    f.write_all_at(&footer, END_OF_DATA).unwrap();
}

// ---------------------------------------------------------------------------
// Concurrent writers
// ---------------------------------------------------------------------------

/// `VhdReader` is `Sync`, `write_at` takes `&self`, and `capi.rs` hands
/// the reader out inside an `Arc`, so two threads writing at once is a
/// supported use rather than a hypothetical one. These two tests are the
/// only thing in the suite that exercises it.
///
/// Both look for a timing window rather than a fixed ordering, so each
/// runs several trials against a fresh image. One trial that happened to
/// serialise would prove nothing either way.
const CONCURRENCY_TRIALS: usize = 8;

/// The all-sparse fixture's data ends at sector 5, one block costs a
/// bitmap sector plus the 4 KiB block, and the footer mirror trails
/// whatever the last block is. So the file length says exactly how many
/// blocks were allocated.
const SPARSE_FIXTURE_END_OF_DATA: u64 = 512 * 5;
const SPARSE_FIXTURE_BLOCK_TOTAL: u64 = 512 + 4096;

fn file_len_after_allocations(n: u64) -> u64 {
    SPARSE_FIXTURE_END_OF_DATA + n * SPARSE_FIXTURE_BLOCK_TOTAL + FOOTER_SIZE as u64
}

/// Run `(offset, payload)` writes on one thread each, released together.
fn write_concurrently(reader: &Arc<VhdReader>, writes: [(u64, u8); 2], len: usize) {
    let gate = Arc::new(Barrier::new(writes.len()));
    let handles: Vec<_> = writes
        .into_iter()
        .map(|(offset, fill)| {
            let reader = Arc::clone(reader);
            let gate = Arc::clone(&gate);
            std::thread::spawn(move || {
                let payload = vec![fill; len];
                gate.wait();
                reader.write_at(offset, &payload).unwrap();
            })
        })
        .collect();
    for h in handles {
        h.join().expect("writer thread panicked");
    }
}

fn assert_reads_back(reader: &VhdReader, offset: u64, fill: u8, len: usize, what: &str) {
    let mut got = vec![0u8; len];
    reader.read_at(offset, &mut got).unwrap();
    assert!(
        got.iter().all(|&b| b == fill),
        "{what}: expected {len} bytes of {fill:#04x} at offset {offset}, got {:02x?}...",
        &got[..8.min(got.len())]
    );
}

/// Two threads writing into the *same* unallocated block must leave one
/// allocation behind, with both payloads readable.
///
/// Two distinct ways this fails without a lock held across the whole of
/// one block's write:
///
///   * "Read the BAT entry, then allocate if it is unallocated" is a
///     check-then-act. Both threads see `BAT_UNALLOCATED`, both
///     allocate, the image grows by two blocks, one BAT entry wins, and
///     the loser's payload is stranded in a block nothing points at.
///   * The sector bitmap's read-modify-write is a check-then-act too.
///     Both threads read the same bitmap, and the second write-back
///     drops the first thread's bits — the payload bytes are in the
///     block, but the bitmap says that sector was never written, so it
///     reads back as zero.
///
/// The file-length assertion catches the first; the read-back
/// assertions catch either.
#[test]
fn concurrent_writes_into_one_unallocated_block_allocate_once() {
    // Both offsets sit inside block 0 ([0, 4096)) and in different
    // sectors, so a correct run leaves both payloads intact.
    const A_OFF: u64 = 0;
    const B_OFF: u64 = 2048;
    const LEN: usize = 512;

    for trial in 0..CONCURRENCY_TRIALS {
        let path = tmp_path(&format!("concurrent_same_block_{trial}"));
        build_dynamic_vhd_all_sparse(&path);

        let r = Arc::new(VhdReader::open_rw(&path).unwrap());
        write_concurrently(&r, [(A_OFF, 0xA1), (B_OFF, 0xB2)], LEN);
        r.flush_writes().unwrap();
        drop(r);

        let len = std::fs::metadata(&path).unwrap().len();
        let expected = file_len_after_allocations(1);
        assert_eq!(
            len, expected,
            "trial {trial}: two writers into one unallocated block must allocate it \
             once — the image is {len} bytes, one allocation is {expected}"
        );

        let r2 = VhdReader::open(&path).unwrap();
        assert_reads_back(
            &r2,
            A_OFF,
            0xA1,
            LEN,
            &format!("trial {trial}, first writer"),
        );
        assert_reads_back(
            &r2,
            B_OFF,
            0xB2,
            LEN,
            &format!("trial {trial}, second writer"),
        );
        drop(r2);
    }
}

/// Two threads writing into *different* unallocated blocks must produce
/// two allocations at distinct offsets, both intact.
///
/// This is what the tail offset exists for. If both allocations
/// resolved to the same tail — or if one thread's failure handling
/// reset the tail over the other thread's live reservation — the second
/// block would be written on top of the first.
#[test]
fn concurrent_writes_into_different_unallocated_blocks_allocate_both() {
    // Block 0 is [0, 4096); block 1 is [4096, 8192).
    const A_OFF: u64 = 0;
    const B_OFF: u64 = 4096;
    const LEN: usize = 512;

    for trial in 0..CONCURRENCY_TRIALS {
        let path = tmp_path(&format!("concurrent_two_blocks_{trial}"));
        build_dynamic_vhd_all_sparse(&path);

        let r = Arc::new(VhdReader::open_rw(&path).unwrap());
        write_concurrently(&r, [(A_OFF, 0xC3), (B_OFF, 0xD4)], LEN);
        r.flush_writes().unwrap();
        drop(r);

        let len = std::fs::metadata(&path).unwrap().len();
        let expected = file_len_after_allocations(2);
        assert_eq!(
            len, expected,
            "trial {trial}: writers into two unallocated blocks must allocate both \
             — the image is {len} bytes, two allocations are {expected}"
        );

        let r2 = VhdReader::open(&path).unwrap();
        assert_reads_back(&r2, A_OFF, 0xC3, LEN, &format!("trial {trial}, block 0"));
        assert_reads_back(&r2, B_OFF, 0xD4, LEN, &format!("trial {trial}, block 1"));
        drop(r2);
    }
}

#[test]
fn fs_core_blockread_size_matches_virtual() {
    let path = tmp_path("fs_core");
    let virt_size = 4096u64;
    let mut f = File::create(&path).unwrap();
    f.write_all(&vec![0u8; virt_size as usize]).unwrap();
    let footer = build_footer(DiskType::Fixed, u64::MAX, virt_size);
    f.write_all(&footer).unwrap();
    drop(f);

    let r = VhdReader::open(&path).unwrap();
    assert_eq!(<VhdReader as fs_core::BlockRead>::size_bytes(&r), virt_size);
}

/// A dynamic VHD whose `max_table_entries` is absurd is refused at open
/// rather than used to size an allocation.
///
/// The field is a `u32` read straight off disk. Unbounded, it asks for
/// up to 16 GiB — `vec![0u8; max_table_entries * 4]` — before a single
/// byte of the BAT has been read, so a hostile image needs only a header
/// to exhaust memory. The struct's own comment used to assert the table
/// was "always small", which was a claim about a number nothing checked.
///
/// The bound is the image's own arithmetic: the table must fit inside
/// the file it lives in.
#[test]
fn absurd_max_table_entries_is_refused_before_allocating() {
    let path = tmp_path("absurd_bat");
    build_dynamic_vhd(&path, &[0xABu8; 4096], 0xFF);
    {
        use std::io::Read;
        // Rewrite max_table_entries (offset 28 of the dynamic header,
        // which this fixture puts at sector 1) AND its checksum, so the
        // header parses and the size bound is what rejects the image
        // rather than the checksum.
        let mut hdr = [0u8; DYN_HEADER_SIZE];
        let mut rf = std::fs::File::open(&path).unwrap();
        rf.seek(SeekFrom::Start(512)).unwrap();
        rf.read_exact(&mut hdr).unwrap();

        hdr[28..32].copy_from_slice(&0xFFFF_FFFFu32.to_be_bytes());
        hdr[36..40].copy_from_slice(&0u32.to_be_bytes());
        let cs = dyn_cs(&hdr);
        hdr[36..40].copy_from_slice(&cs.to_be_bytes());

        // Seek-then-write rather than a positional write: the
        // positional syscalls live in `std::os::unix`, and CI runs this
        // on Windows too.
        let mut wf = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        wf.seek(SeekFrom::Start(512)).unwrap();
        wf.write_all(&hdr).unwrap();
    }

    // `VhdReader` is not Debug, so `expect_err` is unavailable.
    let msg = match VhdReader::open(&path) {
        Ok(_) => panic!("a BAT larger than the image must be refused, not allocated"),
        Err(e) => format!("{e}"),
    };
    assert!(
        msg.contains("BAT"),
        "the refusal should name the BAT, got: {msg}"
    );
}

// ---------------------------------------------------------------------
// A BAT entry is a sector index chosen by the image, and the address it
// names has to be one a block can legitimately occupy.
//
// `build_dynamic_vhd`'s layout, for the numbers below: footer mirror at
// 0, dynamic header at 512 (1024 bytes), BAT at sector 3 (1536), block 0
// bitmap at sector 5 (2560), block 0 data at 3072, trailing footer at
// 7168. So the first byte a block may occupy is 2048.
// ---------------------------------------------------------------------

const FIXTURE_BAT_OFFSET: u64 = 512 * 3;

/// Overwrite bytes in an image that already exists.
fn patch(path: &Path, off: u64, bytes: &[u8]) {
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    f.write_all_at(bytes, off).unwrap();
}

/// Build the standard dynamic fixture with BAT entry 0 replaced.
fn dynamic_with_bat0(name: &str, entry: u32) -> TempPath {
    let path = tmp_path(name);
    let block: Vec<u8> = (0u8..=255u8).cycle().take(4096).collect();
    build_dynamic_vhd(&path, &block, 0xFF);
    patch(&path, FIXTURE_BAT_OFFSET, &entry.to_be_bytes());
    path
}

#[test]
fn a_bat_entry_pointing_into_the_metadata_is_refused_at_open() {
    // Entry 0 makes the footer mirror the block's sector bitmap and the
    // bytes after it the block's data.
    let path = dynamic_with_bat0("bat_zero", 0);
    let err = VhdReader::open(&path)
        .err()
        .expect("an image whose BAT points at its own footer must be refused");
    assert!(matches!(err, vhd::Error::Corrupt(_)), "got {err:?}");
}

#[test]
fn a_bat_entry_landing_on_the_last_metadata_sector_is_refused_at_open() {
    // Sector 3 is the BAT itself — one sector below the first address a
    // block may occupy, so this pins the boundary rather than just the
    // obviously-absurd zero.
    let path = dynamic_with_bat0("bat_on_table", 3);
    let err = VhdReader::open(&path)
        .err()
        .expect("an image whose BAT points at the BAT must be refused");
    assert!(matches!(err, vhd::Error::Corrupt(_)), "got {err:?}");
}

#[test]
fn a_bat_entry_at_the_first_legal_sector_is_accepted() {
    // Sector 4 is `align_up(BAT end, 512)` — the first byte a block may
    // occupy. The pair with the test above is the point: a bound wants
    // the last value it must refuse and the first it must accept, or a
    // bound one sector too strict passes just as quietly as one a sector
    // too loose.
    let path = dynamic_with_bat0("bat_first_legal", 4);
    VhdReader::open(&path).expect("a block at the first legal sector must open");
}

#[test]
fn a_bat_entry_past_the_end_of_the_image_is_refused_at_open() {
    let path = dynamic_with_bat0("bat_past_end", 0x00FF_FFFF);
    let err = VhdReader::open(&path)
        .err()
        .expect("an image whose BAT points past its own end must be refused");
    assert!(matches!(err, vhd::Error::Corrupt(_)), "got {err:?}");
}

#[test]
fn a_bat_entry_whose_block_reaches_the_trailing_footer_is_refused_at_open() {
    // The data area ends where the trailing footer begins, not at the
    // end of the file. Entry 6 puts the block at 3072, so it ends at
    // 3072 + 4608 == 7680, which is exactly `dev_size` — and its last
    // sector IS the footer. Measured against a bound of `dev_size`,
    // which is what this was before: the image opened, and reading
    // virtual sector 7 of block 0 returned
    //
    //     Ok(())  buf=[63, 6f, 6e, 65, 63, 74, 69, 78, ...]  "conectix"
    //
    // the VHD footer cookie, as the guest's file data. That is the
    // defect this walk exists to stop, arriving through the check meant
    // to stop it.
    let path = dynamic_with_bat0("bat_over_footer", 6);
    let err = VhdReader::open(&path)
        .err()
        .expect("a block whose last sector is the trailing footer must be refused");
    assert!(matches!(err, vhd::Error::Corrupt(_)), "got {err:?}");
}

#[test]
fn a_bat_entry_whose_block_ends_exactly_at_the_footer_is_accepted() {
    // Sector 5 is where `build_dynamic_vhd` really puts block 0:
    // 2560 + 4608 == 7168 == dev_size - 512, so the block ends exactly
    // where the footer begins. This is the last value the bound must
    // accept, and tightening it by one footer too many — the obvious way
    // to overshoot the correction above — would reject the last
    // legitimate block of every canonical image.
    let path = dynamic_with_bat0("bat_last_legal", 5);
    let r = VhdReader::open(&path).expect("a block ending at the footer must open");
    let mut buf = vec![0u8; 4096];
    r.read_at(0, &mut buf).unwrap();
    assert_eq!(
        buf,
        (0u8..=255u8).cycle().take(4096).collect::<Vec<u8>>(),
        "and it must still read"
    );
}

#[test]
fn two_bat_entries_naming_one_block_are_refused_at_open() {
    // Entry 1 is normally unallocated; pointing it at block 0 makes two
    // virtual blocks share one host block, so a write through either
    // silently changes the other.
    let path = tmp_path("bat_aliased");
    let block: Vec<u8> = (0u8..=255u8).cycle().take(4096).collect();
    build_dynamic_vhd(&path, &block, 0xFF);
    patch(&path, FIXTURE_BAT_OFFSET + 4, &5u32.to_be_bytes());

    let err = VhdReader::open(&path)
        .err()
        .expect("an image with two BAT entries at one address must be refused");
    assert!(matches!(err, vhd::Error::Corrupt(_)), "got {err:?}");
}

#[test]
fn the_unmodified_fixture_still_opens_and_reads() {
    // The positive control for the four refusals above: the same
    // builder, untouched, must keep working.
    let path = tmp_path("bat_ok");
    let block: Vec<u8> = (0u8..=255u8).cycle().take(4096).collect();
    build_dynamic_vhd(&path, &block, 0xFF);
    let r = VhdReader::open(&path).unwrap();
    let mut buf = vec![0u8; 4096];
    r.read_at(0, &mut buf).unwrap();
    assert_eq!(buf, block);
}

/// Rebuild the all-sparse fixture with its trailing footer one byte past
/// the sector boundary, so the file is 3073 bytes long and its last 512
/// bytes are still a valid footer.
///
/// This is the shape the `open_*_on_device` entry points make reachable:
/// they accept any `BlockDevice`, so the device's length is not this
/// crate's to assume.
fn dynamic_with_unaligned_length(name: &str) -> TempPath {
    let path = tmp_path(name);
    build_dynamic_vhd_all_sparse(&path);
    let bytes = std::fs::read(&path).unwrap();
    let footer = bytes[bytes.len() - FOOTER_SIZE..].to_vec();
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    f.write_all_at(&footer, 512 * 5 + 1).unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 512 * 5 + 1 + 512);
    path
}

/// A sparse image whose length is not a multiple of 512 is refused,
/// because its blocks cannot sit where a BAT entry can name them.
///
/// Before this, the image opened, `write_at` returned `Ok(())`, and the
/// bytes it wrote were unreachable for ever: the block landed at 2561,
/// the BAT recorded 2561 / 512 == 5, and the reader went to 2560.
#[test]
fn a_sparse_image_whose_length_is_not_a_multiple_of_512_is_refused() {
    let path = dynamic_with_unaligned_length("unaligned_len");
    let err = VhdReader::open(&path)
        .err()
        .expect("an off-grid sparse image must be refused");
    assert!(matches!(err, vhd::Error::Corrupt(_)), "got {err:?}");

    let err = VhdReader::open_rw(&path)
        .err()
        .expect("and refused for writing too");
    assert!(matches!(err, vhd::Error::Corrupt(_)), "got {err:?}");
}

/// The positive control: the same fixture at its natural length still
/// opens, allocates on first write, and reads the bytes back.
#[test]
fn an_aligned_sparse_image_still_allocates_and_reads_back() {
    let path = tmp_path("aligned_len");
    build_dynamic_vhd_all_sparse(&path);
    let payload = [0xABu8; 16];
    {
        let r = VhdReader::open_rw(&path).unwrap();
        r.write_at(0, &payload).unwrap();
        r.flush_writes().unwrap();
    }
    let r = VhdReader::open(&path).unwrap();
    let mut buf = [0u8; 16];
    r.read_at(0, &mut buf).unwrap();
    assert_eq!(buf, payload);
}

/// A partial-sector write into a block somebody else allocated must not
/// publish the rest of the sector.
///
/// The fixture is the case the format allows and this crate did not
/// handle: block 0 is allocated, its bitmap is all clear — so every
/// sector reads as zero — and its data area holds `0xEE`, because the
/// format leaves the bytes under a clear bit undefined and a writer is
/// free to leave anything there.
#[test]
fn a_partial_sector_write_publishes_only_what_was_written() {
    let path = tmp_path("partial_sector");
    build_dynamic_vhd(&path, &[0xEEu8; 4096], 0x00);

    // Before anything is written, the block reads as zeros.
    {
        let r = VhdReader::open(&path).unwrap();
        let mut buf = vec![0u8; 512];
        r.read_at(0, &mut buf).unwrap();
        assert!(
            buf.iter().all(|&b| b == 0),
            "an all-clear bitmap must read as zeros, got {:02x?}",
            &buf[..8]
        );
    }

    {
        let r = VhdReader::open_rw(&path).unwrap();
        r.write_at(100, &[0xABu8; 16]).unwrap();
        r.flush_writes().unwrap();
    }

    let r = VhdReader::open(&path).unwrap();
    let mut buf = vec![0u8; 512];
    r.read_at(0, &mut buf).unwrap();

    assert_eq!(&buf[100..116], &[0xABu8; 16], "the payload did not land");
    let stray: Vec<usize> = (0..512)
        .filter(|&i| !(100..116).contains(&i) && buf[i] != 0)
        .collect();
    assert!(
        stray.is_empty(),
        "{} bytes the caller never wrote were published; first at {:?}",
        stray.len(),
        &stray[..stray.len().min(8)]
    );
}

/// A write that starts mid-sector and ends mid-sector two sectors later
/// leaves both partial ends undefined-free, and does not disturb the
/// sector between them, which the payload covers in full.
#[test]
fn a_write_spanning_three_sectors_publishes_only_what_was_written() {
    let path = tmp_path("partial_span");
    build_dynamic_vhd(&path, &[0xEEu8; 4096], 0x00);

    let start = 512 + 300u64; // mid-sector 1
    let len = 512 + 400usize; // through sector 2, ending mid-sector 3
    {
        let r = VhdReader::open_rw(&path).unwrap();
        r.write_at(start, &vec![0xCDu8; len]).unwrap();
        r.flush_writes().unwrap();
    }

    let r = VhdReader::open(&path).unwrap();
    let mut buf = vec![0u8; 4 * 512];
    r.read_at(0, &mut buf).unwrap();

    let s = start as usize;
    assert_eq!(&buf[s..s + len], &vec![0xCDu8; len][..]);
    let stray: Vec<usize> = (0..buf.len())
        .filter(|&i| !(s..s + len).contains(&i) && buf[i] != 0)
        .collect();
    assert!(
        stray.is_empty(),
        "{} bytes the caller never wrote were published; first at {:?}",
        stray.len(),
        &stray[..stray.len().min(8)]
    );
}

/// Build a differencing child that names `parent_path` and declares
/// `parent_unique_id`.
fn build_differencing_vhd_claiming(
    child_path: &Path,
    parent_path: &Path,
    parent_unique_id: [u8; 16],
) {
    const SECTOR: u64 = 512;
    const DYN_HEADER_OFF: u64 = SECTOR;
    const BAT_OFF: u64 = SECTOR * 3;
    const BLOCK_SIZE: u32 = 4096;
    const VIRT_SIZE: u64 = 8 * 1024;

    let footer = build_footer(DiskType::Differencing, DYN_HEADER_OFF, VIRT_SIZE);

    let mut hdr = [0u8; DYN_HEADER_SIZE];
    hdr[0..8].copy_from_slice(DYN_HEADER_COOKIE);
    hdr[8..16].copy_from_slice(&u64::MAX.to_be_bytes());
    hdr[16..24].copy_from_slice(&BAT_OFF.to_be_bytes());
    hdr[24..28].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    hdr[28..32].copy_from_slice(&2u32.to_be_bytes());
    hdr[32..36].copy_from_slice(&BLOCK_SIZE.to_be_bytes());
    hdr[40..56].copy_from_slice(&parent_unique_id);
    let parent_name = parent_path.file_name().unwrap().to_string_lossy();
    for (i, c) in parent_name.encode_utf16().enumerate() {
        if i * 2 + 2 > 512 {
            break;
        }
        hdr[64 + i * 2..64 + i * 2 + 2].copy_from_slice(&c.to_be_bytes());
    }
    let cs = dyn_cs(&hdr);
    hdr[36..40].copy_from_slice(&cs.to_be_bytes());

    // BAT: both blocks unallocated, so every byte comes from the parent
    // — which is what makes the parent's identity the whole answer.
    let mut bat = [0u8; 512];
    bat[0..4].copy_from_slice(&BAT_UNALLOCATED.to_be_bytes());
    bat[4..8].copy_from_slice(&BAT_UNALLOCATED.to_be_bytes());

    let end_of_data = BAT_OFF + 512;
    let mut f = File::create(child_path).unwrap();
    f.set_len(end_of_data + FOOTER_SIZE as u64).unwrap();
    f.write_all_at(&footer, 0).unwrap();
    f.write_all_at(&hdr, DYN_HEADER_OFF).unwrap();
    f.write_all_at(&bat, BAT_OFF).unwrap();
    f.write_all_at(&footer, end_of_data).unwrap();
}

/// A parent whose `unique_id` and contents are given.
fn build_fixed_parent(path: &Path, unique_id: [u8; 16], fill: u8) {
    const VIRT_SIZE: u64 = 8 * 1024;
    let mut f = File::create(path).unwrap();
    f.write_all(&vec![fill; VIRT_SIZE as usize]).unwrap();
    let footer = build_footer_with_id(DiskType::Fixed, u64::MAX, VIRT_SIZE, unique_id);
    f.write_all(&footer).unwrap();
}

/// A child must not accept a parent that is not the one it names.
///
/// Resolution is by name, so any file at that path that parses as a VHD
/// was taken. A differencing image is a delta — the child holds only
/// the sectors it changed — so bolting it onto a different parent
/// produces a disk that is internally consistent, opens without
/// complaint, and is wrong, with no checksum over the composite for
/// anything downstream to catch.
///
/// Before this: `read Ok(()) first8=[bb, bb, bb, bb, bb, bb, bb, bb]`,
/// the wrong parent's bytes served as the child's.
#[test]
fn a_parent_that_is_not_the_one_the_child_names_is_refused() {
    let parent = tmp_path("identity_wrong_parent");
    build_fixed_parent(&parent, [0x22; 16], 0xBB);

    let child = tmp_path("identity_wrong_child");
    build_differencing_vhd_claiming(&child, &parent, [0x11; 16]);

    let err = VhdReader::open(&child)
        .err()
        .expect("a parent whose unique_id does not match must be refused");
    match err {
        vhd::Error::ParentNotFound(m) => {
            assert!(
                m.contains("22222222"),
                "the message must name what it found: {m}"
            );
            assert!(m.contains("11111111"), "and what the child asked for: {m}");
        }
        other => panic!("expected ParentNotFound, got {other:?}"),
    }
}

/// Two ids that differ in exactly one byte are still different ids, and
/// there is a case at each end.
///
/// One minimal difference discriminates only at its own position. The
/// pair in the test above differ in every byte, so a comparison looking
/// at only `unique_id[0]` refuses them and passes; a pair differing
/// only in byte 15 catches that, and a comparison looking at only
/// `unique_id[15]` then passes instead. Any comparison examining a
/// strict subset of the bytes misses one end or the other, so both ends
/// are needed to close the class.
///
/// A GUID's bytes are not ordered by significance, so two real ids
/// agreeing in fifteen of sixteen positions is realistic rather than
/// contrived — at either end.
#[test]
fn a_parent_id_differing_in_one_byte_at_either_end_is_still_a_different_parent() {
    for at in [0usize, 15] {
        let mut theirs = [0x22u8; 16];
        let mut ours = [0x22u8; 16];
        theirs[at] = 0xFE;
        ours[at] = 0xFF;

        let parent = tmp_path(&format!("identity_byte{at}_parent"));
        build_fixed_parent(&parent, theirs, 0xBB);

        let child = tmp_path(&format!("identity_byte{at}_child"));
        build_differencing_vhd_claiming(&child, &parent, ours);

        assert!(
            matches!(VhdReader::open(&child), Err(vhd::Error::ParentNotFound(_))),
            "a difference in byte {at} is still a different parent"
        );
    }
}

/// The positive control: the right parent still opens, and the child
/// still reads through to it.
///
/// Without this, refusing every parent would pass the test above. The
/// child here has both blocks unallocated, so every byte it serves
/// comes from the parent — which is what makes the parent's identity
/// the whole answer rather than a detail.
#[test]
fn a_parent_whose_unique_id_matches_is_accepted_and_read_through() {
    let parent = tmp_path("identity_right_parent");
    build_fixed_parent(&parent, [0x22; 16], 0xBB);

    let child = tmp_path("identity_right_child");
    build_differencing_vhd_claiming(&child, &parent, [0x22; 16]);

    let r = VhdReader::open(&child).expect("the named parent must be accepted");
    assert!(r.has_parent());
    let mut buf = vec![0u8; 512];
    r.read_at(0, &mut buf).unwrap();
    assert!(
        buf.iter().all(|&b| b == 0xBB),
        "every byte comes from the parent"
    );
}
