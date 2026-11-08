#!/usr/bin/env bash
# Self-test for codex-mcp-sync.sh — the derivation from `.mcp.json` to
# `codex mcp add`, asserted on fixtures rather than on whatever happens to be
# in the repo's own `.mcp.json` today.
#
# It runs everything through `--dry-run`, so it needs no `codex` on PATH and
# registers nothing. That is deliberate: this suite has to run in CI, where
# Codex is not installed, and it must never write to a developer's
# `$CODEX_HOME`.
#
# The cases that matter most are the REFUSALS. A sync script's worst failure is
# passing over a server it did not understand, because the output then reads
# "synced" while the tool is missing from the session — so every unrecognised
# shape is asserted to exit non-zero with its reason.
set -uo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
sync="$here/../codex-mcp-sync.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

pass=0
fail=0

# write_json <file> <<<content
w() { cat >"$work/$1"; }

# expect_ok <name> <needle> <args...>
expect_ok() {
    local name="$1" needle="$2" out rc
    shift 2
    out="$(bash "$sync" --dry-run "$@" 2>&1)"
    rc=$?
    if [ "$rc" -ne 0 ]; then
        printf 'FAIL %s: expected exit 0, got %d:\n%s\n' "$name" "$rc" "$out"
        fail=$((fail + 1))
        return
    fi
    if ! grep -qF -- "$needle" <<<"$out"; then
        printf 'FAIL %s: output lacks %s\n%s\n' "$name" "$needle" "$out"
        fail=$((fail + 1))
        return
    fi
    printf 'ok   %s\n' "$name"
    pass=$((pass + 1))
}

# expect_ok_re <name> <extended-regex> <args...>
#
# Separate from expect_ok because one property — that an argument containing a
# space arrives as ONE argument — has no single spelling to match against.
# `printf %q` renders it `A=1\ B=2` on bash 3.2 and may single-quote it
# elsewhere; both are correct, and asserting one of them would make this suite
# fail on a bash upgrade rather than on a regression.
expect_ok_re() {
    local name="$1" re="$2" out rc
    shift 2
    out="$(bash "$sync" --dry-run "$@" 2>&1)"
    rc=$?
    if [ "$rc" -ne 0 ]; then
        printf 'FAIL %s: expected exit 0, got %d:\n%s\n' "$name" "$rc" "$out"
        fail=$((fail + 1))
        return
    fi
    if ! grep -Eq -- "$re" <<<"$out"; then
        printf 'FAIL %s: output does not match /%s/\n%s\n' "$name" "$re" "$out"
        fail=$((fail + 1))
        return
    fi
    printf 'ok   %s\n' "$name"
    pass=$((pass + 1))
}

# expect_refusal <name> <needle> <args...>
expect_refusal() {
    local name="$1" needle="$2" out rc
    shift 2
    out="$(bash "$sync" --dry-run "$@" 2>&1)"
    rc=$?
    if [ "$rc" -eq 0 ]; then
        printf 'FAIL %s: expected a refusal, exited 0:\n%s\n' "$name" "$out"
        fail=$((fail + 1))
        return
    fi
    if ! grep -qF -- "$needle" <<<"$out"; then
        printf 'FAIL %s: refused but not with %s\n%s\n' "$name" "$needle" "$out"
        fail=$((fail + 1))
        return
    fi
    printf 'ok   %s (refused for the right reason)\n' "$name"
    pass=$((pass + 1))
}

# No approval list: every declared server is registered.
w no-settings.json <<'JSON'
{"mcpServers":{"alpha":{"type":"stdio","command":"uvx","args":["pkg@latest","--read-only"]}}}
JSON
expect_ok 'a stdio server, no approval list' \
    'codex mcp add alpha -- uvx pkg@latest --read-only' \
    --file "$work/no-settings.json" --settings "$work/absent.json"

# The flags must survive in order and keep their values — this is the whole
# reason the script derives instead of restating: `--read-only` and `--profile`
# are the AWS server's guardrails, and a hand-typed copy is where they get lost.
expect_ok 'flag order and values are preserved' \
    'pkg@latest --read-only' \
    --file "$work/no-settings.json" --settings "$work/absent.json"

