# `info` and `get` on images the oracle made: every canonical key, with the
# right type, and the values qemu-img reports for the same file.
#
# `-f vpc` on every qemu-img call: a fixed VHD is a raw image with a footer
# after it, and qemu-img probes it as raw, one sector too large.
#
# qemu-img sizes a VHD from the footer's current size (and, for some
# creators, its CHS geometry), so the size compared is what the footer
# records, never the size that was asked for: `qemu-img create -f vpc 8M`
# makes an 8,390,656-byte disk, not an 8 MiB one.
source "$(dirname "$0")/lib.sh"

cd "$SANDBOX" || exit 1

for sub in fixed dynamic; do
    for size in 8M 10000K; do
        img="$sub-$size.vhd"
        qemu-img create -q -f vpc -o subformat=$sub "$img" "$size"
        img.vhd "$img" info >"$img.json"
        check "info on $img exits 0" test $? -eq 0
        qemu-img info -f vpc --output=json "$img" >"$img.qemu.json"

        jq_check "$img: the canonical keys, in order" \
            '[keys_unsorted[]] == ["format","virtual_size","block_size","backing","dirty","vhd"]' "$img.json"
        jq_check "$img: format is vhd" '.format == "vhd"' "$img.json"
        jq_check "$img: virtual_size is a number" '.virtual_size | type == "number"' "$img.json"
        jq_check "$img: backing is null" '.backing == null' "$img.json"
        jq_check "$img: dirty is false" '.dirty == false' "$img.json"
        jq_check "$img: disk_type is $sub" ".vhd.disk_type == \"$sub\"" "$img.json"
        jq_check "$img: the creator is qemu's" '.vhd.creator.application == "qemu"' "$img.json"
        jq_check "$img: the geometry is three numbers" \
            '.vhd.geometry | [.cylinders, .heads, .sectors_per_track] | all(type == "number")' "$img.json"
        if [ "$sub" = fixed ]; then
            jq_check "$img: a fixed image has no block size" '.block_size == null' "$img.json"
        else
            jq_check "$img: a dynamic image has qemu's 2 MiB blocks" '.block_size == 2097152' "$img.json"
        fi

        # The oracle's view of the same file.
        ours="$(jq '.virtual_size' "$img.json")"
        theirs="$(jq '."virtual-size"' "$img.qemu.json")"
        check "$img: virtual_size $ours is qemu-img's $theirs" test "$ours" = "$theirs"
        check "$img: qemu-img reads it as vpc" \
            test "$(jq -r '.format' "$img.qemu.json")" = vpc

        # get KEY answers one key, as an object; --text answers the bare value.
        got="$(img.vhd "$img" get virtual_size --text)"
        check "$img: get virtual_size --text is $ours (got '$got')" test "$got" = "$ours"
        img.vhd "$img" get vhd.disk_type >"$img.key.json"
        jq_check "$img: get vhd.disk_type is one key" \
            "keys == [\"vhd.disk_type\"] and .[\"vhd.disk_type\"] == \"$sub\"" "$img.key.json"
    done
done

# info and get are the same verb.
img.vhd fixed-8M.vhd get >get.json
img.vhd fixed-8M.vhd info >info.json
same "get and info report the same thing" get.json info.json

# --text is key: value lines, nested keys dotted.
img.vhd dynamic-8M.vhd info --text >info.txt
check "info --text carries format: vhd" grep -qx 'format: vhd' info.txt
check "info --text dots the nested keys" grep -qx 'vhd.disk_type: dynamic' info.txt

# An unknown key is the caller's mistake: status 2, and nothing on stdout.
expect_error "get of an unknown key" 2 img.vhd fixed-8M.vhd get no_such_key

finish
