#!/usr/bin/env bash
# Self-test for agent-task.yml's three extracted scripts.
#
# WHY A TEST RATHER THAN A RUN. `agent-task.yml` is `workflow_dispatch` only
# and needs `CLAUDE_CODE_OAUTH_TOKEN` and `AGENT_APP_KEY`, neither of which
# this repo has — so dispatching it dies at the agent step having proved
# nothing about `agent-task-ids.sh`, and leaves an `agent-wip/<run-id>` branch
# on the remote for the trouble. All three scripts are pure logic over
# environment variables, so a test covers them completely and covers them on
# every PR instead of once.
#
# THE LOAD-BEARING HALF IS THE VALIDATION. `resume` is a `workflow_dispatch`
# input that reaches `refs/heads/agent-wip/$RESUME` and the agent's prompt, so
# it is attacker-shaped text until proven numeric. The rejection cases below
# are the point of this file; the happy paths are there so a regex that
# rejected everything could not pass.
#
# Run: bash scripts/ci/agent-task-test.sh
set -uo pipefail

cd "$(dirname "$0")/../.." || exit 2

pass=0
fail=0
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

ok() {
	pass=$((pass + 1))
	printf 'ok   %s\n' "$1"
}
no() {
	fail=$((fail + 1))
	printf 'FAIL %s\n       want: %s\n        got: %s\n' "$1" "$2" "$3"
}
check() {
	[ "$3" = "$2" ] && ok "$1" || no "$1" "$2" "$3"
}

# --- agent-task-ids.sh: accepted shapes -----------------------------------
ids() { # RESUME -> the $GITHUB_OUTPUT body, newline-joined, or "EXIT:<code>"
	: > "$tmp/o"
	if GITHUB_OUTPUT="$tmp/o" RESUME="$1" RUN_ID=12345 BASE=main \
		bash scripts/ci/agent-task-ids.sh > /dev/null 2>&1; then
		tr '\n' ' ' < "$tmp/o" | sed 's/ $//'
	else
		echo "EXIT:$?"
	fi
}

check 'ids: a fresh task keys off the run id' \
	'task_id=12345 wip=agent-wip/12345 base=main checkout_ref=refs/heads/main' \
	"$(ids '')"
check 'ids: a resume checks out the checkpoint branch' \
	'task_id=999 wip=agent-wip/999 base=main checkout_ref=refs/heads/agent-wip/999' \
	"$(ids 999)"

# --- agent-task-ids.sh: every rejection ----------------------------------
# Each of these would otherwise be interpolated into a git ref. The command
# substitution and traversal cases are the ones that matter; the rest prove
# the regex is anchored at both ends rather than merely "contains a digit".
for bad in \
	'main;rm -rf /' \
	'$(whoami)' \
	'`id`' \
	'../../refs/heads/main' \
	'1/../../evil' \
	'12345 ' \
	' 12345' \
	'12a' \
	'abc' \
	'-1' \
	'1.0' \
	'99999999999999999999999'; do
	got="$(ids "$bad")"
	case $got in
	EXIT:1) ok "ids: rejects '$bad'" ;;
	*) no "ids: rejects '$bad'" 'EXIT:1' "$got" ;;
	esac
done

# A 20-digit value is the documented upper bound and must still be accepted,
# or the bound is really "reject everything long".
check 'ids: accepts the 20-digit bound' \
	'task_id=12345678901234567890 wip=agent-wip/12345678901234567890 base=main checkout_ref=refs/heads/agent-wip/12345678901234567890' \
	"$(ids 12345678901234567890)"

# BASE is repo configuration, not user input, but it reaches a ref too.
: > "$tmp/o"
GITHUB_OUTPUT="$tmp/o" RESUME= RUN_ID=7 BASE=release/1.x \
	bash scripts/ci/agent-task-ids.sh > /dev/null 2>&1
check 'ids: a slashed default branch survives intact' \
	'checkout_ref=refs/heads/release/1.x' \
	"$(grep '^checkout_ref=' "$tmp/o")"

# --- agent-task-summary.sh ------------------------------------------------
# Runs on `always()`, so NOTHING in it may fail — least of all `jq` on
# agent-generated text. Absent, empty and malformed are three different
# answers and all three must render.
summary() { # RESULT -> the "Reported status" cell
	: > "$tmp/s"
	GITHUB_STEP_SUMMARY="$tmp/s" TASK_ID=42 WIP=agent-wip/42 OUTCOME=failure RESULT="$1" \
		bash scripts/ci/agent-task-summary.sh > /dev/null 2>&1 ||
		{ echo "EXIT:$?"; return; }
	sed -n 's/^| Reported status | `\(.*\)` |$/\1/p' "$tmp/s"
}

check 'summary: a reported status is shown' 'blocked' "$(summary '{"status":"blocked"}')"
check 'summary: absent output reads none' 'none' "$(summary '')"
check 'summary: JSON without a status reads none' 'none' "$(summary '{"other":1}')"
check 'summary: malformed JSON reads unparseable' 'unparseable' "$(summary 'not json')"
check 'summary: a bare string reads unparseable' 'unparseable' "$(summary '"hello')"

# Both outcomes appear, and that is deliberate: the step outcome is green
# whenever the agent process exited 0 — including when it exited 0 having
# concluded it could not do the task — so collapsing them would hide the
# interesting run.
: > "$tmp/s"
GITHUB_STEP_SUMMARY="$tmp/s" TASK_ID=42 WIP=agent-wip/42 OUTCOME=success RESULT='{"status":"failed"}' \
	bash scripts/ci/agent-task-summary.sh > /dev/null 2>&1
if grep -q '| Agent step | `success` |' "$tmp/s" &&
	grep -q '| Reported status | `failed` |' "$tmp/s"; then
	ok 'summary: step outcome and reported status are both shown'
else
	no 'summary: step outcome and reported status are both shown' \
		'success + failed' "$(tr '\n' ' ' < "$tmp/s")"
fi

# The resume hint has to carry the task id, or a failed run cannot be resumed.
grep -q '| Resume with | `agent-resume 42` |' "$tmp/s" &&
	ok 'summary: names the resume command' ||
	no 'summary: names the resume command' 'agent-resume 42' 'absent'

# --- agent-task-checkpoint.sh --------------------------------------------
# Not run here: it pushes to `origin`. What IS asserted is the guard, because
# an unset WIP would push to `refs/heads/` and the `:?` is the only thing
# between that and the remote.
if WIP= RUN_ID=1 bash scripts/ci/agent-task-checkpoint.sh > /dev/null 2>&1; then
	no 'checkpoint: refuses an empty WIP' 'nonzero exit' 'exit 0'
else
	ok 'checkpoint: refuses an empty WIP'
fi
if WIP=agent-wip/1 RUN_ID= bash scripts/ci/agent-task-checkpoint.sh > /dev/null 2>&1; then
	no 'checkpoint: refuses an empty RUN_ID' 'nonzero exit' 'exit 0'
else
	ok 'checkpoint: refuses an empty RUN_ID'
fi

echo "---"
echo "pass=$pass fail=$fail"
if [ "$pass" -eq 0 ]; then
	echo "FAIL asserted nothing"
	exit 2
fi
[ "$fail" -eq 0 ] || exit 1
echo "agent-task: ok"
