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

use std::path::Path;
use std::process::Command;

mod common;
use common::{tmp_path_with, TempPath};
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
    tmp_path_with("vhd_qemu", name, ext)
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

/// Several populated blocks, packed by the reference tool, still open
/// and read back — the acceptance half of the BAT overlap walk (#55).
///
/// A 512 KiB pattern fits in one 2 MiB block, so the test above never
/// hands us two allocated blocks. Here blocks 0, 1 and 3 carry data and
/// block 2 is zero, so the tool allocates three blocks and packs them
/// end to end. The test asserts that packing from the image's own BAT
/// before opening it: without two entries exactly one block apart, an
/// overlap check that was a sector too strict would pass here unseen.
#[test]
fn our_reader_opens_blocks_the_reference_tool_packed_end_to_end() {
    const MIB: usize = 1024 * 1024;
    let raw = raw_path("packed-src");
    let vhd = vhd_path("packed-dst");

    let mut data = pattern(8 * MIB);
    data[4 * MIB..6 * MIB].fill(0);
    std::fs::write(&raw, &data).unwrap();
    qemu_convert_raw_to_vpc(&raw, &vhd);

    let bytes = std::fs::read(&vhd).unwrap();
    let be32 = |o: usize| u32::from_be_bytes(bytes[o..o + 4].try_into().unwrap());
    let be64 = |o: usize| u64::from_be_bytes(bytes[o..o + 8].try_into().unwrap());
    let footer = bytes.len() - FOOTER_SIZE;
    let dyn_hdr = be64(footer + 16) as usize;
    let bat = be64(dyn_hdr + 16) as usize;
    let entries = be32(dyn_hdr + 28) as usize;
    let block_size = be32(dyn_hdr + 32) as u64;
    let bitmap = (block_size / 512).div_ceil(8).div_ceil(512) * 512;
    let mut allocated: Vec<u64> = (0..entries)
        .map(|i| be32(bat + 4 * i))
        .filter(|&e| e != u32::MAX)
        .map(|e| e as u64 * 512)
        .collect();
    allocated.sort_unstable();
    assert!(
        allocated.len() >= 2
            && allocated
                .windows(2)
                .any(|w| w[1] - w[0] == bitmap + block_size),
        "the reference tool did not pack two blocks end to end ({allocated:?}, \
         block {block_size}, bitmap {bitmap}), so this image cannot witness the boundary"
    );

    let r = VhdReader::open(&vhd).expect("an image the reference tool packed must open");
    let mut buf = vec![0u8; data.len()];
    r.read_at(0, &mut buf).unwrap();
    assert!(
        buf == data,
        "byte mismatch reading a multi-block qemu-produced VHD"
    );
}

