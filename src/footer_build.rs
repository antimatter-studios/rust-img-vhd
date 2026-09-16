//! Footer composition for newly-created VHD images.
//!
//! Produces a fully-valid 512-byte fixed-VHD footer per Microsoft's
//! VHD specification (October 2006). Stdlib only — no UUID crate, no
//! time crate.

use crate::footer::{compute_checksum, FOOTER_COOKIE, FOOTER_SIZE};
use crate::format::footer_offsets as at;
use crate::format::{chs, disk_type_wire, SECTOR_SIZE};

/// Write a big-endian `u32` at `off`. The mirror of
/// [`crate::format::read_u32`]; the builder and the parser now index
/// the same constants, so a field can only move in both at once.
fn write_u32(f: &mut [u8], off: usize, v: u32) {
    f[off..off + 4].copy_from_slice(&v.to_be_bytes());
}

/// Write a big-endian `u64` at `off`. See [`write_u32`].
fn write_u64(f: &mut [u8], off: usize, v: u64) {
    f[off..off + 8].copy_from_slice(&v.to_be_bytes());
}

/// VHD epoch is 2000-01-01 00:00:00 UTC. Unix epoch + 30 years.
const VHD_EPOCH_UNIX_SECS: u64 = 946_684_800;

/// Creator-application tag. Four ASCII bytes; chosen to be unobtrusive
/// and to avoid name-dropping any third-party tool. "am  " = "Antimatter".
const CREATOR_APP: [u8; 4] = *b"am  ";

/// Creator version (major.minor in high/low halves of a u32). 0.1.
const CREATOR_VERSION: u32 = 0x0000_0001;

/// Creator host OS. Spec defines two well-known values:
///   "Wi2k" (0x5769_326B) — Windows
///   "Mac " (0x4D61_6320) — Macintosh
/// We use the Mac tag since the build target is macOS, but the field is
/// informational only — readers don't gate on it.
const CREATOR_HOST_OS: u32 = 0x4D61_6320; // "Mac "

/// Compose a complete 512-byte fixed-VHD footer.
///
/// `virtual_size_bytes` is written into both `original_size` and
/// `current_size`. The CHS geometry is computed via the canonical
/// pseudo-code from the VHD spec section "Disk Geometry". A v4 UUID is
/// generated for `unique_id`. The checksum is the one's-complement of
/// the byte sum with the checksum field zeroed (same routine as
/// [`crate::footer::compute_checksum`]).
pub fn build_fixed_footer(virtual_size_bytes: u64) -> [u8; FOOTER_SIZE] {
    let mut f = [0u8; FOOTER_SIZE];

    // Cookie.
    f[at::COOKIE].copy_from_slice(FOOTER_COOKIE);
    // Features: bit 1 ("reserved" — must be set per spec).
    write_u32(&mut f, at::FEATURES, 0x0000_0002);
    // File format version 1.0.
    write_u32(&mut f, at::FILE_FORMAT_VERSION, 0x0001_0000);
    // Data offset: 0xFFFF... for fixed (no dynamic header).
    write_u64(&mut f, at::DATA_OFFSET, u64::MAX);
    // Timestamp: seconds since VHD epoch (2000-01-01 UTC).
    write_u32(&mut f, at::TIMESTAMP, vhd_timestamp_now());
    // Creator app, version, host OS.
    f[at::CREATOR_APPLICATION..at::CREATOR_APPLICATION + 4].copy_from_slice(&CREATOR_APP);
    write_u32(&mut f, at::CREATOR_VERSION, CREATOR_VERSION);
    write_u32(&mut f, at::CREATOR_HOST_OS, CREATOR_HOST_OS);
    // Original + current size.
    write_u64(&mut f, at::ORIGINAL_SIZE, virtual_size_bytes);
    write_u64(&mut f, at::CURRENT_SIZE, virtual_size_bytes);
    // Disk geometry: u16 cyls + u8 heads + u8 spt.
    let (cyls, heads, spt) = chs_for_size(virtual_size_bytes);
    f[at::DISK_GEOMETRY..at::DISK_GEOMETRY + 2].copy_from_slice(&cyls.to_be_bytes());
    f[at::DISK_GEOMETRY + 2] = heads;
    f[at::DISK_GEOMETRY + 3] = spt;
    // Disk type.
    write_u32(&mut f, at::DISK_TYPE, disk_type_wire::FIXED);
    // Checksum is computed last with bytes 64..68 zeroed.
    // Unique ID (v4 UUID, 16 bytes).
    let uuid = generate_uuid_v4();
    f[at::UNIQUE_ID].copy_from_slice(&uuid);
    // Saved state = 0 (already zero).
    // Reserved 427 bytes already zero.

    let cs = compute_checksum(&f);
    write_u32(&mut f, at::CHECKSUM.start, cs);
    f
}

