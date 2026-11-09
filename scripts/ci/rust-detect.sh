#!/usr/bin/env bash
# rust.yml's detect step: does the backend gate need to run?
#
# The last of the four detect steps to become a script — frontend.yml,
# infra.yml and images.yml already had one, and this one stayed inline because
# its classification had already been lifted into scripts/ci/detect-paths.sh.
# What was left inline was the EVENT BRANCHING, which is the half the other
# three scripts also own, so leaving it in YAML meant the one detect you could
# not unit-test was the one guarding the most expensive gate.
#
# The classification itself still lives in detect-paths.sh and is sourced, not
# reimplemented: scripts/ci/detect-paths-test.sh exercises `detect_classify`
# directly, and a test that re-derived the branching could pass while the
# workflow did something else. The `key` step sources that same file through
# scripts/ci/pass-key.sh, which is what stops the filter and the key drifting.
#
# `merge_group` filters exactly like a PR: an infra-only or skills-only change
# in the queue would otherwise pay the full backend gate there. Main pushes
# select all changes since a successful ancestor; schedules, manual runs and
# missing history select everything.
#
# Inputs (env): EVENT, BASE_SHA, HEAD_SHA, PUSH_BEFORE, PUSH_AFTER, FULL_CI.
# Outputs ($GITHUB_OUTPUT): code,
# erd, drift, empty.
# Run: EVENT=.. BASE_SHA=.. HEAD_SHA=.. bash scripts/ci/rust-detect.sh
set -euo pipefail

cd "$(dirname "$0")/../.."

# shellcheck source=scripts/ci/detect-paths.sh
. scripts/ci/detect-paths.sh

# A push must include changes missed by failed/superseded runs. The workflow
# supplies the last successful ancestor, never the immediately previous push.
full=false
case "${EVENT:-}" in
pull_request|merge_group) range="${BASE_SHA:?}...${HEAD_SHA:?}" ;;
push)
    if [ -n "${SUCCESS_BASE:-}" ] && git merge-base --is-ancestor "$SUCCESS_BASE" "${PUSH_AFTER:?}"; then
        range="$SUCCESS_BASE..$PUSH_AFTER"
    else
        full=true
    fi
    ;;
*) full=true ;;
esac
[ "${FULL_CI:-false}" != true ] || full=true
if [ "$full" = true ]; then
    verdict=$(printf 'code=true\nerd=true\ndrift=true\nempty=false')
    changed=''
else
    changed="$(git diff --no-renames --name-only "$range")"
    verdict="$(printf '%s\n' "$changed" | detect_classify)"
fi
printf '%s\n' "$verdict" >> "$GITHUB_OUTPUT"
# Existing fixture callers exercise classification without Cargo installed.
if [ "${SELECT_PACKAGES:-false}" = true ]; then
    relevant="$(printf '%s\n' "$changed" | GATE_EXEMPT="$DETECT_EXEMPT" GATE_COUPLED="$DETECT_CONTRACT" \
        awk 'length && ($0 !~ ENVIRON["GATE_EXEMPT"] || $0 ~ ENVIRON["GATE_COUPLED"])')"
    printf '%s\n' "$relevant" | SELECT_FULL="$full" python3 scripts/ci/rust-select.py >> "$GITHUB_OUTPUT"
fi
