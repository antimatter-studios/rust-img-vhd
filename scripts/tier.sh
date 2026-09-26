#!/usr/bin/env bash
# tier.sh LABEL LOG-NAME MAX-LINES MAX-BYTES -- COMMAND [ARG...]
#
# One test tier, run QUIETLY and under a budget. The whole run goes to
# tmp/logs/<LOG-NAME>.log; a pass prints one verdict line naming the log, a
# failure prints one line naming the status and that log, and a run that
# passed but printed more than its budget fails with status 65.
#
# ONE WRAPPER FOR BOTH CALLERS. chores.yml runs the tiers for a person at a
# terminal and .github/workflows/ci.yml runs them for the gate, and they run
# the SAME command through the SAME budget -- so a tier that has outgrown its
# budget says so here, before the push, rather than in a CI log nobody was
# going to read. tests/ci_profile.rs checks that the two files agree on
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
# VERBOSE. `OUTPUT_BUDGET_VERBOSE=1`, or `--verbose`/`-v` in the chore
# invocation's CLI_ARGS (`chore test:debug -- --verbose`), streams the run as
# it happens as well as logging it. It does NOT lift the budget: the log is
# the same size either way, and a tier that has outgrown its budget should
# say so whether or not anybody was watching.
#
# THE VARIABLE WAS `FLTH_VERBOSE` until the wrapper moved to rust-fs-core,
# and so was `FLTH_FAIL_TAIL`. The canonical script reads OUTPUT_BUDGET_*
# and does not read the old names -- a rename that FAILS SILENTLY, because
# nothing errors and the run simply stays quiet. Anybody arriving here
# looking for `FLTH_VERBOSE` wants `OUTPUT_BUDGET_VERBOSE`; core's script
# says so on stderr if the old one is set.
#
# A FAILING TIER NO LONGER PRINTS A TAIL BY DEFAULT. The vendored copy this
# replaced printed 40 lines; core's prints one line naming the log and its
# size, and `--tail N` or OUTPUT_BUDGET_FAIL_TAIL=N asks for more. CI uploads
# tmp/logs/, so the whole run is still there to be fetched once, by the
# reader who wants it, rather than pasted into every transcript that follows.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# THE WRAPPER BELONGS TO rust-fs-core AND IS NOT COMMITTED HERE.
#
# scripts/output-budget.sh used to be a vendored copy, taken from
# fs-linux-test-harness at v0.1.0. A committed copy is a copy that drifts:
# measured on 2026-09-22 the family had several of them, reached four
# different ways, each repository internally consistent and nothing comparing
# them. The canonical file now lives in rust-fs-core and is resolved HERE, at
# run time, copied into tmp/ for the length of the run and deleted after it.
# See antimatter-studios/rust-fs-core#153.
#
# THE CONTRACT IS `--version`, NOT A DIGEST. rust-fs-ntfs pins a SHA-256 of
# the file. Seven repositories each pinning a digest is exactly the lockstep
# this migration removes -- every fix in core would need a commit in all of
# them before any could take it. `rust-fs-core-output-budget 1` is the API
# version, it is what this script requires, and a copy that answers something
# else is refused outright rather than used or worked around.
#
# WHERE IT LOOKS, IN ORDER:
#
#   1. $FS_CORE_ROOT, when set. An override wins, or it is not an override:
#      tests/output_budget.rs points it at scratch directories to prove the
#      refusals, and .github/workflows/ci.yml points it at the core checkout
#      that carries the wrapper. A relative value is resolved against this
#      repository, which is what keeps it writable on every runner (below).
#   2. The ../rust-fs-core sibling, when that checkout has the file. It is
#      the layout Cargo.toml's path dependency already requires, so a
#      coordinated local change to the wrapper is exercised here on the next
#      run rather than after a release.
#   3. Whatever `cargo metadata` says the am-fs-core package root is. Today
#      that is the sibling again; it becomes the answer that matters on the
#      day this crate depends on the published crate instead (#96, #99).
#
# THE SIBLING IS TRIED BEFORE CARGO, AND THAT IS LOAD-BEARING: this suite
# runs on windows-latest under Git Bash, where `cargo metadata` reports
# `C:\...` -- not a path Git Bash can test or copy. The sibling path is POSIX
# on every runner because this script builds it from its own location. For
# the same reason $FS_CORE_ROOT is accepted as a path relative to $REPO: a
# workflow can then say `../rust-fs-core-budget` and mean it on all three.
#
# NOTHING IS A FALLBACK. Each rule either produces a file that answers the
# API string or fails the run; a wrong copy is not a reason to try the next
# source, because "some other output-budget.sh was found" is how a family
# ends up with several again.
API_VERSION="rust-fs-core-output-budget 1"
MIN_CORE="v0.2.13"

