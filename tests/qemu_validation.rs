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

/// CHS-derived virtual size, per the VHD spec's "Disk Geometry"
/// pseudo-code (Microsoft VHD spec, October 2006). This mirrors the
/// `chs_for_size` routine the writer uses to encode the footer's
/// geometry field, then folds C*H*S back into bytes.
///
/// The VHD footer carries TWO size notions: `current_size` (the exact
/// requested byte count) and the legacy CHS geometry (cylinders, heads,
/// sectors-per-track). For sizes that aren't an exact C*H*S product the
/// geometry rounds *down* — e.g. 8192 sectors yields C=120, H=4, S=17 →
/// 8160 sectors = 4_177_920 bytes, 16_384 bytes short of the requested
/// 4 MiB. Both numbers are spec-correct; they simply describe the disk
/// at different granularities.
fn chs_derived_size(size_bytes: u64) -> u64 {
    let mut total_sectors = size_bytes / 512;
    if total_sectors > 65535u64 * 16 * 255 {
        total_sectors = 65535u64 * 16 * 255;
    }

    let (cylinders_times_heads, heads, sectors_per_track): (u64, u32, u32) =
        if total_sectors >= 65535 * 16 * 63 {
            (total_sectors / 255, 16, 255)
        } else {
            let mut spt: u32 = 17;
            let mut cth = total_sectors / spt as u64;
            let mut heads: u32 = cth.div_ceil(1024) as u32;
            if heads < 4 {
                heads = 4;
            }
            if cth >= (heads as u64 * 1024) || heads > 16 {
                spt = 31;
                heads = 16;
                cth = total_sectors / spt as u64;
            }
            if cth >= (heads as u64 * 1024) {
                spt = 63;
                heads = 16;
                cth = total_sectors / spt as u64;
            }
            (cth, heads, spt)
        };

    let cylinders = if heads == 0 {
        0
    } else {
        cylinders_times_heads / heads as u64
    };
    cylinders * heads as u64 * sectors_per_track as u64 * 512
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

/// Cross-write (geometry): qemu reads back a coherent virtual size from
/// the footer our writer encoded into a fixed VHD.
///
/// vpc-specific quirk: the footer stores both the requested
/// `current_size` (4 MiB here) AND a legacy CHS geometry. For 8192
/// sectors the spec's CHS algorithm yields C=120, H=4, S=17 → 8160
/// sectors = 4_177_920 bytes, 16_384 short of 4 MiB. qemu's `vpc`
/// driver derives its reported `virtual-size` from this geometry, so
/// older qemu reports the CHS-derived 4_177_920 while newer qemu
/// (which prefers the footer's current-size) reports the full
/// 4_194_304. Both are spec-correct readings of the *same* footer — the
/// difference is a qemu-version behaviour, not a bug in our writer.
///
/// We therefore accept either spec-legitimate value: the footer's
/// current-size (`our_size`) or the CHS-derived size. Asserting a single
/// hard equality against current-size would wrongly fail against the
/// many qemu builds that report the CHS-rounded size (this is exactly
/// what the CI runner does). The functional proof — qemu reads a
/// coherent geometry out of the footer we produced — is preserved.
#[test]
fn qemu_reports_our_fixed_vhd_virtual_size() {
    let vhd = vhd_path("geom");
    const REQUESTED: u64 = 4 * 1024 * 1024;
    let r = VhdReader::create_fixed(&vhd, REQUESTED).unwrap();
    let our_size = r.virtual_size();
    drop(r);

    assert_eq!(
        our_size, REQUESTED,
        "writer must keep the requested current-size"
    );

    let chs_size = chs_derived_size(REQUESTED);
    assert_eq!(chs_size, 4_177_920, "VHD CHS rounding for 8192 sectors");

    let qemu_size = qemu_vpc_virtual_size(&vhd);
    assert!(
        qemu_size == our_size || qemu_size == chs_size,
        "qemu vpc virtual-size {qemu_size} must match either the footer's \
         current-size {our_size} or its CHS-derived size {chs_size}",
    );
}
