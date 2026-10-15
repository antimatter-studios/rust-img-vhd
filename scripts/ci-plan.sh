#!/usr/bin/env bash
# ci-plan.sh EVENT  which CI profile a GitHub event runs, as $GITHUB_OUTPUT lines
#
# A pull request runs the quick profile: unit and integration tests on one
# runner, clippy, fmt, the script guards and semver. Every pull request in a
# stack runs it, so each layer is checked cheaply before it queues.
#
# Every other event runs the full profile: the merge queue's run, a push to
# main, a manual dispatch, a tag, a workflow_call, and any event this script
# does not name. The full profile is the three-OS matrix, the
# release suite, the qemu-img validation, the installed CLI and the
# standalone build. An unknown or empty event runs full, so a new trigger
# can only ever cost time, never skip a leg.
set -euo pipefail

event="${1-}"
case "$event" in
    pull_request)
        profile=quick
        matrix='["ubuntu-latest"]'
        ;;
    *)
        profile=full
        matrix='["ubuntu-latest","macos-latest","windows-latest"]'
        ;;
esac
printf 'profile=%s\nmatrix=%s\n' "$profile" "$matrix"
