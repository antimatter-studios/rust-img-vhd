# tests/cli/lib.sh — what every tests/cli/test-*.sh sources.
#
# The house style: `ok`/`fail`, a `fails` counter, `set -uo pipefail` (not
# -e, so later checks still run after one fails), a sandbox under the
# repository's tmp/, and `finish` last, which prints the count
# scripts/test-floor.sh reads and the trailing `<name>: all checks passed`
# line scripts/test-cli.sh requires.
#
# The tools are whatever PATH finds: scripts/test-cli.sh has already made
# sure they are ours before any file runs.
set -uo pipefail

NAME="$(basename "$0" .sh)"
REPO="$(cd "$(dirname "$0")/../.." && pwd -P)"
CRATE="$(sed -n 's/^name *= *"\(.*\)"/\1/p' "$REPO/Cargo.toml" | head -n 1)"

passed=0
fails=0
ok() { passed=$((passed + 1)); }
fail() {
    echo "FAIL  $NAME: $*" >&2
    fails=$((fails + 1))
}

# check DESCRIPTION COMMAND...: ok if COMMAND succeeds, else fail naming it.
check() {
    local what="$1"
    shift
    if "$@"; then ok; else fail "$what"; fi
}

# jq_check DESCRIPTION FILTER FILE: ok if jq -e FILTER holds for FILE.
jq_check() {
    local what="$1" filter="$2" file="$3"
    if jq -e "$filter" "$file" >/dev/null 2>&1; then
        ok
    else
        fail "$what: jq -e '$filter' is false for: $(head -c 2000 "$file")"
    fi
}

# expect_error DESCRIPTION CODE COMMAND...: COMMAND fails with exit status
# CODE, prints nothing on stdout, and says why on stderr as the structured
# error `{"error": "...", "code": CODE}`. The message is left in
# $SANDBOX/error.json for a caller that wants to look at its words.
expect_error() {
    local what="$1" code="$2"
    shift 2
    "$@" >"$SANDBOX/error.out" 2>"$SANDBOX/error.json"
    local status=$?
    if [ "$status" -ne "$code" ]; then
        fail "$what: exited $status, not $code: $(head -c 500 "$SANDBOX/error.json")"
    elif [ -s "$SANDBOX/error.out" ]; then
        fail "$what: printed $(wc -c <"$SANDBOX/error.out") bytes on stdout beside its error"
    elif ! jq -e --argjson code "$code" '(.error | type == "string") and .code == $code' \
        "$SANDBOX/error.json" >/dev/null 2>&1; then
        fail "$what: stderr is not a structured error with code $code: $(head -c 500 "$SANDBOX/error.json")"
    else
        ok
    fi
}

# same DESCRIPTION FILE-A FILE-B: the two files hold the same bytes.
same() {
    local what="$1" a="$2" b="$3"
    if cmp -s "$a" "$b"; then ok; else fail "$what: $(cmp "$a" "$b" 2>&1 | head -n 1)"; fi
}

mkdir -p "$REPO/tmp"
SANDBOX="$(mktemp -d "$REPO/tmp/cli-$NAME.XXXXXX")"
trap 'rm -rf "$SANDBOX"' EXIT HUP INT TERM

finish() {
    if [ "$fails" -gt 0 ]; then
        echo "test result: FAILED. $passed passed; $fails failed"
        exit 1
    fi
    echo "test result: ok. $passed passed; 0 failed"
    echo "$NAME: all checks passed"
    exit 0
}
