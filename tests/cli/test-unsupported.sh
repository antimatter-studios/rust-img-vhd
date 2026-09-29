# A verb the library cannot do still exists and says so: exit status 3, a
# structured error beginning `not implemented`, and the image untouched.
source "$(dirname "$0")/lib.sh"

cd "$SANDBOX" || exit 1
qemu-img create -q -f vpc disk.vhd 8M
cp disk.vhd before.vhd

# not_implemented DESCRIPTION COMMAND...
not_implemented() {
    local what="$1"
    shift
    expect_error "$what" 3 "$@"
    jq_check "$what: the error says not implemented" \
        '.error | startswith("not implemented")' "$SANDBOX/error.json"
}

not_implemented "resize" img.vhd disk.vhd resize 16M
not_implemented "set" img.vhd disk.vhd set vhd.saved_state false
same "no refused verb changed the image" disk.vhd before.vhd

# --text turns the error into a line for a person, with the same status.
img.vhd disk.vhd resize 16M --text 2>resize.txt
check "resize --text exits 3" test $? -eq 3
check "resize --text says not implemented" grep -q 'img.vhd: not implemented' resize.txt

finish