expectations() {
    echo "         Expected one of:" >&2
    echo "           \$FS_CORE_ROOT/scripts/output-budget.sh   (absolute, or relative to this repository)" >&2
    echo "           $REPO/../rust-fs-core/scripts/output-budget.sh" >&2
    echo "           <the am-fs-core package root>/scripts/output-budget.sh" >&2
    echo "         answering '$API_VERSION' to --version." >&2
    echo "         The wrapper ships in rust-fs-core from v0.2.11 and has been quiet" >&2
    echo "         on failure since $MIN_CORE, which is the minimum this repository" >&2
    echo "         asks for: check that ref out beside this repository, or name a" >&2
    echo "         checkout of it with FS_CORE_ROOT." >&2
}

WRAPPER=""
FOUND_VIA=""
if [ -n "${FS_CORE_ROOT:-}" ]; then
    root="$FS_CORE_ROOT"
    case "$root" in /*) ;; *) root="$REPO/$root" ;; esac
    WRAPPER="$root/scripts/output-budget.sh"
    FOUND_VIA="FS_CORE_ROOT ($FS_CORE_ROOT)"
    if [ ! -f "$WRAPPER" ]; then
        echo "tier.sh: FS_CORE_ROOT is set to '$FS_CORE_ROOT' and there is no" >&2
        echo "         scripts/output-budget.sh under it." >&2
        expectations
        exit 1
    fi
elif [ -f "$REPO/../rust-fs-core/scripts/output-budget.sh" ]; then
    WRAPPER="$REPO/../rust-fs-core/scripts/output-budget.sh"
    FOUND_VIA="the ../rust-fs-core sibling"
else
    # `--locked`, so this reports the resolution the lockfile records rather
    # than quietly resolving a new one. python3 reads the JSON: the package
    # objects carry nested arrays, so there is no line-oriented spelling of
    # this that is not a guess.
    CORE_DIR="$(cargo metadata --format-version 1 --locked \
        --manifest-path "$REPO/Cargo.toml" 2>/dev/null | python3 -c '
import json, sys
try:
    packages = json.load(sys.stdin)["packages"]
except Exception:
    sys.exit(0)
print(next((p["manifest_path"].rsplit("/", 1)[0]
            for p in packages if p["name"] == "am-fs-core"), ""))
' 2>/dev/null || true)"
    if [ -n "$CORE_DIR" ] && [ -f "$CORE_DIR/scripts/output-budget.sh" ]; then
        WRAPPER="$CORE_DIR/scripts/output-budget.sh"
        FOUND_VIA="cargo metadata ($CORE_DIR)"
    fi
fi

if [ -z "$WRAPPER" ]; then
    echo "tier.sh: no rust-fs-core output-budget wrapper could be resolved, and this" >&2
    echo "         repository deliberately keeps no copy of its own." >&2
    expectations
    exit 1
fi

VERSION="$(bash "$WRAPPER" --version 2>/dev/null || true)"
if [ "$VERSION" != "$API_VERSION" ]; then
    echo "tier.sh: the output-budget wrapper found via $FOUND_VIA answers" >&2
    echo "         '${VERSION:-nothing}' to --version, not '$API_VERSION'." >&2
    echo "         That is a wrong or too-old copy, so this run stops here rather" >&2
    echo "         than looking elsewhere: a second source is how a family ends up" >&2
    echo "         with several wrappers again." >&2
    expectations
    exit 1
fi

[ $# -ge 5 ] || { echo "tier.sh: usage: tier.sh LABEL LOG MAX-LINES MAX-BYTES -- CMD..." >&2; exit 2; }
LABEL="$1"; LOG_NAME="$2"; MAX_LINES="$3"; MAX_BYTES="$4"; shift 4
[ "${1:-}" = "--" ] && shift
[ $# -gt 0 ] || { echo "tier.sh: no command" >&2; exit 2; }

# THE RUN GETS ITS OWN COPY, AND ONLY FOR THE LENGTH OF THE RUN. The resolved
# wrapper may be a checkout somebody is editing, or a read-only registry
# directory; copying it means a tier that has started cannot be changed
# underneath itself. tmp/ is gitignored and is where the tier logs already
# live. The trap removes it on every exit, including a failing tier.
BUDGET="$REPO/tmp/output-budget.$$.sh"
mkdir -p "$REPO/tmp"
cp "$WRAPPER" "$BUDGET"
trap 'rm -f "$BUDGET"' EXIT

# `chore test:debug -- --verbose` arrives as CLI_ARGS. output-budget.sh reads
# OUTPUT_BUDGET_VERBOSE itself, so mapping the flag onto it is all that is
# needed -- and it means the environment variable and the flag cannot
# disagree.
case " ${CLI_ARGS:-} " in
    *" --verbose "*|*" -v "*) export OUTPUT_BUDGET_VERBOSE=1 ;;
esac

# `bash "$BUDGET"` rather than running it directly: a Windows checkout
# arrives without the executable bit, and this suite runs on windows-latest.
# Not `exec`, because the trap above has to outlive the command.
status=0
bash "$BUDGET" \
    --log "$REPO/tmp/logs/$LOG_NAME.log" \
    --max-lines "$MAX_LINES" \
    --max-bytes "$MAX_BYTES" \
    --label "$LABEL" \
    -- "$@" || status=$?
exit "$status"
