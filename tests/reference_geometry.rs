//! CHS geometry checked against something other than itself.
//!
//! The footer's legacy `disk_geometry` word is produced by
//! [`vhd::footer_build::chs_for_size`], a literal transcription of the
//! pseudo-code in the Microsoft VHD Image Format Specification (October
//! 2006), section "Disk Geometry". A transcription can be wrong — a
//! rung of the sectors-per-track ladder mistyped, a `>=` written as a
//! `>`, `1024` written as `1000` — and the only way to notice is to
//! compare the result with a number that did not come out of the same
//! transcription.
//!
//! This suite holds two kinds of such numbers, and nothing else. It
//! deliberately contains no second implementation of the ladder: a copy
//! of the algorithm agrees with the original by construction and would
//! pass whether or not either one is right.
//!
//! ## 1. Geometries produced by an independent implementation
//!
//! [`REFERENCE_TOOL_GEOMETRIES`] is captured from images created by the
//! third-party VHD implementation that the `qemu-validation` suite
//! cross-checks against — its own answer, from its own code, for its
//! own images. Each row is the `(cylinders, heads, sectors-per-track)`
//! triple read out of bytes 56..60 of a footer that tool wrote, paired
//! with `C * H * S * 512`: the exact byte size that geometry describes.
//!
//! The check is a fixed-point one. For a size that is exactly a
//! representable geometry, the spec's ladder must return that geometry
//! back — every division divides evenly, so there is nothing to round.
//! An implementation that agrees with the reference on all of these has
//! the same ladder: the same rungs in the same order, the same head
//! bound, the same 1024. One that does not, does not.
//!
//! Note that these are *not* "what the reference tool answers for a
//! requested size". That tool rounds a creation request **up** to a
//! geometry that covers it, where the spec's algorithm rounds **down**;
//! asking both the same question would produce a spurious mismatch (at
//! 4 MiB it answers `C=121` where the spec answers `C=120`). The sizes
//! here are the geometries themselves, where the two questions coincide.
//!
//! ## 2. Sizes worked out by hand from the spec text
//!
//! [`SPEC_DERIVED_GEOMETRIES`] pins the cases the reference rows cannot
//! reach — chiefly the truncation, which is exactly where a
//! rounding-up creator and the spec part company, plus the two clamps
//! and the boundaries between ladder rungs. Each row records the
//! arithmetic it came from so it can be re-checked against the spec
//! without running anything.
//!
//! The live counterpart of section 1 — regenerating those geometries
//! from the reference tool rather than trusting the pinned copy — is
//! `qemu_geometry_is_a_fixed_point_of_our_ladder` in
//! `tests/qemu_validation.rs`, behind the `qemu-validation` feature.

use vhd::footer_build::chs_for_size;

/// `(size_bytes, cylinders, heads, sectors_per_track)`.
///
/// Geometries read out of footers written by an independent VHD
/// implementation, with `size_bytes = C * H * S * 512`. See the module
/// docs for why the size column is derived from the geometry rather
/// than being the size that was requested of that tool.
const REFERENCE_TOOL_GEOMETRIES: &[(u64, u16, u8, u8)] = &[
    (1_079_296, 31, 4, 17),
    (4_212_736, 121, 4, 17),
    (8_390_656, 241, 4, 17),
    (67_125_248, 964, 8, 17),
    // Heads = 9 — a value neither the `heads < 4` floor nor the
    // 16-head rungs can produce, so it only appears if the head
    // calculation is right.
    (71_364_096, 911, 9, 17),
    // Four rows where `cylinders * heads` sits in a band whose ceiling
    // against 1024 and against 1000 differ, so they pin the head
    // divisor itself rather than just the ladder it feeds.
    (79_746_048, 1018, 9, 17),
    (88_084_480, 1012, 10, 17),
    (104_865_792, 1004, 12, 17),
    (132_126_720, 1012, 15, 17),
    (167_862_272, 661, 16, 31),
    (537_255_936, 1041, 16, 63),
    (2_147_991_552, 4162, 16, 63),
    // The 63-rung at scale, and two rows on the 255-spt maxed-out
    // branch — the branch the reference tool reaches only for disks
    // past ~31.5 GiB.
    (33_286_127_616, 64496, 16, 63),
    (33_822_351_360, 16191, 16, 255),
    (136_365_219_840, 65279, 16, 255),
    (136_899_993_600, 65535, 16, 255),
];