/// Seconds since the VHD epoch (2000-01-01 00:00:00 UTC), clamped to a
/// non-negative u32. If the system clock is before the epoch (or
/// `SystemTime::now` errors), returns 0.
fn vhd_timestamp_now() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if unix_secs <= VHD_EPOCH_UNIX_SECS {
        0
    } else {
        let v = unix_secs - VHD_EPOCH_UNIX_SECS;
        if v > u32::MAX as u64 {
            u32::MAX
        } else {
            v as u32
        }
    }
}

/// Legacy CHS geometry for a disk of `size_bytes`, per the VHD spec's
/// "Disk Geometry" pseudo-code (Microsoft VHD Image Format
/// Specification, October 2006), reproduced literally.
///
/// Input is a **byte count**; the spec's pseudo-code starts from a
/// sector count, and the conversion happens on the first line of the
/// body. Output is `(cylinders, heads, sectors_per_track)` — the three
/// fields of the footer's `disk_geometry` word.
///
/// The geometry does not have to describe the disk exactly, and in
/// general it does not: every division in the ladder truncates, so
/// `C * H * S * 512` is the requested size **rounded down** to the
/// nearest representable geometry. At 4 MiB (8192 sectors) the answer
/// is `C=120, H=4, S=17` — 8160 sectors, 16 KiB short of the request.
/// That is why the footer carries `current_size` separately: the byte
/// count is exact, the geometry is an approximation, and a reader that
/// derives one from the other will be off by up to a track.
///
/// Sizes above [`chs::MAX_ADDRESSABLE_SECTORS`] clamp to it, so the
/// returned tuple always fits the footer's `u16`/`u8`/`u8` fields.
///
/// Public because the geometry tests validate it directly against
/// geometries produced by an independent VHD implementation — see
/// `tests/reference_geometry.rs`.
pub fn chs_for_size(size_bytes: u64) -> (u16, u8, u8) {
    let total_sectors = (size_bytes / SECTOR_SIZE).min(chs::MAX_ADDRESSABLE_SECTORS);

    let (cylinders_times_heads, heads, sectors_per_track) =
        if total_sectors >= chs::LADDER_CEILING_SECTORS {
            // Past the last rung: nothing smaller can describe the disk, so
            // the geometry saturates.
            let spt = chs::MAX_SECTORS_PER_TRACK;
            (total_sectors / spt as u64, chs::MAX_HEADS, spt)
        } else {
            let mut spt = chs::SPT_FIRST;
            let mut cth = total_sectors / spt as u64;
            let mut heads = cth.div_ceil(chs::CYLINDERS_PER_HEAD_LIMIT) as u32;
            if heads < chs::MIN_HEADS {
                heads = chs::MIN_HEADS;
            }
            if cth >= (heads as u64 * chs::CYLINDERS_PER_HEAD_LIMIT) || heads > chs::MAX_HEADS {
                spt = chs::SPT_SECOND;
                heads = chs::MAX_HEADS;
                cth = total_sectors / spt as u64;
            }
            if cth >= (heads as u64 * chs::CYLINDERS_PER_HEAD_LIMIT) {
                spt = chs::SPT_THIRD;
                heads = chs::MAX_HEADS;
                cth = total_sectors / spt as u64;
            }
            (cth, heads, spt)
        };

    let cylinders = if heads == 0 {
        0
    } else {
        cylinders_times_heads / heads as u64
    };
    (
        cylinders.min(u16::MAX as u64) as u16,
        heads.min(u8::MAX as u32) as u8,
        sectors_per_track.min(u8::MAX as u32) as u8,
    )
}

