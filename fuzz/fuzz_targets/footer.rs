#![no_main]
//! The 512-byte footer.
//!
//! Cylinders, heads, sectors-per-track and current_size are multiplied
//! together on the read path, and the checksum is over the footer's own
//! bytes with its checksum field zeroed -- so a crafted footer can be
//! made self-consistent and still describe a disk that does not exist.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = img_vhd::Footer::parse(data);
    let _ = img_vhd::footer::compute_checksum(data);
});
