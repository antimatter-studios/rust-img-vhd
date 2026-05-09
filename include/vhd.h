/*
 * am-img-vhd C ABI — opens a VHD (Microsoft Virtual Hard Disk) and
 * returns a generic FsCoreDevice handle. Once opened, all further
 * interaction goes through fs_core.h's device API.
 *
 * Link with libam_img_vhd.a and include this header alongside fs_core.h.
 *
 * MIT license. (c) 2026 Antimatter Studios.
 */

#ifndef AM_IMG_VHD_H
#define AM_IMG_VHD_H

#include "fs_core.h"

#ifdef __cplusplus
extern "C" {
#endif

/*
 * Open `path` (NUL-terminated UTF-8) as a VHD image, read-only. Returns
 * a generic device handle; free via `fs_core_device_close`.
 *
 * On failure returns NULL and `fs_core_last_error_message()` has detail.
 *
 * Currently supported variants:
 *   - Fixed VHD (footer at end of file, plain pass-through)
 *   - Dynamic VHD (BAT + sparse blocks + per-sector bitmap)
 *   - Differencing VHD (parent-VHD chain, fall-through reads)
 */
FsCoreDevice *vhd_open(const char *path);

/*
 * Open `path` read-write as a VHD image. Fixed and dynamic VHDs accept
 * writes; `fs_core_device_write_at` on a differencing handle returns
 * FS_CORE_READ_ONLY until that path lands.
 *
 * On failure returns NULL.
 */
FsCoreDevice *vhd_open_rw(const char *path);

/*
 * Create a fresh fixed-VHD at `path` of `virtual_size_bytes` bytes and
 * return a RW device handle. `virtual_size_bytes` must be a positive
 * multiple of 512.
 *
 * On failure returns NULL.
 */
FsCoreDevice *vhd_create_fixed(const char *path, uint64_t virtual_size_bytes);

/*
 * Stack a VHD reader on top of an existing FsCoreDevice handle (for
 * example, a callback-backed device wrapping an FSKit
 * FSBlockDeviceResource, or a slice reader). On success the returned
 * handle owns the input device; the caller must NOT call
 * fs_core_device_close on `inner` afterwards. On failure the input is
 * freed automatically and the function returns NULL.
 *
 * Differencing images are rejected here because parent resolution
 * needs a real path.
 */
FsCoreDevice *vhd_open_on_device(FsCoreDevice *inner);

/*
 * Read-write variant of vhd_open_on_device. The input device must
 * report writable; otherwise the open fails with FS_CORE_READ_ONLY
 * and the input is freed.
 */
FsCoreDevice *vhd_open_rw_on_device(FsCoreDevice *inner);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* AM_IMG_VHD_H */