/// The size a created image should have so that its CHS geometry
/// describes it exactly: the request rounded UP to `C * H * S * 512`.
///
/// [`chs_for_size`] rounds DOWN, so writing a request's own geometry
/// beside its `current_size` puts two different disk sizes in one
/// footer. A reader that derives the size from CHS (Virtual PC always;
/// qemu-img for some creator strings) then loses up to a track at the
/// tail, and below 34,816 bytes -- where the ladder yields zero
/// cylinders -- the whole disk. Rounding the request up makes the two
/// fields agree by construction.
///
/// This is qemu-img's rule for `create -f vpc`: step the sector count
/// up until the ladder's geometry for it covers the request, then use
/// that geometry's size. `qemu-img create -f vpc x.vhd 4M` writes
/// 4,212,736 bytes (121/4/17), and a 4 KiB request becomes 34,816
/// (1/4/17).
///
/// A request larger than the most the geometry can address
/// ([`chs::MAX_ADDRESSABLE_SECTORS`]) cannot be described exactly at
/// any size and is returned unchanged. `size_bytes` must be a multiple
/// of 512.
pub fn size_with_exact_geometry(size_bytes: u64) -> u64 {
    let wanted = size_bytes / SECTOR_SIZE;
    let described = |sectors: u64| {
        let (c, h, s) = chs_for_size(sectors * SECTOR_SIZE);
        c as u64 * h as u64 * s as u64
    };
    let mut sectors = wanted;
    while sectors <= chs::MAX_ADDRESSABLE_SECTORS {
        let covered = described(sectors);
        if covered >= wanted {
            return covered * SECTOR_SIZE;
        }
        sectors += 1;
    }
    size_bytes
}

/// Generate a v4 (random) UUID per RFC 4122. Source of randomness:
///   1. `/dev/urandom` if available (Unix, macOS).
///   2. Fallback: a SplitMix64-style PRNG seeded from `SystemTime`.
///
/// Returns 16 bytes with version (high nibble of byte 6) set to 4 and
/// variant (high two bits of byte 8) set to 0b10.
fn generate_uuid_v4() -> [u8; 16] {
    let mut buf = [0u8; 16];
    if !fill_from_urandom(&mut buf) {
        fill_from_clock_prng(&mut buf);
    }
    // Set version 4.
    buf[6] = (buf[6] & 0x0F) | 0x40;
    // Set variant 10xxxxxx.
    buf[8] = (buf[8] & 0x3F) | 0x80;
    buf
}

/// Try to read 16 bytes from `/dev/urandom`. Returns `false` if the
/// device is unavailable (non-Unix targets, hardened sandbox, etc.).
fn fill_from_urandom(buf: &mut [u8]) -> bool {
    use std::fs::File;
    use std::io::Read;
    match File::open("/dev/urandom") {
        Ok(mut f) => f.read_exact(buf).is_ok(),
        Err(_) => false,
    }
}

