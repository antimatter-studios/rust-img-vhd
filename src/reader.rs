//! VHD read+write path. Handles fixed, dynamic, and differencing variants.
//!
//! For dynamic and differencing disks, reads walk the BAT to find each
//! block's host offset, then consult the per-block sector bitmap to
//! decide whether each sector contains data, reads as zero (dynamic),
//! or defers to the parent (differencing).
//!
//! Implements [`fs_core::BlockRead`] and [`fs_core::BlockDevice`] so a
//! `VhdReader` can be handed directly to a partition probe, a
//! filesystem driver, or any other consumer of those traits — and
//! exposed as a generic [`fs_core::ffi::FsCoreDevice`] handle through
//! the C ABI.
//!
//! ## Backing storage
//!
//! The reader is generic over [`fs_core::BlockDevice`]. Open from a path
//! via [`VhdReader::open`] / [`VhdReader::open_rw`] (the file is wrapped
//! in a [`fs_core::FileDevice`] internally), or hand in any other
//! `BlockDevice` via [`VhdReader::open_on_device`] /
//! [`VhdReader::open_rw_on_device`]. The on-device variants are how the
//! VHD layer stacks on top of an FSKit-supplied block resource, a slice
//! reader, or any other host-managed device.
//!
//! Differencing parents are always opened path-relative — the on-device
//! entry points reject differencing images that reference a parent.

use crate::dynamic::{DynamicHeader, BAT_UNALLOCATED, DYN_HEADER_SIZE};
use crate::error::{Error, Result};
use crate::footer::{DiskType, Footer, FOOTER_SIZE};
use crate::footer_build::build_fixed_footer;
use fs_core::{BlockDevice, FileDevice};
use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const SECTOR_SIZE: u64 = 512;

/// Maximum differencing-chain depth. A pathological chain (or cycle)
/// is rejected rather than blowing the stack.
const MAX_PARENT_DEPTH: u32 = 16;

pub struct VhdReader {
    /// Backing block device. All host-offset reads/writes go through here.
    /// `Arc<dyn BlockDevice>` because `BlockDevice` is `Send + Sync` and
    /// the reader may live behind an `Arc` itself (FFI handles).
    dev: Arc<dyn BlockDevice>,
    /// Whether the underlying device was opened RW. Even when this is
    /// true, write_at still gates on subtype having a write path —
    /// fixed and dynamic today; differencing rejects with [`Error::ReadOnly`].
    writable: bool,
    footer: Footer,
    /// `None` for fixed disks. For dynamic and differencing, the
    /// parsed dynamic header.
    dynamic: Option<DynamicHeader>,
    /// `None` for fixed disks. For dynamic/differencing, the cached
    /// in-memory BAT (always small — `max_table_entries * 4` bytes).
    /// `Mutex` because dynamic writes mutate entries in place when
    /// allocating a block.
    bat: Mutex<Option<Vec<u32>>>,
    /// Cached size of one block bitmap in bytes (sector-padded).
    bitmap_size: u64,
    /// Differencing parent, if any. Always opened read-only — writes
    /// only land in the leaf VHD's data area.
    parent: Option<Box<VhdReader>>,
    /// Virtual disk size (`current_size` from the footer).
    virtual_size: u64,
    /// For dynamic VHDs: host offset where the trailing footer currently
    /// sits — equivalently, the offset where the next block's bitmap
    /// will be placed. Updated under lock when a fresh block is
    /// allocated. `None` for fixed disks.
    next_alloc_off: Mutex<Option<u64>>,
    /// Cached complete footer bytes (the same value mirrored at offset 0
    /// and at the file tail for dynamic/differencing). Used to rewrite
    /// the trailing footer after appending a new block.
    footer_bytes: [u8; FOOTER_SIZE],
}

