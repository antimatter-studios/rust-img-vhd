#![no_main]
//! A whole image, opened and read.
//!
//! The footer at the end, the mirror copy at the start, the dynamic
//! header behind it and the block allocation table behind that are four
//! structures each read from an offset the one before it supplied. Which
//! footer copy gets believed when they disagree is a decision a crafted
//! image influences.
use libfuzzer_sys::fuzz_target;
use vhd_fuzz::walk;

fuzz_target!(|data: &[u8]| {
    walk(data);
});
