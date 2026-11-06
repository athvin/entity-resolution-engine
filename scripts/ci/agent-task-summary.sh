#!/usr/bin/env bash
# agent-task.yml's job summary: the agent step's outcome, what the agent
# itself reported, and how to resume.
#
# THE TWO OUTCOMES ARE DIFFERENT CLAIMS and the table shows both on purpose.
# `OUTCOME` is the step's own result, which is green whenever the agent
# process exited 0 — including when it exited 0 having concluded it could not
# do the task. `RESULT` is the agent's structured verdict. A run where those
# two disagree is the interesting one, and collapsing them to a single
# "status" would hide it.
#
# `jq` is given `// "none"` and a `|| echo unparseable` fallback because
# structured output is agent-generated text: absent, empty and malformed are
# three different things and none of them may fail a summary step that runs on
# `always()`.
#
# Inputs (env): TASK_ID, WIP, OUTCOME, RESULT (may be empty).
# Run: TASK_ID=.. WIP=.. OUTCOME=.. RESULT=.. bash scripts/ci/agent-task-summary.sh
set -euo pipefail

: "${TASK_ID:?TASK_ID must be set}"

status=none
if [ -n "${RESULT:-}" ]; then
	status=$(printf '%s' "$RESULT" | jq -r '.status // "none"' 2>/dev/null || echo unparseable)
fi

{
	echo "### Agent task \`$TASK_ID\`"
	echo
	echo "| | |"
	echo "|---|---|"
	echo "| Agent step | \`${OUTCOME:-}\` |"
	echo "| Reported status | \`$status\` |"
	echo "| Checkpoint branch | \`${WIP:-}\` |"
	echo "| Resume with | \`agent-resume $TASK_ID\` |"
} >> "$GITHUB_STEP_SUMMARY"
