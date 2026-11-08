#!/usr/bin/env bash
# The PreToolUse guard the NON-Claude agents run — the two
# `.claude/settings.json` protections neither of them has an equivalent for, in
# one hook.
#
# Two agents install this, and both sync scripts point here:
#   * Codex (and Kimi K3 reached through `codex --profile kimi`)
#     -> `scripts/codex-hooks-sync.sh`, into `$CODEX_HOME/hooks.json`
#   * Kimi Code CLI, a separate binary
#     -> `scripts/kimi-hooks-sync.sh`, into `$KIMI_CODE_HOME/config.toml`
#
# One file serves both because their payloads agree on the two fields this
# reads. Measured, not assumed — Kimi Code CLI 2.5.2 sends `tool_name: "Bash"`
# and `tool_input.command` as a string, the same shape Codex 0.167.0 and Claude
# Code send. It adds `tool_call_id` and `client_type`, which nothing here needs.
#
# Neither agent has a `permissions.deny` list and neither reads project-scoped
# hooks, so under both of them these were simply absent:
#
#   1. `permissions.deny` — `pkill`, `killall` and the destructive `aws` verbs.
#      Read from `.claude/settings.json` HERE rather than restated, so the two
#      agents cannot be given different deny lists by someone editing one file.
#   2. `scripts/hooks/aws-write-guard.sh` — the graded aws/terraform/kubectl/
#      helm decision table, delegated to with `ER_GUARD_ASK_AS_DENY=1` because
#      `ask` fails open on BOTH non-Claude agents (see that script's header).
#
# Either sync script is what passes the repo path in.
#
# Protocol, as measured rather than as documented, and IDENTICAL on both agents
# in the one respect that decides this script's shape:
#   - silent exit 0                        -> the call proceeds
#   - `permissionDecision: deny` on stdout -> the call is refused
#   - exit 2                               -> refused as well
#   - `ask`, `allow`, or non-JSON stdout   -> the call PROCEEDS
# The last line is why this script only ever says `deny` or nothing. On Codex
# an `ask` is logged `PreToolUse Failed` and runs; on Kimi Code CLI an `ask`
# verdict was measured running the command too (a `touch` sentinel appeared).
# `ask` is not a weaker `deny` on either of them — it is an allow.
#
# Both are also fail-OPEN on a script error: exit 1 ran the command on Kimi,
# and Kimi's own documentation states the design ("preventing hook errors from
# becoming blockers") and warns it should not be the sole security barrier.
# That is why the parse-error path below still delegates rather than refusing.
#
# One difference that does NOT affect this file but does affect whether it ever
# runs: Codex skips a hook it has not trusted, silently, until approved once
# interactively. Kimi Code CLI has no trust step — a `[[hooks]]` entry fired on
# a brand-new config home with no approval.
#
# On a malformed or unreadable settings file this guard still delegates rather
# than refusing everything: a hook that fails closed on its own parse error
# bricks the session, and the aws half is the half that protects the account.
# The "did the deny list actually parse" assertion lives in each sync
# script and in `scripts/ci/{codex,kimi}-hooks-sync-test.sh`, where failing
# is free.
set -uo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
repo="${ER_REPO_ROOT:-$(cd "$here/../.." && pwd)}"
settings="$repo/.claude/settings.json"
aws_guard="$here/aws-write-guard.sh"

payload=$(cat)
command=$(printf '%s' "$payload" | jq -r '.tool_input.command // ""' 2>/dev/null || true)

# Nothing to inspect — not a shell call, or a tool whose input has no command.
[[ -n $command ]] || exit 0

deny() {
	jq -n --arg r "$1" '{
		hookSpecificOutput: {
			hookEventName: "PreToolUse",
			permissionDecision: "deny",
			permissionDecisionReason: $r
		}
	}'
	exit 0
}

# Strip leading environment assignments, for the same reason aws-write-guard
# does: `AWS_PROFILE=x aws organizations …` must match an `aws organizations *`
# pattern, or every entry in the deny list is one prefix away from bypassed.
bare=$command
while [[ $bare =~ ^[[:space:]]*[A-Za-z_][A-Za-z0-9_]*=[^[:space:]]*[[:space:]]+(.*)$ ]]; do
	bare=${BASH_REMATCH[1]}
done

# --- 1. the deny list, derived from the file Claude Code reads --------------
# `Bash(pkill *)` becomes the glob `pkill *`, matched with `case` so the `*`
# means what it means in the settings file. A pattern ending ` *` is also
# matched against the bare verb, so `pkill` with no arguments is covered —
# Claude Code's own matcher does not do that, and the gap is not worth
# inheriting.
if [[ -r $settings ]]; then
	while IFS= read -r pattern; do
		[[ -n $pattern ]] || continue
		for candidate in "$command" "$bare"; do
			# shellcheck disable=SC2254
			case "$candidate" in
			$pattern)
				deny "Denied by permissions.deny in .claude/settings.json: \`$pattern\`. This agent has no deny list of its own, so this guard applies the one Claude Code uses."
				;;
			esac
			if [[ $pattern == *" *" ]]; then
				verb=${pattern% \*}
				[[ $candidate == "$verb" ]] &&
					deny "Denied by permissions.deny in .claude/settings.json: \`$pattern\` (matched with no arguments)."
			fi
		done
	done < <(jq -r '.permissions.deny[]? | select(startswith("Bash(")) | ltrimstr("Bash(") | rtrimstr(")")' \
		"$settings" 2>/dev/null || true)
fi

# --- 2. the graded aws/terraform/kubectl/helm table ------------------------
# Delegated rather than duplicated. `ask` collapses to `deny` there because
# neither agent can present a prompt, and `ask` runs the command on both.
if [[ -x $aws_guard || -r $aws_guard ]]; then
	printf '%s' "$payload" | ER_GUARD_ASK_AS_DENY=1 bash "$aws_guard"
	exit $?
fi

exit 0
