#!/usr/bin/env bash
# The PreToolUse guard Codex runs — the two `.claude/settings.json` protections
# that have no Codex equivalent, in one hook.
#
# Codex has no `permissions.deny` list and no project-scoped hooks, so under it
# both of these were simply absent:
#
#   1. `permissions.deny` — `pkill`, `killall` and the destructive `aws` verbs.
#      Read from `.claude/settings.json` HERE rather than restated, so the two
#      agents cannot be given different deny lists by someone editing one file.
#   2. `scripts/hooks/aws-write-guard.sh` — the graded aws/terraform/kubectl/
#      helm decision table, delegated to with `ER_GUARD_ASK_AS_DENY=1` because
#      `ask` fails open on Codex (see that script's header).
#
# Installed by `scripts/codex-hooks-sync.sh`, which is also what passes the
# repo path in. Codex's PreToolUse payload is byte-compatible with Claude
# Code's — `tool_name`, `tool_input.command` as a string — measured against
# 0.167.0, which is why one guard can serve both.
#
# Protocol, as measured rather than as documented:
#   - silent exit 0                       -> `Completed`, the call proceeds
#   - `permissionDecision: deny` on stdout -> `Blocked`, the call is refused
#   - exit 2                               -> `Blocked` as well
#   - `ask`, `allow`, or non-JSON stdout   -> the call PROCEEDS
# The last line is why this script only ever says `deny` or nothing.
#
# On a malformed or unreadable settings file this guard still delegates rather
# than refusing everything: a hook that fails closed on its own parse error
# bricks the session, and the aws half is the half that protects the account.
# The "did the deny list actually parse" assertion lives in
# `scripts/ci/codex-hooks-sync-test.sh`, where failing is free.
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
				deny "Denied by permissions.deny in .claude/settings.json: \`$pattern\`. Codex has no deny list of its own, so this guard applies the one Claude Code uses."
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
# nothing on this agent can present a prompt.
if [[ -x $aws_guard || -r $aws_guard ]]; then
	printf '%s' "$payload" | ER_GUARD_ASK_AS_DENY=1 bash "$aws_guard"
	exit $?
fi

exit 0
