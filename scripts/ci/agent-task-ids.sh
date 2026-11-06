#!/usr/bin/env bash
# agent-task.yml's identity step: settle the task id, its checkpoint branch
# and the ref to check out.
#
# EVERY VALUE HERE ENDS UP IN A GIT REF AND IN THE PROMPT, which is why
# `resume` is validated against `^[0-9]{1,20}$` and only the validated copy is
# used downstream. It is a `workflow_dispatch` input, so it is attacker-shaped
# text until proven otherwise: unvalidated, `refs/heads/agent-wip/$RESUME`
# would accept a ref path of the caller's choosing. The 20-digit bound is
# there because `^[0-9]+$` alone would accept a megabyte of digits.
#
# A resume checks out the checkpoint branch; a fresh task checks out the
# default branch and the next step creates the branch from it.
#
# Inputs (env): RESUME (may be empty), RUN_ID, BASE.
# Outputs ($GITHUB_OUTPUT): task_id, wip, base, checkout_ref.
# Run: RESUME= RUN_ID=.. BASE=main bash scripts/ci/agent-task-ids.sh
set -euo pipefail

: "${RUN_ID:?RUN_ID must be set}"
: "${BASE:?BASE must name the default branch}"

if [ -n "${RESUME:-}" ]; then
	if ! printf '%s' "$RESUME" | grep -Eq '^[0-9]{1,20}$'; then
		echo "::error::resume must be the numeric run id that started the task, got '$RESUME'"
		exit 1
	fi
	task_id="$RESUME"
	checkout_ref="refs/heads/agent-wip/$RESUME"
else
	task_id="$RUN_ID"
	checkout_ref="refs/heads/$BASE"
fi

{
	echo "task_id=$task_id"
	echo "wip=agent-wip/$task_id"
	echo "base=$BASE"
	echo "checkout_ref=$checkout_ref"
} >> "$GITHUB_OUTPUT"
