#!/usr/bin/env python3
"""make-differencing.py CHILD PARENT [FIRST:COUNT ...] -- turn the dynamic VHD
CHILD into a differencing image whose parent is PARENT, in place, holding
only the sectors named.

Neither this repository nor the oracle can create a differencing VHD:
`qemu-img create -f vpc -b ...` answers "Backing file not supported for
file format 'vpc'". So the suite takes a dynamic image -- made and written
by qemu-img/qemu-io, never by the tool under test -- and rewrites the three
fields that make it a child, straight from the published specification
rather than through this crate's code:

  footer (both copies)  disk type 3 -> 4, checksum recomputed
  dynamic header        parent unique id = PARENT's footer unique id,
                        parent unicode name = PARENT's file name (UTF-16BE),
                        checksum recomputed

  block bitmaps         a set bit only for the sectors FIRST:COUNT name
                        (guest sector numbers), every other bit clear

In a differencing image a set bit is the child's sector and a clear one
reads through to the parent. qemu-io marks every sector of a block it
allocates, so without the last rewrite the child would hold whole blocks
and the read-through inside a block would go untested.

Both images must have the same virtual size, and PARENT must sit beside
CHILD, which is where the reader looks for it.
"""

import os
import struct
import sys

FOOTER = 512
DISK_TYPE_AT, CHECKSUM_AT, UNIQUE_ID_AT = 60, 64, 68
DIFFERENCING = 4
# In the dynamic header: parent unique id, unicode name, checksum.
H_CHECKSUM_AT, H_PARENT_ID_AT, H_PARENT_NAME_AT = 36, 40, 64
H_SIZE, H_NAME_LEN = 1024, 512
H_TABLE_AT, H_ENTRIES_AT, H_BLOCK_SIZE_AT = 16, 28, 32
SECTOR, UNALLOCATED = 512, 0xFFFFFFFF


def checksum(block, at):
    """One's complement of the byte sum, with the checksum field zeroed."""
    total = sum(block[:at]) + sum(block[at + 4 :])
    return (~total) & 0xFFFFFFFF


def rewrite_footer(footer):
    footer = bytearray(footer)
    if footer[:8] != b"conectix":
        sys.exit("make-differencing: no footer cookie")
    struct.pack_into(">I", footer, DISK_TYPE_AT, DIFFERENCING)
    struct.pack_into(">I", footer, CHECKSUM_AT, checksum(footer, CHECKSUM_AT))
    return bytes(footer)


def main(child, parent, held):
    with open(parent, "rb") as f:
        f.seek(-FOOTER, os.SEEK_END)
        parent_footer = f.read(FOOTER)
    parent_id = parent_footer[UNIQUE_ID_AT : UNIQUE_ID_AT + 16]
    name = os.path.basename(parent).encode("utf-16-be")
    if len(name) > H_NAME_LEN:
        sys.exit("make-differencing: the parent's name is too long")

    with open(child, "r+b") as f:
        f.seek(-FOOTER, os.SEEK_END)
        tail_at = f.tell()
        tail = f.read(FOOTER)
        header_at = struct.unpack_from(">Q", tail, 16)[0]
        new_footer = rewrite_footer(tail)
        for at in (0, tail_at):
            f.seek(at)
            f.write(new_footer)

        f.seek(header_at)
        header = bytearray(f.read(H_SIZE))
        if header[:8] != b"cxsparse":
            sys.exit("make-differencing: no dynamic header cookie")
        header[H_PARENT_ID_AT : H_PARENT_ID_AT + 16] = parent_id
        header[H_PARENT_NAME_AT : H_PARENT_NAME_AT + H_NAME_LEN] = name.ljust(
            H_NAME_LEN, b"\0"
        )
        struct.pack_into(">I", header, H_CHECKSUM_AT, checksum(header, H_CHECKSUM_AT))
        f.seek(header_at)
        f.write(header)

        table_at = struct.unpack_from(">Q", header, H_TABLE_AT)[0]
        entries = struct.unpack_from(">I", header, H_ENTRIES_AT)[0]
        block_size = struct.unpack_from(">I", header, H_BLOCK_SIZE_AT)[0]
        per_block = block_size // SECTOR
        bitmap_len = -(-per_block // 8 // SECTOR) * SECTOR
        f.seek(table_at)
        table = struct.unpack(f">{entries}I", f.read(4 * entries))
        for block, entry in enumerate(table):
            if entry == UNALLOCATED:
                continue
            bitmap = bytearray(bitmap_len)
            for first, count in held:
                for sector in range(first, first + count):
                    if sector // per_block == block:
                        bit = sector % per_block
                        bitmap[bit // 8] |= 0x80 >> (bit % 8)
            f.seek(entry * SECTOR)
            f.write(bitmap)


if __name__ == "__main__":
    if len(sys.argv) < 3:
        sys.exit("usage: make-differencing.py CHILD PARENT [FIRST:COUNT ...]")
    held = [tuple(int(n) for n in arg.split(":")) for arg in sys.argv[3:]]
    main(sys.argv[1], sys.argv[2], held)
