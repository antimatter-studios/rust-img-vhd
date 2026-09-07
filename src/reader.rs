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

use crate::format::SECTOR_SIZE;

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
    /// in-memory BAT, `max_table_entries * 4` bytes.
    ///
    /// Bounded at open: the table must describe at least the declared
    /// virtual size and must fit inside the image. Before that check the
    /// comment here claimed it was "always small", which was an
    /// assertion about a field read straight off disk.
    ///
    /// This is the write lock for sparse images, not merely a container
    /// lock, and [`VhdReader::write_sparse`] holds it across one whole
    /// block's write rather than just around the entry read. See that
    /// function for what breaks otherwise.
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
    /// will be placed. `None` for fixed disks.
    ///
    /// Only ever read or advanced from
    /// [`VhdReader::allocate_block_locked`], which runs with `bat`
    /// already held — so the two fields describe one consistent
    /// allocation state and there is no second lock order to get wrong.
    /// It stays a separate `Mutex` only because `&self` methods need to
    /// mutate it.
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
        Self::open_inner(Arc::new(dev), true, MAX_PARENT_DEPTH, Some(p.to_path_buf()))
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
                // A BAT entry is a sector index, so every block in a
                // sparse image has to sit on a sector boundary — and the
                // allocator places blocks relative to the device's tail.
                // A device whose length is not a multiple of 512 puts
                // that tail off-grid, and then the entry written for the
                // first allocated block names the sector BELOW the data,
                // silently, because the division truncates. Measured on
                // a 3073-byte image built from the all-sparse fixture:
                //
                //     write_at -> Ok(())
                //     bat[0]=5 -> host 2560   (the block is at 2561)
                //     reopen read -> Ok(())  buf=[00 x 16]
                //
                // Sixteen bytes written, no error, and nothing can ever
                // read them back.
                //
                // Refusing is the honest answer rather than rounding the
                // tail down: the footer was found at `dev_size - 512`,
                // so an unaligned length means the image's own layout is
                // already off-grid and the bytes between the last block
                // and the footer belong to nothing.
                if !dev_size.is_multiple_of(SECTOR_SIZE) {
                    return Err(Error::Corrupt(
                        "sparse image length is not a multiple of 512, so its blocks cannot sit on sector boundaries",
                    ));
                }
                let mut hdr_bytes = [0u8; DYN_HEADER_SIZE];
                dev.read_at(footer.data_offset, &mut hdr_bytes)
                    .map_err(fs_core_to_vhd_error)?;
                let dyn_hdr = DynamicHeader::parse(&hdr_bytes)?;

                // BAT.
                //
                // `max_table_entries` comes straight off disk. Bounding it
                // before allocating is the whole point: it is a u32, so an
                // unbounded read of it asks for up to 16 GiB on a hostile
                // or corrupt image, before a single byte of the table has
                // been read.
                //
                // Two bounds, and both are the image's own arithmetic
                // rather than a number invented here. The table must
                // describe at least the declared virtual size, and it
                // cannot be larger than the file it lives in.
                let bat_entries = dyn_hdr.max_table_entries as usize;
                let bat_bytes_len = (bat_entries as u64)
                    .checked_mul(4)
                    .ok_or(Error::Corrupt("BAT size overflows"))?;

                let block_size = dyn_hdr.block_size as u64;
                if block_size == 0 {
                    return Err(Error::Corrupt("dynamic header declares block_size 0"));
                }
                let blocks_needed = virtual_size.div_ceil(block_size);
                if (bat_entries as u64) < blocks_needed {
                    return Err(Error::Corrupt(
                        "BAT is too small to describe the declared virtual size",
                    ));
                }

                let table_end = dyn_hdr
                    .table_offset
                    .checked_add(bat_bytes_len)
                    .ok_or(Error::Corrupt("BAT extent overflows"))?;
                if table_end > dev_size {
                    return Err(Error::Corrupt("BAT extends past the end of the image"));
                }

                let mut bat_bytes = vec![0u8; bat_bytes_len as usize];
                dev.read_at(dyn_hdr.table_offset, &mut bat_bytes)
                    .map_err(fs_core_to_vhd_error)?;
                let mut bat = Vec::with_capacity(bat_entries);
                for chunk in bat_bytes.chunks_exact(4) {
                    bat.push(u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
                }

                let bitmap_size = dyn_hdr.bitmap_size_bytes();

                // The table's SIZE was bounded above; its CONTENTS were
                // not, and an entry is a sector index the image chose.
                // `read_sparse` multiplied it by 512 and read a sector
                // bitmap from wherever it landed, so an entry of 0 made
                // the footer mirror the bitmap and the bytes after it the
                // block's data — and the read succeeded. Measured on the
                // standard fixture with entry 0 zeroed, a read at virtual
                // offset 1024 came back as the image's own BAT:
                //
                //     Ok(())  buf=[00, 00, 00, 00, ff, ff, ff, ff, ...]
                //
                // where the block really holds 00 01 02 03 04 05 06 07.
                //
                // So walk the table once, here, where the numbers that
                // bound it have all just been computed. An image whose
                // table cannot be believed is refused before a caller
                // holds a reader for it, rather than one block at a time
                // on the read path.
                let metadata_end = [
                    FOOTER_SIZE as u64,
                    footer
                        .data_offset
                        .checked_add(DYN_HEADER_SIZE as u64)
                        .ok_or(Error::Corrupt("dynamic header extent overflows"))?,
                    table_end,
                ]
                .into_iter()
                .max()
                .expect("the array is not empty");
                let data_start = metadata_end.div_ceil(SECTOR_SIZE) * SECTOR_SIZE;
                let block_total = bitmap_size
                    .checked_add(dyn_hdr.block_size as u64)
                    .ok_or(Error::Corrupt("block extent overflows"))?;

                // The data area ends where the trailing footer begins,
                // not at the end of the file. `dev_size` was the first
                // bound written here and it is a footer too generous: a
                // block ending at exactly `dev_size` has the footer as
                // its last sector, so a read of that sector returns
                // `conectix` and the rest of the footer as the guest's
                // file data — which is the defect this walk exists to
                // stop, arriving through the check meant to stop it.
                //
                // This is the same number the allocator uses for the
                // tail, computed once and shared rather than written
                // twice with a footer between the two spellings.
                let next_alloc = dev_size.saturating_sub(FOOTER_SIZE as u64);
                let data_end = next_alloc;

                let mut allocated: Vec<u32> = Vec::new();
                for &entry in &bat {
                    if entry == BAT_UNALLOCATED {
                        continue;
                    }
                    let block_off = (entry as u64) * SECTOR_SIZE;
                    if block_off < data_start {
                        return Err(Error::Corrupt(
                            "BAT entry names a block inside the image's own metadata",
                        ));
                    }
                    let block_end = block_off
                        .checked_add(block_total)
                        .ok_or(Error::Corrupt("BAT entry's block extent overflows"))?;
                    if block_end > data_end {
                        return Err(Error::Corrupt(
                            "BAT entry names a block running into the trailing footer or past it",
                        ));
                    }
                    allocated.push(entry);
                }
                // Two entries at one address is an aliased image: a write
                // through one virtual block silently changes another.
                // One sort answers it.
                allocated.sort_unstable();
                if allocated.windows(2).any(|w| w[0] == w[1]) {
                    return Err(Error::Corrupt("two BAT entries name the same block"));
                }

                let parent = if footer.disk_type == DiskType::Differencing {
                    let child_path = owning_path.as_deref().ok_or(Error::Unsupported(
                        "differencing VHD opened on a raw device; parent resolution needs a path",
                    ))?;
                    Some(Box::new(open_parent(
                        child_path,
                        &dyn_hdr,
                        depth_remaining,
                    )?))
                } else {
                    None
                };

                // The trailing footer sits at dev_size - 512, so
                // `next_alloc` (computed above, where the BAT walk needs
                // the same number) is where the next block goes: future
                // allocations land at that offset and push the footer
                // forward, assuming the device was sized to "end of data
                // + footer" — the canonical layout.

                (
                    Some(dyn_hdr),
                    Some(bat),
                    bitmap_size,
                    parent,
                    Some(next_alloc),
                )
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
        self.writable && matches!(self.footer.disk_type, DiskType::Fixed | DiskType::Dynamic)
    }

    /// Write exactly `buf.len()` bytes starting at virtual `offset`.
    /// Supported on fixed and dynamic VHDs; differencing returns
    /// [`Error::ReadOnly`].
    ///
    /// Safe to call from several threads on one shared reader — which
    /// is not a courtesy, it is forced: this type is `Sync`, this
    /// method takes `&self`, and the C ABI hands the reader out inside
    /// an `Arc`, so there is no way to hand a caller a single-writer
    /// handle even if we wanted to. On a dynamic image the internal
    /// bookkeeping (block allocation and the per-block sector bitmap)
    /// is serialised for that reason (see `write_sparse`). Two writes
    /// to *overlapping* byte
    /// ranges still race with each other for which bytes win, exactly
    /// as two writes to one file descriptor would — the guarantee here
    /// is that the image structure stays consistent and that no
    /// non-overlapping write is lost.
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

    /// Is the sector at `sector_in_block` present in this image?
    ///
    /// # The bit order is the format's, not an arbitrary choice
    ///
    /// A block's bitmap is **MSB-first**: sector 0 is bit 7 of byte 0,
    /// sector 7 is bit 0 of byte 0, sector 8 is bit 7 of byte 1. That
    /// is what `7 - (n % 8)` says, and it is the single fact the read
    /// and write paths have to agree on — they were open-coding it
    /// separately, with different bounds discipline, which reads like
    /// one of them is wrong.
    ///
    /// # Why there is no bounds check
    ///
    /// [`DynamicHeader::bitmap_size_bytes`] derives the bitmap from
    /// `block_size`: one bit per sector, rounded up to a whole sector.
    /// So a bitmap always covers every sector its block can hold, and
    /// an index past the end would mean a caller had asked about a
    /// sector outside the block — a bug here, not corrupt input.
    ///
    /// # Panics
    ///
    /// If `sector_in_block` is outside the block, which the above says
    /// cannot happen. Panicking beats returning `false`: a silent
    /// `false` reads as "this sector is a hole" and hands the caller
    /// zeroes for data that exists.
    fn bitmap_get(bitmap: &[u8], sector_in_block: u64) -> bool {
        let byte = (sector_in_block / 8) as usize;
        let bit = 7 - (sector_in_block % 8) as u8;
        (bitmap[byte] >> bit) & 1 == 1
    }

    /// Mark the sector at `sector_in_block` present.
    ///
    /// Same MSB-first ordering, same sizing argument, as
    /// [`Self::bitmap_get`].
    ///
    /// # Panics
    ///
    /// If `sector_in_block` is outside the block. The write path used
    /// to return `Error::Corrupt("bitmap index out of range")` here,
    /// which named the input as the fault; the input has nothing to do
    /// with it, since the bitmap's size comes from the block size and
    /// the sector comes from a caller's offset.
    fn bitmap_set(bitmap: &mut [u8], sector_in_block: u64) {
        let byte = (sector_in_block / 8) as usize;
        let bit = 7 - (sector_in_block % 8) as u8;
        bitmap[byte] |= 1 << bit;
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
                let bit_set = Self::bitmap_get(&bitmap, sector_in_block);

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
    /// Crash-safety order for a block that has to be allocated: the
    /// allocation lands first — zeroed block, then BAT entry, then
    /// footer mirror, with the detail on
    /// [`VhdReader::allocate_block_locked`] — and only then the
    /// caller's bytes and the bitmap bits that make them visible, with
    /// a `dev.flush()` between steps. A crash part-way leaves the BAT
    /// entry still unallocated, or the BAT updated with a stale footer
    /// mirror; both keep the image readable (the leaked tail bytes are
    /// recoverable by a disk-image checker, and on re-open the next
    /// allocation overwrites them).
    ///
    /// ## Concurrent writers
    ///
    /// `VhdReader` is `Sync`, `write_at` takes `&self`, and `capi.rs`
    /// hands the reader out inside an `Arc`, so this function has to
    /// work when two threads are inside it at once. It does, and the
    /// mechanism is blunt: `self.bat` is held for the whole of one
    /// block's write, so blocks are mutated one at a time image-wide.
    /// Sparse writes therefore serialise. They already `fsync` per
    /// block, so the lock is not what makes them slow, and one lock is
    /// the version of this that is obviously correct.
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

            // Everything below runs under one lock hold, because two
            // check-then-act sequences live in it and neither survives
            // another writer acting in the gap:
            //
            //   * "is this BAT entry unallocated, and if so allocate a
            //     block for it" — two threads writing into the same
            //     unallocated block would both read BAT_UNALLOCATED and
            //     both allocate. One BAT entry wins; the loser's block
            //     is orphaned in the file with the loser's payload
            //     inside it, and that write is silently lost.
            //   * "read the sector bitmap, set some bits, write it
            //     back" in set_bitmap_range — two threads writing
            //     different sectors of one block would read the same
            //     bitmap and the second write-back would drop the
            //     first's bits. The payload bytes land, but the bitmap
            //     says those sectors were never written, so they read
            //     back as zero.
            //
            // Releasing the lock between the read and the allocation —
            // which is what this used to do, in the name of not holding
            // it across device I/O — is exactly the gap the first of
            // those needs.
            let mut bat_guard = self.bat.lock().unwrap();
            let bat = bat_guard
                .as_ref()
                .ok_or(Error::Corrupt("sparse write but no BAT"))?;
            if block_idx >= bat.len() {
                return Err(Error::Corrupt("block_idx past BAT"));
            }
            let bat_entry = bat[block_idx];

            let block_host_off = if bat_entry == BAT_UNALLOCATED {
                self.allocate_block_locked(&mut bat_guard, block_idx)?
            } else {
                bat_entry as u64 * SECTOR_SIZE
            };

            // Before the payload lands, give the parts of any
            // partly-covered sector the contents a reader sees there
            // today — because the bitmap bits set below publish whole
            // sectors, not the byte range the caller asked for.
            self.define_partial_sectors(block_host_off, block_idx, in_block, chunk_len as u64)?;

            // Splice the user payload into the block at [in_block,
            // in_block + chunk_len).
            let src = &buf[written..written + chunk_len];
            let data_off = block_host_off + bitmap_size + in_block;
            self.dev_write(data_off, src)?;

            // Mark the touched sectors in the bitmap.
            self.set_bitmap_range(block_host_off, in_block, chunk_len as u64)?;
            self.dev_flush()?;
            drop(bat_guard);

            cursor += chunk_len as u64;
            written += chunk_len;
        }

        Ok(())
    }

    /// Allocate a fresh block at the device tail, point BAT entry
    /// `block_idx` at it, push the trailing footer mirror past it.
    /// Returns the host offset where the new block's bitmap starts.
    ///
    /// Callable only with the BAT lock already held; `bat_guard` is
    /// that lock. Taking it by `&mut` rather than re-locking inside is
    /// what makes it a compile-time fact that the caller's "this entry
    /// is unallocated" test and the allocation below cannot be
    /// separated by another writer — and it makes this the only code
    /// that touches `next_alloc_off`, so the tail needs no lock
    /// discipline of its own beyond "under `bat`".
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
    /// The tail is *read* before step 1 and *advanced* between steps 1
    /// and 2, and that placement is the whole of the failure handling —
    /// there is no rollback, because the commit point is the last
    /// moment the space is provably free rather than a guess made
    /// afterwards:
    ///
    ///   * Fail in step 1 and the tail never moved. All that is out
    ///     there is a zeroed range nothing references, and the next
    ///     allocation reuses the same offset.
    ///   * Fail in step 2 or step 3 and the tail has already moved.
    ///     From the moment the BAT write is issued the entry may be on
    ///     the device whatever the call returns, so the range has to be
    ///     treated as live and never handed out again. The block leaks;
    ///     a disk-image checker recovers it, and unlike aliasing it
    ///     costs no data.
    ///
    /// The version this replaces reserved the tail up front and, on a
    /// step-1 failure, "rolled back" by assigning the tail an absolute
    /// value — its own pre-advance offset. That is not an undo: it
    /// discards any reservation another thread made in between, and the
    /// next allocation then writes over a block already in use. Steps 2
    /// and 3 had no unwind at all and left the tail advanced past a
    /// block that was never published.
    fn allocate_block_locked(
        &self,
        bat_guard: &mut std::sync::MutexGuard<'_, Option<Vec<u32>>>,
        block_idx: usize,
    ) -> Result<u64> {
        let dyn_hdr = self
            .dynamic
            .as_ref()
            .ok_or(Error::Corrupt("allocate but no dynamic header"))?;
        let block_size = dyn_hdr.block_size as u64;
        let bitmap_size = self.bitmap_size;
        let block_total = bitmap_size + block_size;

        // Where the next block goes. Read, not reserved: nothing else
        // can allocate while the caller holds the BAT lock, so the
        // offset stays ours until we commit it below.
        let new_block_off = {
            let tail = self.next_alloc_off.lock().unwrap();
            tail.ok_or(Error::Corrupt("allocate but no tail offset (fixed?)"))?
        };

        // Step 1: zero-init bitmap + data area at the new tail.
        // FileDevice's write_at extends the file as needed; on
        // non-growable BlockDevice impls this surfaces an I/O error
        // up to the caller, which is the right behaviour.
        // IN CHUNKS, not in one buffer the size of a block.
        //
        // `block_size` comes from the dynamic header, which is checked
        // for being a power of two and at least a sector and has no
        // upper bound -- neither does either BAT bound, because a
        // larger block makes the table SMALLER. So 0x8000_0000 asks for
        // a 2 GiB buffer here before writing anything.
        //
        // A megabyte is small enough that a pathological block size
        // costs time rather than memory, and large enough that the loop
        // is not the cost. The bitmap plus data area is written whole
        // either way; only the buffer is bounded.
        const ZERO_CHUNK: usize = 1024 * 1024;
        let zeros = vec![0u8; ZERO_CHUNK.min(block_total as usize)];
        let mut written = 0u64;
        while written < block_total {
            let n = (block_total - written).min(zeros.len() as u64) as usize;
            self.dev_write(new_block_off + written, &zeros[..n])?;
            written += n as u64;
        }
        self.dev_flush()?;

        // Commit the tail before step 2 is even issued. The instant
        // the BAT write goes to the device we stop being able to prove
        // the entry did not land: a write that reports an error may
        // still have reached the platter, and a flush that fails leaves
        // it in exactly the same unknown state. So this is the last
        // moment at which the space is provably free.
        //
        // Committing here leaks a block if step 2 fails. Committing
        // after step 2 would instead hand the same host range out twice
        // whenever the entry landed and the call reported failure — the
        // second allocation zeroes a range the BAT already points at.
        // A leak is recoverable by a disk-image checker; aliasing is
        // silent data loss.
        {
            let mut tail = self.next_alloc_off.lock().unwrap();
            *tail = Some(new_block_off + block_total);
        }

        // Step 2: publish the new block in the on-disk BAT.
        let bat_value = bat_entry_for(new_block_off)?;
        let bat_entry_off = dyn_hdr.table_offset + (block_idx as u64) * 4;
        self.dev_write(bat_entry_off, &bat_value.to_be_bytes())?;
        self.dev_flush()?;

        // Durable: mirror the entry into the cached BAT through the
        // caller's guard. Reached only on success, so a failed step 2
        // leaves the cached BAT saying "unallocated" — which is the
        // safe direction, since the next write to this block allocates
        // afresh at the already-advanced tail and overwrites the entry
        // rather than sharing a range with it.
        let bat = bat_guard
            .as_mut()
            .ok_or(Error::Corrupt("allocate but no BAT"))?;
        bat[block_idx] = bat_value;

        // Step 3: rewrite the trailing footer mirror at the new tail.
        // The bytes haven't changed (footer.current_size etc are
        // unchanged); we just put the same 512 bytes at the new offset.
        let new_footer_off = new_block_off + block_total;
        self.dev_write(new_footer_off, &self.footer_bytes)?;
        self.dev_flush()?;

        Ok(new_block_off)
    }

    /// Give the untouched parts of a partly-written sector the contents
    /// a reader sees there today, so that setting the sector's bitmap
    /// bit does not publish bytes the caller never wrote.
    ///
    /// The unit the bitmap describes is a **sector**. A clear bit means
    /// "this sector reads as zero" — or, for a differencing child,
    /// "read it from the parent". It does *not* mean the corresponding
    /// bytes in the block's data area are zero: the format leaves them
    /// undefined and a writer is free to leave anything there. So a
    /// write that starts or ends mid-sector inside a block that already
    /// exists, over a sector whose bit was clear, used to flip the rest
    /// of that sector from "reads as zero" to "reads whatever was in
    /// the file". Measured on a fixture whose block 0 is allocated with
    /// an all-clear bitmap and a data area of `0xEE`, writing 16 bytes
    /// at offset 100 and reading `[0, 512)` back:
    ///
    /// ```text
    /// before write, read@0  = [00, 00, 00, 00, 00, 00, 00, 00]
    /// after  write, [0..8]  = [ee, ee, ee, ee, ee, ee, ee, ee]
    /// bytes published that the caller never wrote: 496
    /// ```
    ///
    /// Images this crate allocated itself were never exposed, because
    /// `allocate_block_locked` zeroes the whole range first. It bites on
    /// an image somebody else allocated — which `open_rw` and
    /// `open_rw_on_device` explicitly support.
    ///
    /// Only the first and last sector of a chunk can be partly covered;
    /// everything between them is overwritten in full, so what was there
    /// does not matter.
    ///
    /// The contents come from [`VhdReader::read_block_unallocated`],
    /// which is zeros for a dynamic image and the parent's bytes for a
    /// differencing child. The differencing write path is refused today,
    /// so only the first arm runs — but writing it this way is what
    /// stops the second one arriving with the same hole, where it would
    /// be worse: "defer to parent" would become "return stale leaf
    /// bytes", which are plausible rather than obviously wrong.
    fn define_partial_sectors(
        &self,
        block_host_off: u64,
        block_idx: usize,
        in_block: u64,
        len: u64,
    ) -> Result<()> {
        let block_size = self
            .dynamic
            .as_ref()
            .ok_or(Error::Corrupt("sparse write but no dynamic header"))?
            .block_size as u64;
        let bitmap_size = self.bitmap_size;

        let mut bitmap = vec![0u8; bitmap_size as usize];
        self.dev_read(block_host_off, &mut bitmap)?;

        let first = in_block / SECTOR_SIZE;
        let last = (in_block + len - 1) / SECTOR_SIZE;

        // A single-sector write has one partly-covered sector, not two.
        let ends: &[u64] = if first == last {
            &[first]
        } else {
            &[first, last]
        };
        for &sector in ends {
            // Already published: its bytes are somebody's real data.
            if Self::bitmap_get(&bitmap, sector) {
                continue;
            }
            let sector_start = sector * SECTOR_SIZE;
            let head = in_block.saturating_sub(sector_start).min(SECTOR_SIZE);
            let covered_end = (in_block + len)
                .saturating_sub(sector_start)
                .min(SECTOR_SIZE);
            if head == 0 && covered_end == SECTOR_SIZE {
                continue; // the payload covers the whole sector
            }

            let mut sector_bytes = vec![0u8; SECTOR_SIZE as usize];
            let virt = (block_idx as u64) * block_size + sector_start;
            self.read_block_unallocated(virt, &mut sector_bytes)?;

            // Only the parts the payload will not cover, so the payload
            // write stays the one that publishes the rest.
            if head > 0 {
                self.dev_write(
                    block_host_off + bitmap_size + sector_start,
                    &sector_bytes[..head as usize],
                )?;
            }
            if covered_end < SECTOR_SIZE {
                self.dev_write(
                    block_host_off + bitmap_size + sector_start + covered_end,
                    &sector_bytes[covered_end as usize..],
                )?;
            }
        }
        Ok(())
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
            Self::bitmap_set(&mut bitmap, sector);
        }

        self.dev_write(block_host_off, &bitmap)?;
        Ok(())
    }
}

