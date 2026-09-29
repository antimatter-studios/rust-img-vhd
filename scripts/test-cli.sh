#!/usr/bin/env bash
# test-cli.sh — THE `cli` TIER: the command-line tools as installed, found
# on PATH, not a cargo target.
#
# The same suite checks a checkout's build (`chore cli:install` stages it and
# prints the PATH line) and a Homebrew install (`brew install
# antimatter-studios/tap/rust-img-vhd`): it tests whatever PATH resolves,
# because that is what a user runs.
#
# STEP 1, BEFORE ANY TOOL IS TESTED: the tools are present and are ours.
#   - jq and rust-img-vhd must resolve, and rust-img-vhd must answer
#     --version as this crate;
#   - `rust-img-vhd doctor` must pass: every dotted name on PATH is our
#     program at our version. If one is shadowed -- a distribution's
#     another formula's img.vhd, an older install of ours -- the tier fails
#     with doctor's report: what wins, and the fix.
#   - the oracle, qemu-img and qemu-io, and python3 (which builds the one
#     image shape neither we nor qemu-img can create: a differencing VHD),
#     must resolve.
# NOTHING SKIPS. A missing tool fails the tier naming what provides it; a
# tier that tested someone else's img.vhd and reported green would be
# worse than no tier, and one that skipped the oracle would be a tier that
# checked us against ourselves.
#
# STEP 2: every tests/cli/test-*.sh, by glob, so a new one needs no edit
# here. Each prints its failures, a `test result: ok. N passed; ...` line
# (the count scripts/test-floor.sh reads, as it does cargo's), and LAST
# `<name>: all checks passed`. A file that exits 0 without that last line
# stopped early and is a failure: `exit 0` part-way through is not evidence
# a file finished.
#
# Quiet: the tier runs under scripts/tier.sh, which keeps the whole run in
# tmp/logs/cli.log.
#
# CLI_TESTS names another directory of test-*.sh files, for
# tests/scripts/test-cli-tier.sh, which proves every refusal above refuses.
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
TESTS="${CLI_TESTS:-$REPO/tests/cli}"
CRATE="$(sed -n 's/^name *= *"\(.*\)"/\1/p' "$REPO/Cargo.toml" | head -n 1)"
INSTALL="build and stage it with \`chore cli:install\` (it prints the PATH line to use), or install it with \`brew install antimatter-studios/tap/rust-img-vhd\`"

refuse() {
    echo "test-cli: $*" >&2
    exit 1
}

command -v jq >/dev/null 2>&1 ||
    refuse "jq is not on PATH; the suite reads every JSON report with it. Install it: \`brew install jq\`, or \`apt-get install jq\`."
for oracle in qemu-img qemu-io; do
    command -v "$oracle" >/dev/null 2>&1 ||
        refuse "$oracle is not on PATH; it is the independent oracle every image is checked against. Install it: \`brew install qemu\`, or \`apt-get install qemu-utils\`."
done
command -v man >/dev/null 2>&1 ||
    refuse "man is not on PATH; tests/cli/test-docs.sh asks it to find each installed page. Install it: \`apt-get install man-db\`."
command -v python3 >/dev/null 2>&1 ||
    refuse "python3 is not on PATH; tests/cli/make-differencing.py builds the differencing image. Install it: \`brew install python\`, or \`apt-get install python3\`."

entry="$(command -v rust-img-vhd 2>/dev/null || true)"
[ -n "$entry" ] || refuse "rust-img-vhd is not on PATH: $INSTALL."
answer="$(rust-img-vhd --version 2>/dev/null || true)"
case "$answer" in
    "rust-img-vhd ($CRATE) "*) ;;
    *) refuse "$entry is not $CRATE's rust-img-vhd: --version answered '$answer'. Put ours first on PATH: $INSTALL." ;;
esac
echo "== $answer, at $entry"

echo "== rust-img-vhd doctor"
if ! report="$(rust-img-vhd doctor --text 2>&1)"; then
    printf '%s\n' "$report" >&2
    refuse "doctor found a tool on PATH that is not this program; its fixes are above. Nothing was tested."
fi
printf '%s\n' "$report"

shopt -s nullglob
files=("$TESTS"/test-*.sh)
[ "${#files[@]}" -gt 0 ] || refuse "no test-*.sh in $TESTS to run"

failed=0
for file in "${files[@]}"; do
    name="$(basename "$file" .sh)"
    echo "== $name"
    output="$(bash "$file" 2>&1)"
    status=$?
    printf '%s\n' "$output"
    last="$(printf '%s\n' "$output" | tail -n 1)"
    if [ "$status" -ne 0 ]; then
        echo "FAIL  $name exited $status" >&2
        failed=$((failed + 1))
    elif [ "$last" != "$name: all checks passed" ]; then
        echo "FAIL  $name exited 0 without its last line, '$name: all checks passed': it stopped early" >&2
        failed=$((failed + 1))
    fi
done

if [ "$failed" -gt 0 ]; then
    echo "test-cli: $failed of ${#files[@]} files failed" >&2
    exit 1
fi
echo "test-cli: ${#files[@]} files, all checks passed"