/// `(label, size_bytes, cylinders, heads, sectors_per_track)`.
///
/// Worked out by hand from the spec's pseudo-code. `ts` below is
/// `size_bytes / 512`; `cth` is the pseudo-code's `cylinderTimesHeads`.
const SPEC_DERIVED_GEOMETRIES: &[(&str, u64, u16, u8, u8)] = &[
    // ts=2048; cth=2048/17=120; heads=ceil(120/1024)=1 -> floored to 4;
    // 120 < 4*1024; cyls=120/4=30.
    ("1 MiB", 1024 * 1024, 30, 4, 17),
    // ts=8192; cth=8192/17=481; heads floored to 4; cyls=481/4=120.
    // 120*4*17 = 8160 sectors: the geometry is 16 KiB SHORT of 4 MiB,
    // which is the truncation this row exists to pin. A creator that
    // rounds up to cover the request answers 121 here.
    ("4 MiB", 4 * 1024 * 1024, 120, 4, 17),
    // ts=16384; cth=16384/17=963; heads floored to 4; cyls=963/4=240.
    ("8 MiB", 8 * 1024 * 1024, 240, 4, 17),
    // ts=131072; cth=131072/17=7710; heads=ceil(7710/1024)=8;
    // 7710 < 8*1024 so the 17-rung holds; cyls=7710/8=963.
    ("64 MiB", 64 * 1024 * 1024, 963, 8, 17),
    // ts=139264; cth=139264/17=8192 EXACTLY; heads=ceil(8192/1024)=8;
    // 8192 >= 8*1024 is true by equality, so the ladder steps to 31.
    // The one row that distinguishes `>=` from `>` in that test.
    (
        "68 MiB (exact 1024 multiple)",
        68 * 1024 * 1024,
        280,
        16,
        31,
    ),
    // ts=327680; cth=327680/17=19275; heads=ceil(19275/1024)=19 > 16
    // -> spt=31, heads=16, cth=327680/31=10570; 10570 < 16*1024 so the
    // ladder stops at 31; cyls=10570/16=660.
    ("160 MiB", 160 * 1024 * 1024, 660, 16, 31),
    // ts=1048576; the 17-rung gives heads=61 > 16 -> 31-rung gives
    // cth=33825 >= 16*1024 -> 63-rung: cth=1048576/63=16644;
    // cyls=16644/16=1040.
    ("512 MiB", 512 * 1024 * 1024, 1040, 16, 63),
    // ts=4194304; same two steps down to the 63-rung;
    // cth=4194304/63=66576; cyls=66576/16=4161.
    ("2 GiB", 2 * 1024 * 1024 * 1024, 4161, 16, 63),
    // ts=66059279 — ONE SECTOR below the `>= 65535*16*63` test, so the
    // maxed-out branch must not fire: 63-rung, cth=66059279/63=1048559,
    // cyls=1048559/16=65534.
    (
        "one sector below the 255-spt threshold",
        66_059_279 * 512,
        65534,
        16,
        63,
    ),
    // ts=66059280 == 65535*16*63 exactly: the maxed-out branch fires.
    // spt=255, heads=16, cth=66059280/255=259056, cyls=259056/16=16191.
    // Note the cylinder count DROPS from 65534 to 16191 across this
    // one-sector step — the two rows together pin which side of the
    // comparison each size falls on.
    ("at the 255-spt threshold", 66_059_280 * 512, 16191, 16, 255),
    // ts capped at 65535*16*255=267382800; cth=267382800/255=1048560;
    // cyls=1048560/16=65535, the largest a u16 can hold.
    ("the CHS ceiling", 65535 * 16 * 255 * 512, 65535, 16, 255),
    // Two orders of magnitude past the ceiling: clamps to the same
    // triple rather than wrapping the u16.
    ("1 PiB, far past the ceiling", 1 << 50, 65535, 16, 255),
];

#[test]
fn our_ladder_reproduces_geometries_written_by_an_independent_implementation() {
    for &(size, c, h, s) in REFERENCE_TOOL_GEOMETRIES {
        assert_eq!(
            size,
            c as u64 * h as u64 * s as u64 * 512,
            "table row {c}/{h}/{s} does not describe {size} bytes — the row itself is wrong"
        );
        assert_eq!(
            chs_for_size(size),
            (c, h, s),
            "geometry for {size} bytes disagrees with the reference implementation"
        );
    }
}

#[test]
fn our_ladder_matches_geometries_derived_from_the_spec_text() {
    for &(label, size, c, h, s) in SPEC_DERIVED_GEOMETRIES {
        assert_eq!(
            chs_for_size(size),
            (c, h, s),
            "geometry for {label} ({size} bytes)"
        );
    }
}

/// The geometry describes at most the requested disk, never more —
/// which is what makes `current_size` and not `C*H*S*512` the
/// authoritative size. Checked over both tables plus a sweep, so it
/// holds for sizes no row names.
#[test]
fn geometry_never_describes_more_than_the_requested_size() {
    let ceiling = 65535u64 * 16 * 255 * 512;
    let sizes = SPEC_DERIVED_GEOMETRIES
        .iter()
        .map(|&(_, size, ..)| size)
        .chain(REFERENCE_TOOL_GEOMETRIES.iter().map(|&(size, ..)| size))
        .chain((0..64).map(|i| (i + 1) * 512 * 1013));

    for size in sizes {
        let (c, h, s) = chs_for_size(size);
        let described = c as u64 * h as u64 * s as u64 * 512;
        let expected_cap = size.min(ceiling);
        assert!(
            described <= expected_cap,
            "geometry {c}/{h}/{s} describes {described} bytes for a {size}-byte disk"
        );
    }
}
