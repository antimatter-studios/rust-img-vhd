# Every name this repository installs resolves on PATH, answers --version
# as itself and this crate, at the one version the entry point reports,
# and carries an example in its --help.
source "$(dirname "$0")/lib.sh"

# The names are written here, not read from the binary: a binary that
# forgot one would otherwise agree with itself.
EXPECTED="img.vhd"

version="$(rust-img-vhd --version | sed -n "s/^rust-img-vhd ($CRATE) //p")"
check "rust-img-vhd --version names a version" test -n "$version"

listed="$(rust-img-vhd generate names | tr '\n' ' ' | sed 's/ $//')"
check "rust-img-vhd generate names lists exactly '$EXPECTED' (got '$listed')" \
    test "$listed" = "$EXPECTED"

for name in $EXPECTED rust-img-vhd; do
    path="$(command -v "$name" 2>/dev/null || true)"
    if [ -z "$path" ]; then
        fail "$name is not on PATH"
        continue
    fi
    ok
    for flag in --version -V; do
        got="$("$name" "$flag" 2>&1)"
        check "$path $flag answered '$got', not '$name ($CRATE) $version'" \
            test "$got" = "$name ($CRATE) $version"
    done
    help="$("$name" --help 2>&1)"
    check "$name --help carries no example" grep -q '^Examples:' <<<"$help"
done

# The repository-named form reaches every tool, and nothing can shadow it.
for name in $EXPECTED; do
    verb="${name%%.*}"
    got="$(rust-img-vhd "$verb" --version 2>&1)"
    check "rust-img-vhd $verb --version answered '$got'" test "$got" = "$name ($CRATE) $version"
done

# A bare entry point shows its help and says nothing was done.
rust-img-vhd >"$SANDBOX/bare.out" 2>&1
check "a bare rust-img-vhd exits 2" test $? -eq 2
check "a bare rust-img-vhd lists the tools" grep -q 'img' "$SANDBOX/bare.out"

finish
