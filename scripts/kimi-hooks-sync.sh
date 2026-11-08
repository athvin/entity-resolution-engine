#!/usr/bin/env bash
# kimi-hooks-sync — install this repo's PreToolUse guard into Kimi Code CLI.
#
# `.claude/settings.json` wires `scripts/hooks/aws-write-guard.sh` as a
# `PreToolUse` hook and carries a `permissions.deny` list. Neither reaches Kimi
# Code CLI: it has no deny list, and its hooks live ONLY in
# `$KIMI_CODE_HOME/config.toml` (default `~/.kimi-code/config.toml`). There is
# deliberately no project-level hook file — Kimi's own documentation says so,
# and it is the right call: a checked-out file that could inject hooks would be
# a code-execution vector.
#
#   scripts/kimi-hooks-sync.sh              # install into $KIMI_CODE_HOME/config.toml
#   scripts/kimi-hooks-sync.sh --dry-run    # print the managed block, write nothing
#   scripts/kimi-hooks-sync.sh --remove     # take it back out
#
# WHAT THIS BUYS, MEASURED AGAINST KIMI CODE CLI 2.5.2. A probe installed a
# PreToolUse hook in a throwaway config home and asked a session to `touch` a
# sentinel:
#
#   verdict                                  sentinel appeared?
#   `permissionDecision: deny` on stdout     no   -> BLOCKED
#   exit 2                                   no   -> BLOCKED
#   `permissionDecision: ask` on stdout      YES  -> allowed
#   exit 1 (script error)                    YES  -> allowed, fail-open
#
# So `ask` is an ALLOW here exactly as it is on Codex, which is why the guard is
# installed with `ER_GUARD_ASK_AS_DENY=1` semantics and only ever emits `deny`.
# And the guard is fail-open on its own error by design — Kimi's documentation
# states it ("preventing hook errors from becoming blockers") and warns hooks
# "should not be used as the sole security barrier". Treat this as a seatbelt
# over IAM, not as the thing standing between the agent and the account.
#
# ONE WAY KIMI IS BETTER THAN CODEX HERE: there is no hook-trust step. A Codex
# hook that has not been approved interactively is skipped SILENTLY, which reads
# as protection while providing none. The probe's hook fired on a brand-new
# config home with no approval at all, so installing is sufficient.
#
# WHY A COMMENT-DELIMITED BLOCK rather than a TOML parser. Kimi's `[[hooks]]`
# tables accept EXACTLY four fields — `event`, `matcher`, `command`, `timeout` —
# and an extra field fails the entire config load, taking the provider and model
# tables with it. So there is nowhere to put a machine-readable marker inside
# the table, and this script brackets its block with marker comments instead.
# Appending a table-array header at end-of-file is always valid TOML, which is
# what makes the append safe without rewriting the file.
#
# Written for bash 3.2 (macOS `/bin/bash`).
set -euo pipefail

mode=install
root="$(cd "$(dirname "$0")/.." && pwd)"
kimi_home="${KIMI_CODE_HOME:-$HOME/.kimi-code}"
settings=""

die() {
	printf 'kimi-hooks-sync: %s\n' "$*" >&2
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
	--kimi-home)
		shift
		kimi_home="${1:-}"
		;;
	--settings)
		shift
		settings="${1:-}"
		;;
	-h | --help)
		sed -n '2,46p' "$0" | sed 's/^# \{0,1\}//'
		exit 0
		;;
	*) die "unknown argument: $1" ;;
	esac
	shift
done

command -v jq >/dev/null 2>&1 || die "jq is required"

guard="$root/scripts/hooks/agent-pretooluse-guard.sh"
settings="${settings:-$root/.claude/settings.json}"
config="$kimi_home/config.toml"

[ -f "$guard" ] || die "$guard is missing (renamed?)"
[ -f "$settings" ] || die "$settings is missing (renamed?)"

# Assert the derivation actually derived something. A deny list that parsed to
# nothing would install a guard protecting only half of what it claims, and the
# failure is silent at runtime — so it is caught here, where failing is free.
deny_count=$(jq -r '[.permissions.deny[]? | select(startswith("Bash("))] | length' "$settings")
[ "${deny_count:-0}" -gt 0 ] ||
	die "no Bash(...) entries under .permissions.deny in $settings — the deny list would be empty, which is not a guard"

begin="# >>> er-kimi-pretooluse-guard — managed by scripts/kimi-hooks-sync.sh >>>"
end="# <<< er-kimi-pretooluse-guard <<<"

# `matcher` is matched against the TOOL NAME for PreToolUse, and `Bash` is the
# only built-in tool whose input carries a shell command. The guard exits 0
# immediately on any payload without `.tool_input.command`, so a wider matcher
# would only add overhead.
block=$(
	cat <<EOF
$begin
[[hooks]]
event = "PreToolUse"
matcher = "Bash"
command = "ER_REPO_ROOT=$root bash $guard"
timeout = 20
$end
EOF
)

# Strip any previous copy of our block, keep everything else byte-for-byte.
# Implemented in awk rather than sed so the markers never have to be escaped.
strip_block() {
	awk -v b="$begin" -v e="$end" '
		$0 == b { skipping = 1; next }
		$0 == e { skipping = 0; next }
		!skipping { print }
	' "$1"
}

existing=""
if [ -f "$config" ]; then
	existing=$(strip_block "$config")
fi

if [ "$mode" = remove ]; then
	updated="$existing"
else
	if [ -n "$existing" ]; then
		updated="$existing
$block"
	else
		updated="$block"
	fi
fi

if [ "$mode" = dry ]; then
	printf '%s would become:\n\n' "$config"
	printf '%s\n' "$updated"
	printf '\nderived %d deny-list pattern(s) from %s\n' "$deny_count" "$settings"
	exit 0
fi

mkdir -p "$kimi_home"
printf '%s\n' "$updated" >"$config"

# The config is Kimi's own, and a corrupt one costs the developer their
# provider and model tables. `kimi doctor` validates it, so use it when it is
# on PATH and say so plainly when it is not.
if command -v kimi >/dev/null 2>&1; then
	if KIMI_CODE_HOME="$kimi_home" kimi doctor >/dev/null 2>&1; then
		verdict="kimi doctor: config.toml still valid"
	else
		die "kimi doctor rejected $config after the edit — the previous content is above the markers; fix before starting a session"
	fi
else
	verdict="kimi is not on PATH, so the result is unvalidated — run \`kimi doctor\` yourself"
fi

if [ "$mode" = remove ]; then
	printf 'kimi-hooks-sync: removed the guard from %s\n  %s\n' "$config" "$verdict"
	exit 0
fi

printf 'kimi-hooks-sync: installed into %s (%d deny-list pattern(s) derived)\n  %s\n' \
	"$config" "$deny_count" "$verdict"
cat <<'EOF'

Unlike Codex there is no hook to approve — it is live in the next session.
Confirm by asking one to run a denied command; the refusal names the pattern.

Hooks here are fail-OPEN: a crash, a timeout or any non-2 error exit lets the
command through, and an `ask` verdict is an allow. This guard is a seatbelt
over the IAM denies, not a replacement for them.
EOF
