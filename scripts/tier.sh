#!/usr/bin/env bash
# tier.sh LABEL LOG-NAME MAX-LINES MAX-BYTES -- COMMAND [ARG...]
#
# One test tier, run QUIETLY and under a budget. The whole run goes to
# tmp/logs/<LOG-NAME>.log; a pass prints one verdict line naming the log, a
# failure prints the tail of it, and a run that passed but printed more than
# its budget fails with status 65.
#
# ONE WRAPPER FOR BOTH CALLERS. chores.yml runs the tiers for a person at a
# terminal and .github/workflows/ci.yml runs them for the gate, and they run
# the SAME command through the SAME budget -- so a tier that has outgrown its
# budget says so here, before the push, rather than in a CI log nobody was
# going to read. tests/output_budget.rs checks that the two files agree on
# every tier's numbers; the duplication is deliberate (the workflow cannot
# read chores.yml without installing chore on three runner platforms) and it
# is checked rather than trusted.
#
# WHY THE BUDGET IS PART OF THE TASK. A passing run that prints three
# thousand lines hides the twenty that matter, and every reader pays for it:
# a person scrolling, a CI log viewer, and an agent working in the
# repository, which re-reads its whole transcript on each step and so pays
# for one verbose run many times over. Measured across this constellation:
# 4,661M cache-read tokens against 9.5M of output, and command output was the
# largest single contributor a repository controls.
#
# The budgets themselves are in chores.yml, next to the command each one
# bounds, and every one of them was MEASURED -- see the table there. Raise
# one deliberately when a tier grows, the way the executed-test floors are
# raised; a budget nobody can breach measures nothing.
#
# VERBOSE. `FLTH_VERBOSE=1`, or `--verbose`/`-v` in the chore invocation's
# CLI_ARGS (`chore test:debug -- --verbose`), streams the run as it happens
# as well as logging it. It does NOT lift the budget: the log is the same
# size either way, and a tier that has outgrown its budget should say so
# whether or not anybody was watching.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUDGET="$REPO/scripts/output-budget.sh"

if [ ! -f "$BUDGET" ]; then
    echo "tier.sh: scripts/output-budget.sh is missing." >&2
    exit 1
fi

[ $# -ge 5 ] || { echo "tier.sh: usage: tier.sh LABEL LOG MAX-LINES MAX-BYTES -- CMD..." >&2; exit 2; }
LABEL="$1"; LOG_NAME="$2"; MAX_LINES="$3"; MAX_BYTES="$4"; shift 4
[ "${1:-}" = "--" ] && shift
[ $# -gt 0 ] || { echo "tier.sh: no command" >&2; exit 2; }

# `chore test:debug -- --verbose` arrives as CLI_ARGS. output-budget.sh reads
# FLTH_VERBOSE itself, so mapping the flag onto it is all that is needed --
# and it means the environment variable and the flag cannot disagree.
case " ${CLI_ARGS:-} " in
    *" --verbose "*|*" -v "*) export FLTH_VERBOSE=1 ;;
esac

# `bash "$BUDGET"` rather than exec'ing it directly: a Windows checkout
# arrives without the executable bit, and this suite runs on windows-latest.
exec bash "$BUDGET" \
    --log "$REPO/tmp/logs/$LOG_NAME.log" \
    --max-lines "$MAX_LINES" \
    --max-bytes "$MAX_BYTES" \
    --label "$LABEL" \
    -- "$@"
