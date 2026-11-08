#!/usr/bin/env bash
# Self-test for kimi-mcp-sync.sh — the derivation from `.mcp.json` to
# `$KIMI_CODE_HOME/mcp.json`.
#
# What this is really guarding. Kimi Code CLI has no `type` key: it infers
# stdio from `command` and HTTP from `url`, and needs `transport = "sse"`
# spelled out. A sync that passed Claude Code's `"type": "stdio"` straight
# through would write a file Kimi might accept while ignoring the field, or
# reject wholesale — and either way the failure surfaces as "the tool isn't
# there", in a session, days later. So the translation is pinned per transport.
#
# The other half is the silent-drop case: a server shape the script does not
# understand must FAIL, not be skipped. A sync that quietly synced three of four
# servers reports success and leaves one tool missing.
#
# Everything runs against fixture files and a fixture home. `kimi` is never
# invoked, so this works on a runner with no Kimi install; jq is the only
# dependency.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
sync="$here/../kimi-mcp-sync.sh"

[ -f "$sync" ] || {
	printf 'kimi-mcp-sync-test: %s is missing (renamed?)\n' "$sync" >&2
	exit 2
}
command -v jq >/dev/null 2>&1 || {
	echo 'kimi-mcp-sync-test: jq is required' >&2
	exit 2
}

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

pass=0
fail=0
ok() {
	pass=$((pass + 1))
	printf '  PASS  %s\n' "$1"
}
no() {
	fail=$((fail + 1))
	printf '  FAIL  %s\n' "$1" >&2
	[ -n "${2:-}" ] && printf '        %s\n' "$2" >&2
	return 0
}

# A settings file with NO approval list, so the translation cases are not
# filtered by the real repo's `enabledMcpjsonServers`. Passing "" used to fall
# back to the real .claude/settings.json, which lists only `aws` — the http,
# sse and bad-shape fixtures were dropped before translation and three
# assertions failed for a reason that had nothing to do with what they test.
echo '{}' >"$tmp/nolist.json"

run() { # $1 mcp.json, $2 settings-or-"" (""=no approval list), rest: extra args
	local mcp=$1 settings=$2
	shift 2
	rm -rf "$tmp/home"
	[ -n "$settings" ] || settings="$tmp/nolist.json"
	bash "$sync" --kimi-home "$tmp/home" --file "$mcp" --settings "$settings" \
		"$@" >"$tmp/out" 2>"$tmp/err"
}

# --- stdio: `type` translated away, command/args/env/cwd forwarded -----------
cat >"$tmp/stdio.json" <<'EOF'
{ "mcpServers": { "aws": {
  "type": "stdio", "command": "uvx", "args": ["x", "--read-only"],
  "env": {"AWS_REGION": "us-east-2"}, "cwd": "/srv"
} } }
EOF
if run "$tmp/stdio.json" ""; then
	got=$(jq -c '.mcpServers.aws' "$tmp/home/mcp.json")
	want='{"command":"uvx","args":["x","--read-only"],"env":{"AWS_REGION":"us-east-2"},"cwd":"/srv"}'
	if [ "$got" = "$want" ]; then
		ok "stdio: drops Claude Code's 'type', forwards command/args/env/cwd"
	else
		no "stdio translation is wrong" "want $want
        got  $got"
	fi
else
	no "stdio fixture failed to sync" "$(cat "$tmp/err")"
fi

# --- http: url forwarded, no transport key ---------------------------------
cat >"$tmp/http.json" <<'EOF'
{ "mcpServers": { "linear": {
  "type": "http", "url": "https://mcp.linear.app/mcp", "headers": {"X-A": "1"}
} } }
EOF
if run "$tmp/http.json" ""; then
	got=$(jq -c '.mcpServers.linear' "$tmp/home/mcp.json")
	want='{"url":"https://mcp.linear.app/mcp","headers":{"X-A":"1"}}'
	if [ "$got" = "$want" ]; then
		ok "http: url and headers forwarded, no spurious transport key"
	else
		no "http translation is wrong" "want $want
        got  $got"
	fi
else
	no "http fixture failed to sync" "$(cat "$tmp/err")"
fi

# --- sse: transport must be made EXPLICIT ----------------------------------
# The one case where dropping `type` silently would change behaviour: without
# `transport`, Kimi treats a url entry as modern HTTP and a legacy SSE server
# never connects.
cat >"$tmp/sse.json" <<'EOF'
{ "mcpServers": { "legacy": { "type": "sse", "url": "https://x.example.com/sse" } } }
EOF
if run "$tmp/sse.json" ""; then
	got=$(jq -r '.mcpServers.legacy.transport // "ABSENT"' "$tmp/home/mcp.json")
	if [ "$got" = "sse" ]; then
		ok "sse: transport is written explicitly, not inferred"
	else
		no "sse transport was not made explicit" "got '$got'"
	fi
