//! Pure-Rust VHD (Microsoft Virtual Hard Disk) reader.
//!
//! Supports fixed, dynamic, and differencing variants. Implements
//! [`fs_core::BlockRead`] and [`fs_core::BlockDevice`] so the reader
//! plugs straight into the partition probe + filesystem driver stack.
//!
//! See the crate-level
//! [`README`](https://github.com/antimatter-studios/rust-img-vhd) for
//! design and roadmap.

#![deny(unsafe_op_in_unsafe_fn)]

pub mod capi;
pub mod dynamic;
pub mod error;
pub mod footer;
pub mod footer_build;
pub mod reader;

pub use error::{Error, Result};
pub use footer::{DiskType, Footer};
pub use reader::VhdReader;
