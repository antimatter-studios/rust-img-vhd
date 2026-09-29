# Every installed tool has its man page and its zsh, bash and fish
# completions where the install prefix keeps them -- share/ beside the bin/
# that PATH found the tool in, the layout of the release tarball and of a
# Homebrew prefix alike -- man(1) finds each page, each page names every
# subcommand the tool's --help lists, and each subcommand's --help carries
# an example.
source "$(dirname "$0")/lib.sh"

for name in $(rust-img-vhd generate names) rust-img-vhd; do
    path="$(command -v "$name" 2>/dev/null || true)"
    if [ -z "$path" ]; then
        fail "$name is not on PATH"
        continue
    fi
    share="$(cd "$(dirname "$path")/.." && pwd)/share"
    case "$name" in mkfs.* | fsck.*) section=8 ;; *) section=1 ;; esac
    page="$share/man/man$section/$name.$section"
    check "$name has a man page at $page" test -s "$page"
    # And man(1) finds it there, which is what a person will type.
    found="$(MANPATH="$share/man" man -w "$name" 2>/dev/null)"
    check "man -w $name finds $page (found '$found')" test "$found" = "$page"
    for verb in $("$name" --help | awk '/^Commands:/ { on = 1; next } on && /^  [a-z]/ { print $1 } on && !/^  / { on = 0 }'); do
        [ "$verb" = help ] && continue
        check "$name's man page mentions $verb" grep -q -- "$verb" "$page"
        # Every subcommand's --help carries an example. The image tool takes
        # its target first; `help` never looks at it.
        case "$name" in
            img.*) sub_help="$("$name" disk.img "$verb" --help 2>&1)" ;;
            *) sub_help="$("$name" "$verb" --help 2>&1)" ;;
        esac
        check "$name $verb --help carries an example" grep -q '^Examples:' <<<"$sub_help"
    done
    check "$name has a zsh completion" test -s "$share/zsh/site-functions/_$name"
    check "$name has a bash completion" test -s "$share/bash-completion/completions/$name"
    check "$name has a fish completion" test -s "$share/fish/vendor_completions.d/$name.fish"
done

finish
