# `read` on images the oracle made and filled: the whole disk is
# byte-identical to `qemu-img convert -O raw`, and every range -- inside a
# block, across a block boundary, in a block never allocated, the last
# byte -- is the same slice of that raw image.
source "$(dirname "$0")/lib.sh"

cd "$SANDBOX" || exit 1

# slice FILE OFFSET LENGTH: those bytes of FILE, on stdout.
slice() {
    dd if="$1" bs=1 skip="$2" count="$3" 2>/dev/null
}

MiB=1048576
for sub in fixed dynamic; do
    img="$sub.vhd"
    qemu-img create -q -f vpc -o subformat=$sub "$img" 8M
    # Patterns in the first block, straddling the first block boundary
    # (2 MiB, qemu's block size), and in the last sector. Block 2, [4, 6)
    # MiB, is never written: a dynamic image leaves it unallocated.
    size="$(qemu-img info -f vpc --output=json "$img" | jq '."virtual-size"')"
    last=$((size - 512))
    qemu-io -f vpc -c "write -P 0x5a 4096 8192" -c "write -P 0xc3 $((2 * MiB - 1000)) 3000" \
        -c "write -P 0x7e $last 512" "$img" >/dev/null
    qemu-img convert -f vpc -O raw "$img" "$sub.qemu.raw"

    img.vhd "$img" read >"$sub.ours.raw"
    check "read of the whole $sub disk exits 0" test $? -eq 0
    same "the whole $sub disk, read, is qemu-img's raw image" "$sub.ours.raw" "$sub.qemu.raw"

    img.vhd "$img" read -o "$sub.o.raw"
    same "read -o writes the same bytes as read to stdout ($sub)" "$sub.o.raw" "$sub.qemu.raw"
    check "read -o leaves no .partial file ($sub)" test ! -e "$sub.o.raw.partial"

    for range in "4096 8192" "0 512" "$((2 * MiB - 1000)) 3000" "$((4 * MiB)) 65536" "$((size - 1)) 1" "$last 512"; do
        set -- $range
        img.vhd "$img" read --offset "$1" --length "$2" >"$sub.range"
        slice "$sub.qemu.raw" "$1" "$2" >"$sub.want"
        same "$sub: read --offset $1 --length $2" "$sub.range" "$sub.want"
    done

    # --offset alone reads to the end; --length alone reads from 0.
    img.vhd "$img" read --offset "$last" >"$sub.tail"
    slice "$sub.qemu.raw" "$last" 512 >"$sub.want"
    same "$sub: read --offset alone reads to the end" "$sub.tail" "$sub.want"
    img.vhd "$img" read --length 16K >"$sub.head"
    slice "$sub.qemu.raw" 0 16384 >"$sub.want"
    same "$sub: read --length alone reads from 0, and takes a suffix" "$sub.head" "$sub.want"

    # A range past the end is refused whole, before a byte is written.
    expect_error "$sub: a range past the end" 1 img.vhd "$img" read --offset "$last" --length 513
    expect_error "$sub: an offset past the end" 1 img.vhd "$img" read --offset "$((size + 1))"
done

# A closed pipe is the reader's choice, not a failure.
img.vhd dynamic.vhd read 2>pipe.err | head -c 100 >/dev/null
check "read into a pipe closed early exits 0" test "${PIPESTATUS[0]}" -eq 0
check "read into a pipe closed early says nothing on stderr" test ! -s pipe.err

expect_error "read of a file that is not there" 1 img.vhd missing.vhd read
head -c 4096 /dev/urandom >noise.bin
expect_error "read of a file that is not a VHD" 1 img.vhd noise.bin read

finish
