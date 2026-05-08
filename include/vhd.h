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
 * Open `path` read-write as a VHD image. Only fixed VHDs accept writes
 * today: `fs_core_device_write_at` on a dynamic or differencing handle
 * returns FS_CORE_READ_ONLY until those paths land.
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

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* AM_IMG_VHD_H */
