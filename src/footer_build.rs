//! Footer composition for newly-created VHD images.
//!
//! Produces a fully-valid 512-byte fixed-VHD footer per Microsoft's
//! VHD specification (October 2006). Stdlib only — no UUID crate, no
//! time crate.

use crate::footer::{compute_checksum, FOOTER_COOKIE, FOOTER_SIZE};

/// Disk type byte for fixed VHDs (per spec).
const DISK_TYPE_FIXED: u32 = 2;

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
    f[0..8].copy_from_slice(FOOTER_COOKIE);
    // Features: bit 1 ("reserved" — must be set per spec).
    f[8..12].copy_from_slice(&0x0000_0002u32.to_be_bytes());
    // File format version 1.0.
    f[12..16].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    // Data offset: 0xFFFF... for fixed (no dynamic header).
    f[16..24].copy_from_slice(&u64::MAX.to_be_bytes());
    // Timestamp: seconds since VHD epoch (2000-01-01 UTC).
    f[24..28].copy_from_slice(&vhd_timestamp_now().to_be_bytes());
    // Creator app, version, host OS.
    f[28..32].copy_from_slice(&CREATOR_APP);
    f[32..36].copy_from_slice(&CREATOR_VERSION.to_be_bytes());
    f[36..40].copy_from_slice(&CREATOR_HOST_OS.to_be_bytes());
    // Original + current size.
    f[40..48].copy_from_slice(&virtual_size_bytes.to_be_bytes());
    f[48..56].copy_from_slice(&virtual_size_bytes.to_be_bytes());
    // Disk geometry: u16 cyls + u8 heads + u8 spt.
    let (cyls, heads, spt) = chs_for_size(virtual_size_bytes);
    f[56..58].copy_from_slice(&cyls.to_be_bytes());
    f[58] = heads;
    f[59] = spt;
    // Disk type.
    f[60..64].copy_from_slice(&DISK_TYPE_FIXED.to_be_bytes());
    // Checksum is computed last with bytes 64..68 zeroed.
    // Unique ID (v4 UUID, 16 bytes).
    let uuid = generate_uuid_v4();
    f[68..84].copy_from_slice(&uuid);
    // Saved state = 0 (already zero).
    // Reserved 427 bytes already zero.

    let cs = compute_checksum(&f);
    f[64..68].copy_from_slice(&cs.to_be_bytes());
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

/// Disk geometry pseudo-code from the VHD spec section "Disk Geometry"
/// (October 2006), reproduced literally. Input: total sectors of the
/// virtual disk (size_bytes / 512). Output: (cylinders, heads,
/// sectors-per-track).
///
/// The spec caps everything below the magic 65535*16*255 ceiling and
/// promises the result is "as close as possible" to the requested size.
fn chs_for_size(size_bytes: u64) -> (u16, u8, u8) {
    // VHD spec uses 512-byte sectors throughout.
    let mut total_sectors = size_bytes / 512;
    if total_sectors > 65535u64 * 16 * 255 {
        total_sectors = 65535u64 * 16 * 255;
    }

    let (cylinders_times_heads, heads, sectors_per_track) = if total_sectors >= 65535 * 16 * 63 {
        // Maxed out.
        let spt: u32 = 255;
        let cth = total_sectors / spt as u64;
        let heads: u32 = 16;
        (cth, heads, spt)
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
    (
        cylinders.min(u16::MAX as u64) as u16,
        heads.min(u8::MAX as u32) as u8,
        sectors_per_track.min(u8::MAX as u32) as u8,
    )
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
}
