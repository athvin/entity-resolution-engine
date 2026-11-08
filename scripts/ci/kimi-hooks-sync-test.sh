#!/usr/bin/env bash
# Self-test for kimi-hooks-sync.sh — installing this repo's PreToolUse guard
# into `$KIMI_CODE_HOME/config.toml`.
#
# Three things here are load-bearing and none of them are style:
#
#   1. THE BLOCK MAY CARRY ONLY FOUR FIELDS. Kimi's `[[hooks]]` tables accept
#      `event`, `matcher`, `command` and `timeout`, and an extra field fails the
#      ENTIRE config load — taking the provider and model tables with it, so the
#      developer's Kimi install stops working rather than just losing a hook.
#      A marker field inside the table is therefore impossible, which is why the
#      block is delimited by comments and why that is asserted here.
#   2. THE DENY LIST MUST DERIVE NON-EMPTY. It is read out of
#      `.claude/settings.json` at runtime, so a restructured settings file would
#      install a guard that matches nothing — and the runtime symptom is a
#      command running normally, which looks exactly like success.
#   3. THE GUARD MUST ONLY EVER SAY `deny`. Measured against Kimi Code CLI
#      2.5.2: a `permissionDecision: ask` verdict ALLOWED the command (the
#      sentinel `touch` landed), as did any non-2 error exit. `ask` is not a
#      weaker deny on this agent any more than it is on Codex.
#
# `kimi` is never invoked, so this runs on a runner with no Kimi install.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
sync="$root/scripts/kimi-hooks-sync.sh"
guard="$root/scripts/hooks/agent-pretooluse-guard.sh"
settings="$root/.claude/settings.json"

for f in "$sync" "$guard" "$settings"; do
	[ -f "$f" ] || {
		printf 'kimi-hooks-sync-test: %s is missing (renamed?)\n' "$f" >&2
		exit 2
	}
