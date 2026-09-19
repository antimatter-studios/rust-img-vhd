#![no_main]
//! The dynamic disk header: the block size everything is divided by,
//! and the offset of the block allocation table.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(header) = vhd::dynamic::DynamicHeader::parse(data) {
        let _ = header.bitmap_size_bytes();
    }
    let _ = vhd::dynamic::compute_checksum(data);
});
