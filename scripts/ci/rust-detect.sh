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
# in the queue would otherwise pay the full backend gate there, which is what
# it skips today on the PR. A push skips only for a proven marketing-only
# change; other main pushes, schedules and manual runs still run everything.
#
# Inputs (env): EVENT, BASE_SHA, HEAD_SHA, PUSH_BEFORE, PUSH_AFTER, FULL_CI.
# Outputs ($GITHUB_OUTPUT): code,
# erd, drift, empty.
# Run: EVENT=.. BASE_SHA=.. HEAD_SHA=.. bash scripts/ci/rust-detect.sh
set -euo pipefail

cd "$(dirname "$0")/../.."

# shellcheck source=scripts/ci/detect-paths.sh
. scripts/ci/detect-paths.sh

# Unlike PRs, main keeps its full verification for every non-marketing push.
# An absent/unresolvable before-SHA cannot prove isolation, so falls through.
if [ "${EVENT:-}" = push ] && [ "${FULL_CI:-false}" != true ] \
   && [ -n "${PUSH_BEFORE:-}" ] && [ -n "${PUSH_AFTER:-}" ] \
   && git cat-file -e "$PUSH_BEFORE^{commit}" 2>/dev/null; then
	changed="$(git diff --no-renames --name-only "$PUSH_BEFORE..$PUSH_AFTER")"
	outside="$(printf '%s\n' "$changed" | grep -cvE "$MARKETING_INCLUDE" || true)"
	if [ -n "$changed" ] && [ "$outside" -eq 0 ]; then
		printf '%s\n' "$changed" | detect_classify >> "$GITHUB_OUTPUT"
		exit 0
	fi
fi

if [ "${FULL_CI:-false}" = true ] || { [ "${EVENT:-}" != "pull_request" ] && [ "${EVENT:-}" != "merge_group" ]; }; then
	{
		echo "code=true"
		echo "erd=true"
		echo "drift=true"
		# `empty` is about a PULL REQUEST introducing no files. A push, a
		# schedule or a dispatch has no base...head to be empty of, and the
		# `empty pull request` job is gated on the event as well, so false is
		# the only answer that means anything here.
		echo "empty=false"
	} >> "$GITHUB_OUTPUT"
	exit 0
fi

# `base...head` = what the PR (or the queue's candidate) introduces.
changed="$(git diff --no-renames --name-only "${BASE_SHA:?}...${HEAD_SHA:?}")"
printf 'changed paths:\n%s\n' "$changed"

verdict="$(printf '%s\n' "$changed" | detect_classify)"
printf 'verdict:\n%s\n' "$verdict"
printf '%s\n' "$verdict" >> "$GITHUB_OUTPUT"
