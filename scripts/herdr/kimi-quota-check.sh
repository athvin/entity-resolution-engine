#!/bin/sh
# Single-shot probe: is the Kimi K3 API (the shared `kimi` profile's coding
# membership quota) answering right now, or still behind the account's
# rolling rate limit?
#
# Exit codes: 0 = answering (up), 1 = still 429 (down), 2 = probe itself
# failed for an unrelated reason -- treat as unknown, not down.
#
# Why a real turn, not a lighter check: GET /models and the TUI's own
# `/status` screen ("Limits: data not available yet") do not touch the
# generation quota, and the API's 429 carries no Retry-After or reset header
# (confirmed 2026-11-07 against https://api.kimi.ai/coding/v1/responses), so
# there is nothing to compute a reset time from authoritatively. A trivial,
# ephemeral, read-only-sandboxed `codex exec` turn through the same `kimi`
# profile the live panes use is the only thing that actually exercises it.
#
# Usage: sh kimi-quota-check.sh          # prints one status line, sets exit code
set -eu

# cron's PATH has neither `codex` (~/.local/bin) nor GNU `timeout`
# (Homebrew; stock macOS ships no timeout(1) at all).
PATH="$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export PATH

state_dir="${HERDR_STATE_DIR:-$HOME/.local/state/herdr}"
mkdir -p "$state_dir"
state_file="$state_dir/kimi-quota.env"
lock_file="$state_dir/kimi-quota-check.lock"

# Guard against an overlapping run (e.g. the 5-minute timer firing while a
# prior probe is still inside its 90s timeout).
if ! shlock -f "$lock_file" -p $$; then
    echo "kimi-quota-check: another probe is already running, skipping" >&2
    exit 2
fi
trap 'rm -f "$lock_file"' EXIT

# shellcheck disable=SC1090
STATUS="" FIRST_DOWN_AT="" LAST_UP_AT="" NOTIFIED_AT=""
[ -f "$state_file" ] && . "$state_file"

now=$(date +%s)
out="$(mktemp "${TMPDIR:-/tmp}/kimi-probe.XXXXXX")"
trap 'rm -f "$lock_file" "$out"' EXIT

# The shared `kimi` profile's auth command reads the key from ~/Desktop,
# which is a TCC-protected folder. Measured 2026-11-07: every run of this
# probe launched by the launchd agent hung for the full 5s auth-command
# timeout and failed with "Reconnecting... (provider auth command `/bin/bash`
# timed out after 5000 ms)" -- not a quota signal at all, a local permission
# wall silently misreported as UNKNOWN. A manually-run interactive shell
# never hit it (Terminal/this session already holds the Desktop grant), which
# is exactly why it went undetected until the watch log was read closely.
# Override the auth command to read a copy of the same key kept outside any
# TCC-protected folder, rather than touching the shared kimi.config.toml used
# by the live interactive panes.
key_copy="$state_dir/kimi-k3-api-key"
if [ ! -f "$key_copy" ]; then
    cp "$HOME/Desktop/kimik3_api_key.txt" "$key_copy"
    chmod 600 "$key_copy"
fi

set +e
timeout 90 codex exec -p kimi -s read-only --ephemeral --skip-git-repo-check \
    -c "model_providers.kimi.auth.args=[\"-c\",\"cat $key_copy\"]" \
    --json "Reply with exactly: pong" < /dev/null > "$out" 2>/dev/null
set -e

if grep -q '"type":"turn.completed"' "$out" 2>/dev/null; then
    new_status="up"
elif grep -qE '429 Too Many Requests' "$out" 2>/dev/null; then
    new_status="down"
else
    new_status="unknown"
fi

if [ "$new_status" = "down" ] && [ "$STATUS" != "down" ]; then
    FIRST_DOWN_AT="$now"
fi
if [ "$new_status" = "up" ]; then
    LAST_UP_AT="$now"
    FIRST_DOWN_AT=""
fi

{
    echo "STATUS=$new_status"
    echo "FIRST_DOWN_AT=$FIRST_DOWN_AT"
    echo "LAST_UP_AT=$LAST_UP_AT"
    echo "LAST_CHECK_AT=$now"
    echo "NOTIFIED_AT=$NOTIFIED_AT"
} > "$state_file"

human_now=$(date -j -f %s "$now" '+%H:%M:%S' 2>/dev/null || date '+%H:%M:%S')
case "$new_status" in
    up)
        echo "$human_now  kimi: UP"
        exit 0
        ;;
    down)
        if [ -n "$FIRST_DOWN_AT" ]; then
            down_for=$(( (now - FIRST_DOWN_AT) / 60 ))
            eta=$(date -j -f %s $((FIRST_DOWN_AT + 5*3600)) '+%H:%M' 2>/dev/null || echo "?")
            echo "$human_now  kimi: DOWN (429) -- down ${down_for}m, ETA ~$eta (5h-from-first-429 estimate, not authoritative)"
        else
            echo "$human_now  kimi: DOWN (429)"
        fi
        exit 1
        ;;
    *)
        echo "$human_now  kimi: UNKNOWN -- probe produced neither a completed turn nor a 429; see $out" >&2
        cat "$out" >&2
        exit 2
        ;;
esac
