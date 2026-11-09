#!/usr/bin/env bash
# Run Symposium in rust/ even when the agent session starts at the repo root.
# Symposium uses the hook payload's cwd for workspace discovery, not just the
# process cwd, so both must point at the Cargo workspace.
set -euo pipefail

agent="${1:-}"
case "$agent" in
	claude | codex) ;;
	*) printf 'symposium-session-start: expected claude or codex\n' >&2; exit 2 ;;
esac

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
workspace="$root/rust"
bin="$root/.tools/bin/cargo-agents"

[ -x "$bin" ] || exit 0
[ -d "$workspace" ] || exit 0
command -v jq >/dev/null 2>&1 || {
	printf 'symposium-session-start: jq is required\n' >&2
	exit 2
}

cd "$workspace"
jq -c --arg cwd "$workspace" '.cwd = $cwd' |
	SYMPOSIUM_HOME="$root/.symposium" "$bin" hook "$agent" session-start
