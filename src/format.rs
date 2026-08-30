//! The VHD on-disk layout, named once.
//!
//! The 512-byte footer and the 1024-byte dynamic header are documented
//! at the top of [`crate::footer`] and [`crate::dynamic`]. Before this
//! module those layouts were then *re-typed* as raw slice indices in
//! the parser, the builder, and every test fixture — seven hand-written
//! copies of the checksum window alone.
//!
//! That spread has a specific failure mode worth stating, because it is
//! not the usual one. The tests here are thorough, but each fixture
//! encodes the offsets itself, so **the tests cannot catch an offset
//! error**: a wrong offset in the parser is only caught by a fixture
//! that has the right one, and a fixture written from the same mistaken
//! reading agrees with it. Centralising does not fix that by itself —
//! now everything agrees with one constant, right or wrong — so the
//! guard has to come from outside this crate. Two already exist:
//! `tests/reference_geometry.rs` checks the geometry ladder against
//! geometries produced by an independent implementation, and
//! `tests/qemu_validation.rs` round-trips whole images through an
//! external disk-image tool. Those are what make the constants below
//! answerable.
//!
//! # The test fixtures deliberately do not use these constants
//!
//! Every fixture in `footer.rs`, `dynamic.rs`, `tests/synthetic.rs` and
//! `tests/corruption.rs` writes its bytes at *literal* offsets, and
//! that is on purpose. The parser and the builder must agree, so they
//! share one definition. The fixtures are the second opinion: they were
//! written from the specification, and because they do not import this
//! table, a wrong constant here shows up as a test failure rather than
//! as a crate that agrees with itself.
//!
//! Measured, so it is not a hope: moving `DISK_TYPE` by one byte fails
//! 37 tests, shifting the footer's checksum window fails 22, moving the
//! dynamic header's `BLOCK_SIZE` fails 17. Rewriting those fixtures to
//! use the constants would take all three to zero.
//!
//! [`layout_matches_the_published_specification`] is the same idea
//! written down once explicitly, so the intent survives someone tidying
//! the fixtures.

/// The VHD format is defined in terms of 512-byte sectors throughout —
/// BAT entries count them, block bitmaps have one bit per one, and the
/// footer occupies exactly one.
///
/// This is *not* the block device's sector size and must not be
/// confused with it: a VHD stored on a 4 KiB-sector disk still has
/// 512-byte VHD sectors, because the number is part of the file format
/// rather than a property of the medium.
pub const SECTOR_SIZE: u64 = 512;

/// Byte offsets of the footer's fields, per the layout at the top of
/// [`crate::footer`].
pub mod footer_offsets {
    use std::ops::Range;

    pub const COOKIE: Range<usize> = 0..8;
    pub const FEATURES: usize = 8;
    pub const FILE_FORMAT_VERSION: usize = 12;
    pub const DATA_OFFSET: usize = 16;
    pub const TIMESTAMP: usize = 24;
    pub const CREATOR_APPLICATION: usize = 28;
    pub const CREATOR_VERSION: usize = 32;
    pub const CREATOR_HOST_OS: usize = 36;
    pub const ORIGINAL_SIZE: usize = 40;
    pub const CURRENT_SIZE: usize = 48;
    /// CHS, packed: `u16` cylinders, `u8` heads, `u8` sectors-per-track.
    pub const DISK_GEOMETRY: usize = 56;
    pub const DISK_TYPE: usize = 60;
    /// Zeroed while the checksum over the footer is computed.
    pub const CHECKSUM: Range<usize> = 64..68;
    pub const UNIQUE_ID: Range<usize> = 68..84;
    pub const SAVED_STATE: usize = 84;
}

/// Byte offsets of the dynamic header's fields, per the layout at the
/// top of [`crate::dynamic`].
pub mod dynamic_offsets {
    use std::ops::Range;

