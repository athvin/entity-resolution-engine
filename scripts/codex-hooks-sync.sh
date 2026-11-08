#!/usr/bin/env bash
# codex-hooks-sync — install this repo's PreToolUse guard into Codex.
#
# `.claude/settings.json` wires `scripts/hooks/aws-write-guard.sh` as a
# `PreToolUse` hook and carries a `permissions.deny` list. Neither reaches
# Codex: it has no deny list at all, and a project-level `.codex/hooks.json`
# or `.agents/hooks.json` never fires — only `$CODEX_HOME/hooks.json` does
# (measured against Codex 0.167.0; those project paths exist in the binary
# inside its Claude-Code *migration* reader, which is what makes them look
# repo-scoped).
#
# So, like the MCP servers, this is per-developer state and the repo's job is
# to derive it rather than to document it:
#
#   scripts/codex-hooks-sync.sh              # install into $CODEX_HOME/hooks.json
#   scripts/codex-hooks-sync.sh --dry-run    # print the resulting file, write nothing
#   scripts/codex-hooks-sync.sh --remove     # take it back out
#
# WHAT YOU MUST KNOW TO GET ANY PROTECTION FROM THIS. A hook Codex has not
# trusted is skipped SILENTLY — no warning, no log line, the command simply
# runs. Trust is a `[hooks.state."<file>:<event>:<i>:<j>"] trusted_hash` entry
# in `$CODEX_HOME/config.toml`, and `--dangerously-bypass-hook-trust` does
# **not** persist one (verified: four bypassed runs wrote no state). So either
#
#   * start `codex` interactively once and approve the hook, which writes the
#     trust entry and makes every later run — including `codex exec` — honour
#     it; or
#   * pass `--dangerously-bypass-hook-trust` on every invocation.
#
# This script prints that instruction after installing, because an installed
# hook that never runs is worse than no hook: it reads as protection.
#
# The guard is effective where it matters: a `deny` blocks the call even under
# `--dangerously-bypass-approvals-and-sandbox`, which is the invocation this
# repo's Codex and Kimi sessions actually use. Sandbox bypass is not hook
# bypass.
#
# Written for bash 3.2 (macOS `/bin/bash`).
set -euo pipefail

mode=install
root="$(cd "$(dirname "$0")/.." && pwd)"
codex_home="${CODEX_HOME:-$HOME/.codex}"

die() {
	printf 'codex-hooks-sync: %s\n' "$*" >&2
	exit 1
}

while [ $# -gt 0 ]; do
	case "$1" in
	--dry-run) mode=dry ;;
	--remove) mode=remove ;;
	# Fixture overrides; the self-test is their only caller.
	--root)
		shift
		root="${1:-}"
		;;
	--codex-home)
		shift
		codex_home="${1:-}"
		;;
	-h | --help)
		sed -n '2,40p' "$0" | sed 's/^# \{0,1\}//'
		exit 0
		;;
	*) die "unknown argument: $1" ;;
	esac
	shift
done

command -v jq >/dev/null 2>&1 || die "jq is required"

guard="$root/scripts/hooks/codex-pretooluse-guard.sh"
settings="$root/.claude/settings.json"
hooks_file="$codex_home/hooks.json"

[ -f "$guard" ] || die "$guard is missing (renamed?)"
[ -f "$settings" ] || die "$settings is missing (renamed?)"

# Assert the derivation actually derived something. A deny list that parsed to
# nothing would install a guard protecting only half of what it claims, and the
# failure is silent at runtime — so it is caught here, where failing is free.
deny_count=$(jq -r '[.permissions.deny[]? | select(startswith("Bash("))] | length' "$settings")
[ "${deny_count:-0}" -gt 0 ] ||
	die "no Bash(...) entries under .permissions.deny in $settings — the deny list would be empty, which is not a guard"

# The marker is how this script finds its own entry again on a re-run or a
# --remove, without disturbing hooks a developer added by hand.
marker="er-codex-pretooluse-guard"
entry=$(jq -n --arg cmd "ER_REPO_ROOT=$root bash $guard" --arg m "$marker" '{
  type: "command",
  timeout: 20,
  command: $cmd,
  description: $m
}')

existing='{}'
if [ -f "$hooks_file" ]; then
	jq -e . "$hooks_file" >/dev/null 2>&1 || die "$hooks_file exists and is not valid JSON — fix or move it first"
	existing=$(cat "$hooks_file")
fi

# Drop any previous copy of ours, keep everything else, then append if
# installing. Idempotent by construction: install twice, one entry.
updated=$(printf '%s' "$existing" | jq --argjson e "$entry" --arg m "$marker" --arg mode "$mode" '
  (.hooks //= {})
  | (.hooks.PreToolUse //= [])
  | .hooks.PreToolUse = [
      .hooks.PreToolUse[]
      | (.hooks //= [])
      | .hooks |= map(select((.description // "") != $m))
    ]
  | .hooks.PreToolUse = [ .hooks.PreToolUse[] | select((.hooks | length) > 0) ]
  | if $mode == "remove" then . else
      .hooks.PreToolUse += [ { hooks: [ $e ] } ]
    end
  | if (.hooks.PreToolUse | length) == 0 then del(.hooks.PreToolUse) else . end
')

if [ "$mode" = dry ]; then
	printf '%s would become:\n' "$hooks_file"
	printf '%s\n' "$updated"
	printf '\nderived %d deny-list pattern(s) from %s\n' "$deny_count" "$settings"
	exit 0
fi

mkdir -p "$codex_home"
printf '%s\n' "$updated" >"$hooks_file"

if [ "$mode" = remove ]; then
	printf 'codex-hooks-sync: removed the guard from %s\n' "$hooks_file"
	exit 0
fi

printf 'codex-hooks-sync: installed into %s (%d deny-list pattern(s) derived)\n' "$hooks_file" "$deny_count"
cat <<'EOF'

A hook Codex has not trusted is SKIPPED SILENTLY. Do one of these or you have
installed nothing:

  1. Run `codex` interactively once and approve the hook when prompted. That
     writes the trusted_hash to $CODEX_HOME/config.toml and every later run,
     `codex exec` included, honours it.
  2. Or pass --dangerously-bypass-hook-trust on every invocation.

Confirm it is live by asking a session to run a denied command; the refusal
reads `Command blocked by PreToolUse hook: …`.
EOF