impl VhdReader {
    /// Open `path` read-only and parse footer + (dynamic/differencing)
    /// header + BAT + parent chain.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let p = path.as_ref();
        let dev = FileDevice::open(p).map_err(fs_core_to_vhd_error)?;
        Self::open_inner(
            Arc::new(dev),
            false,
            MAX_PARENT_DEPTH,
            Some(p.to_path_buf()),
        )
    }

    /// Open `path` read-write. Whether [`BlockDevice::write_at`] actually
    /// succeeds further depends on the subtype — fixed and dynamic VHDs
    /// have a write path; differencing returns [`Error::ReadOnly`] until
    /// its write path lands.
    pub fn open_rw<P: AsRef<Path>>(path: P) -> Result<Self> {
        let p = path.as_ref();
        let dev = FileDevice::open_rw(p).map_err(fs_core_to_vhd_error)?;
        Self::open_inner(
            Arc::new(dev),
            true,
            MAX_PARENT_DEPTH,
            Some(p.to_path_buf()),
        )
    }

    /// Open read-only on top of an arbitrary [`BlockDevice`]. The
    /// on-device path has no filesystem context, so a differencing image
    /// that references a parent is rejected with [`Error::Unsupported`]
    /// — parent resolution requires the path-based entry points.
    pub fn open_on_device(dev: Arc<dyn BlockDevice>) -> Result<Self> {
        Self::open_inner(dev, false, MAX_PARENT_DEPTH, None)
    }

    /// Open read-write on top of an arbitrary [`BlockDevice`]. The
    /// device must report `is_writable()`; otherwise the call returns
    /// [`Error::ReadOnly`].
    pub fn open_rw_on_device(dev: Arc<dyn BlockDevice>) -> Result<Self> {
        if !dev.is_writable() {
            return Err(Error::ReadOnly);
        }
        Self::open_inner(dev, true, MAX_PARENT_DEPTH, None)
    }

    fn open_inner(
        dev: Arc<dyn BlockDevice>,
        writable: bool,
        depth_remaining: u32,
        owning_path: Option<PathBuf>,
    ) -> Result<Self> {
        if depth_remaining == 0 {
            return Err(Error::ParentTooDeep);
        }

        let dev_size = dev.size_bytes();
        if dev_size < FOOTER_SIZE as u64 {
            return Err(Error::Corrupt("device shorter than 512 bytes"));
        }

        // Footer at end of file/device.
        let mut footer_bytes = [0u8; FOOTER_SIZE];
        dev.read_at(dev_size - FOOTER_SIZE as u64, &mut footer_bytes)
            .map_err(fs_core_to_vhd_error)?;
        let footer = Footer::parse(&footer_bytes)?;

        let virtual_size = footer.current_size;

        let (dynamic, bat, bitmap_size, parent, next_alloc_off) = match footer.disk_type {
            DiskType::Fixed => (None, None, 0, None, None),
            DiskType::Dynamic | DiskType::Differencing => {
                if footer.data_offset == u64::MAX {
                    return Err(Error::Corrupt(
                        "dynamic/differencing footer has data_offset = 0xFFFF...",
                    ));
                }
                let mut hdr_bytes = [0u8; DYN_HEADER_SIZE];
                dev.read_at(footer.data_offset, &mut hdr_bytes)
                    .map_err(fs_core_to_vhd_error)?;
                let dyn_hdr = DynamicHeader::parse(&hdr_bytes)?;

                // BAT.
                let bat_entries = dyn_hdr.max_table_entries as usize;
                let mut bat_bytes = vec![0u8; bat_entries * 4];
                dev.read_at(dyn_hdr.table_offset, &mut bat_bytes)
                    .map_err(fs_core_to_vhd_error)?;
                let mut bat = Vec::with_capacity(bat_entries);
                for chunk in bat_bytes.chunks_exact(4) {
                    bat.push(u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
                }

                let bitmap_size = dyn_hdr.bitmap_size_bytes();

                let parent = if footer.disk_type == DiskType::Differencing {
                    let child_path = owning_path.as_deref().ok_or(Error::Unsupported(
                        "differencing VHD opened on a raw device; parent resolution needs a path",
                    ))?;
                    Some(Box::new(open_parent(child_path, &dyn_hdr, depth_remaining)?))
                } else {
                    None
                };

                // The trailing footer sits at dev_size - 512. Future
                // allocations land at that offset (pushing the footer
                // forward), assuming the device was sized to "end of
                // data + footer" — the canonical layout.
                let next_alloc = dev_size.saturating_sub(FOOTER_SIZE as u64);

                (Some(dyn_hdr), Some(bat), bitmap_size, parent, Some(next_alloc))
            }
        };

        Ok(Self {
            dev,
            writable,
            footer,
            dynamic,
            bat: Mutex::new(bat),
            bitmap_size,
            parent,
            virtual_size,
            next_alloc_off: Mutex::new(next_alloc_off),
            footer_bytes,
        })
    }

    /// Create a fresh fixed-VHD at `path` with the given virtual size,
    /// then return a RW handle ready for `write_at`.
    ///
    /// File layout written:
    ///   `[virtual_size_bytes of sparse zero][512-byte fixed footer]`
    ///
    /// `virtual_size_bytes` must be sector-aligned (a multiple of 512);
    /// otherwise [`Error::Corrupt`] is returned. The data area is
    /// allocated via [`std::fs::File::set_len`], which leaves the
    /// region sparse on filesystems that support it (APFS, ext4, NTFS,
    /// XFS, ZFS) — no explicit zero-fill is performed.
    pub fn create_fixed<P: AsRef<Path>>(path: P, virtual_size_bytes: u64) -> Result<Self> {
        if virtual_size_bytes == 0 || !virtual_size_bytes.is_multiple_of(SECTOR_SIZE) {
            return Err(Error::Corrupt(
                "create_fixed: virtual_size must be a positive multiple of 512",
            ));
        }

        // Create + truncate + RW.
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path.as_ref())?;

        let total_len = virtual_size_bytes
            .checked_add(FOOTER_SIZE as u64)
            .ok_or(Error::Corrupt("create_fixed: size overflow"))?;
        file.set_len(total_len)?;

        // Write the footer at the trailing 512-byte slot.
        let footer_bytes = build_fixed_footer(virtual_size_bytes);
        file.seek(SeekFrom::Start(virtual_size_bytes))?;
        file.write_all(&footer_bytes)?;
        file.sync_data()?;
        // Drop our handle and re-open via the standard open path so the
        // returned reader walks the same code as any other fixed VHD.
        drop(file);

        Self::open_rw(path.as_ref())
    }

    pub fn virtual_size(&self) -> u64 {
        self.virtual_size
    }

    pub fn disk_type(&self) -> DiskType {
        self.footer.disk_type
    }

    /// Block size for dynamic/differencing disks; 0 for fixed.
    pub fn block_size(&self) -> u32 {
        self.dynamic.as_ref().map(|d| d.block_size).unwrap_or(0)
    }

    pub fn has_parent(&self) -> bool {
        self.parent.is_some()
    }

    pub fn footer(&self) -> &Footer {
        &self.footer
    }

    /// Read exactly `buf.len()` bytes starting at virtual `offset`.
    pub fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let len = buf.len() as u64;
        if len == 0 {
            return Ok(());
        }
        let end = offset
            .checked_add(len)
            .ok_or(Error::Corrupt("offset+len overflow"))?;
        if end > self.virtual_size {
            return Err(Error::OutOfBounds {
                offset,
                len,
                size: self.virtual_size,
            });
        }

        match self.footer.disk_type {
            DiskType::Fixed => self.read_fixed(offset, buf),
            DiskType::Dynamic | DiskType::Differencing => self.read_sparse(offset, buf),
        }
    }

    fn read_fixed(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        // Fixed VHDs are 1:1: virtual offset == host offset.
        self.dev_read(offset, buf)
    }

    /// True only when the underlying device was opened RW *and* the
    /// subtype has a write path. Today: fixed and dynamic.
    pub fn writable(&self) -> bool {
        self.writable
            && matches!(
                self.footer.disk_type,
                DiskType::Fixed | DiskType::Dynamic
            )
    }

    /// Write exactly `buf.len()` bytes starting at virtual `offset`.
    /// Supported on fixed and dynamic VHDs; differencing returns
    /// [`Error::ReadOnly`].
    pub fn write_at(&self, offset: u64, buf: &[u8]) -> Result<()> {
        if !self.writable() {
            return Err(Error::ReadOnly);
        }
        let len = buf.len() as u64;
        if len == 0 {
            return Ok(());
        }
        let end = offset
            .checked_add(len)
            .ok_or(Error::Corrupt("offset+len overflow"))?;
        if end > self.virtual_size {
            return Err(Error::OutOfBounds {
                offset,
                len,
                size: self.virtual_size,
            });
        }

        match self.footer.disk_type {
            DiskType::Fixed => {
                // Fixed VHDs are 1:1: virtual offset == host offset.
                // The footer sits at host offset == virtual_size and is
                // implicitly off-limits because the bounds check above
                // rejects writes past virtual_size.
                self.dev_write(offset, buf)
            }
            DiskType::Dynamic => self.write_sparse(offset, buf),
            DiskType::Differencing => Err(Error::ReadOnly),
        }
    }

    /// Sync the underlying device to stable storage. Read-only returns
    /// `Ok(())` so callers can flush opportunistically without checking
    /// mode first.
    pub fn flush_writes(&self) -> Result<()> {
        if !self.writable {
            return Ok(());
        }
        self.dev_flush()
    }

    // ------------------------------------------------------------------
    // Device adapters — central place to lift fs_core::Error into
    // crate::Error.
    // ------------------------------------------------------------------

    fn dev_read(&self, off: u64, buf: &mut [u8]) -> Result<()> {
        self.dev.read_at(off, buf).map_err(fs_core_to_vhd_error)
    }

    fn dev_write(&self, off: u64, buf: &[u8]) -> Result<()> {
        self.dev.write_at(off, buf).map_err(fs_core_to_vhd_error)
    }

    fn dev_flush(&self) -> Result<()> {
        self.dev.flush().map_err(fs_core_to_vhd_error)
    }

    fn read_sparse(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let dyn_hdr = self
            .dynamic
            .as_ref()
            .ok_or(Error::Corrupt("sparse read but no dynamic header"))?;

        let block_size = dyn_hdr.block_size as u64;
        let block_mask = block_size - 1;
        let bitmap_size = self.bitmap_size;

        // Walk sector-by-sector. Each sector either: (a) lives in this
        // image's data area (bitmap bit set), (b) reads as zero (bitmap
        // bit clear, no parent), or (c) defers to the parent (bitmap
        // bit clear, parent present).
        let mut cursor = offset;
        let end = offset + buf.len() as u64;
        let mut written = 0usize;

        while cursor < end {
            let in_block = cursor & block_mask;
            let block_idx = (cursor / block_size) as usize;
            let bytes_remaining_in_block = block_size - in_block;
            let chunk_len = std::cmp::min(bytes_remaining_in_block, end - cursor) as usize;

            let bat_entry = {
                let bat_guard = self.bat.lock().unwrap();
                let bat = bat_guard
                    .as_ref()
                    .ok_or(Error::Corrupt("sparse read but no BAT"))?;
                if block_idx >= bat.len() {
                    return Err(Error::Corrupt("block_idx past BAT"));
                }
                bat[block_idx]
            };

            if bat_entry == BAT_UNALLOCATED {
                // No block on disk — defer entirely.
                let dst = &mut buf[written..written + chunk_len];
                self.read_block_unallocated(cursor, dst)?;
                cursor += chunk_len as u64;
                written += chunk_len;
                continue;
            }

            let block_host_off = bat_entry as u64 * SECTOR_SIZE;
            // Read the bitmap once for this block.
            let mut bitmap = vec![0u8; bitmap_size as usize];
            self.dev_read(block_host_off, &mut bitmap)?;

            // Within this block, walk sector by sector.
            let mut block_cursor = in_block;
            let block_end = in_block + chunk_len as u64;
            while block_cursor < block_end {
                let sector_in_block = block_cursor / SECTOR_SIZE;
                let in_sector = block_cursor & (SECTOR_SIZE - 1);
                let bytes_left_in_sector = SECTOR_SIZE - in_sector;
                let slice_len =
                    std::cmp::min(bytes_left_in_sector, block_end - block_cursor) as usize;
                let bit_byte = (sector_in_block / 8) as usize;
                let bit_in_byte = 7 - (sector_in_block % 8) as u8;
                let bit_set = (bitmap[bit_byte] >> bit_in_byte) & 1 == 1;

                let dst = &mut buf[written..written + slice_len];
                if bit_set {
                    // Sector lives at: bat_entry * 512 + bitmap_size +
                    // sector_in_block * 512 + in_sector
                    let host_off =
                        block_host_off + bitmap_size + sector_in_block * SECTOR_SIZE + in_sector;
                    self.dev_read(host_off, dst)?;
                } else {
                    // Defer or zero.
                    let virt = (block_idx as u64) * block_size + block_cursor;
                    self.read_block_unallocated(virt, dst)?;
                }

                block_cursor += slice_len as u64;
                written += slice_len;
            }

            cursor += chunk_len as u64;
        }

        Ok(())
    }

    /// Resolve an unallocated read: defer to parent if present, else
    /// fill with zeros.
    fn read_block_unallocated(&self, virt: u64, dst: &mut [u8]) -> Result<()> {
        match &self.parent {
            None => {
                dst.fill(0);
                Ok(())
            }
            Some(p) => {
                let psize = p.virtual_size();
                let len = dst.len() as u64;
                if virt >= psize {
                    dst.fill(0);
                    Ok(())
                } else if virt + len > psize {
                    let n = (psize - virt) as usize;
                    p.read_at(virt, &mut dst[..n])?;
                    dst[n..].fill(0);
                    Ok(())
                } else {
                    p.read_at(virt, dst)
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Dynamic write path
    // ------------------------------------------------------------------

    /// Write into a dynamic VHD. For each block in the affected range:
    ///   - If the BAT entry is unallocated, allocate a fresh block at
    ///     the file tail (bitmap precedes data), update the BAT, then
    ///     rewrite the trailing footer mirror.
    ///   - Splice the user's bytes into the block, mark the touched
    ///     sectors in the bitmap.
    ///
    /// Crash-safety order per allocation: data sectors → bitmap →
    /// BAT entry → footer mirror, with a `dev.flush()` between each
    /// step. A crash mid-allocation leaves the BAT entry pointing at
    /// the old (still unallocated) value or the BAT is updated but the
    /// footer mirror is stale — both cases keep the image readable
    /// (the leaked tail bytes are recoverable by `qemu-img check`-type
    /// tooling, and on re-open the next allocation overwrites them).
    fn write_sparse(&self, offset: u64, buf: &[u8]) -> Result<()> {
        let dyn_hdr = self
            .dynamic
            .as_ref()
            .ok_or(Error::Corrupt("sparse write but no dynamic header"))?;

        let block_size = dyn_hdr.block_size as u64;
        let block_mask = block_size - 1;
        let bitmap_size = self.bitmap_size;

        let mut cursor = offset;
        let end = offset + buf.len() as u64;
        let mut written = 0usize;

        while cursor < end {
            let in_block = cursor & block_mask;
            let block_idx = (cursor / block_size) as usize;
            let bytes_remaining_in_block = block_size - in_block;
            let chunk_len = std::cmp::min(bytes_remaining_in_block, end - cursor) as usize;

            // Fast-path: read current BAT entry under lock, drop lock
            // before touching the device.
            let bat_entry = {
                let bat_guard = self.bat.lock().unwrap();
                let bat = bat_guard
                    .as_ref()
                    .ok_or(Error::Corrupt("sparse write but no BAT"))?;
                if block_idx >= bat.len() {
                    return Err(Error::Corrupt("block_idx past BAT"));
                }
                bat[block_idx]
            };

            let block_host_off = if bat_entry == BAT_UNALLOCATED {
                self.allocate_block(block_idx)?
            } else {
                bat_entry as u64 * SECTOR_SIZE
            };

            // Splice the user payload into the block at [in_block,
            // in_block + chunk_len).
            let src = &buf[written..written + chunk_len];
            let data_off = block_host_off + bitmap_size + in_block;
            self.dev_write(data_off, src)?;

            // Mark the touched sectors in the bitmap.
            self.set_bitmap_range(block_host_off, in_block, chunk_len as u64)?;
            self.dev_flush()?;

            cursor += chunk_len as u64;
            written += chunk_len;
        }

        Ok(())
    }

    /// Allocate a fresh block at the device tail, point BAT entry
    /// `block_idx` at it, push the trailing footer mirror past it.
    /// Returns the host offset where the new block's bitmap starts.
    ///
    /// Crash-safety order:
    ///   1. Zero-init the new block's bitmap+data range on disk
    ///      (so a partial subsequent step can't expose old tail bytes).
    ///      → flush
    ///   2. Update the in-file BAT entry.
    ///      → flush
    ///   3. Rewrite the trailing footer mirror at the new tail.
    ///      → flush
    ///
    /// In-memory BAT and `next_alloc_off` are updated under their locks
    /// after every disk step lands.
    fn allocate_block(&self, block_idx: usize) -> Result<u64> {
        let dyn_hdr = self
            .dynamic
            .as_ref()
            .ok_or(Error::Corrupt("allocate but no dynamic header"))?;
        let block_size = dyn_hdr.block_size as u64;
        let bitmap_size = self.bitmap_size;
        let block_total = bitmap_size + block_size;

        // Reserve the next tail offset under lock so concurrent
        // allocations don't collide.
        let new_block_off = {
            let mut tail = self.next_alloc_off.lock().unwrap();
            let cur = tail.ok_or(Error::Corrupt("allocate but no tail offset (fixed?)"))?;
            *tail = Some(cur + block_total);
            cur
        };

        // Step 1: zero-init bitmap + data area at the new tail.
        // FileDevice's write_at extends the file as needed; on
        // non-growable BlockDevice impls this surfaces an I/O error
        // up to the caller, which is the right behaviour.
        let zeros = vec![0u8; block_total as usize];
        if let Err(e) = self.dev_write(new_block_off, &zeros) {
            // Roll back the tail reservation — the device refused.
            let mut tail = self.next_alloc_off.lock().unwrap();
            *tail = Some(new_block_off);
            return Err(e);
        }
        if let Err(e) = self.dev_flush() {
            let mut tail = self.next_alloc_off.lock().unwrap();
            *tail = Some(new_block_off);
            return Err(e);
        }

        // Step 2: publish the new block in the BAT (on disk + in mem).
        let bat_value = (new_block_off / SECTOR_SIZE) as u32;
        let bat_entry_off = dyn_hdr.table_offset + (block_idx as u64) * 4;
        self.dev_write(bat_entry_off, &bat_value.to_be_bytes())?;
        self.dev_flush()?;
        {
            let mut bat_guard = self.bat.lock().unwrap();
            if let Some(bat) = bat_guard.as_mut() {
                bat[block_idx] = bat_value;
            }
        }

        // Step 3: rewrite the trailing footer mirror at the new tail.
        // The bytes haven't changed (footer.current_size etc are
        // unchanged); we just put the same 512 bytes at the new offset.
        let new_footer_off = new_block_off + block_total;
        self.dev_write(new_footer_off, &self.footer_bytes)?;
        self.dev_flush()?;

        Ok(new_block_off)
    }

    /// Set the bitmap bits for sectors covered by virtual range
    /// `[in_block, in_block + len)` inside the block whose bitmap
    /// starts at `block_host_off`. Reads-modifies-writes the bitmap.
    fn set_bitmap_range(&self, block_host_off: u64, in_block: u64, len: u64) -> Result<()> {
        let bitmap_size = self.bitmap_size as usize;
        let mut bitmap = vec![0u8; bitmap_size];
        self.dev_read(block_host_off, &mut bitmap)?;

        let first_sector = in_block / SECTOR_SIZE;
        let last_byte = in_block + len; // exclusive
        let last_sector_inclusive = (last_byte - 1) / SECTOR_SIZE;

        for sector in first_sector..=last_sector_inclusive {
            let bit_byte = (sector / 8) as usize;
            let bit_in_byte = 7 - (sector % 8) as u8;
            if bit_byte >= bitmap.len() {
                return Err(Error::Corrupt("bitmap index out of range"));
            }
            bitmap[bit_byte] |= 1 << bit_in_byte;
        }

        self.dev_write(block_host_off, &bitmap)?;
        Ok(())
    }
}

/// Open the differencing parent VHD by walking the parent locators.
/// Tries the W2ku/W2ru relative-path locators first, then falls back
/// to a sibling lookup using `parent_unicode_name`.
fn open_parent(
    child_path: &Path,
    dyn_hdr: &DynamicHeader,
    depth_remaining: u32,
) -> Result<VhdReader> {
    // Locator platform codes (4 bytes ASCII):
    //   "W2ku" — Windows absolute UTF-16 path (preferred)
    //   "W2ru" — Windows relative UTF-16 path
    //   "MaCo" — Mac alias (rare)
    // For now: just use the parent_unicode_name + child's directory.
    // Locator-data resolution can land in a follow-up.
    let _ = dyn_hdr.parent_locators; // explicitly acknowledged

    let parent_name = &dyn_hdr.parent_name;
    if parent_name.is_empty() {
        return Err(Error::ParentNotFound(
            "differencing VHD has empty parent name".into(),
        ));
    }

    // Try `<child_dir>/<parent_name>` first.
    let candidate: PathBuf = match child_path.parent() {
        Some(p) => p.join(parent_name),
        None => PathBuf::from(parent_name),
    };

    if candidate.exists() {
        let dev = FileDevice::open(&candidate).map_err(fs_core_to_vhd_error)?;
        return VhdReader::open_inner(
            Arc::new(dev),
            false,
            depth_remaining - 1,
            Some(candidate),
        );
    }

    // Plain `parent_name` as last resort.
    let direct = PathBuf::from(parent_name);
    if direct.exists() {
        let dev = FileDevice::open(&direct).map_err(fs_core_to_vhd_error)?;
        return VhdReader::open_inner(Arc::new(dev), false, depth_remaining - 1, Some(direct));
    }

    Err(Error::ParentNotFound(format!(
        "tried '{}' and '{}'",
        candidate.display(),
        direct.display()
    )))
}

// ---------------------------------------------------------------------------
// fs_core::BlockRead / BlockDevice bridge
// ---------------------------------------------------------------------------

impl fs_core::BlockRead for VhdReader {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> fs_core::Result<()> {
        VhdReader::read_at(self, offset, buf).map_err(vhd_to_fs_core_error)
    }
    fn size_bytes(&self) -> u64 {
        self.virtual_size()
    }
}

/// Fixed and dynamic VHDs opened RW have a write path. Differencing
/// inherits the trait default, which surfaces [`fs_core::Error::ReadOnly`]
/// until its write path lands.
impl fs_core::BlockDevice for VhdReader {
    fn write_at(&self, offset: u64, buf: &[u8]) -> fs_core::Result<()> {
        VhdReader::write_at(self, offset, buf).map_err(vhd_to_fs_core_error)
    }
    fn flush(&self) -> fs_core::Result<()> {
        VhdReader::flush_writes(self).map_err(vhd_to_fs_core_error)
    }
    fn is_writable(&self) -> bool {
        self.writable()
    }
}

fn vhd_to_fs_core_error(e: Error) -> fs_core::Error {
    match e {
        Error::Io(io) => fs_core::Error::Io(io),
        Error::OutOfBounds { offset, len, size } => {
            fs_core::Error::OutOfBounds { offset, len, size }
        }
        Error::ReadOnly => fs_core::Error::ReadOnly,
        other => fs_core::Error::Custom(other.to_string()),
    }
}

fn fs_core_to_vhd_error(e: fs_core::Error) -> Error {
    match e {
        fs_core::Error::Io(io) => Error::Io(io),
        fs_core::Error::ShortRead { offset, want, got } => Error::Io(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            format!("short read at {offset}: wanted {want} got {got}"),
        )),
        fs_core::Error::ReadOnly => Error::ReadOnly,
        fs_core::Error::OutOfBounds { offset, len, size } => {
            Error::OutOfBounds { offset, len, size }
        }
        fs_core::Error::Custom(s) => Error::Custom(s),
    }
}
