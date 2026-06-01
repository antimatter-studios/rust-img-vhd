//! Reader-level corruption and round-trip tests for fixed VHDs.
//!
//! These build a real fixed VHD via `VhdReader::create_fixed` (so the
//! footer is genuine), then surgically corrupt the trailing 512-byte
//! footer to confirm the reader rejects it cleanly, plus a larger
//! write→reopen→read round-trip than the synthetic suite covers.

use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use vhd::{Error, VhdReader};

const VSIZE: u64 = 1024 * 1024; // 1 MiB fixed disk

fn tmp_path(name: &str) -> TempPath {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let mut p = std::env::temp_dir();
    p.push(format!("vhd_corrupt_{}_{n}_{name}.vhd", std::process::id()));
    TempPath(p)
}

/// RAII temp-file path: removes the backing file on drop so a panicking
/// assertion can't leak fixtures into the temp dir across CI runs.
struct TempPath(PathBuf);
impl std::ops::Deref for TempPath {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}
impl AsRef<Path> for TempPath {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}
impl Drop for TempPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Create a fixed VHD with `VSIZE` bytes of virtual capacity and return
/// its path. The 512-byte footer lives at file offset `VSIZE`.
fn make_fixed(name: &str) -> TempPath {
    let path = tmp_path(name);
    let r = VhdReader::create_fixed(&path, VSIZE).unwrap();
    r.flush_writes().unwrap();
    path
}

fn footer_offset() -> u64 {
    VSIZE
}

fn patch(path: &Path, offset: u64, bytes: &[u8]) {
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    f.seek(SeekFrom::Start(offset)).unwrap();
    f.write_all(bytes).unwrap();
    f.flush().unwrap();
}

#[test]
fn created_fixed_vhd_opens_as_baseline() {
    let path = make_fixed("baseline");
    let r = VhdReader::open(&path).unwrap();
    assert_eq!(r.virtual_size(), VSIZE);
    let mut buf = [0u8; 16];
    r.read_at(0, &mut buf).unwrap();
    assert!(buf.iter().all(|&b| b == 0));
}

#[test]
fn corrupt_footer_cookie_is_not_vhd() {
    let path = make_fixed("bad_cookie");
    patch(&path, footer_offset(), b"NOTcooki");
    let err = VhdReader::open(&path)
        .err()
        .expect("expected NotVhd, got Ok");
    assert!(matches!(err, Error::NotVhd), "got {err:?}");
}

#[test]
fn corrupt_footer_checksum_is_rejected() {
    let path = make_fixed("bad_checksum");
    // Flip a byte in the footer's original_size field (offset +40)
    // without fixing the stored checksum.
    patch(&path, footer_offset() + 40, &[0xFF]);
    match VhdReader::open(&path) {
        Err(Error::BadChecksum { what, .. }) => assert_eq!(what, "footer"),
        other => panic!("expected footer BadChecksum, got {:?}", other.err()),
    }
}

#[test]
fn corrupt_footer_disk_type_is_unsupported() {
    let path = make_fixed("bad_disktype");
    // disk_type is a big-endian u32 at footer offset +60. Set it to 9
    // (outside the {2,3,4} set) and repair the checksum so parsing
    // reaches the disk-type check and rejects with UnsupportedDiskType
    // rather than BadChecksum.
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    let mut footer = vec![0u8; 512];
    use std::io::Read;
    f.seek(SeekFrom::Start(footer_offset())).unwrap();
    f.read_exact(&mut footer).unwrap();
    footer[60..64].copy_from_slice(&9u32.to_be_bytes());
    let cs = vhd::footer::compute_checksum(&footer);
    footer[64..68].copy_from_slice(&cs.to_be_bytes());
    f.seek(SeekFrom::Start(footer_offset())).unwrap();
    f.write_all(&footer).unwrap();
    f.flush().unwrap();
    drop(f);

    match VhdReader::open(&path) {
        Err(Error::UnsupportedDiskType(9)) => {}
        other => panic!("expected UnsupportedDiskType(9), got {:?}", other.err()),
    }
}

#[test]
fn fixed_vhd_large_write_round_trips_across_sectors() {
    let path = tmp_path("roundtrip");
    let r = VhdReader::create_fixed(&path, VSIZE).unwrap();

    // Write a 300 KiB pattern at a non-sector-multiple offset so it
    // straddles many 512-byte sectors and is not block-aligned.
    let start = 100 * 1024 + 17;
    let payload: Vec<u8> = (0..300 * 1024).map(|i| (i % 251) as u8).collect();
    r.write_at(start, &payload).unwrap();
    r.flush_writes().unwrap();
    drop(r);

    let r2 = VhdReader::open(&path).unwrap();
    let mut got = vec![0u8; payload.len()];
    r2.read_at(start, &mut got).unwrap();
    assert_eq!(got, payload);

    // Bytes just before the written window are still zero.
    let mut before = [0u8; 16];
    r2.read_at(start - 16, &mut before).unwrap();
    assert!(before.iter().all(|&b| b == 0));
}
