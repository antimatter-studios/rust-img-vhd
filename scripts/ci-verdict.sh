#!/usr/bin/env bash
# ci-verdict.sh JOB...  ci-ok's verdict over every job it needs
#
# Reads "<job> <result>" lines on stdin, one per entry of `needs`, and fails
# unless every job named on the command line reported and concluded
# success. A skip is accepted in exactly two cases:
#
#   - CODE=false: the change was documentation alone, and every job but
#     `changes` may skip (rust-fs-core's scripts/code-changed.sh decides);
#   - PROFILE=quick: the job is named in FULL_ONLY, the legs the quick
#     pull-request profile does not run (scripts/ci-plan.sh decides).
#
# A missing or invalid PROFILE or CODE fails: it means the planning job did
# not run to completion, and nothing it gates can be judged. A job named on
# the command line with no line on stdin fails as missing. Failure and
# cancellation always fail.
set -euo pipefail

profile="${PROFILE-}"
code="${CODE-}"
full_only=" ${FULL_ONLY-} "

case "$profile" in
    quick | full) ;;
    *)
        echo "ci-verdict.sh: the plan is missing or invalid (profile='$profile')" >&2
        exit 1
        ;;
esac
case "$code" in
    true | false) ;;
    *)
        echo "ci-verdict.sh: the change classification is missing or invalid (code='$code')" >&2
        exit 1
        ;;
esac

seen=" "
bad=""
while read -r job result; do
    [ -n "$job" ] || continue
    seen="$seen$job "
    case "$result" in
        success) continue ;;
        skipped)
            if [ "$job" != changes ] && [ "$code" = false ]; then
                continue
            fi
            if [ "$profile" = quick ] && [[ "$full_only" == *" $job "* ]]; then
                continue
            fi
            ;;
    esac
    bad="$bad${bad:+, }$job: ${result:-no result}"
done

for job in "$@"; do
    if [[ "$seen" != *" $job "* ]]; then
        bad="$bad${bad:+, }$job: missing"
    fi
done

if [ -n "$bad" ]; then
    echo "ci-verdict.sh: not 'success': $bad -- failure, cancellation, a missing job and an unexpected skip all mean the gate did not run to completion" >&2
    exit 1
fi
echo "all gating jobs succeeded (profile $profile, code changed: $code)"
