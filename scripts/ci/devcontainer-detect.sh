#!/usr/bin/env bash
# devcontainer.yml's detect step: does the image need rebuilding?
#
# THE `detect` PATTERN RATHER THAN A `paths:` FILTER, on a workflow that is not
# a required status check and so could legally carry one. #584 asked for it this
# way for a specific reason: a `paths:`-filtered workflow NEVER REPORTS on a PR
# it skips, and a required context that never reports blocks every PR forever
# (CLAUDE.md; the ci-and-supply-chain skill). So promoting this job to required
# later must be a branch-protection edit, not a workflow rewrite — which it only
# is if the skip is already expressed as a job that reports `skipped`.
#
# Do NOT add this job to the required set until it has reported green on `main`
# once: a required context for a job that does not yet exist there is the other
# half of the same trap.
#
# Inputs (env): EVENT, BASE_SHA, HEAD_SHA, PUSH_BEFORE, PUSH_AFTER.
# Outputs ($GITHUB_OUTPUT): devcontainer.
#
# The include regex lives in scripts/ci/devcontainer-paths.sh and is sourced,
# not reimplemented, for the reason rust-detect.sh gives: a test that re-derived
# the filter could pass while the workflow did something else.
#
# Run: EVENT=.. BASE_SHA=.. HEAD_SHA=.. bash scripts/ci/devcontainer-detect.sh
set -euo pipefail

cd "$(dirname "$0")/../.."

case "$EVENT" in
pull_request)
	# `base...head` = what the PR introduces.
	RANGE="${BASE_SHA:?}...${HEAD_SHA:?}"
	;;
push)
	# The merge that just landed, which is the run that PUSHES the image. An
	# absent, all-zero or unresolvable before-SHA (a branch's first push, a
	# force-push) FAILS SAFE to building: a wasted ~5-minute image build
	# beats a `main` whose published prebuild silently does not match the
	# tree, because the next cold `Reopen in Container` would pull it and
	# then be wrong in a way nothing reports.
	if [ -z "${PUSH_BEFORE:-}" ] ||
		[ "${PUSH_BEFORE:-}" = "0000000000000000000000000000000000000000" ] ||
		! git cat-file -e "${PUSH_BEFORE}^{commit}" 2>/dev/null; then
		echo "devcontainer=true" >> "$GITHUB_OUTPUT"
		exit 0
	fi
	RANGE="${PUSH_BEFORE}...${PUSH_AFTER:?}"
	;;
*)
	# workflow_dispatch: nothing to diff against, so build.
	echo "devcontainer=true" >> "$GITHUB_OUTPUT"
	exit 0
	;;
esac

changed="$(git diff --name-only "$RANGE")"
printf 'changed paths:\n%s\n' "$changed"

# shellcheck source=scripts/ci/devcontainer-paths.sh
. scripts/ci/devcontainer-paths.sh

# `grep -c`, never `grep -q`: `grep -q` closes the pipe on its first match,
# which SIGPIPEs `git diff` under `pipefail` and makes a FOUND match report as
# missing, intermittently. `|| true` absorbs grep's exit 1 on zero matches.
hits="$(printf '%s\n' "$changed" | grep -cE "$DEVCONTAINER_INCLUDE" || true)"
if [ "${hits:-0}" -gt 0 ]; then
	echo "devcontainer=true" >> "$GITHUB_OUTPUT"
else
	echo "devcontainer=false" >> "$GITHUB_OUTPUT"
fi
