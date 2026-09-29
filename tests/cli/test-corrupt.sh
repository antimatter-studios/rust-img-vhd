# Damaged images: a trailing footer that fails its checksum beside a good
# mirror is recovered, and says so; both copies bad, a block table entry
# past the end of the file, and a file that is not a VHD all fail with a
# structured error and nothing on stdout.
source "$(dirname "$0")/lib.sh"

cd "$SANDBOX" || exit 1

# poke FILE OFFSET BYTE: overwrite one byte.
poke() {
    printf "\\$(printf '%03o' "$3")" | dd of="$1" bs=1 seek="$2" conv=notrunc 2>/dev/null
}
filesize() { wc -c <"$1" | tr -d ' '; }

qemu-img create -q -f vpc good.vhd 8M
qemu-io -f vpc -c "write -P 0x42 0 65536" good.vhd >/dev/null
qemu-img convert -f vpc -O raw good.vhd good.raw

# The trailing footer's checksum no longer matches; the mirror at 0 does.
cp good.vhd tail-bad.vhd
poke tail-bad.vhd $(($(filesize tail-bad.vhd) - 512 + 64)) 0
img.vhd tail-bad.vhd info >tail-bad.json
check "an image with a damaged trailing footer still opens" test $? -eq 0
jq_check "get says the footer came from the mirror" \
    '.vhd.footer_recovered_from_mirror == true' tail-bad.json
jq_check "a healthy image says it did not" '.vhd.footer_recovered_from_mirror == false' \
    <(img.vhd good.vhd info)
img.vhd tail-bad.vhd read >tail-bad.raw
same "the recovered image reads what the healthy one held" tail-bad.raw good.raw

# Both copies damaged: nothing to recover from.
cp tail-bad.vhd both-bad.vhd
poke both-bad.vhd 64 0
expect_error "both footer copies damaged" 1 img.vhd both-bad.vhd info
expect_error "both footer copies damaged, read" 1 img.vhd both-bad.vhd read

# The first block table entry points far past the end of the file.
table="$(od -An -tx1 -j $((512 + 16 + 4)) -N 4 good.vhd | tr -d ' \n')"
table=$((16#$table))
cp good.vhd bat-past-end.vhd
for i in 0 1 2 3; do poke bat-past-end.vhd $((table + i)) 127; done
expect_error "a block table entry past the end of the file" 1 \
    img.vhd bat-past-end.vhd read --length 512

# Not a VHD at all, and an empty file.
head -c 65536 /dev/urandom >noise.bin
expect_error "a file of noise" 1 img.vhd noise.bin info
: >empty.vhd
expect_error "an empty file" 1 img.vhd empty.vhd info

finish
