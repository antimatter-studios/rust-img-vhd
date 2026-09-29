# `write --offset` puts exactly the bytes on stdin at exactly that offset,
# and qemu-img reads them there: within one block, across a block
# boundary, into a block never allocated, at the last byte. Both ways
# round -- images we created, and images qemu-img created -- and the
# expected disk is built with dd, not with this tool.
source "$(dirname "$0")/lib.sh"

cd "$SANDBOX" || exit 1

MiB=1048576

# overlay FILE OFFSET INPUT: INPUT's bytes over FILE at OFFSET.
overlay() {
    dd if="$3" of="$1" bs=1 seek="$2" conv=notrunc 2>/dev/null
}

# random NAME N: N random bytes in NAME.
random() {
    head -c "$2" /dev/urandom >"$1"
}

# exercise IMAGE LABEL: write the four shapes into IMAGE, check what we and
# qemu-img then read.
exercise() {
    local img="$1" label="$2"
    local size last
    size="$(img.vhd "$img" get virtual_size --text)"
    last=$((size - 1))
    dd if=/dev/zero of="$label.want" bs=512 count=$((size / 512)) 2>/dev/null

    random "$label.in1" 700
    random "$label.in2" 5000
    random "$label.in3" 3000
    random "$label.in4" 1

    # Within the first block, from a regular file on stdin.
    img.vhd "$img" write --offset 4096 <"$label.in1" >"$label.w1.json"
    jq_check "$label: write reports the offset and the count" \
        '.offset == 4096 and .bytes == 700' "$label.w1.json"
    overlay "$label.want" 4096 "$label.in1"
    # Across the first block boundary, through a pipe.
    cat "$label.in2" | img.vhd "$img" write --offset $((2 * MiB - 1000)) >/dev/null
    check "$label: a piped write across a block boundary exits 0" test "${PIPESTATUS[1]}" -eq 0
    overlay "$label.want" $((2 * MiB - 1000)) "$label.in2"
    # Into a block nothing has touched.
    img.vhd "$img" write --offset $((5 * MiB + 17)) <"$label.in3" >/dev/null
    overlay "$label.want" $((5 * MiB + 17)) "$label.in3"
    # The last byte of the disk.
    img.vhd "$img" write --offset "$last" <"$label.in4" >/dev/null
    overlay "$label.want" "$last" "$label.in4"

    # Each write reads back as itself.
    for w in "4096 700 in1" "$((2 * MiB - 1000)) 5000 in2" "$((5 * MiB + 17)) 3000 in3" "$last 1 in4"; do
        set -- $w
        img.vhd "$img" read --offset "$1" --length "$2" >"$label.back"
        same "$label: read --offset $1 --length $2 returns what was written" "$label.back" "$label.$3"
    done

    # The oracle reads exactly those bytes at exactly those offsets, and
    # zeros everywhere else.
    qemu-img convert -f vpc -O raw "$img" "$label.qemu.raw"
    same "$label: qemu-img reads exactly the bytes written, where they were written" \
        "$label.qemu.raw" "$label.want"
    img.vhd "$img" read >"$label.ours.raw"
    same "$label: our whole-disk read agrees with qemu-img" "$label.ours.raw" "$label.want"
    check "$label: qemu-img still reads it as the size it was" \
        test "$(qemu-img info -f vpc --output=json "$img" | jq '."virtual-size"')" = "$size"
}

# Images we created.
img.vhd ours-fixed.vhd create 8M --type fixed >/dev/null
exercise ours-fixed.vhd ours-fixed
img.vhd ours-dynamic.vhd create 8M >/dev/null
exercise ours-dynamic.vhd ours-dynamic
img.vhd ours-small.vhd create 8M --block-size 512K >/dev/null
exercise ours-small.vhd ours-small-blocks

# Images the oracle created.
qemu-img create -q -f vpc -o subformat=fixed qemu-fixed.vhd 8M
exercise qemu-fixed.vhd qemu-fixed
qemu-img create -q -f vpc qemu-dynamic.vhd 8M
exercise qemu-dynamic.vhd qemu-dynamic

# Nothing on stdin writes nothing, and says so.
img.vhd ours-dynamic.vhd write --offset 0 </dev/null >empty.json
jq_check "an empty write reports 0 bytes" '.bytes == 0' empty.json

# Input that would run past the end is refused before anything is written,
# from a file and from a pipe.
cp ours-dynamic.vhd before.vhd
size="$(img.vhd ours-dynamic.vhd get virtual_size --text)"
random past.bin 1024
expect_error "a file running past the end" 1 img.vhd ours-dynamic.vhd write --offset $((size - 512)) <past.bin
cat past.bin | img.vhd ours-dynamic.vhd write --offset $((size - 512)) >/dev/null 2>past.json
check "a pipe running past the end exits 1" test "${PIPESTATUS[1]}" -eq 1
expect_error "an offset past the end" 1 img.vhd ours-dynamic.vhd write --offset $((size + 1)) </dev/null
same "no refused write changed the image" ours-dynamic.vhd before.vhd

# The image redirected onto its own stdin is refused.
expect_error "the image as its own input" 1 img.vhd ours-dynamic.vhd write --offset 0 <ours-dynamic.vhd
same "the refused self-write changed nothing" ours-dynamic.vhd before.vhd

expect_error "write without --offset" 2 img.vhd ours-dynamic.vhd write </dev/null

finish
