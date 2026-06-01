//! Cross-validation against `qemu-img` (VHD is the `vpc` format).
//!
//! Gated behind the `qemu-validation` feature so regular `cargo test`
//! does not require qemu-img on PATH. Run with:
//!
//!     cargo test --features qemu-validation --test qemu_validation
//!
//! Licensing posture: `qemu-img` is invoked as a separate OS process.
//! No QEMU source or binary is linked into this crate, and `qemu-img`
//! is never bundled into a release artifact.
//!
//! Two notes specific to VHD/vpc:
//!
//!   * `qemu-img check` is NOT supported for vpc, so structural
//!     validation here is done with `convert` (byte round-trip) and
//!     `info` (geometry) rather than a checker.
//!   * VHD derives its virtual size from CHS geometry, so qemu rounds a
//!     requested size up to the next geometry boundary. Tests compare
//!     our reader's `virtual_size()` against qemu's reported size rather
//!     than a hardcoded constant.
//!   * A *fixed* VHD has no footer copy at offset 0, so qemu's format
//!     auto-probe scores it as `raw`; we always pass `-f vpc` so qemu
//!     treats our images as VHD.

#![cfg(feature = "qemu-validation")]

use std::path::{Path, PathBuf};
use std::process::Command;

use vhd::{footer::DiskType, VhdReader};

const QEMU_IMG: &str = "qemu-img";

fn run_qemu(args: &[&str]) -> std::process::Output {
    Command::new(QEMU_IMG)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("failed to invoke `{QEMU_IMG}` ({e}); install qemu-utils?"))
}