else
	no "sse fixture failed to sync" "$(cat "$tmp/err")"
fi

# --- a shape it does not understand must FAIL, not be skipped --------------
cat >"$tmp/bad.json" <<'EOF'
{ "mcpServers": { "mystery": { "type": "stdio", "runner": "nope" } } }
EOF
if run "$tmp/bad.json" ""; then
	no "a server with neither command nor url was accepted" \
		"a silently-skipped server reads as a successful sync with the tool missing"
else
	if grep -q "unsupported server shape" "$tmp/err"; then
		ok "refuses a server with neither command nor url, naming it"
	else
		no "failed, but not with the shape error" "$(head -2 "$tmp/err")"
	fi
fi

# --- examined-nothing guard ------------------------------------------------
echo '{ "mcpServers": {} }' >"$tmp/empty.json"
if run "$tmp/empty.json" ""; then
	no "an empty .mcpServers reported a successful sync" "examining nothing must fail"
else
	ok "empty .mcpServers fails rather than reporting a clean sync"
fi

# --- enabledMcpjsonServers is honoured -------------------------------------
cat >"$tmp/two.json" <<'EOF'
{ "mcpServers": {
  "yes": { "type": "stdio", "command": "a" },
  "nope": { "type": "stdio", "command": "b" }
} }
EOF
echo '{ "enabledMcpjsonServers": ["yes"] }' >"$tmp/settings.json"
if run "$tmp/two.json" "$tmp/settings.json"; then
	keys=$(jq -r '.mcpServers | keys | join(",")' "$tmp/home/mcp.json")
	if [ "$keys" = "yes" ]; then
		ok "a server absent from enabledMcpjsonServers is not synced"
	else
		no "the approval list was not honoured" "synced: $keys"
	fi
else
	no "approval-list fixture failed to sync" "$(cat "$tmp/err")"
fi

# And with no settings file at all, there is no list to obey.
if run "$tmp/two.json" "$tmp/missing-settings.json"; then
	n=$(jq -r '.mcpServers | keys | length' "$tmp/home/mcp.json")
	if [ "$n" -eq 2 ]; then
		ok "no settings file means no approval list, so every server syncs"
	else
		no "a missing settings file changed the server set" "synced $n of 2"
	fi
else
	no "missing-settings fixture failed to sync" "$(cat "$tmp/err")"
fi

# --- idempotence, and a hand-added server survives -------------------------
rm -rf "$tmp/home"
mkdir -p "$tmp/home"
echo '{ "mcpServers": { "mine": { "command": "hand-added" } } }' >"$tmp/home/mcp.json"
bash "$sync" --kimi-home "$tmp/home" --file "$tmp/stdio.json" >/dev/null 2>&1
bash "$sync" --kimi-home "$tmp/home" --file "$tmp/stdio.json" >/dev/null 2>&1
keys=$(jq -r '.mcpServers | keys | sort | join(",")' "$tmp/home/mcp.json")
if [ "$keys" = "aws,mine" ]; then
	ok "two installs leave one derived entry and preserve a hand-added server"
else
	no "merge or idempotence is wrong" "keys: $keys"
fi

# --- --remove takes ours out and leaves theirs -----------------------------
bash "$sync" --kimi-home "$tmp/home" --file "$tmp/stdio.json" --remove >/dev/null 2>&1
keys=$(jq -r '.mcpServers | keys | join(",")' "$tmp/home/mcp.json")
if [ "$keys" = "mine" ]; then
	ok "--remove deletes only the derived servers"
else
	no "--remove removed the wrong set" "keys: $keys"
fi

# --- a corrupt destination is refused, not overwritten ---------------------
printf 'not json at all\n' >"$tmp/home/mcp.json"
if bash "$sync" --kimi-home "$tmp/home" --file "$tmp/stdio.json" >/dev/null 2>"$tmp/err"; then
	no "an existing non-JSON mcp.json was overwritten" "it may be a developer's own file"
else
	ok "refuses an existing mcp.json that is not valid JSON"
fi

# --- --dry-run writes nothing ----------------------------------------------
rm -rf "$tmp/home"
bash "$sync" --kimi-home "$tmp/home" --file "$tmp/stdio.json" --dry-run >/dev/null 2>&1
if [ -e "$tmp/home/mcp.json" ]; then
	no "--dry-run wrote the file"
else
	ok "--dry-run writes nothing"
fi

printf '\nkimi-mcp-sync-test: %s passed, %s failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ] || exit 1
