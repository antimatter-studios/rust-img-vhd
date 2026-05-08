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
use std::path::{Path, PathBuf};

use vhd::footer::{compute_checksum as footer_cs, FOOTER_COOKIE, FOOTER_SIZE};
use vhd::dynamic::{compute_checksum as dyn_cs, DYN_HEADER_COOKIE, DYN_HEADER_SIZE, BAT_UNALLOCATED};
use vhd::{DiskType, VhdReader};

fn tmp_path(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let mut p = std::env::temp_dir();
    p.push(format!("vhd_synth_{}_{n}_{name}.vhd", std::process::id()));
    p
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
    let mut f = [0u8; FOOTER_SIZE];
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

    let _ = std::fs::remove_file(&path);
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
    let _ = std::fs::remove_file(&path);
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
fn build_dynamic_vhd(path: &PathBuf, block0_data: &[u8], bitmap: u8) {
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
            buf[first_diff], block[first_diff], buf.len(), block.len()
        );
    }

    let _ = std::fs::remove_file(&path);
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
    let _ = std::fs::remove_file(&path);
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
    let _ = std::fs::remove_file(&path);
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

fn build_differencing_vhd(
    child_path: &PathBuf,
    parent_path: &Path,
    bitmap: u8,
    child_block_byte: u8,
) {
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

    let _ = std::fs::remove_file(&path);
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

    let _ = std::fs::remove_file(&path);
}

#[test]
fn write_past_virtual_size_returns_out_of_bounds() {
    let path = tmp_path("create_oob");
    let virt_size = 8u64 * 1024;
    let r = VhdReader::create_fixed(&path, virt_size).unwrap();
    let buf = vec![0u8; 16];
    let err = r.write_at(virt_size - 8, &buf).unwrap_err();
    assert!(matches!(err, vhd::Error::OutOfBounds { .. }), "got {err:?}");
    let _ = std::fs::remove_file(&path);
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
    let _ = std::fs::remove_file(&path);
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
    let _ = std::fs::remove_file(&path);
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
    let _ = std::fs::remove_file(&path);
}

#[test]
fn dynamic_is_not_writable() {
    let path = tmp_path("dynamic_not_writable");
    let block: Vec<u8> = vec![0u8; 4096];
    build_dynamic_vhd(&path, &block, 0xFF);

    // Even when opened RW, dynamic returns false (no write path yet).
    let r = VhdReader::open_rw(&path).unwrap();
    assert_eq!(r.disk_type(), DiskType::Dynamic);
    assert!(!r.writable());
    assert!(!<VhdReader as fs_core::BlockDevice>::is_writable(&r));
    let buf = [0u8; 16];
    let err = r.write_at(0, &buf).unwrap_err();
    assert!(matches!(err, vhd::Error::ReadOnly), "got {err:?}");
    let _ = std::fs::remove_file(&path);
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
    let _ = std::fs::remove_file(&path);
}

#[test]
fn create_fixed_rejects_unaligned_size() {
    let path = tmp_path("create_unaligned");
    match VhdReader::create_fixed(&path, 1024 + 1) {
        Ok(_) => panic!("expected error for unaligned size"),
        Err(e) => assert!(matches!(e, vhd::Error::Corrupt(_)), "got {e:?}"),
    }
    let _ = std::fs::remove_file(&path);
}

// ---------------------------------------------------------------------------
// fs_core::BlockRead bridge sanity
// ---------------------------------------------------------------------------

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

    let _ = std::fs::remove_file(&path);
}
