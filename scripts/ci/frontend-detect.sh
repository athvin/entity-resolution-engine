#!/usr/bin/env bash
# frontend.yml's detect step: does the frontend gate need to run?
#
# Inputs (env): EVENT, BASE_SHA, HEAD_SHA, PUSH_BEFORE, PUSH_AFTER, MQ_BASE,
# MQ_HEAD, FULL_CI. Outputs ($GITHUB_OUTPUT): frontend, marketing_only.
set -euo pipefail
cd "$(dirname "$0")/../.."

if [ "${FULL_CI:-false}" = true ]; then
  printf 'frontend=true\nmarketing_only=false\n' >> "$GITHUB_OUTPUT"
  exit 0
fi
#
# The gate's input set lives in scripts/ci/frontend-paths.sh, which the
# workflow's content-addressed `key` step sources too — one regex, so the
# filter and the key cannot disagree.
# Which two commits bound "what changed" depends on the event.
case "${EVENT:-}" in
  pull_request)
    # `base...head` = what the PR introduces.
    RANGE="$BASE_SHA...$HEAD_SHA"
    ;;
  merge_group)
    # The queue's candidate against the main it will land on — the
    # same question a PR asks, about the exact tree that will merge.
    RANGE="$MQ_BASE...$MQ_HEAD"
    ;;
  push)
    # The merge that just landed. A squash-merged PR already ran this
    # same gate against an all-but-identical tree, so re-running it
    # for a backend-only merge buys nothing and costs the Playwright
    # container every time. `before` is all-zeroes on a branch's first
    # push and unresolvable after a force-push, so fall back to
    # running rather than skipping — a wasted run beats a silent one.
    if [ -z "${SUCCESS_BASE:-}" ] || [ "$SUCCESS_BASE" = "0000000000000000000000000000000000000000" ] \
       || ! git merge-base --is-ancestor "$SUCCESS_BASE" "$PUSH_AFTER" 2>/dev/null; then
      printf 'frontend=true\nmarketing_only=false\n' >> "$GITHUB_OUTPUT"
      exit 0
    fi
    RANGE="$SUCCESS_BASE..$PUSH_AFTER"
    ;;
  *)
    # schedule / workflow_dispatch: nothing to diff against.
    printf 'frontend=true\nmarketing_only=false\n' >> "$GITHUB_OUTPUT"
    exit 0
    ;;
esac
# The shared classifier selects no frontend work, static site coverage, or
# the full app gate. Include both ends of renames so moving shared code into
# marketing still triggers the full app gate.
changed="$(git diff --no-renames --name-only "$RANGE")"
. scripts/ci/frontend-paths.sh
printf '%s\n' "$changed" | frontend_classify >> "$GITHUB_OUTPUT"
