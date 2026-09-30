#!/usr/bin/env bash
# package-cli.sh <version> <label> [bin-dir]
#
# Package the built command-line tool as a release tarball in the current
# directory, check it, and print its file name on stdout.
#
#   <version>   the release version, without the leading `v`
#   <label>     the platform: darwin-arm64 or linux-x86_64
#   [bin-dir]   where the built `rust-img-vhd` is (default: target/release)
#
# THE TARBALL IS THE CONTRACT with whatever installs it, and every
# repository in the family uses one layout: an install prefix, copied as-is,
# so an installer needs to know nothing about which tools are in it.
#
#   bin/rust-img-vhd                             the multi-call binary
#   bin/img.vhd -> rust-img-vhd                  a relative symlink per name
#   share/man/man1/<name>.1, <name>-<verb>.1     generated man pages
#   share/zsh/site-functions/_<name>             generated completions
#   share/bash-completion/completions/<name>
#   share/fish/vendor_completions.d/<name>.fish
#   share/rust-img-vhd/CAVEATS                   at most four lines
#   LICENSE
#
# Cargo refuses a dot in a target name, so THE DOTTED NAMES ARE MADE HERE,
# as symlinks. The man pages and completions come from the binary itself
# (`rust-img-vhd generate ...`), from the argument definitions it parses
# with, so they cannot describe a flag it does not take.
#
# THEN IT CHECKS WHAT IT BUILT, from the unpacked tarball: exactly the
# members above; each dotted name a relative symlink to the binary; a man
# page and three completions for every name; CAVEATS at most four lines;
# and every name answering --help, and --version as `<name> (<crate>)
# <version>`, which identifies it among same-named programs and catches a
# tag that disagrees with Cargo.toml. The names are written here, not read
# from the binary: a binary that forgot one would otherwise agree with
# itself. On any failure nothing is printed on stdout and no tarball is
# left. tests/scripts/test-package-cli.sh holds this script to all of that.
set -euo pipefail

version="${1:-}"
label="${2:-}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bin_dir="${3:-$root/target/release}"

repo="rust-img-vhd"
names=(img.vhd)

die() { echo "package-cli: $*" >&2; exit 1; }

[ -n "$version" ] || die "usage: package-cli.sh <version> <label> [bin-dir]"
[ -n "$label" ] || die "usage: package-cli.sh <version> <label> [bin-dir]"

crate="$(sed -n 's/^name = "\(.*\)"$/\1/p' "$root/Cargo.toml" | head -n 1)"
[ -n "$crate" ] || die "no package name in $root/Cargo.toml"

tarball="$crate-$version-$label.tar.gz"
work="$(mktemp -d)"

# ON ANY FAILURE, NO TARBALL: not a partial one, and not one a previous run
# left under the same name, which a caller could otherwise take for this
# run's output.
cleanup() {
    local status=$?
    rm -rf "$work"
    [ "$status" -eq 0 ] || rm -f "$tarball"
    return "$status"
}
trap cleanup EXIT

built="$bin_dir/$repo"
[ -x "$built" ] || die "no built $repo at $built (cargo build --release --locked --features cli --bin $repo)"

stage="$work/stage"
mkdir -p "$stage/bin" "$stage/share/$repo"
cp "$built" "$stage/bin/$repo"
chmod 755 "$stage/bin/$repo"

listed="$("$stage/bin/$repo" generate names | tr '\n' ' ' | sed 's/ $//')" ||
    die "$repo generate names failed"
[ "$listed" = "${names[*]}" ] ||
    die "$repo generate names lists '$listed', but this script packages '${names[*]}'"
for name in "${names[@]}"; do
    ln -s "$repo" "$stage/bin/$name"
done
"$stage/bin/$repo" generate man "$stage/share" >/dev/null || die "$repo generate man failed"
"$stage/bin/$repo" generate completions "$stage/share" >/dev/null ||
    die "$repo generate completions failed"
cp "$root/packaging/CAVEATS" "$stage/share/$repo/CAVEATS"
cp "$root/LICENSE" "$stage/LICENSE"

# COPYFILE_DISABLE keeps macOS tar from adding ._ AppleDouble members.
COPYFILE_DISABLE=1 tar -czf "$tarball" -C "$stage" bin share LICENSE

# ---- The checks, on what was packed rather than on what was staged. ----
unpacked="$work/unpacked"
mkdir -p "$unpacked"
tar -xzf "$tarball" -C "$unpacked"

# Every member but the man pages is named exactly; the pages are one per
# name and one per subcommand, so they are checked by name below and by
# pattern here.
want=("bin/$repo" "share/$repo/CAVEATS" LICENSE)
for name in "${names[@]}" "$repo"; do
    [ "$name" = "$repo" ] || want+=("bin/$name")
    want+=("share/man/man1/$name.1"
        "share/zsh/site-functions/_$name"
        "share/bash-completion/completions/$name"
        "share/fish/vendor_completions.d/$name.fish")
done
for member in "${want[@]}"; do
    [ -e "$unpacked/$member" ] || [ -L "$unpacked/$member" ] || die "$tarball has no $member"
done
# Files and links only: whether a tar lists directories varies by tar.
extra="$(cd "$unpacked" && find . \( -type f -o -type l \) | sed 's|^\./||' | sort |
    while read -r member; do
        # Each pattern opens with "(": bash 3.2, macOS's /bin/bash, ends the
        # $( ) at a bare pattern's ")" and fails to parse the rest.
        case " ${want[*]} " in (*" $member "*) continue ;; esac
        case "$member" in
            (share/man/man1/*.1) ;;
            (*) echo "$member" ;;
        esac
    done)"
[ -z "$extra" ] || die "$tarball holds members outside the layout: $(echo $extra)"

for name in "${names[@]}"; do
    link="$unpacked/bin/$name"
    [ -L "$link" ] || die "bin/$name is not a symlink"
    [ "$(readlink "$link")" = "$repo" ] ||
        die "bin/$name points at '$(readlink "$link")', not the relative '$repo'"
done

lines="$(wc -l <"$unpacked/share/$repo/CAVEATS" | tr -d ' ')"
[ -s "$unpacked/share/$repo/CAVEATS" ] || die "share/$repo/CAVEATS is empty"
[ "$lines" -le 4 ] || die "share/$repo/CAVEATS is $lines lines; an installer prints it, and four is the most"

for name in "$repo" "${names[@]}"; do
    exe="$unpacked/bin/$name"
    [ -x "$exe" ] || die "bin/$name is not executable in $tarball"
    "$exe" --help >/dev/null || die "$name --help failed"
    reported="$("$exe" --version)" || die "$name --version failed"
    [ "$reported" = "$name ($crate) $version" ] ||
        die "$name --version says '$reported', expected '$name ($crate) $version'"
done

printf '%s\n' "$tarball"
