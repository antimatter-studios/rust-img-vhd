#!/usr/bin/env bash
# Rebuild fuzz/corpus from images qemu-img wrote.
#
# A VHD footer is 512 bytes of almost entirely attacker-controlled
# geometry -- cylinders, heads, sectors-per-track, current_size,
# block_size -- multiplied together on the read path. A grain or block
# size that multiplies out to zero and then divides was a real finding
# in a sibling crate on 2026-09-06.
#
# The seeds are files the reference implementation produced. A random
# byte string is refused by the "conectix" cookie on the first line of
# the footer parser and never reaches the arithmetic underneath.
#
# Usage: scripts/make-fuzz-corpus.sh
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
work="$(mktemp -d "${TMPDIR:-/tmp}/vhd-fuzz-corpus.XXXXXX")"
trap 'rm -rf "$work"' EXIT

command -v qemu-img >/dev/null || {
    echo "qemu-img not found; install qemu-utils" >&2
    exit 1
}

# Two runs either side of a hole, so a dynamic VHD's block allocation
# table has both allocated and unallocated entries rather than being
# uniformly one or the other.
payload="$work/payload"
python3 - "$payload" <<'PY'
import sys
with open(sys.argv[1], 'wb') as f:
    f.write(b'A' * 8_000)
    f.seek(500_000)
    f.write(b'B' * 8_000)
PY

rm -rf "$here/fuzz/corpus"
mkdir -p "$here/fuzz/corpus"/{image,footer,dynamic_header}

build() {
    local name="$1"; shift
    qemu-img convert -f raw -O vpc "$@" "$payload" "$here/fuzz/corpus/image/$name.vhd" \
        2>/dev/null || {
        echo "qemu-img could not build the '$name' image" >&2
        exit 1
    }
}

# The two subformats are different files, not a flag: a fixed VHD is the
# raw data with a footer glued on, a dynamic one has a second header, a
# block allocation table and a sector bitmap per block.
build dynamic -o subformat=dynamic
build fixed   -o subformat=fixed
# force_size changes how the geometry fields are filled in, which is the
# part of the footer most likely to be arithmetic rather than a magic
# number.
build forced  -o subformat=dynamic,force_size=on

python3 - "$here/fuzz/corpus" <<'PY'
import os, struct, sys

root = sys.argv[1]
COOKIE = b'conectix'
FOOTER_LEN = 512

for img_name in sorted(os.listdir(os.path.join(root, 'image'))):
    stem = img_name[:-len('.vhd')]
    img = open(os.path.join(root, 'image', img_name), 'rb').read()

    # The authoritative footer is the LAST 512 bytes. A dynamic VHD also
    # keeps a copy at offset 0, which is what makes mirror recovery
    # possible -- and what makes "which copy did you believe" a question
    # worth fuzzing.
    tail = img[-FOOTER_LEN:]
    assert tail[:8] == COOKIE, f"{img_name}: no footer cookie at the end"
    with open(os.path.join(root, 'footer', f'{stem}-tail.bin'), 'wb') as f:
        f.write(tail)

    if img[:8] == COOKIE:
        with open(os.path.join(root, 'footer', f'{stem}-head.bin'), 'wb') as f:
            f.write(img[:FOOTER_LEN])
        # The dynamic header follows the head copy of the footer. It
        # carries the block size and the table offset, and its own
        # 1024-byte length.
        head = img[FOOTER_LEN:FOOTER_LEN + 1024]
        assert head[:8] == b'cxsparse', f"{img_name}: no dynamic header after the footer"
        with open(os.path.join(root, 'dynamic_header', f'{stem}.bin'), 'wb') as f:
            f.write(head)
PY

echo "corpus rebuilt under fuzz/corpus:"
find "$here/fuzz/corpus" -type f | sort | sed "s#$here/##"
echo "total: $(find "$here/fuzz/corpus" -type f | wc -l) seeds, $(du -sh "$here/fuzz/corpus" | cut -f1)"
