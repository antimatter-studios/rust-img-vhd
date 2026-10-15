#!/usr/bin/env bash
# test-ci-profile.sh  the CI plan and ci-ok's verdict, as truth tables
#
# scripts/ci-plan.sh decides which legs an event runs; scripts/ci-verdict.sh
# decides whether ci-ok accepts what ran. A mistake in either is silent: a
# merge-group run planned as quick lands code no full leg tested, and a
# verdict that accepts a skip it should not reports green over a leg that
# never ran. Each row here is one event or one set of results, and the
# expected answer is written out rather than derived.
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PLAN="$REPO/scripts/ci-plan.sh"
VERDICT="$REPO/scripts/ci-verdict.sh"
fails=0
ok() { echo "ok    $1"; }
fail() { echo "FAIL  $1" >&2; fails=$((fails + 1)); }

FULL='["ubuntu-latest","macos-latest","windows-latest"]'
QUICK='["ubuntu-latest"]'

plan_is() { # event profile matrix
    local out want
    out="$(bash "$PLAN" "$1")" || { fail "ci-plan.sh '$1' exited non-zero"; return; }
    want="$(printf 'profile=%s\nmatrix=%s' "$2" "$3")"
    if [ "$out" = "$want" ]; then ok "event '$1' plans $2"; else fail "event '$1' planned '$out', expected '$want'"; fi
}

plan_is pull_request quick "$QUICK"
plan_is merge_group full "$FULL"
plan_is push full "$FULL"
plan_is workflow_dispatch full "$FULL"
plan_is workflow_call full "$FULL"
plan_is schedule full "$FULL"
plan_is pull_request_target full "$FULL"
plan_is "" full "$FULL"
plan_is some_future_event full "$FULL"

JOBS="changes test test-release qemu-validation fmt cli semver standalone"
FULL_ONLY_JOBS="test-release qemu-validation cli standalone"

verdict_is() { # want(pass|fail) label profile code results
    local want="$1" label="$2" status
    PROFILE="$3" CODE="$4" FULL_ONLY="$FULL_ONLY_JOBS" bash "$VERDICT" $JOBS <<<"$5" >/dev/null 2>&1
    status=$?
    if [ "$want" = pass ] && [ "$status" -eq 0 ]; then ok "$label: accepted"
    elif [ "$want" = fail ] && [ "$status" -ne 0 ]; then ok "$label: refused"
    else fail "$label: expected $want, exit status $status"; fi
}

results() { # job=result overrides on top of every job succeeding
    local job line out=""
    for job in $JOBS; do
        line="$job success"
        for o in "$@"; do
            [ "${o%%=*}" = "$job" ] && line="$job ${o#*=}"
            [ "$o" = "-$job" ] && line=""
        done
        [ -n "$line" ] && out="$out$line"$'\n'
    done
    printf '%s' "$out"
}

QUICK_SKIPS=(test-release=skipped qemu-validation=skipped cli=skipped standalone=skipped)

verdict_is pass "full profile, every job succeeded" full true "$(results)"
verdict_is pass "quick profile, every job succeeded" quick true "$(results)"
verdict_is pass "quick profile, full-only legs skipped" quick true "$(results "${QUICK_SKIPS[@]}")"
verdict_is fail "full profile, full-only legs skipped" full true "$(results "${QUICK_SKIPS[@]}")"
verdict_is fail "full profile, qemu-validation skipped" full true "$(results qemu-validation=skipped)"
verdict_is fail "full profile, cli skipped" full true "$(results cli=skipped)"
verdict_is fail "quick profile, test skipped" quick true "$(results test=skipped)"
verdict_is fail "quick profile, fmt skipped" quick true "$(results fmt=skipped)"
verdict_is fail "quick profile, semver skipped" quick true "$(results semver=skipped)"
verdict_is fail "quick profile, a full-only leg failed" quick true "$(results test-release=failure)"
verdict_is fail "quick profile, a full-only leg cancelled" quick true "$(results qemu-validation=cancelled)"
verdict_is fail "full profile, test failed" full true "$(results test=failure)"
verdict_is fail "full profile, semver cancelled" full true "$(results semver=cancelled)"
verdict_is pass "documentation only, code legs skipped" full false "$(results test=skipped test-release=skipped qemu-validation=skipped)"
verdict_is fail "documentation only, changes skipped" full false "$(results changes=skipped)"
verdict_is fail "documentation only, fmt failed" quick false "$(results fmt=failure)"
verdict_is fail "a needed job is missing" full true "$(results -standalone)"
verdict_is fail "a needed job has no result" full true "$(results cli=)"
verdict_is fail "no plan" "" true "$(results)"
verdict_is fail "an invalid plan" fast true "$(results)"
verdict_is fail "no change classification" full "" "$(results)"
verdict_is fail "an invalid change classification" full maybe "$(results)"
verdict_is fail "no results at all" full true ""

if [ "$fails" -gt 0 ]; then
    echo "test-ci-profile.sh: $fails check(s) failed" >&2
    exit 1
fi
echo "test-ci-profile.sh: all checks passed"