/// The BAT entry that names the block at `host_off`.
///
/// A BAT entry is a **sector index**, so the division only tells the
/// truth when three things hold, and a bare `as u32` checks none of
/// them:
///
/// * `host_off` is sector-aligned. Otherwise the division truncates and
///   the entry names the sector below the block — a read shifted by up
///   to 511 bytes, with no error anywhere.
/// * The index fits a `u32`. VHD's addressing tops out near that limit,
///   and a host offset at or past 2 TiB wraps into a small index that
///   points back into the image's own metadata.
/// * The index is not `BAT_UNALLOCATED`. A block landing at host offset
///   `0x1FFF_FFFF_E00` produces exactly that value, and the entry then
///   publishes the block as *absent*: the data is written, the footer
///   is moved past it, and every later read of that block returns
///   zeros.
fn bat_entry_for(host_off: u64) -> Result<u32> {
    if !host_off.is_multiple_of(SECTOR_SIZE) {
        return Err(Error::Corrupt(
            "a block was placed at an offset that is not a sector boundary",
        ));
    }
    let entry = u32::try_from(host_off / SECTOR_SIZE).map_err(|_| {
        Error::Corrupt("a block was placed past the highest sector a BAT entry can name")
    })?;
    if entry == BAT_UNALLOCATED {
        return Err(Error::Corrupt(
            "a block was placed at the one sector the BAT reserves to mean \"absent\"",
        ));
    }
    Ok(entry)
}

