#!/bin/sh
# Existing local recovery watcher; scheduling is managed outside this repo. Probes the shared Kimi K3 quota via
# kimi-quota-check.sh, logs every run, and on the down -> up transition sends
# the fleet-coordination context message to pane `codex-1` exactly once
# (gated by NOTIFIED_AT in the shared state file, so re-running after
# recovery does not re-send it).
#
# Requires a named `codex-1` agent in the configured session.
# State and logs stay outside the repository.
set -eu

dir="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
state_dir="${HERDR_STATE_DIR:-$HOME/.local/state/herdr}"
mkdir -p "$state_dir"
log_file="$state_dir/codex1-watch.log"
state_file="$state_dir/kimi-quota.env"

export HERDR_SOCKET_PATH="${HERDR_SOCKET_PATH:-$HOME/.config/herdr/herdr.sock}"
herdr_bin="${HERDR_BIN_PATH:-$HOME/.local/bin/herdr}"

line="$(sh "$dir/kimi-quota-check.sh" 2>&1)" || true
echo "$line" >> "$log_file"

# shellcheck disable=SC1090
STATUS="" NOTIFIED_AT=""
[ -f "$state_file" ] && . "$state_file"

if [ "$STATUS" = "up" ] && [ -z "$NOTIFIED_AT" ]; then
    message='Fleet coordination update: a Claude Code session (herdr name `coordinator`, this workspace) is now acting as head decision maker for this fleet. You were rate-limited (429, Kimi K3 quota) and are being contacted the moment an automated probe confirmed the API answers again. How to reach me: herdr agent prompt coordinator "<message>" --wait --timeout 120000 -- check my status first with herdr agent get coordinator or herdr agent read coordinator --source recent-unwrapped --lines 60. I will check on you periodically and flag you to the user if you stall again. Reply with a one-line status (current task + health) so I know you are really back.'

    if "$herdr_bin" agent prompt codex-1 "$message" > "$state_dir/last-notify-response.json" 2>&1; then
        now=$(date +%s)
        sed -i '' "s/^NOTIFIED_AT=.*/NOTIFIED_AT=$now/" "$state_file"
        echo "$(date '+%H:%M:%S')  notified codex-1 of recovery" >> "$log_file"
    else
        echo "$(date '+%H:%M:%S')  recovery detected but herdr agent prompt to codex-1 FAILED -- see $state_dir/last-notify-response.json" >> "$log_file"
    fi
fi

if [ "$STATUS" = "down" ]; then
    # Still down: clear any stale NOTIFIED_AT from a previous up window so the
    # *next* recovery sends the message again instead of staying silent.
    if [ -n "$NOTIFIED_AT" ]; then
        sed -i '' "s/^NOTIFIED_AT=.*/NOTIFIED_AT=/" "$state_file"
    fi
fi