/// SplitMix64 seeded from the wall clock + a process-local counter.
/// Not cryptographically strong; only used when `/dev/urandom` is
/// unreachable. UUID v4 collision resistance is best-effort here; for
/// the fixed-VHD use case the unique_id is informational.
fn fill_from_clock_prng(buf: &mut [u8]) {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut state = nanos.wrapping_add(n.wrapping_mul(0x9E37_79B9_7F4A_7C15));

    for chunk in buf.chunks_mut(8) {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        let bytes = z.to_be_bytes();
        for (dst, src) in chunk.iter_mut().zip(bytes.iter()) {
            *dst = *src;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::footer::Footer;

    /// ABOVE THE CEILING THE SIZE IS KEPT AND THE GEOMETRY SATURATES
    /// (review on #78). At the ceiling itself the geometry still describes
    /// the disk exactly; one sector past it, the request comes back
    /// unchanged and CHS reports the ceiling, as qemu-img writes for a
    /// 200 GiB fixed VHD. A change that silently capped or refused such a
    /// size, or started rounding it, fails here.
    #[test]
    fn a_size_above_the_chs_ceiling_is_kept_with_saturated_geometry() {
        let ceiling = chs::MAX_ADDRESSABLE_SECTORS * SECTOR_SIZE;
        assert_eq!(chs_for_size(ceiling), (65535, 16, 255));
        assert_eq!(size_with_exact_geometry(ceiling), ceiling);

        for above in [ceiling + SECTOR_SIZE, 200 * (1u64 << 30)] {
            assert_eq!(
                size_with_exact_geometry(above),
                above,
                "a request above the ceiling is created at its own size"
            );
            assert_eq!(
                chs_for_size(above),
                (65535, 16, 255),
                "and its geometry saturates at the maximum"
            );
        }
    }

    #[test]
    fn built_footer_round_trips_through_parser() {
        let f = build_fixed_footer(64 * 1024 * 1024);
        let parsed = Footer::parse(&f).expect("parse");
        assert_eq!(parsed.current_size, 64 * 1024 * 1024);
        assert_eq!(parsed.original_size, 64 * 1024 * 1024);
        assert_eq!(parsed.disk_type, crate::footer::DiskType::Fixed);
        assert_eq!(parsed.data_offset, u64::MAX);
    }

    #[test]
    fn uuid_v4_has_correct_version_and_variant_bits() {
        for _ in 0..32 {
            let id = generate_uuid_v4();
            assert_eq!(id[6] & 0xF0, 0x40, "version nibble");
            assert_eq!(id[8] & 0xC0, 0x80, "variant bits");
        }
    }

    #[test]
    fn chs_geometry_is_within_spec_bounds_for_typical_sizes() {
        for size_mib in [1u64, 8, 64, 512, 2048] {
            let (c, h, s) = chs_for_size(size_mib * 1024 * 1024);
            assert!(c > 0, "{size_mib}MiB cylinders");
            assert!(h > 0 && h <= 16, "{size_mib}MiB heads");
            assert!(s > 0, "{size_mib}MiB spt");
        }
    }

    #[test]
    fn chs_geometry_clamps_at_and_beyond_the_spec_ceiling() {
        // The spec caps total addressable sectors at 65535*16*255. At and
        // beyond that size the geometry must saturate to the maxed-out
        // 16-head / 255-spt configuration and never overflow the u16/u8
        // fields.
        let ceiling_bytes = 65535u64 * 16 * 255 * 512;
        let (c_at, h_at, s_at) = chs_for_size(ceiling_bytes);
        assert_eq!(h_at, 16);
        assert_eq!(s_at, 255);
        assert!(c_at > 0);

        // A pebibyte is far past the ceiling — must clamp to the same
        // geometry rather than wrapping.
        let (c_over, h_over, s_over) = chs_for_size(1u64 << 50);
        assert_eq!(h_over, 16);
        assert_eq!(s_over, 255);
        assert_eq!(c_over, c_at, "beyond-ceiling sizes clamp identically");
    }

    #[test]
    fn built_footer_for_large_disk_has_saturated_geometry() {
        // build_fixed_footer must round-trip even for a disk at the CHS
        // ceiling: the footer parses and reports the requested size.
        let size = 65535u64 * 16 * 255 * 512;
        let f = build_fixed_footer(size);
        let parsed = Footer::parse(&f).expect("parse");
        assert_eq!(parsed.current_size, size);
    }
}