/// Open the differencing parent VHD.
///
/// # It does not walk the parent locators
///
/// The header's W2ku/W2ru locators are read and deliberately ignored:
/// resolution is `parent_unicode_name` against the child's own
/// directory, so a parent that is not a sibling is not found.
///
/// This comment said the opposite — locators first, sibling lookup as a
/// fallback — while an inline comment a few lines down said what the
/// code really does. Of two contradicting comments the reader meets
/// this one first, and it described a function that does not exist.
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
        let parent =
            VhdReader::open_inner(Arc::new(dev), false, depth_remaining - 1, Some(candidate))?;
        return check_parent_identity(parent, dyn_hdr);
    }

    // Plain `parent_name` as last resort.
    let direct = PathBuf::from(parent_name);
    if direct.exists() {
        let dev = FileDevice::open(&direct).map_err(fs_core_to_vhd_error)?;
        let parent =
            VhdReader::open_inner(Arc::new(dev), false, depth_remaining - 1, Some(direct))?;
        return check_parent_identity(parent, dyn_hdr);
    }

    Err(Error::ParentNotFound(format!(
        "tried '{}' and '{}'",
        candidate.display(),
        direct.display()
    )))
}

// ---------------------------------------------------------------------------
// fs_core::BlockRead / BlockDevice bridge
/// The parent a child found is the parent it named, or it is not its
/// parent.
///
/// A differencing image is a delta: the child holds only the sectors it
/// changed, and every other sector comes from the parent. Resolution was
/// purely by *name*, so any file at that path that happened to parse as
/// a VHD was accepted — and bolting a delta onto a different parent
/// produces a disk that is internally consistent, opens without
/// complaint, and is wrong. There is no checksum over the composite, so
/// nothing downstream detects it either.
///
/// Measured on a child declaring `parent_unique_id = 0x11...` and a
/// parent whose footer carries `0x22...`:
///
/// ```text
/// opened a child whose parent is not the one it names;
/// read Ok(())  first8=[bb, bb, bb, bb, bb, bb, bb, bb]
/// ```
///
/// The format is explicit that these two must match, and this crate
/// already parses both: `parent_unique_id` off the dynamic header, and
/// the parent's `unique_id` off its footer. They were read and never
/// compared.
///
/// `parent_timestamp` is deliberately not enforced. The spec allows a
/// parent modified after the child was created and readers commonly
/// proceed, so a mismatch there is a warning rather than a refusal —
/// and there is nowhere to report a warning yet.
fn check_parent_identity(parent: VhdReader, dyn_hdr: &DynamicHeader) -> Result<VhdReader> {
    if parent.footer().unique_id != dyn_hdr.parent_unique_id {
        return Err(Error::ParentNotFound(format!(
            "found a parent whose unique_id is {} where the child names {}",
            hex16(&parent.footer().unique_id),
            hex16(&dyn_hdr.parent_unique_id),
        )));
    }
    Ok(parent)
}

