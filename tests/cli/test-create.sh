# `create` makes images the oracle reads: the type asked for, the size the
# footer records (rounded up to a whole CHS geometry, as qemu-img rounds),
# nothing allocated in a dynamic image, every byte zero.
source "$(dirname "$0")/lib.sh"

cd "$SANDBOX" || exit 1

# zeros FILE N: N zero bytes in FILE.
zeros() {
    dd if=/dev/zero of="$1" bs=512 count=$(($2 / 512)) 2>/dev/null
}

# 8M and 10000K are not sizes a CHS geometry describes exactly, and are
# rounded up; 8390656 is qemu-img's own rounding of 8M, which is.
for type in fixed dynamic; do
    for size in 8M 10000K 8390656; do
        img="$type-$size.vhd"
        img.vhd "$img" create "$size" --type "$type" >"$img.json"
        check "create $size --type $type exits 0" test $? -eq 0
        jq_check "$img: the report is the info envelope" \
            '[keys_unsorted[]] == ["format","virtual_size","block_size","backing","dirty","vhd"]' "$img.json"
        jq_check "$img: disk_type is $type" ".vhd.disk_type == \"$type\"" "$img.json"
        img.vhd "$img" info >"$img.info.json"
        same "$img: create reports what info then reads" "$img.json" "$img.info.json"

        ours="$(jq '.virtual_size' "$img.json")"
        theirs="$(qemu-img info -f vpc --output=json "$img" | jq '."virtual-size"')"
        check "$img: virtual_size $ours is qemu-img's $theirs" test "$ours" = "$theirs"
        asked="$(case "$size" in 8M) echo 8388608 ;; 10000K) echo 10240000 ;; *) echo "$size" ;; esac)"
        check "$img: $ours is at least the $asked asked for" test "$ours" -ge "$asked"
        check "$img: $ours is a whole number of sectors" test $((ours % 512)) -eq 0

        # The subformat, as the oracle sees it: a fixed image maps every
        # byte to data, a fresh dynamic one maps none.
        qemu-img map -f vpc --output=json "$img" >"$img.map.json"
        if [ "$type" = fixed ]; then
            jq_check "$img: qemu-img maps every byte to data" 'all(.[]; .data == true)' "$img.map.json"
        else
            jq_check "$img: qemu-img maps no byte to data" 'all(.[]; .data == false)' "$img.map.json"
        fi

        # Every byte zero, as qemu-img reads it and as we do.
        qemu-img convert -f vpc -O raw "$img" "$img.qemu.raw"
        zeros "$img.zero.raw" "$ours"
        same "$img: qemu-img reads every byte zero" "$img.qemu.raw" "$img.zero.raw"
        img.vhd "$img" read >"$img.ours.raw"
        same "$img: we read every byte zero" "$img.ours.raw" "$img.zero.raw"
    done
done

# Exact sizes are not rounded, and the text says when one was.
check "an exact CHS size is created as asked" \
    test "$(jq '.virtual_size' dynamic-8390656.vhd.json)" -eq 8390656
img.vhd rounded.vhd create 8M --text >rounded.txt
check "--text says the size was rounded up" grep -q 'rounded up' rounded.txt

# A dynamic image's block size is the one asked for.
img.vhd small-blocks.vhd create 8M --block-size 512K >small-blocks.json
jq_check "--block-size 512K makes 512 KiB blocks" '.block_size == 524288' small-blocks.json
jq_check "--block-size 512K makes as many blocks as the disk needs" \
    '.vhd.block_count == ((.virtual_size + 524287) / 524288 | floor)' small-blocks.json
theirs="$(qemu-img info -f vpc --output=json small-blocks.vhd | jq '."virtual-size"')"
check "qemu-img reads the 512 KiB-block image's size" \
    test "$(jq '.virtual_size' small-blocks.json)" = "$theirs"

# An existing file is not replaced without --force.
cp dynamic-8M.vhd keep.vhd
cp keep.vhd keep.before
expect_error "create over an existing file" 1 img.vhd keep.vhd create 16M
same "the refused create left the file as it was" keep.vhd keep.before
img.vhd keep.vhd create 16M --force >forced.json
jq_check "create --force replaces it" '.virtual_size >= 16777216' forced.json

# Sizes and block sizes that cannot be are the caller's mistake: status 2.
expect_error "a size that is not whole sectors" 2 img.vhd bad.vhd create 1000
expect_error "a zero size" 2 img.vhd bad.vhd create 0
expect_error "a block size that is not a power of two" 2 img.vhd bad.vhd create 8M --block-size 3000
expect_error "a block size for a fixed image" 2 img.vhd bad.vhd create 8M --type fixed --block-size 512K
expect_error "a type that does not exist" 2 img.vhd bad.vhd create 8M --type differencing
check "no refused create made a file" test ! -e bad.vhd

finish
