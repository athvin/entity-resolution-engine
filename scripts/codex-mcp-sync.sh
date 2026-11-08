#!/usr/bin/env bash
# codex-mcp-sync — register this repo's MCP servers with Codex, derived from
# `.mcp.json` so the two configurations cannot be hand-drifted.
#
# The skills and the project instructions reach Codex through symlinks
# (`.agents/skills`, `AGENTS.md`; see `scripts/ci/agent-bridge-check.sh`). The
# MCP servers cannot, and that is a measured fact rather than an assumption:
# a project-level `.codex/config.toml` IS merged into the session config — a
# `sandbox_mode` set there takes effect — but its `[mcp_servers.*]` table is
# ignored. Probed against Codex 0.167.0 with a real stdio MCP server declared
# both ways: declared in the project file the tool is absent from the session,
# injected with `-c` it is present. `codex mcp list` agrees, reporting none.
#
# So Codex reads MCP servers from `$CODEX_HOME/config.toml` only, which is
# per-developer state outside the repository. What this script buys is that
# nobody retypes the invocation: it reads `.mcp.json` and nothing else.
#
#   scripts/codex-mcp-sync.sh              # register, replacing same-named
#   scripts/codex-mcp-sync.sh --dry-run    # print the commands, touch nothing
#
# `--dry-run` needs no `codex` on PATH — that is what lets the self-test run in
# CI, where it is not installed.
#
# A server declared in `.mcp.json` but absent from `enabledMcpjsonServers` in
# `.claude/settings.json` is NOT registered: that list is the repo's approval
# record, and honouring it is the point of having it. Such a server is reported
# by name rather than passed over quietly.
#
# Anything this script does not understand is a hard failure. A silently
# skipped server is the worst outcome available — it reads as "synced" while
# the tool is missing.
#
# Written for bash 3.2, which is what `/bin/bash` is on macOS: every array
# expansion that can be empty carries the `${a[@]+...}` guard, because 3.2
# treats an empty array as unset under `set -u`.
set -euo pipefail

dry_run=0
root="$(cd "$(dirname "$0")/.." && pwd)"
mcp_json=""
settings_json=""

die() {
    printf 'codex-mcp-sync: %s\n' "$*" >&2
    exit 1
}

while [ $# -gt 0 ]; do
    case "$1" in
        --dry-run) dry_run=1 ;;
        # Fixture overrides; the self-test is their only caller.
        --file) shift; mcp_json="${1:-}" ;;
        --settings) shift; settings_json="${1:-}" ;;
        -h|--help) sed -n '2,32p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) die "unknown argument: $1" ;;
    esac
    shift
done

mcp_json="${mcp_json:-$root/.mcp.json}"
settings_json="${settings_json:-$root/.claude/settings.json}"

command -v jq >/dev/null 2>&1 || die "jq is required"
[ -f "$mcp_json" ] || die "$mcp_json is missing (renamed?)"
jq -e . "$mcp_json" >/dev/null 2>&1 || die "$mcp_json is not valid JSON"

if [ "$dry_run" -eq 0 ]; then
    command -v codex >/dev/null 2>&1 || die "codex is not on PATH; install it or pass --dry-run"
fi

# The approval list, if there is one. No settings file, or a settings file with
# no `enabledMcpjsonServers`, means there is no list to honour — the key is
# optional in Claude Code too — and every declared server is registered.
enabled_known=0
enabled=""
if [ -f "$settings_json" ] && jq -e '.enabledMcpjsonServers' "$settings_json" >/dev/null 2>&1; then
    enabled_known=1
    enabled="$(jq -r '.enabledMcpjsonServers[]' "$settings_json")"
fi

names=()
while IFS= read -r n; do
    [ -n "$n" ] && names+=("$n")
done < <(jq -r '.mcpServers // {} | keys[]' "$mcp_json")

# Examined-nothing guard: an empty or restructured `.mcp.json` must fail, not
# report a clean sync. A sync that noticed nothing reads exactly like one with
# nothing to do.
[ "${#names[@]}" -gt 0 ] || die "no servers under .mcpServers in $mcp_json — examining nothing"

# In a dry run every command is printed, including the remove — the point is to
# show the whole effect. In a real run the remove is expected to fail on a name
# Codex does not have yet, so its output is dropped and its status ignored.
emit() {
    printf '%q ' "$@"
    printf '\n'
}
drop_existing() {
    if [ "$dry_run" -eq 1 ]; then
        emit codex mcp remove "$1"
    else
        codex mcp remove "$1" >/dev/null 2>&1 || true
    fi
}
add() {
    if [ "$dry_run" -eq 1 ]; then
        emit "$@"
    else
        "$@"
    fi
}

registered=0
skipped=0
for name in "${names[@]}"; do
    if [ "$enabled_known" -eq 1 ] && ! grep -qxF "$name" <<<"$enabled"; then
        printf 'skip: %s — declared in .mcp.json but not in enabledMcpjsonServers\n' "$name"
        skipped=$((skipped + 1))
        continue
    fi

    type="$(jq -r --arg n "$name" '.mcpServers[$n].type // ""' "$mcp_json")"
    url="$(jq -r --arg n "$name" '.mcpServers[$n].url // ""' "$mcp_json")"
    cmd="$(jq -r --arg n "$name" '.mcpServers[$n].command // ""' "$mcp_json")"

    # Environment, as repeated `--env KEY=VALUE`. NUL-delimited so a value
    # containing whitespace survives the trip.
    env_flags=()
    while IFS= read -r -d '' kv; do
        env_flags+=(--env "$kv")
    done < <(jq -j --arg n "$name" \
        '(.mcpServers[$n].env // {}) | to_entries[] | "\(.key)=\(.value)\u0000"' "$mcp_json")

    if [ -n "$url" ]; then
        case "$type" in
            ""|http|sse|streamable-http) ;;
            *) die "$name: type \"$type\" declares a url; only http/sse are understood" ;;
        esac
        [ "${#env_flags[@]}" -eq 0 ] ||
            die "$name: codex takes --env on stdio servers only; a url server with env must be configured by hand"
        drop_existing "$name"
        add codex mcp add "$name" --url "$url"
        registered=$((registered + 1))
        continue
    fi

    [ -n "$cmd" ] || die "$name: declares neither \`command\` nor \`url\` — unrecognised shape"
    case "$type" in
        ""|stdio) ;;
        *) die "$name: type \"$type\" declares a command; only stdio is understood" ;;
    esac

    args=()
    while IFS= read -r -d '' a; do
        args+=("$a")
    done < <(jq -j --arg n "$name" '(.mcpServers[$n].args // [])[] | . + "\u0000"' "$mcp_json")

    drop_existing "$name"
    add codex mcp add "$name" ${env_flags[@]+"${env_flags[@]}"} -- "$cmd" ${args[@]+"${args[@]}"}
    registered=$((registered + 1))
done

if [ "$registered" -eq 0 ]; then
    die "registered nothing — all $skipped declared server(s) are absent from enabledMcpjsonServers"
fi

if [ "$dry_run" -eq 1 ]; then
    printf 'codex-mcp-sync: dry run — %d server(s) would be registered, %d skipped\n' "$registered" "$skipped"
else
    printf 'codex-mcp-sync: %d server(s) registered, %d skipped\n' "$registered" "$skipped"
    printf 'verify with: codex mcp list\n'
fi
