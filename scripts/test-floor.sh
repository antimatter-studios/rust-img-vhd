#!/usr/bin/env bash
# test-floor.sh TIER FLOOR  — the tier ran at least FLOOR tests
#
# THE FAILURE A BUDGET CANNOT SEE. scripts/tier.sh fails a tier that PRINTS
# more than it is allowed to; nothing fails a tier that prints almost nothing
# because it ran almost nothing. `cargo test` exits 0 on "0 passed; 0
# failed", so a filter that selects nothing, a build that produced no test
# binaries, and a suite that ran in full all report the same green. That is
# #99, and it is why every tier carries a number.
#
# THIS IS THE FLOOR THAT USED TO BE INLINE IN ci.yml, moved to a file and
# not otherwise changed. It was eight lines of shell repeated in four jobs
# and in three sibling repositories; the numbers still live at the call site,
# where they belong, and the counting does not.
#
# It reads the tier's log (tmp/logs/<TIER>.log, written by tier.sh) rather
# than a pipe, so it cannot swallow the test run's own verdict — a
# `cargo test | test-floor.sh` would report this script's exit status and
# discard the suite's, which is the defect tests/ci_profile.rs exists to
# prevent.
#
# A COUNT ANSWERS "DID ANYTHING RUN", NOT "DID IT CHECK ANYTHING". A test
# that returns early because a tool is missing still counts as passed, which
# is what #97 was. The answer to that is a test that panics rather than
# returns, not a bigger number here.
#
# The floor is MEASURED, and it only ever goes up: a floor lowered to make a
# run pass is a floor that has stopped measuring anything.
set -euo pipefail

[ $# -eq 2 ] || { echo "usage: test-floor.sh TIER FLOOR" >&2; exit 2; }
TIER="$1"
FLOOR="$2"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOG="$REPO/tmp/logs/$TIER.log"

if [ ! -f "$LOG" ]; then
    echo "test-floor.sh: $LOG is missing -- the $TIER tier did not run." >&2
    exit 1
fi

# `test result: ok. 37 passed; 0 failed; ...`, one line per test binary.
# `-a` because a test that prints a byte sequence cargo's log cannot decode
# makes grep call the file binary and report nothing at all.
ran="$(grep -aoE 'test result: ok\. [0-9]+ passed' "$LOG" | awk '{ sum += $4 } END { print sum + 0 }')"
if [ "$ran" -lt "$FLOOR" ]; then
    echo "::error::only $ran tests executed in the $TIER tier, floor is $FLOOR -- a run that executes fewer than that stopped early rather than passed"
    echo "test-floor.sh: the $TIER tier executed $ran tests; the floor is $FLOOR." >&2
    echo "               A tier that runs fewer tests than it used to has stopped" >&2
    echo "               early rather than passed. The whole run is in $LOG." >&2
    exit 1
fi
printf '%s\n' "$TIER: $ran tests executed (floor $FLOOR)"