    pub const COOKIE: Range<usize> = 0..8;
    pub const DATA_OFFSET: usize = 8;
    pub const TABLE_OFFSET: usize = 16;
    pub const HEADER_VERSION: usize = 24;
    pub const MAX_TABLE_ENTRIES: usize = 28;
    pub const BLOCK_SIZE: usize = 32;
    /// Zeroed while the checksum over the header is computed.
    pub const CHECKSUM: Range<usize> = 36..40;
    pub const PARENT_UNIQUE_ID: Range<usize> = 40..56;
    pub const PARENT_TIMESTAMP: usize = 56;
    /// UTF-16 big-endian, zero-padded to its full width.
    pub const PARENT_UNICODE_NAME: Range<usize> = 64..576;
    /// Eight 24-byte structs.
    pub const PARENT_LOCATORS: usize = 576;
    pub const PARENT_LOCATOR_STRIDE: usize = 24;
    pub const PARENT_LOCATOR_COUNT: usize = 8;
}

/// The `disk_type` wire values. Named because 2, 3 and 4 appear in the
/// parser, the builder and five test fixtures, where the bare digit
/// says nothing.
pub mod disk_type_wire {
    pub const FIXED: u32 = 2;
    pub const DYNAMIC: u32 = 3;
    pub const DIFFERENCING: u32 = 4;
}

