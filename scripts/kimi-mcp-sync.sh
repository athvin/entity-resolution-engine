#!/usr/bin/env bash
# kimi-mcp-sync — give Kimi Code CLI the MCP servers `.mcp.json` declares.
#
# Kimi Code CLI is a separate binary from Codex, not `codex --profile kimi`, and
# it reads none of the Claude Code config. Its MCP servers come from
# `$KIMI_CODE_HOME/mcp.json` (default `~/.kimi-code/mcp.json`), so like the
# Codex pair this is per-developer state the repo DERIVES rather than documents:
#
#   scripts/kimi-mcp-sync.sh              # write $KIMI_CODE_HOME/mcp.json
#   scripts/kimi-mcp-sync.sh --dry-run    # print the resulting file, write nothing
#   scripts/kimi-mcp-sync.sh --remove     # take the derived servers back out
#
# WHY NOT JUST COPY `.mcp.json`. The shapes are close but not equal, and the
# differences are the whole reason this is a script:
#
#   * Claude Code's `"type": "stdio" | "http" | "sse"` does not exist in Kimi's
#     schema. Kimi infers stdio from a `command` key and HTTP from a `url` key,
#     and wants `"transport": "sse"` spelled out for the legacy transport. The
#     `type` key is therefore TRANSLATED, never passed through.
#   * Kimi's per-server keys are camelCase where Claude Code has none at all
#     (`startupTimeoutMs`, `enabledTools`), so only keys both understand are
#     forwarded. An unrecognised key is not worth the risk: Kimi's `[[hooks]]`
#     tables fail the entire config load on an extra field, and that is the
#     neighbouring schema.
#
# A project-level `.kimi-code/mcp.json` also exists and is deliberately NOT
# used: its stdio entries execute on session start, which makes a checked-out
# file a code-execution vector. The user-level file is the one this writes.
#
# Servers a developer added themselves (through `/mcp-config`) are preserved —
# only the names `.mcp.json` declares are rewritten, and `--remove` deletes only
# those. Idempotent by construction: run twice, same file.
#
# Written for bash 3.2 (macOS `/bin/bash`).
set -euo pipefail

mode=install
root="$(cd "$(dirname "$0")/.." && pwd)"
kimi_home="${KIMI_CODE_HOME:-$HOME/.kimi-code}"
mcp_json=""
settings_json=""

die() {
	printf 'kimi-mcp-sync: %s\n' "$*" >&2
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
	--file)
		shift
		mcp_json="${1:-}"
		;;
	--settings)
		shift
		settings_json="${1:-}"
		;;
	-h | --help)
		sed -n '2,32p' "$0" | sed 's/^# \{0,1\}//'
		exit 0
		;;
	*) die "unknown argument: $1" ;;
	esac
	shift
done

command -v jq >/dev/null 2>&1 || die "jq is required"

mcp_json="${mcp_json:-$root/.mcp.json}"
settings_json="${settings_json:-$root/.claude/settings.json}"
out="$kimi_home/mcp.json"

[ -f "$mcp_json" ] || die "$mcp_json is missing (renamed?)"
jq -e . "$mcp_json" >/dev/null 2>&1 || die "$mcp_json is not valid JSON"

# Examined-nothing guard. An empty or restructured `.mcp.json` must fail rather
# than report a clean sync — "synced 0 servers" reads exactly like "nothing to
# do", and the tool is then simply missing from the session.
declared=$(jq -r '.mcpServers // {} | keys | length' "$mcp_json")
[ "${declared:-0}" -gt 0 ] ||
	die "no servers under .mcpServers in $mcp_json — examining nothing"

# Translate each declared server into Kimi's schema, honouring
# `enabledMcpjsonServers` when `.claude/settings.json` has one — absent, there is
# no list to obey (the key is optional in Claude Code too) and every declared
# server syncs. An unknown transport is a hard failure rather than a silent
# drop: a server quietly passed over is the failure mode this script exists to
# prevent.
derived=$(jq \
	--slurpfile s <(
		if [ -f "$settings_json" ]; then cat "$settings_json"; else echo '{}'; fi
	) \
	'
  ($s[0].enabledMcpjsonServers // null) as $enabled
  | .mcpServers
  | with_entries(
      select(.key as $k | $enabled == null or ($enabled | index($k)))
    )
  | with_entries(
      .value as $v
      | .value = (
          if ($v.command // "") != "" then
            { command: $v.command }
            + (if $v.args then { args: $v.args } else {} end)
            + (if $v.env then { env: $v.env } else {} end)
            + (if $v.cwd then { cwd: $v.cwd } else {} end)
          elif ($v.url // "") != "" then
            { url: $v.url }
            + (if ($v.type // "") == "sse" then { transport: "sse" } else {} end)
            + (if $v.headers then { headers: $v.headers } else {} end)
          else
            ("unsupported server shape for \(.key): no .command and no .url" | halt_error(3))
          end
        )
      | .value += (if $v.enabled == false then { enabled: false } else {} end)
    )
' "$mcp_json") || die "could not translate $mcp_json into Kimi's schema (see the error above)"

managed_names=$(printf '%s' "$derived" | jq -r 'keys[]')
synced=$(printf '%s' "$derived" | jq -r 'keys | length')
skipped=$((declared - synced))

# Merge over whatever is already there, so a hand-added server survives. On
# --remove, delete the derived names and leave the rest.
existing='{}'
if [ -f "$out" ]; then
	jq -e . "$out" >/dev/null 2>&1 || die "$out exists and is not valid JSON — fix or move it first"
	existing=$(cat "$out")
fi

if [ "$mode" = remove ]; then
	updated=$(printf '%s' "$existing" | jq --argjson d "$derived" '
    (.mcpServers //= {})
    | .mcpServers |= with_entries(select(.key as $k | ($d | keys | index($k)) | not))
  ')
else
	updated=$(printf '%s' "$existing" | jq --argjson d "$derived" '
    (.mcpServers //= {})
    | .mcpServers = (.mcpServers + $d)
  ')
fi

if [ "$mode" = dry ]; then
	printf '%s would become:\n' "$out"
	printf '%s\n' "$updated"
	printf '\nderived %d of %d declared server(s)' "$synced" "$declared"
	if [ "$skipped" -gt 0 ]; then
		printf ', %d not in enabledMcpjsonServers' "$skipped"
	fi
	printf '\n'
	exit 0
fi

mkdir -p "$kimi_home"
printf '%s\n' "$updated" >"$out"

if [ "$mode" = remove ]; then
	printf 'kimi-mcp-sync: removed %d derived server(s) from %s\n' "$synced" "$out"
	exit 0
fi

printf 'kimi-mcp-sync: wrote %d of %d declared server(s) to %s\n' "$synced" "$declared" "$out"
if [ "$skipped" -gt 0 ]; then
	printf '  %d skipped — declared in .mcp.json but not in enabledMcpjsonServers\n' "$skipped"
fi
printf '%s\n' "$managed_names" | sed 's/^/  + /'
cat <<'EOF'

A server added mid-session is not registered in an already-open session — Kimi
reads mcp.json at session start. Run `/mcp` in a NEW session to confirm the
status, and `/mcp-config login <name>` if the server needs OAuth.
EOF