fn assert_qemu(args: &[&str]) {
    let out = run_qemu(args);
    assert!(
        out.status.success(),
        "`qemu-img {}` failed:\n--- stdout ---\n{}\n--- stderr ---\n{}",
        args.join(" "),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

fn tmp(ext: &str, name: &str) -> TempPath {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let mut p = std::env::temp_dir();
    p.push(format!("vhd_qemu_{}_{n}_{name}.{ext}", std::process::id()));
    TempPath(p)
}

/// RAII temp-file path: removes the backing file on drop so a panicking
/// assertion can't leak fixtures into the temp dir across CI runs.
struct TempPath(PathBuf);
impl std::ops::Deref for TempPath {
    type Target = std::path::Path;
    fn deref(&self) -> &std::path::Path {
        &self.0
    }
}
impl AsRef<std::path::Path> for TempPath {
    fn as_ref(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for TempPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn vhd_path(name: &str) -> TempPath {
    tmp("vhd", name)
}
fn raw_path(name: &str) -> TempPath {
    tmp("raw", name)
}

fn qemu_create(path: &Path, size: &str, subformat: Option<&str>) {
    match subformat {
        Some(sf) => assert_qemu(&[
            "create",
            "-f",
            "vpc",
            "-o",
            &format!("subformat={sf}"),
            path.to_str().unwrap(),
            size,
        ]),
        None => assert_qemu(&["create", "-f", "vpc", path.to_str().unwrap(), size]),
    }
}

fn qemu_convert_raw_to_vpc(raw: &Path, vhd: &Path) {
    assert_qemu(&[
        "convert",
        "-f",
        "raw",
        "-O",
        "vpc",
        raw.to_str().unwrap(),
        vhd.to_str().unwrap(),
    ]);
}

fn qemu_convert_vpc_to_raw(vhd: &Path, raw: &Path) {
    assert_qemu(&[
        "convert",
        "-f",
        "vpc",
        "-O",
        "raw",
        vhd.to_str().unwrap(),
        raw.to_str().unwrap(),
    ]);
}

fn qemu_vpc_virtual_size(path: &Path) -> u64 {
    let out = run_qemu(&["info", "-f", "vpc", "--output=json", path.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "qemu-img info failed:\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("info JSON parses");
    assert_eq!(v["format"], "vpc");
    v["virtual-size"].as_u64().expect("virtual-size is u64")
}

fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

#[test]
fn qemu_img_is_callable() {
    let out = run_qemu(&["--version"]);
    assert!(
        out.status.success(),
        "qemu-img --version exited non-zero — qemu-utils not installed?"
    );
}

/// Cross-read (dynamic): a blank dynamic VHD from qemu reads as zeros,
/// and our reader's virtual size matches what qemu reports. Misparsing
/// the footer/dynamic-header/BAT would surface as garbage or a wrong
/// size.
#[test]
fn our_reader_matches_empty_qemu_dynamic_image() {
    let p = vhd_path("empty-dyn");
    qemu_create(&p, "4M", None); // qemu default vpc subformat is dynamic

    let r = VhdReader::open(&p).unwrap();
    assert_eq!(r.disk_type(), DiskType::Dynamic);
    assert_eq!(r.virtual_size(), qemu_vpc_virtual_size(&p));

    let mut buf = vec![0u8; 65_536];
    r.read_at(0, &mut buf).unwrap();
    assert!(
        buf.iter().all(|&b| b == 0),
        "empty dynamic VHD must read zeros"
    );
}

/// Cross-read (fixed): a blank fixed VHD from qemu reads as zeros and is
/// classified as Fixed.
#[test]
fn our_reader_matches_empty_qemu_fixed_image() {
    let p = vhd_path("empty-fixed");
    qemu_create(&p, "2M", Some("fixed"));

    let r = VhdReader::open(&p).unwrap();
    assert_eq!(r.disk_type(), DiskType::Fixed);
    assert_eq!(r.virtual_size(), qemu_vpc_virtual_size(&p));

    let mut buf = vec![0u8; 4096];
    r.read_at(0, &mut buf).unwrap();
    assert!(buf.iter().all(|&b| b == 0));
}

/// Cross-read (populated): convert a raw pattern into VHD via qemu, read
/// it back with our reader, compare byte-for-byte. qemu emits a dynamic
/// VHD here, so this exercises the BAT + per-block bitmap decode against
/// a real layout.
#[test]
fn our_reader_matches_qemu_populated_pattern() {
    let raw = raw_path("pat-src");
    let vhd = vhd_path("pat-dst");

    let data = pattern(512 * 1024);
    std::fs::write(&raw, &data).unwrap();
    qemu_convert_raw_to_vpc(&raw, &vhd);

    let r = VhdReader::open(&vhd).unwrap();
    let mut buf = vec![0u8; data.len()];
    r.read_at(0, &mut buf).unwrap();
    assert_eq!(buf, data, "byte mismatch reading qemu-produced VHD");
}

/// Cross-write (content): build a fixed VHD with our writer, write a
/// payload, then have qemu convert it to raw and confirm the bytes
/// survived. The strongest single check that our footer + fixed layout
/// is what qemu expects.
#[test]
fn qemu_extracts_bytes_from_vhd_we_created() {
    let vhd = vhd_path("we-made");
    let raw = raw_path("we-made");

    let payload = b"bytes-qemu-must-see-back-0123456789";
    let r = VhdReader::create_fixed(&vhd, 4 * 1024 * 1024).unwrap();
    r.write_at(8192, payload).unwrap();
    r.flush_writes().unwrap();
    drop(r);

    qemu_convert_vpc_to_raw(&vhd, &raw);
    let out = std::fs::read(&raw).unwrap();
    assert_eq!(&out[8192..8192 + payload.len()], payload);
    assert!(
        out[..8192].iter().all(|&b| b == 0),
        "leading bytes must be zero"
    );
}

/// Cross-write (geometry): qemu reads back the exact virtual size our
/// writer encoded into a fixed VHD's footer.
#[test]
fn qemu_reports_our_fixed_vhd_virtual_size() {
    let vhd = vhd_path("geom");
    let r = VhdReader::create_fixed(&vhd, 4 * 1024 * 1024).unwrap();
    let our_size = r.virtual_size();
    drop(r);

    assert_eq!(our_size, qemu_vpc_virtual_size(&vhd));
}