# An argument containing a space must arrive as ONE argument.
w spacey.json <<'JSON'
{"mcpServers":{"alpha":{"command":"run","args":["--metadata","A=1 B=2"]}}}
JSON
expect_ok_re 'an argument containing a space stays one argument' \
    "--metadata (A=1\\\\ B=2|'A=1 B=2')( |\$)" \
    --file "$work/spacey.json" --settings "$work/absent.json"

# env becomes repeated --env KEY=VALUE.
w env.json <<'JSON'
{"mcpServers":{"alpha":{"command":"run","env":{"TOKEN_VAR":"X"}}}}
JSON
expect_ok 'env becomes --env KEY=VALUE' \
    '--env TOKEN_VAR=X' \
    --file "$work/env.json" --settings "$work/absent.json"

# A url server takes --url and no positional command.
w url.json <<'JSON'
{"mcpServers":{"remote":{"type":"http","url":"https://example.invalid/mcp"}}}
JSON
expect_ok 'a url server uses --url' \
    'codex mcp add remote --url https://example.invalid/mcp' \
    --file "$work/url.json" --settings "$work/absent.json"

# The approval list is honoured, and the skip is REPORTED rather than silent.
w two.json <<'JSON'
{"mcpServers":{"alpha":{"command":"run"},"beta":{"command":"run"}}}
JSON
w settings.json <<'JSON'
{"enabledMcpjsonServers":["alpha"]}
JSON
expect_ok 'a server absent from enabledMcpjsonServers is reported, not silent' \
    'skip: beta' \
    --file "$work/two.json" --settings "$work/settings.json"
expect_ok 'and the enabled one is still registered' \
    'codex mcp add alpha' \
    --file "$work/two.json" --settings "$work/settings.json"

# --- the refusals ---------------------------------------------------------

# Every declared server disabled: the run registered nothing, and "nothing"
# must not read as success.
w none-enabled.json <<'JSON'
{"enabledMcpjsonServers":[]}
JSON
expect_refusal 'all servers disabled is a failure, not a clean sync' \
    'registered nothing' \
    --file "$work/two.json" --settings "$work/none-enabled.json"

w empty.json <<'JSON'
{"mcpServers":{}}
JSON
expect_refusal 'an empty .mcpServers is a failure' \
    'examining nothing' \
    --file "$work/empty.json" --settings "$work/absent.json"

w renamed.json <<'JSON'
{"servers":{"alpha":{"command":"run"}}}
JSON
expect_refusal 'a renamed top-level key is a failure' \
    'examining nothing' \
    --file "$work/renamed.json" --settings "$work/absent.json"

w neither.json <<'JSON'
{"mcpServers":{"alpha":{"type":"stdio"}}}
JSON
expect_refusal 'a server with neither command nor url' \
    'unrecognised shape' \
    --file "$work/neither.json" --settings "$work/absent.json"

w badtype.json <<'JSON'
{"mcpServers":{"alpha":{"type":"websocket","command":"run"}}}
JSON
expect_refusal 'an unknown transport type' \
    'only stdio is understood' \
    --file "$work/badtype.json" --settings "$work/absent.json"

w urlenv.json <<'JSON'
{"mcpServers":{"remote":{"type":"http","url":"https://example.invalid/mcp","env":{"A":"1"}}}}
JSON
expect_refusal 'a url server carrying env, which codex cannot take' \
    'stdio servers only' \
    --file "$work/urlenv.json" --settings "$work/absent.json"

printf 'bad json' >"$work/broken.json"
expect_refusal 'a malformed .mcp.json' \
    'not valid JSON' \
    --file "$work/broken.json" --settings "$work/absent.json"

expect_refusal 'a missing .mcp.json' \
    'is missing (renamed?)' \
    --file "$work/does-not-exist.json" --settings "$work/absent.json"

echo "---"
echo "pass=$pass fail=$fail"
# A suite that asserted nothing is a failure, for the same reason the script it
# tests refuses to report a sync of zero servers.
if [ "$pass" -eq 0 ]; then
    echo "FAIL asserted nothing"
    exit 2
fi
[ "$fail" -eq 0 ] || exit 1
echo "codex-mcp-sync: ok"
