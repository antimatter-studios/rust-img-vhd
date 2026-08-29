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
//!   * Geometry is cross-checked by reading the CHS triple qemu wrote
//!     into its own footer and asking our ladder about a disk of
//!     exactly that geometry — never by reimplementing the ladder here.
//!     See `qemu_geometry_is_a_fixed_point_of_our_ladder`.

#![cfg(feature = "qemu-validation")]

use std::path::{Path, PathBuf};
use std::process::Command;

use vhd::footer::FOOTER_SIZE;
use vhd::footer_build::chs_for_size;
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

/// The `(cylinders, heads, sectors_per_track)` triple out of the
/// footer at the tail of `path`, bytes 56..60 of the last 512.
///
/// Reading the reference tool's own footer is the point: it is the one
/// number in this suite that our code had no hand in producing.
fn footer_geometry(path: &Path) -> (u16, u8, u8) {
    let bytes = std::fs::read(path).expect("read image");
    assert!(bytes.len() >= FOOTER_SIZE, "image shorter than a footer");
    let footer = &bytes[bytes.len() - FOOTER_SIZE..];
    (
        u16::from_be_bytes([footer[56], footer[57]]),
        footer[58],
        footer[59],
    )
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

    // 4 MiB is 8192 sectors; the spec's ladder answers C=120, H=4,
    // S=17, so the geometry describes 8160 sectors. Spelled out rather
    // than recomputed here — a number this test derives from the code
    // under test cannot contradict it.
    const CHS_DERIVED: u64 = 120 * 4 * 17 * 512; // 4_177_920
    assert_eq!(CHS_DERIVED, 4_177_920);
    let chs_size = CHS_DERIVED;

    let qemu_size = qemu_vpc_virtual_size(&vhd);
    assert!(
        qemu_size == our_size || qemu_size == chs_size,
        "qemu vpc virtual-size {qemu_size} must match either the footer's \
         current-size {our_size} or its CHS-derived size {chs_size}",
    );
}

/// Cross-check (geometry): our CHS ladder against the reference tool's,
/// with no copy of ours standing in for theirs.
///
/// The reference tool creates an image; we read the geometry *it* chose
/// out of the footer *it* wrote, and ask our ladder for the geometry of
/// a disk of exactly `C * H * S * 512` bytes. It must answer the same
/// triple.
///
/// Why that is a real check and not a tautology: for a size that is
/// exactly a representable geometry every division in the spec's ladder
/// divides evenly, so a conforming implementation is a fixed point on
/// its own output. Ours agreeing on all of these means it takes the
/// same rungs in the same order with the same bounds. A mistyped rung,
/// a `>` where the spec says `>=`, or a 1000 where it says 1024 moves
/// at least one of these off its fixed point.
///
/// What this must NOT do is ask both implementations for the geometry
/// of a *requested* size. The reference tool rounds a creation request
/// up to a geometry that covers it; the spec's algorithm truncates. At
/// 4 MiB the tool answers C=121 and the spec answers C=120, and both
/// are right about different questions. Feeding it the geometry rather
/// than the request is what removes that difference.
///
/// `tests/reference_geometry.rs` pins these same geometries as a table
/// so the default `cargo test` keeps the check without the tool; this
/// is the live version that would notice the pins going stale.
#[test]
fn qemu_geometry_is_a_fixed_point_of_our_ladder() {
    // Dynamic subformat throughout: the footer is identical, and the
    // file stays sparse, so the 127 GiB row costs nothing.
    // 76M/84M/100M/126M are not arbitrary: at those sizes the tool
    // settles on head counts between 9 and 15, in a band where
    // `cylinders * heads` rounds up differently against 1024 than
    // against 1000. They are what makes the head divisor checkable
    // here rather than only in the pinned table.
    let sizes = [
        "1M", "4M", "8M", "64M", "68M", "76M", "84M", "100M", "126M", "160M", "512M", "2G", "31G",
        "127G",
    ];

    for size in sizes {
        let p = vhd_path(&format!("geom-{size}"));
        qemu_create(&p, size, None);

        let (c, h, s) = footer_geometry(&p);
        assert!(
            c > 0 && h > 0 && s > 0,
            "reference tool wrote a degenerate geometry {c}/{h}/{s} for {size}"
        );

        let described = c as u64 * h as u64 * s as u64 * 512;
        assert_eq!(
            chs_for_size(described),
            (c, h, s),
            "reference tool chose {c}/{h}/{s} for a {size} image ({described} bytes of \
             geometry); our ladder disagrees about the geometry of that size"
        );
    }
}