/// A reference-tool dynamic image whose trailing footer is zeroed opens
/// from the mirror at offset 0, as it does in the reference tool; with
/// the mirror zeroed too, both refuse (#38).
///
/// The reference tool's answer is asserted in both arms rather than
/// assumed: that it refuses when both copies are gone is what shows it
/// is really using the mirror, not skipping footer validation.
#[test]
fn a_damaged_trailing_footer_is_recovered_from_the_mirror_like_the_reference_tool() {
    let vhd = vhd_path("mirror-fallback");
    qemu_create(&vhd, "4M", None);
    let len = std::fs::metadata(&vhd).unwrap().len();
    let intact_size = qemu_vpc_virtual_size(&vhd);

    let zero = |at: u64| {
        use std::io::{Seek, SeekFrom, Write};
        let mut f = std::fs::OpenOptions::new().write(true).open(&vhd).unwrap();
        f.seek(SeekFrom::Start(at)).unwrap();
        f.write_all(&[0u8; FOOTER_SIZE]).unwrap();
    };

    zero(len - FOOTER_SIZE as u64);
    assert_eq!(
        qemu_vpc_virtual_size(&vhd),
        intact_size,
        "the reference tool no longer recovers a damaged tail, so this test's premise is gone"
    );
    let r = VhdReader::open(&vhd).expect("the mirror at offset 0 is intact");
    assert_eq!(r.virtual_size(), intact_size);
    assert!(r.footer_recovered_from_mirror());
    drop(r);

    zero(0);
    let refused = run_qemu(&["info", "-f", "vpc", vhd.to_str().unwrap()]);
    assert!(
        !refused.status.success(),
        "the reference tool opened an image with no valid footer, so it is not a control"
    );
    assert!(
        VhdReader::open(&vhd).is_err(),
        "no valid footer anywhere must be refused"
    );
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

/// Cross-write (dynamic): a dynamic VHD this crate created and wrote reads
/// back through the reference tool byte for byte (#46).
///
/// Every image of ours qemu-img had read was fixed, so the dynamic write
/// path -- BAT allocation, the sector bitmap, the footer mirror moving
/// past each new block -- had only ever been read by this crate. Writes
/// land in the first block, straddle a block boundary and hit a late
/// block, so blocks are allocated out of order, and the image is made and
/// written through `vhd_tool`, the surface the issue found missing.
#[test]
fn qemu_reads_a_dynamic_vhd_we_created_and_wrote() {
    let vhd = vhd_path("dynamic-we-made");
    let raw = raw_path("dynamic-we-made");
    let tool = env!("CARGO_BIN_EXE_vhd_tool");
    let block = 1u64 << 20;

    let made = Command::new(tool)
        .args([
            "create-dynamic",
            vhd.to_str().unwrap(),
            "16777216",
            "--block-size",
        ])
        .arg(block.to_string())
        .output()
        .unwrap();
    assert!(
        made.status.success(),
        "{}",
        String::from_utf8_lossy(&made.stderr)
    );

    let writes: [(u64, usize, u8); 3] = [
        (12 * block + 17, 3000, 0xC3),
        (block - 1000, 5000, 0xA5),
        (4096, 700, 0x5A),
    ];
    for (i, (offset, len, fill)) in writes.iter().enumerate() {
        let input = tmp("bin", &format!("dynamic-input-{i}"));
        let bytes: Vec<u8> = (0..*len).map(|j| fill.wrapping_add(j as u8)).collect();
        std::fs::write(&input, &bytes).unwrap();
        let wrote = Command::new(tool)
            .args(["write", vhd.to_str().unwrap(), &offset.to_string()])
            .arg(input.as_os_str())
            .output()
            .unwrap();
        assert!(
            wrote.status.success(),
            "{}",
            String::from_utf8_lossy(&wrote.stderr)
        );
    }

    let r = VhdReader::open(&vhd).unwrap();
    assert_eq!(r.disk_type(), DiskType::Dynamic);
    assert_eq!(r.virtual_size(), qemu_vpc_virtual_size(&vhd));
    let size = r.virtual_size() as usize;
    drop(r);

    qemu_convert_vpc_to_raw(&vhd, &raw);
    let theirs = std::fs::read(&raw).unwrap();
    let mut expected = vec![0u8; size];
    for (offset, len, fill) in writes {
        for j in 0..len {
            expected[offset as usize + j] = fill.wrapping_add(j as u8);
        }
    }
    assert_eq!(
        theirs.len(),
        expected.len(),
        "qemu-img's raw is a different size"
    );
    let first_difference = theirs.iter().zip(&expected).position(|(a, b)| a != b);
    assert_eq!(
        first_difference, None,
        "qemu-img read a different byte from our dynamic image"
    );
}

/// The reference tool's reading of `path` as if a CHS-deriving producer
/// had written it: a copy with `creator_application` set to `"vpc "`
/// (and the checksum recomputed), for which qemu-img takes the disk size
/// from the footer's geometry rather than from `current_size`. For our
/// own creator string, `"am  "`, it reads `current_size`. The creator
/// field selects the reading deterministically, on one binary -- it is
/// not a difference between qemu versions (#37).
fn qemu_chs_derived_size(path: &Path) -> u64 {
    let copy = vhd_path("chs-reading");
    let mut bytes = std::fs::read(path).unwrap();
    let at = bytes.len() - FOOTER_SIZE;
    bytes[at + 28..at + 32].copy_from_slice(b"vpc ");
    bytes[at + 64..at + 68].fill(0);
    let sum = !bytes[at..]
        .iter()
        .fold(0u32, |a, &b| a.wrapping_add(b as u32));
    bytes[at + 64..at + 68].copy_from_slice(&sum.to_be_bytes());
    std::fs::write(&copy, &bytes).unwrap();
    qemu_vpc_virtual_size(&copy)
}

/// Cross-write (geometry): both of the reference tool's readings of a
/// fixed VHD we created agree with the size we created (#36, #37).
///
/// A VHD footer carries the size twice -- `current_size` and a legacy
/// CHS geometry -- and readers split on which to believe. The request
/// is rounded up so the two agree (4 MiB -> 4,212,736 = 121/4/17, what
/// `qemu-img create -f vpc x.vhd 4M` writes), so both readings must give
/// the same number. Two single equalities, each asserted:
///
/// - the geometry in the footer we wrote is `chs_for_size` of the size
///   we created -- the check that needs no external opinion;
/// - the reference tool reports that size reading `current_size` (our
///   creator string) AND reading the geometry (a `"vpc "` copy).
///
/// This replaced a disjunction, `qemu_size == our_size || qemu_size ==
/// chs_size`, which could not fail: our creator string always selects
/// the first arm, so any geometry at all passed.
#[test]
fn qemu_reports_our_fixed_vhd_virtual_size() {
    let vhd = vhd_path("geom");
    const REQUESTED: u64 = 4 * 1024 * 1024;
    // Spelled out rather than recomputed: 121 * 4 * 17 * 512, the size
    // the reference tool itself creates for a 4 MiB request.
    const CREATED: u64 = 4_212_736;
    let r = VhdReader::create_fixed(&vhd, REQUESTED).unwrap();
    let our_size = r.virtual_size();
    drop(r);
    assert_eq!(our_size, CREATED, "the request rounds up to 121/4/17");

    assert_eq!(
        footer_geometry(&vhd),
        chs_for_size(our_size),
        "the footer's geometry is not the ladder's geometry for the size created"
    );
    assert_eq!(footer_geometry(&vhd), (121, 4, 17));

    assert_eq!(
        qemu_vpc_virtual_size(&vhd),
        our_size,
        "the reference tool's current_size reading"
    );
    assert_eq!(
        qemu_chs_derived_size(&vhd),
        our_size,
        "the reference tool's CHS reading loses the tail of the image"
    );
}

/// `create_fixed` makes the image the reference tool makes for the same
/// request: same `current_size`, same geometry, for sizes on and off the
/// ladder -- including below 34,816 bytes, where the unrounded geometry
/// has zero cylinders and a CHS reading is an empty disk (#35) -- and the
/// reference tool's CHS reading of ours is the whole disk.
#[test]
fn create_fixed_rounds_every_request_the_way_the_reference_tool_does() {
    const MIB: u64 = 1024 * 1024;
    for requested in [
        512,
        4096,
        34_304,
        34_816,
        35_328,
        MIB,
        4 * MIB,
        8 * MIB,
        64 * MIB + 512,
        512 * MIB,
    ] {
        let theirs = vhd_path(&format!("ref-{requested}"));
        qemu_create(&theirs, &requested.to_string(), Some("fixed"));
        let their_bytes = std::fs::metadata(&theirs).unwrap().len() - FOOTER_SIZE as u64;

        let ours = vhd_path(&format!("ours-{requested}"));
        let r = VhdReader::create_fixed(&ours, requested).unwrap();
        assert_eq!(
            r.virtual_size(),
            their_bytes,
            "{requested}: created size differs from the reference tool's"
        );
        drop(r);
        assert_eq!(
            footer_geometry(&ours),
            footer_geometry(&theirs),
            "{requested}: geometry differs from the reference tool's"
        );
        assert_eq!(
            qemu_chs_derived_size(&ours),
            their_bytes,
            "{requested}: a CHS-deriving reader does not see the whole disk"
        );
    }
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

/// The BAT's file offset and entry count, read out of a real image's own
/// dynamic header.
///
/// Reading the reference tool's layout rather than assuming ours is the
/// point: the bound this exercises has to hold for the images other
/// producers write, not only for the fixtures in `synthetic.rs`.
fn bat_location(path: &Path) -> (u64, u32) {
    let bytes = std::fs::read(path).expect("read image");
    let data_offset = u64::from_be_bytes(bytes[16..24].try_into().unwrap());
    let h = &bytes[data_offset as usize..data_offset as usize + 1024];
    (
        u64::from_be_bytes(h[16..24].try_into().unwrap()),
        u32::from_be_bytes(h[28..32].try_into().unwrap()),
    )
}

fn patch(path: &Path, off: u64, bytes: &[u8]) {
    use std::io::{Seek, SeekFrom, Write};
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    f.seek(SeekFrom::Start(off)).unwrap();
    f.write_all(bytes).unwrap();
}

/// A BAT entry naming the image's own metadata is refused, on an image
/// the reference tool produced.
///
/// The synthetic fixtures pin the same rule, but they are laid out by
/// this repository, so they cannot say whether the bound is right for
/// anybody else's images. This one converts a real pattern to a real
/// dynamic VHD, checks it reads correctly, and only then moves one
/// entry — so a bound that was too tight would fail on the first half
/// rather than pass vacuously on the second.
#[test]
fn a_bat_entry_in_the_metadata_is_refused_on_a_qemu_produced_image() {
    let raw = raw_path("bat-src");
    let vhd = vhd_path("bat-dst");

    let data = pattern(3 * 1024 * 1024);
    std::fs::write(&raw, &data).unwrap();
    qemu_convert_raw_to_vpc(&raw, &vhd);

    // Positive control: the untouched image opens and reads.
    {
        let r = VhdReader::open(&vhd).unwrap();
        assert_eq!(r.disk_type(), DiskType::Dynamic);
        let mut buf = vec![0u8; data.len()];
        r.read_at(0, &mut buf).unwrap();
        assert_eq!(buf, data, "byte mismatch on the untouched qemu image");
    }

    let (bat_off, entries) = bat_location(&vhd);
    assert!(entries > 0, "qemu image should have BAT entries");
    // Sector 0 is the footer mirror.
    patch(&vhd, bat_off, &0u32.to_be_bytes());

    let err = VhdReader::open(&vhd)
        .err()
        .expect("a BAT entry naming the footer mirror must be refused");
    assert!(matches!(err, vhd::Error::Corrupt(_)), "got {err:?}");
}