/// Sixteen bytes as hex, for an error a person reads.
fn hex16(id: &[u8; 16]) -> String {
    id.iter().map(|b| format!("{b:02x}")).collect()
}

// ---------------------------------------------------------------------------
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dynamic::DynamicHeader;

    /// Sector 0 is the **top** bit of byte 0.
    ///
    /// The ordering is the format's and it is the one fact the read and
    /// write paths must agree on. Getting it backwards is invisible to
    /// a round trip through this crate — writer and reader would agree
    /// with each other and disagree with every other VHD tool — so it
    /// is asserted against literal bytes rather than against itself.
    #[test]
    fn the_bitmap_is_most_significant_bit_first() {
        let bitmap = [0b1000_0000u8, 0b0000_0001];
        assert!(
            VhdReader::bitmap_get(&bitmap, 0),
            "sector 0 is bit 7 of byte 0"
        );
        assert!(!VhdReader::bitmap_get(&bitmap, 1));
        assert!(!VhdReader::bitmap_get(&bitmap, 7));
        assert!(!VhdReader::bitmap_get(&bitmap, 8));
        assert!(
            VhdReader::bitmap_get(&bitmap, 15),
            "sector 15 is bit 0 of byte 1"
        );
    }

    /// Setting a sector makes it present, and disturbs nothing else.
    #[test]
    fn setting_a_sector_touches_only_that_sector() {
        let mut bitmap = [0u8; 2];
        VhdReader::bitmap_set(&mut bitmap, 3);
        assert_eq!(bitmap, [0b0001_0000, 0]);
        assert!(VhdReader::bitmap_get(&bitmap, 3));

        VhdReader::bitmap_set(&mut bitmap, 9);
        assert_eq!(bitmap, [0b0001_0000, 0b0100_0000]);

        // Idempotent — the write path marks ranges that may overlap
        // sectors already present.
        VhdReader::bitmap_set(&mut bitmap, 3);
        assert_eq!(bitmap, [0b0001_0000, 0b0100_0000]);

        for sector in 0..16u64 {
            assert_eq!(
                VhdReader::bitmap_get(&bitmap, sector),
                sector == 3 || sector == 9,
                "sector {sector}"
            );
        }
    }

    /// The three ways a bare `as u32` on `host_off / 512` lies.
    ///
    /// Each of these produced a number the write path published as a
    /// BAT entry, and none of them named the block that had just been
    /// written.
    #[test]
    fn a_bat_entry_is_only_computed_from_an_address_it_can_name() {
        // The ordinary case still works.
        assert_eq!(bat_entry_for(2048).unwrap(), 4);

        // Unaligned: the division would truncate to the sector below.
        let err = bat_entry_for(2561).unwrap_err();
        assert!(
            matches!(err, Error::Corrupt(m) if m.contains("sector boundary")),
            "got {err:?}"
        );

        // Past what a u32 sector index can address.
        let err = bat_entry_for((u32::MAX as u64 + 1) * SECTOR_SIZE).unwrap_err();
        assert!(
            matches!(err, Error::Corrupt(m) if m.contains("highest sector")),
            "got {err:?}"
        );

        // Exactly the sentinel: 0xFFFF_FFFF * 512 is a legal-looking
        // offset whose entry means "there is no block here".
        let sentinel_off = BAT_UNALLOCATED as u64 * SECTOR_SIZE;
        let err = bat_entry_for(sentinel_off).unwrap_err();
        assert!(
            matches!(err, Error::Corrupt(m) if m.contains("absent")),
            "got {err:?}"
        );
    }

    /// The invariant that lets both helpers index without checking.
    ///
    /// `bitmap_size_bytes` gives one bit per sector of the block,
    /// rounded up to a whole sector — so the last sector of any block
    /// always has a bit inside the bitmap, with room to spare. That is
    /// the argument the read side used to leave unstated and the write
    /// side used to guard against with an `Error::Corrupt`; it belongs
    /// here, where it can be checked.
    #[test]
    fn a_bitmap_always_covers_every_sector_of_its_block() {
        for block_size in [512u32, 4096, 0x0020_0000, 0x0080_0000] {
            let header = DynamicHeader {
                data_offset: 0,
                table_offset: 0,
                header_version: 0x0001_0000,
                max_table_entries: 1,
                block_size,
                parent_unique_id: [0; 16],
                parent_timestamp: 0,
                parent_name: String::new(),
                parent_locators: Default::default(),
            };
            let bitmap_bytes = header.bitmap_size_bytes();
            let sectors = u64::from(block_size) / SECTOR_SIZE;
            let last_byte_index = (sectors - 1) / 8;
            assert!(
                last_byte_index < bitmap_bytes,
                "block size {block_size}: last sector {} needs byte {last_byte_index}, \
                 bitmap is {bitmap_bytes} bytes",
                sectors - 1
            );
        }
    }
}