done
command -v jq >/dev/null 2>&1 || {
	echo 'kimi-hooks-sync-test: jq is required' >&2
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

# A config with hand-written content on both sides of where ours lands.
seed_config() {
	mkdir -p "$1"
	cat >"$1/config.toml" <<'EOF'
default_model = "kimi-code/k3"

[[hooks]]
event = "Notification"
matcher = "task\\.completed"
command = "true"
EOF
}

home="$tmp/home"
seed_config "$home"
cp "$home/config.toml" "$tmp/original.toml"

# --- install -----------------------------------------------------------------
if bash "$sync" --root "$root" --kimi-home "$home" >"$tmp/out" 2>"$tmp/err"; then
	ok "installs without error"
else
	no "install failed" "$(cat "$tmp/err")"
fi

n=$(grep -c 'event = "PreToolUse"' "$home/config.toml" || true)
if [ "${n:-0}" -eq 1 ]; then
	ok "exactly one PreToolUse block is installed"
else
	no "wrong number of PreToolUse blocks" "found ${n:-0}"
fi

if grep -q 'matcher = "Bash"' "$home/config.toml"; then
	ok "the matcher targets the Bash tool"
else
	no "the matcher is not Bash" "PreToolUse matches on tool NAME"
fi

if grep -q "agent-pretooluse-guard.sh" "$home/config.toml"; then
	ok "the command points at the shared guard"
else
	no "the installed command does not reference the guard"
fi

# --- 1. ONLY the four allowed fields ----------------------------------------
# Read the keys of our managed block and compare against Kimi's whole allowed
# set. An extra key here does not degrade the hook — it breaks the config.
keys=$(awk '
	/^# >>> er-kimi-pretooluse-guard/ { inblock = 1; next }
	/^# <<< er-kimi-pretooluse-guard/ { inblock = 0; next }
	inblock && /^[a-z_]+ *=/ { sub(/ *=.*/, ""); print }
' "$home/config.toml" | sort | tr '\n' ',')
if [ "$keys" = "command,event,matcher,timeout," ]; then
	ok "the block carries only Kimi's four allowed [[hooks]] fields"
else
	no "the block's field set is not the four allowed ones" \
		"got '$keys' — an extra field fails the WHOLE config load"
fi

# --- hand-written content survives ------------------------------------------
if grep -q 'default_model = "kimi-code/k3"' "$home/config.toml" &&
	grep -q 'event = "Notification"' "$home/config.toml"; then
	ok "hand-written config and an unrelated hook both survive"
else
	no "the install clobbered existing config"
fi

# --- idempotence -------------------------------------------------------------
bash "$sync" --root "$root" --kimi-home "$home" >/dev/null 2>&1
bash "$sync" --root "$root" --kimi-home "$home" >/dev/null 2>&1
n=$(grep -c 'event = "PreToolUse"' "$home/config.toml" || true)
m=$(grep -c 'er-kimi-pretooluse-guard' "$home/config.toml" || true)
if [ "${n:-0}" -eq 1 ] && [ "${m:-0}" -eq 2 ]; then
	ok "three installs leave one block and one marker pair"
else
	no "install is not idempotent" "blocks=${n:-0} markers=${m:-0}"
fi

# --- --remove restores the file exactly -------------------------------------
bash "$sync" --root "$root" --kimi-home "$home" --remove >/dev/null 2>&1
if diff -q "$tmp/original.toml" "$home/config.toml" >/dev/null 2>&1; then
	ok "--remove restores the original file byte-for-byte"
else
	no "--remove left the file changed" "$(diff "$tmp/original.toml" "$home/config.toml" | head -5)"
fi

# --- 2. the deny-list derivation must be non-empty --------------------------
real_count=$(jq -r '[.permissions.deny[]? | select(startswith("Bash("))] | length' "$settings")
if [ "${real_count:-0}" -gt 0 ]; then
	ok "the repo's settings.json still yields $real_count Bash(...) deny pattern(s)"
else
	no "no Bash(...) entries under .permissions.deny" "the guard would match nothing"
fi

echo '{ "permissions": { "deny": ["Read(~/.ssh/**)"] } }' >"$tmp/no-bash.json"
rm -rf "$tmp/home2"
if bash "$sync" --root "$root" --kimi-home "$tmp/home2" --settings "$tmp/no-bash.json" \
	>/dev/null 2>"$tmp/err"; then
	no "installed against a settings file with no Bash(...) deny entries" \
		"an empty deny list is not a guard, and the runtime symptom is silence"
else
	ok "refuses to install when the deny list would derive empty"
fi

# --- 3. the guard only ever emits `deny` ------------------------------------
# `ask` and `allow` both run the command on Kimi and on Codex, so a guard that
# emitted either would be decoration. Assert the string does not appear as a
# verdict the guard produces.
if grep -qE 'permissionDecision: *"(ask|allow)"' "$guard"; then
	no "the guard emits an 'ask' or 'allow' verdict" \
		"both are ALLOWS on Kimi and Codex — measured, not assumed"
else
	ok "the guard never emits 'ask' or 'allow', only 'deny'"
fi

if grep -q 'ER_GUARD_ASK_AS_DENY=1' "$guard"; then
	ok "the guard still collapses aws-write-guard's 'ask' to a deny"
else
	no "ER_GUARD_ASK_AS_DENY=1 is gone from the guard" \
		"without it the aws table's whole middle case fails open"
fi

# --- a missing guard or settings file is a hard failure ---------------------
rm -rf "$tmp/home3"
if bash "$sync" --root "$tmp/not-a-repo" --kimi-home "$tmp/home3" >/dev/null 2>&1; then
	no "installed with a bogus --root" "the guard path would not exist"
else
	ok "a root with no guard script fails rather than installing a dead command"
fi

# --- --dry-run writes nothing ------------------------------------------------
rm -rf "$tmp/home4"
bash "$sync" --root "$root" --kimi-home "$tmp/home4" --dry-run >/dev/null 2>&1
if [ -e "$tmp/home4/config.toml" ]; then
	no "--dry-run wrote the config"
else
	ok "--dry-run writes nothing"
fi

printf '\nkimi-hooks-sync-test: %s passed, %s failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ] || exit 1
