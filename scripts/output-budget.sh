#!/usr/bin/env bash
#
# VENDORED, ON PURPOSE, AND THIS IS WHERE IT CAME FROM:
#
#   antimatter-studios/fs-linux-test-harness, scripts/output-budget.sh
#   tag v0.1.0 (88dae74c81a76e4689f4c1fc7557cc4db0668612)
#
# WHY A COPY RATHER THAN THE SIBLING. The filesystem crates in this family
# read this script out of ../fs-linux-test-harness, which `chore siblings`
# checks out for them; they need that repository anyway, because their
# oracles run inside the Linux VM it provides. This crate does not.
#
# Three reasons a sibling would be the wrong trade HERE, in a crate whose
# tests are pure userspace:
#
#   1. THE SIBLING LIST IS A PUBLISHED CONTRACT. chores.yml states that
#      `staticlib` is the whole interface a consumer needs, and the one
#      directory a consumer has to provision beside this checkout is
#      ../rust-fs-core -- which ci.yml clones and nothing else. Adding a
#      Linux VM harness to that list, for a 100-line shell script, would
#      make the sentence in chores.yml false for every consumer.
#
#   2. THE SUITE RUNS ON windows-latest AND macos-latest. The harness is a
#      Vagrant/QEMU/KVM Linux VM; there is no version of "check out the
#      harness" that means anything on those two runners, and they are two
#      thirds of the test matrix.
#
#   3. IT IS ONE FILE WITH NO DEPENDENCIES. Everything it needs is in
#      POSIX sh plus bash arrays.
#
# KEEPING IT HONEST. This is a copy, so it can drift. What is guarded is
# the BEHAVIOUR, not the bytes: tests/output_budget.rs runs this script and
# asserts the contract below -- exit 65 for a breached budget, the
# command's own status for a failure, the tail on failure, silence on
# success, and that --verbose streams without lifting the budget. A
# rewrite that still satisfies those is a fine copy; one that does not
# fails here rather than in whatever reads the log next.
#
# output-budget.sh — run a command quietly, keep everything, and refuse a run
# that prints more than it is allowed to.
#
#   output-budget.sh --log FILE [--max-lines N] [--max-bytes N] [--tail N]
#                    [--label TEXT] -- COMMAND [ARG...]
#
#   --log FILE     where the whole run is written (created; its directory too)
#   --max-lines N  refuse a run whose output exceeds N lines   (default: none)
#   --max-bytes N  refuse a run whose output exceeds N bytes   (default: none)
#   --tail N       lines of the log to show when the command fails (default 40)
#   --label TEXT   what to call the run in the summary line (default: the command)
#   --verbose      stream the output as it happens as well as logging it;
#                  also set by FLTH_VERBOSE=1. Budgets are still enforced.
#
# WHY A BUDGET IS A TEST. A passing run that prints three thousand lines hides
# the twenty that matter, and every reader pays for it: a person scrolling, a
# CI log viewer, and an agent working in the repository, which re-reads its
# whole transcript on each step and so pays for one verbose run many times
# over. Measured on this constellation: 4,661M cache-read tokens against 9.5M
# of output, and command output was the largest single contributor that a
# repository controls. "Keep it quiet" as a convention rots in a week; as a
# number that fails the build it does not.
#
# The budget is not a gag. Everything goes to --log, the path is printed, and
# a failure prints the tail of it, so nothing is lost by making the terminal
# quiet.
#
# EXIT STATUS is the command's own, except that a breached budget exits 65
# when the command itself succeeded — a run that passed but would not fit is
# still a failure, and one you can tell apart from a failing suite.
set -uo pipefail

LOG=""; MAX_LINES=0; MAX_BYTES=0; TAIL=40; LABEL=""
VERBOSE="${FLTH_VERBOSE:-0}"

while [ $# -gt 0 ]; do
    case "$1" in
        --log)       shift; LOG="${1:-}" ;;
        --max-lines) shift; MAX_LINES="${1:-0}" ;;
        --max-bytes) shift; MAX_BYTES="${1:-0}" ;;
        --tail)      shift; TAIL="${1:-40}" ;;
        --label)     shift; LABEL="${1:-}" ;;
        --verbose|-v) VERBOSE=1 ;;
        --)          shift; break ;;
        # The usage block, wherever it sits in the file: a line range
        # breaks the moment a comment above it grows, and this file's
        # provenance header is exactly such a comment.
        -h|--help)   awk '/^# output-budget.sh/ { on = 1 } on && !/^#/ { exit } on' \
                         "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *)           echo "output-budget.sh: unknown argument '$1'" >&2; exit 2 ;;
    esac
    shift
done

[ $# -gt 0 ] || { echo "output-budget.sh: no command (use -- COMMAND ...)" >&2; exit 2; }
[ -n "$LOG" ] || { echo "output-budget.sh: --log is required" >&2; exit 2; }
[ -n "$LABEL" ] || LABEL="$1"

mkdir -p "$(dirname "$LOG")" || exit 2

# The command's own status, not the pipeline's. `tee` succeeds while a suite
# fails, and a step written as `cmd | tee log` reports tee — which is how a red
# suite reads green, and why the test floors in these repositories exist.
if [ "$VERBOSE" = 1 ]; then
    # A pipeline, and the COMMAND's status taken from PIPESTATUS — not tee's,
    # which is the failure this script exists to prevent. Process substitution
    # was the other candidate and is wrong here: the log is still being written
    # when the next line measures it.
    "$@" 2>&1 | tee "$LOG"
    rc=${PIPESTATUS[0]}
else
    "$@" > "$LOG" 2>&1
    rc=$?
fi

lines=$(wc -l < "$LOG" | tr -d ' ')
bytes=$(wc -c < "$LOG" | tr -d ' ')

if [ "$rc" -ne 0 ]; then
    echo "$LABEL: FAILED (exit $rc)" >&2
    if [ "$VERBOSE" != 1 ]; then
        echo "--- last $TAIL lines of $LOG" >&2
        tail -n "$TAIL" "$LOG" >&2
        echo "--- $lines lines total in $LOG" >&2
    fi
    exit "$rc"
fi

over=""
[ "$MAX_LINES" -gt 0 ] && [ "$lines" -gt "$MAX_LINES" ] && over="$lines lines (budget $MAX_LINES)"
if [ "$MAX_BYTES" -gt 0 ] && [ "$bytes" -gt "$MAX_BYTES" ]; then
    [ -n "$over" ] && over="$over, "
    over="$over$bytes bytes (budget $MAX_BYTES)"
fi

if [ -n "$over" ]; then
    echo "$LABEL: passed, but printed $over" >&2
    echo "             Quiet the run, or raise the budget deliberately — see the" >&2
    echo "             output section of the consumer test contract." >&2
    echo "             Full output: $LOG" >&2
    exit 65
fi

printf '%s: ok (%s lines, %s bytes) — %s\n' "$LABEL" "$lines" "$bytes" "$LOG"
exit 0