/// Read a big-endian `u32` at `off`. All VHD integers are big-endian.
///
/// Panics if `off + 4` is past the end — every caller has already
/// checked the buffer is a whole footer or header, so a short slice
/// here is a bug in this crate, not corrupt input.
pub(crate) fn read_u32(b: &[u8], off: usize) -> u32 {
    u32::from_be_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

/// Read a big-endian `u64` at `off`. See [`read_u32`] on bounds.
pub(crate) fn read_u64(b: &[u8], off: usize) -> u64 {
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

/// The VHD checksum: the one's complement of the byte-wise `u32` sum
/// over the first `size` bytes, with `skip` treated as zero.
///
/// The footer and the dynamic header use the same algorithm over
/// different lengths and with the checksum field at different offsets.
/// Those two differences are the arguments; there is no third one, and
/// writing the loop twice made it look as though there might be.
pub(crate) fn ones_complement_checksum(
    bytes: &[u8],
    size: usize,
    skip: std::ops::Range<usize>,
) -> u32 {
    let mut sum: u32 = 0;
    for (i, b) in bytes.iter().enumerate().take(size) {
        if skip.contains(&i) {
            continue;
        }
        sum = sum.wrapping_add(*b as u32);
    }
    !sum
}

/// The CHS geometry ladder's constants, from the VHD specification's
/// "Disk Geometry" pseudo-code (Microsoft VHD Image Format
/// Specification, October 2006).
///
/// These are not tunable and not derived from anything — they are the
/// literal numbers in the published pseudo-code, and the ladder is only
/// correct if it reproduces them exactly. They are named so a reader
/// can see *which* number is which without reconstructing the
/// pseudo-code from the arithmetic.
pub mod chs {
    /// Ceiling on each field, from the footer's `u16`/`u8`/`u8` widths.
    pub const MAX_CYLINDERS: u64 = 65535;
    pub const MAX_HEADS: u32 = 16;
    pub const MAX_SECTORS_PER_TRACK: u32 = 255;

    /// Above this sector count the ladder cannot represent the disk and
    /// the geometry saturates at `MAX_* `.
    pub const MAX_ADDRESSABLE_SECTORS: u64 =
        MAX_CYLINDERS * MAX_HEADS as u64 * MAX_SECTORS_PER_TRACK as u64;

    /// The threshold at which the spec switches to the saturated
    /// geometry: 63 sectors per track is the last rung of the ladder,
    /// so a disk that exceeds it at maximum cylinders and heads cannot
    /// be described any other way.
    pub const LADDER_CEILING_SECTORS: u64 = MAX_CYLINDERS * MAX_HEADS as u64 * 63;

    /// The three sectors-per-track rungs, tried in order.
    pub const SPT_FIRST: u32 = 17;
    pub const SPT_SECOND: u32 = 31;
    pub const SPT_THIRD: u32 = 63;

    /// The head count the ladder starts from, and never goes below.
    pub const MIN_HEADS: u32 = 4;

    /// The cylinders-per-head budget each rung is checked against. A
    /// legacy BIOS limit, not a property of the disk.
    pub const CYLINDERS_PER_HEAD_LIMIT: u64 = 1024;

    // The two ceilings are products of the constants above, so these
    // hold at compile time rather than in a test — a build failure is a
    // better place to learn that an edit made a branch unreachable.
    const _: () = assert!(MAX_ADDRESSABLE_SECTORS == 65535 * 16 * 255);
    const _: () = assert!(LADDER_CEILING_SECTORS == 65535 * 16 * 63);
    // If the last rung were not below saturation, the ladder's else-arm
    // could never run.
    const _: () = assert!(LADDER_CEILING_SECTORS < MAX_ADDRESSABLE_SECTORS);
    // The rungs are tried in ascending order, and the ladder only ever
    // widens the head count.
    const _: () = assert!(SPT_FIRST < SPT_SECOND && SPT_SECOND < SPT_THIRD);
    const _: () = assert!(MIN_HEADS <= MAX_HEADS);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The offset table, checked against the specification's own table.
    ///
    /// This is a deliberate second copy of every number above — the
    /// only kind of test that can catch a wrong constant, since
    /// everything else in the crate now reads the constant rather than
    /// the specification. The literals here come from the Microsoft VHD
    /// Image Format Specification (October 2006), and are reproduced in
    /// the module docs of [`crate::footer`] and [`crate::dynamic`].
    ///
    /// If this test and the constants disagree, do not "fix" the test
    /// until you have re-read the specification: it is the half with
    /// the independent provenance.
    #[test]
    fn layout_matches_the_published_specification() {
        // Footer, 512 bytes.
        assert_eq!(footer_offsets::COOKIE, 0..8);
        assert_eq!(footer_offsets::FEATURES, 8);
        assert_eq!(footer_offsets::FILE_FORMAT_VERSION, 12);
        assert_eq!(footer_offsets::DATA_OFFSET, 16);
        assert_eq!(footer_offsets::TIMESTAMP, 24);
        assert_eq!(footer_offsets::CREATOR_APPLICATION, 28);
        assert_eq!(footer_offsets::CREATOR_VERSION, 32);
        assert_eq!(footer_offsets::CREATOR_HOST_OS, 36);
        assert_eq!(footer_offsets::ORIGINAL_SIZE, 40);
        assert_eq!(footer_offsets::CURRENT_SIZE, 48);
        assert_eq!(footer_offsets::DISK_GEOMETRY, 56);
        assert_eq!(footer_offsets::DISK_TYPE, 60);
        assert_eq!(footer_offsets::CHECKSUM, 64..68);
        assert_eq!(footer_offsets::UNIQUE_ID, 68..84);
        assert_eq!(footer_offsets::SAVED_STATE, 84);

        // Dynamic header, 1024 bytes.
        assert_eq!(dynamic_offsets::COOKIE, 0..8);
        assert_eq!(dynamic_offsets::DATA_OFFSET, 8);
        assert_eq!(dynamic_offsets::TABLE_OFFSET, 16);
        assert_eq!(dynamic_offsets::HEADER_VERSION, 24);
        assert_eq!(dynamic_offsets::MAX_TABLE_ENTRIES, 28);
        assert_eq!(dynamic_offsets::BLOCK_SIZE, 32);
        assert_eq!(dynamic_offsets::CHECKSUM, 36..40);
        assert_eq!(dynamic_offsets::PARENT_UNIQUE_ID, 40..56);
        assert_eq!(dynamic_offsets::PARENT_TIMESTAMP, 56);
        assert_eq!(dynamic_offsets::PARENT_UNICODE_NAME, 64..576);
        assert_eq!(dynamic_offsets::PARENT_LOCATORS, 576);
        assert_eq!(dynamic_offsets::PARENT_LOCATOR_STRIDE, 24);
        assert_eq!(dynamic_offsets::PARENT_LOCATOR_COUNT, 8);

        // Disk types and the sector size.
        assert_eq!(disk_type_wire::FIXED, 2);
        assert_eq!(disk_type_wire::DYNAMIC, 3);
        assert_eq!(disk_type_wire::DIFFERENCING, 4);
        assert_eq!(SECTOR_SIZE, 512);
    }

    /// No field may overlap the next, and none may run past the end of
    /// the structure it lives in.
    ///
    /// The table above pins each offset individually; this checks they
    /// still describe one coherent layout. A field widened without its
    /// neighbour moving passes every individual assertion and fails
    /// here.
    #[test]
    fn no_field_overlaps_its_neighbour() {
        use crate::dynamic::DYN_HEADER_SIZE;
        use crate::footer::FOOTER_SIZE;

        // (start, width) in ascending order.
        let footer = [
            (footer_offsets::COOKIE.start, 8),
            (footer_offsets::FEATURES, 4),
            (footer_offsets::FILE_FORMAT_VERSION, 4),
            (footer_offsets::DATA_OFFSET, 8),
            (footer_offsets::TIMESTAMP, 4),
            (footer_offsets::CREATOR_APPLICATION, 4),
            (footer_offsets::CREATOR_VERSION, 4),
            (footer_offsets::CREATOR_HOST_OS, 4),
            (footer_offsets::ORIGINAL_SIZE, 8),
            (footer_offsets::CURRENT_SIZE, 8),
            (footer_offsets::DISK_GEOMETRY, 4),
            (footer_offsets::DISK_TYPE, 4),
            (
                footer_offsets::CHECKSUM.start,
                footer_offsets::CHECKSUM.len(),
            ),
            (
                footer_offsets::UNIQUE_ID.start,
                footer_offsets::UNIQUE_ID.len(),
            ),
            (footer_offsets::SAVED_STATE, 1),
        ];
        assert_no_overlap(&footer, FOOTER_SIZE, "footer");

        let dynamic = [
            (dynamic_offsets::COOKIE.start, 8),
            (dynamic_offsets::DATA_OFFSET, 8),
            (dynamic_offsets::TABLE_OFFSET, 8),
            (dynamic_offsets::HEADER_VERSION, 4),
            (dynamic_offsets::MAX_TABLE_ENTRIES, 4),
            (dynamic_offsets::BLOCK_SIZE, 4),
            (
                dynamic_offsets::CHECKSUM.start,
                dynamic_offsets::CHECKSUM.len(),
            ),
            (
                dynamic_offsets::PARENT_UNIQUE_ID.start,
                dynamic_offsets::PARENT_UNIQUE_ID.len(),
            ),
            (dynamic_offsets::PARENT_TIMESTAMP, 4),
            (
                dynamic_offsets::PARENT_UNICODE_NAME.start,
                dynamic_offsets::PARENT_UNICODE_NAME.len(),
            ),
            (
                dynamic_offsets::PARENT_LOCATORS,
                dynamic_offsets::PARENT_LOCATOR_STRIDE * dynamic_offsets::PARENT_LOCATOR_COUNT,
            ),
        ];
        assert_no_overlap(&dynamic, DYN_HEADER_SIZE, "dynamic header");
    }

    fn assert_no_overlap(fields: &[(usize, usize)], size: usize, what: &str) {
        let mut reached = 0usize;
        for (start, width) in fields {
            assert!(
                *start >= reached,
                "{what}: field at {start} overlaps the one ending at {reached}"
            );
            reached = start + width;
            assert!(reached <= size, "{what}: field at {start} runs past {size}");
        }
    }
}
