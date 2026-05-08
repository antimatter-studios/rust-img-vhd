//! C ABI for the VHD reader. Returns a generic [`FsCoreDevice`] handle
//! so consumers route through the same opaque-handle convention every
//! sister crate uses.

#![allow(clippy::missing_safety_doc)]

use crate::VhdReader;
use fs_core::ffi::{set_last_error, FsCoreDevice};
use std::ffi::CStr;
use std::os::raw::c_char;
use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::ptr;
use std::sync::Arc;

/// Open `path` (NUL-terminated UTF-8) as a VHD image and return a
/// generic device handle. On failure returns NULL; consult
/// `fs_core_last_error_message()` for detail.
///
/// Currently read-only — `fs_core_device_write_at` returns
/// FS_CORE_READ_ONLY.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vhd_open(path: *const c_char) -> *mut FsCoreDevice {
    if path.is_null() {
        set_last_error("path is null");
        return ptr::null_mut();
    }
    let res = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let cstr = unsafe { CStr::from_ptr(path) };
        let s = match cstr.to_str() {
            Ok(s) => s,
            Err(_) => {
                set_last_error("path is not valid UTF-8");
                return ptr::null_mut();
            }
        };
        match VhdReader::open(s) {
            Ok(r) => FsCoreDevice::into_handle(Arc::new(r)),
            Err(e) => {
                set_last_error(e.to_string());
                ptr::null_mut()
            }
        }
    }));
    match res {
        Ok(p) => p,
        Err(_) => {
            set_last_error("panic in vhd_open");
            ptr::null_mut()
        }
    }
}

/// Open `path` (NUL-terminated UTF-8) read-write as a VHD image. Only
/// fixed VHDs accept writes today; opening a dynamic or differencing
/// VHD this way succeeds, but `fs_core_device_write_at` on the returned
/// handle returns `FS_CORE_READ_ONLY` until those write paths land.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vhd_open_rw(path: *const c_char) -> *mut FsCoreDevice {
    if path.is_null() {
        set_last_error("path is null");
        return ptr::null_mut();
    }
    let res = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let cstr = unsafe { CStr::from_ptr(path) };
        let s = match cstr.to_str() {
            Ok(s) => s,
            Err(_) => {
                set_last_error("path is not valid UTF-8");
                return ptr::null_mut();
            }
        };
        match VhdReader::open_rw(s) {
            Ok(r) => FsCoreDevice::into_handle(Arc::new(r)),
            Err(e) => {
                set_last_error(e.to_string());
                ptr::null_mut()
            }
        }
    }));
    match res {
        Ok(p) => p,
        Err(_) => {
            set_last_error("panic in vhd_open_rw");
            ptr::null_mut()
        }
    }
}

/// Create a fresh fixed-VHD at `path` of `virtual_size_bytes` bytes and
/// return a RW device handle. `virtual_size_bytes` must be a positive
/// multiple of 512.
///
/// On failure returns NULL; consult `fs_core_last_error_message()`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vhd_create_fixed(
    path: *const c_char,
    virtual_size_bytes: u64,
) -> *mut FsCoreDevice {
    if path.is_null() {
        set_last_error("path is null");
        return ptr::null_mut();
    }
    let res = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let cstr = unsafe { CStr::from_ptr(path) };
        let s = match cstr.to_str() {
            Ok(s) => s,
            Err(_) => {
                set_last_error("path is not valid UTF-8");
                return ptr::null_mut();
            }
        };
        match VhdReader::create_fixed(Path::new(s), virtual_size_bytes) {
            Ok(r) => FsCoreDevice::into_handle(Arc::new(r)),
            Err(e) => {
                set_last_error(e.to_string());
                ptr::null_mut()
            }
        }
    }));
    match res {
        Ok(p) => p,
        Err(_) => {
            set_last_error("panic in vhd_create_fixed");
            ptr::null_mut()
        }
    }
}
