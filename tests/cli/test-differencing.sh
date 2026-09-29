# A differencing image reads through to its parent: a sector the child
# holds is the child's, every other is the parent's.
#
# Nothing here or in the oracle creates a differencing VHD (qemu-img:
# "Backing file not supported for file format 'vpc'"), so the child is a
# dynamic image qemu-io wrote, turned into a child of the parent by
# make-differencing.py, which rewrites three fields from the specification.
# The expected bytes are the parent's raw image, from qemu-img, with the
# child's writes laid over it by dd: no step of the expectation goes
# through this crate.
source "$(dirname "$0")/lib.sh"

cd "$SANDBOX" || exit 1
MAKE_DIFFERENCING="$(dirname "$0")/make-differencing.py"
[ -f "$MAKE_DIFFERENCING" ] || MAKE_DIFFERENCING="$REPO/tests/cli/make-differencing.py"

MiB=1048576
qemu-img create -q -f vpc parent.vhd 8M
qemu-io -f vpc -c "write -P 0x11 0 $((6 * MiB))" parent.vhd >/dev/null
qemu-img create -q -f vpc child.vhd 8M
# Inside a block the parent also has, and one sector past the parent's
# data. qemu-io allocates both blocks whole, 0xee and 0xdd where it wrote
# and zeros around them; make-differencing.py marks only the written
# sectors as the child's, so the zeros around them must read through.
qemu-io -f vpc -c "write -P 0xee 1024 1536" -c "write -P 0xdd $((7 * MiB)) 512" \
    -c "write -P 0x99 $((3 * MiB)) 512" child.vhd >/dev/null
python3 "$MAKE_DIFFERENCING" child.vhd parent.vhd 2:3 $((7 * MiB / 512)):1
check "make-differencing.py turned the child into one" test $? -eq 0

qemu-img convert -f vpc -O raw parent.vhd want.raw
printf '\356%.0s' $(seq 1536) | dd of=want.raw bs=1 seek=1024 conv=notrunc 2>/dev/null
printf '\335%.0s' $(seq 512) | dd of=want.raw bs=1 seek=$((7 * MiB)) conv=notrunc 2>/dev/null

img.vhd child.vhd info >child.json
jq_check "the child reports itself differencing" '.vhd.disk_type == "differencing"' child.json
jq_check "the child's backing is its parent's name" '.backing == "parent.vhd"' child.json
jq_check "the child names its parent's unique id" \
    ".vhd.parent.unique_id == \"$(img.vhd parent.vhd get vhd.unique_id --text)\"" child.json

# The 0x99 sector at 3 MiB is in a block the child allocated and marked
# none of: it is the parent's 0x11, not the child's 0x99.
img.vhd child.vhd read >child.raw
same "the child reads through to its parent where it holds nothing" child.raw want.raw

# Opened from another directory: the parent is found beside the child,
# not in the working directory.
mkdir elsewhere
(cd elsewhere && img.vhd ../child.vhd read >../from-elsewhere.raw)
same "the parent is found beside the child" from-elsewhere.raw want.raw

# Without its parent the child cannot be read, and says so.
mv parent.vhd parent.moved
expect_error "a child whose parent is gone" 1 img.vhd child.vhd read
check "the error names the missing parent" grep -q 'parent' "$SANDBOX/error.json"
mv parent.moved parent.vhd

finish
