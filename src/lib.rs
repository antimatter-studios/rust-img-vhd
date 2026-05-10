//! Pure-Rust VHD (Microsoft Virtual Hard Disk) reader + writer.
//!
//! Supports fixed, dynamic, and differencing variants. Implements
//! [`fs_core::BlockRead`] and [`fs_core::BlockDevice`] so the reader
//! plugs straight into the partition probe + filesystem driver stack.
//!
//! Backing storage is a generic [`fs_core::BlockDevice`]. Path-based
//! constructors wrap a [`fs_core::FileDevice`] internally; the
//! `open_on_device` / `open_rw_on_device` constructors take any
//! caller-supplied device (FSKit block resource, slice reader, etc.).
//!
//! Write support is in for fixed and dynamic disks. Differencing
//! writes are still TODO and surface as [`Error::ReadOnly`].
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
