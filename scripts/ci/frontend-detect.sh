#!/usr/bin/env bash
# frontend.yml's detect step: does the frontend gate need to run?
#
# Inputs (env): EVENT, BASE_SHA, HEAD_SHA, PUSH_BEFORE, PUSH_AFTER, MQ_BASE,
# MQ_HEAD. Outputs ($GITHUB_OUTPUT): frontend.
#
# The gate's input set lives in scripts/ci/frontend-paths.sh, which the
# workflow's content-addressed `key` step sources too — one regex, so the
# filter and the key cannot disagree.
# Which two commits bound "what changed" depends on the event.
case "$EVENT" in
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
    if [ -z "$PUSH_BEFORE" ] || [ "$PUSH_BEFORE" = "0000000000000000000000000000000000000000" ] \
       || ! git cat-file -e "$PUSH_BEFORE^{commit}" 2>/dev/null; then
      echo "frontend=true" >> "$GITHUB_OUTPUT"
      exit 0
    fi
    RANGE="$PUSH_BEFORE...$PUSH_AFTER"
    ;;
  *)
    # schedule / workflow_dispatch: nothing to diff against.
    echo "frontend=true" >> "$GITHUB_OUTPUT"
    exit 0
    ;;
esac
# Count changed paths under `frontend/` (or this workflow); any match
# means the gate must run. `grep -c` reads all input (unlike `grep -q`,
# which closes the pipe early and can SIGPIPE `git diff` under
# `pipefail` → a flaky false skip); `|| true` absorbs grep's
# exit-1-on-zero-matches under `set -e`.
changed="$(git diff --name-only "$RANGE")"
. scripts/ci/frontend-paths.sh
hits="$(printf '%s\n' "$changed" | grep -cE "$FRONTEND_INCLUDE" || true)"
if [ "${hits:-0}" -gt 0 ]; then
  echo "frontend=true" >> "$GITHUB_OUTPUT"
else
  echo "frontend=false" >> "$GITHUB_OUTPUT"
fi
