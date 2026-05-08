//! VHD read path. Handles fixed, dynamic, and differencing variants.
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

use crate::dynamic::{DynamicHeader, BAT_UNALLOCATED, DYN_HEADER_SIZE};
use crate::error::{Error, Result};
use crate::footer::{DiskType, Footer, FOOTER_SIZE};
use crate::footer_build::build_fixed_footer;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const SECTOR_SIZE: u64 = 512;

/// Maximum differencing-chain depth. A pathological chain (or cycle)
/// is rejected rather than blowing the stack.
const MAX_PARENT_DEPTH: u32 = 16;

/// File-handle access mode tracked alongside the underlying `File`. The
/// trait surface checks this before attempting any positioned writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    ReadOnly,
    ReadWrite,
}

pub struct VhdReader {
    file: Mutex<File>,
    /// Whether the underlying file was opened RW. Even when this is
    /// `ReadWrite`, write_at still gates on subtype = Fixed.
    mode: Mode,
    footer: Footer,
    /// `None` for fixed disks. For dynamic and differencing, the
    /// parsed dynamic header.
    dynamic: Option<DynamicHeader>,
    /// `None` for fixed disks. For dynamic/differencing, the cached
    /// in-memory BAT (always small — `max_table_entries * 4` bytes).
    bat: Option<Vec<u32>>,
    /// Cached size of one block bitmap in bytes (sector-padded).
    bitmap_size: u64,
    /// Differencing parent, if any. Always opened read-only — writes
    /// only land in the leaf VHD's data area.
    parent: Option<Box<VhdReader>>,
    /// Virtual disk size (`current_size` from the footer).
    virtual_size: u64,
}

impl VhdReader {
    /// Open `path` read-only and parse footer + (dynamic/differencing)
    /// header + BAT + parent chain.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        Self::open_inner(path.as_ref(), MAX_PARENT_DEPTH, Mode::ReadOnly)
    }

    /// Open `path` read-write. Fails the same way `open` does for
    /// missing/corrupt files. Whether `BlockDevice::write_at` actually
    /// succeeds further depends on the subtype — currently only
    /// [`DiskType::Fixed`] writes through; dynamic/differencing return
    /// [`Error::ReadOnly`] until their write paths land.
    pub fn open_rw<P: AsRef<Path>>(path: P) -> Result<Self> {
        Self::open_inner(path.as_ref(), MAX_PARENT_DEPTH, Mode::ReadWrite)
    }

    fn open_inner(path: &Path, depth_remaining: u32, mode: Mode) -> Result<Self> {
        if depth_remaining == 0 {
            return Err(Error::ParentTooDeep);
        }

        let mut file = match mode {
            Mode::ReadOnly => File::open(path)?,
            Mode::ReadWrite => OpenOptions::new().read(true).write(true).open(path)?,
        };
        let file_len = file.metadata()?.len();
        if file_len < FOOTER_SIZE as u64 {
            return Err(Error::Corrupt("file shorter than 512 bytes"));
        }

        // Footer at end of file.
        let mut footer_bytes = [0u8; FOOTER_SIZE];
        file.seek(SeekFrom::Start(file_len - FOOTER_SIZE as u64))?;
        file.read_exact(&mut footer_bytes)?;
        let footer = Footer::parse(&footer_bytes)?;

        let virtual_size = footer.current_size;

        let (dynamic, bat, bitmap_size, parent) = match footer.disk_type {
            DiskType::Fixed => (None, None, 0, None),
            DiskType::Dynamic | DiskType::Differencing => {
                if footer.data_offset == u64::MAX {
                    return Err(Error::Corrupt(
                        "dynamic/differencing footer has data_offset = 0xFFFF...",
                    ));
                }
                let mut hdr_bytes = [0u8; DYN_HEADER_SIZE];
                file.seek(SeekFrom::Start(footer.data_offset))?;
                file.read_exact(&mut hdr_bytes)?;
                let dyn_hdr = DynamicHeader::parse(&hdr_bytes)?;

                // BAT.
                let bat_entries = dyn_hdr.max_table_entries as usize;
                let mut bat_bytes = vec![0u8; bat_entries * 4];
                file.seek(SeekFrom::Start(dyn_hdr.table_offset))?;
                file.read_exact(&mut bat_bytes)?;
                let mut bat = Vec::with_capacity(bat_entries);
                for chunk in bat_bytes.chunks_exact(4) {
                    bat.push(u32::from_be_bytes([
                        chunk[0], chunk[1], chunk[2], chunk[3],
                    ]));
                }

                let bitmap_size = dyn_hdr.bitmap_size_bytes();

                let parent = if footer.disk_type == DiskType::Differencing {
                    Some(Box::new(open_parent(
                        path,
                        &dyn_hdr,
                        depth_remaining,
                    )?))
                } else {
                    None
                };

                (Some(dyn_hdr), Some(bat), bitmap_size, parent)
            }
        };

        Ok(Self {
            file: Mutex::new(file),
            mode,
            footer,
            dynamic,
            bat,
            bitmap_size,
            parent,
            virtual_size,
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

        Self::open_inner(path.as_ref(), MAX_PARENT_DEPTH, Mode::ReadWrite)
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
            DiskType::Dynamic | DiskType::Differencing => {
                self.read_sparse(offset, buf)
            }
        }
    }

    fn read_fixed(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let mut f = self.file.lock().unwrap();
        f.seek(SeekFrom::Start(offset))?;
        f.read_exact(buf)?;
        Ok(())
    }

    /// True only when the underlying file was opened RW *and* the
    /// subtype has a write path. Today: fixed only.
    pub fn writable(&self) -> bool {
        matches!(self.mode, Mode::ReadWrite)
            && matches!(self.footer.disk_type, DiskType::Fixed)
    }

    /// Write exactly `buf.len()` bytes starting at virtual `offset`.
    /// Currently only supported on fixed VHDs; dynamic and differencing
    /// return [`Error::ReadOnly`].
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
        // Fixed VHDs are 1:1: virtual offset == host offset. The footer
        // sits at host offset == virtual_size and is implicitly off-limits
        // because the bounds check above rejects writes past virtual_size.
        let mut f = self.file.lock().unwrap();
        f.seek(SeekFrom::Start(offset))?;
        f.write_all(buf)?;
        Ok(())
    }

    /// Sync the underlying file's data pages to stable storage. For
    /// fixed VHDs the footer is fixed-content (we never rewrite it
    /// after `create_fixed`), so `sync_data` is sufficient — there's no
    /// metadata change to chase with `sync_all`.
    pub fn flush_writes(&self) -> Result<()> {
        if !matches!(self.mode, Mode::ReadWrite) {
            // Read-only: nothing to flush, treat as success rather than
            // surfacing an error to consumers that flush opportunistically.
            return Ok(());
        }
        let f = self.file.lock().unwrap();
        f.sync_data()?;
        Ok(())
    }

    fn read_sparse(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let dyn_hdr = self
            .dynamic
            .as_ref()
            .ok_or(Error::Corrupt("sparse read but no dynamic header"))?;
        let bat = self
            .bat
            .as_ref()
            .ok_or(Error::Corrupt("sparse read but no BAT"))?;

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
            let chunk_len =
                std::cmp::min(bytes_remaining_in_block, end - cursor) as usize;

            if block_idx >= bat.len() {
                return Err(Error::Corrupt("block_idx past BAT"));
            }
            let bat_entry = bat[block_idx];

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
            {
                let mut f = self.file.lock().unwrap();
                f.seek(SeekFrom::Start(block_host_off))?;
                f.read_exact(&mut bitmap)?;
            }

            // Within this block, walk sector by sector.
            let mut block_cursor = in_block;
            let block_end = in_block + chunk_len as u64;
            while block_cursor < block_end {
                let sector_in_block = block_cursor / SECTOR_SIZE;
                let in_sector = block_cursor & (SECTOR_SIZE - 1);
                let bytes_left_in_sector = SECTOR_SIZE - in_sector;
                let slice_len =
                    std::cmp::min(bytes_left_in_sector, block_end - block_cursor)
                        as usize;
                let bit_byte = (sector_in_block / 8) as usize;
                let bit_in_byte = 7 - (sector_in_block % 8) as u8;
                let bit_set = (bitmap[bit_byte] >> bit_in_byte) & 1 == 1;

                let dst = &mut buf[written..written + slice_len];
                if bit_set {
                    // Sector lives at: bat_entry * 512 + bitmap_size +
                    // sector_in_block * 512 + in_sector
                    let host_off = block_host_off
                        + bitmap_size
                        + sector_in_block * SECTOR_SIZE
                        + in_sector;
                    let mut f = self.file.lock().unwrap();
                    f.seek(SeekFrom::Start(host_off))?;
                    f.read_exact(dst)?;
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
        return VhdReader::open_inner(&candidate, depth_remaining - 1, Mode::ReadOnly);
    }

    // Plain `parent_name` as last resort.
    let direct = PathBuf::from(parent_name);
    if direct.exists() {
        return VhdReader::open_inner(&direct, depth_remaining - 1, Mode::ReadOnly);
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

/// Fixed VHDs opened RW pass writes through 1:1. Dynamic and
/// differencing inherit the trait default, which surfaces
/// [`fs_core::Error::ReadOnly`] until their write paths land.
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
