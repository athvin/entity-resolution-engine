#!/usr/bin/env bash
# Self-test for alert-path-check.sh.
#
# A detector breaks by noticing nothing, which reads exactly like a healthy
# account — and this one guards a path that was already silently dead for a
# week. So the cases that matter most here are the FAILING ones: a check that
# cannot fail is worse than no check, because it reads as cover.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHECK="${SCRIPT_DIR}/alert-path-check.sh"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

pass=0
fail=0

# run <name> <expected-exit> <json>
run() {
	local name="$1" want="$2" body="$3" out got
	printf '%s' "$body" >"$TMP/subs.json"
	out="$(bash "$CHECK" "$TMP/subs.json" 2>&1)"
	got=$?
	if [ "$got" -eq "$want" ]; then
		pass=$((pass + 1))
		printf 'ok   %s\n' "$name"
	else
		fail=$((fail + 1))
		printf 'FAIL %s — wanted exit %s, got %s\n%s\n' "$name" "$want" "$got" "$out"
	fi
}

# grep_out <name> <json> <pattern>
grep_out() {
	local name="$1" body="$2" pattern="$3" out
	printf '%s' "$body" >"$TMP/subs.json"
	out="$(bash "$CHECK" "$TMP/subs.json" 2>&1)"
	if printf '%s' "$out" | grep -q -- "$pattern"; then
		pass=$((pass + 1))
		printf 'ok   %s\n' "$name"
	else
		fail=$((fail + 1))
		printf 'FAIL %s — output did not contain %s\n%s\n' "$name" "$pattern" "$out"
	fi
}

CONFIRMED='{"Subscriptions":[{"Protocol":"email","Endpoint":"ops@example.com","SubscriptionArn":"arn:aws:sns:us-east-2:1:er-alerts:abc-123"}]}'
# The exact shape the live account returned on 2026-11-17 (#1107).
PENDING='{"Subscriptions":[{"Protocol":"email","Endpoint":"dupezone813@gmail.com","SubscriptionArn":"PendingConfirmation"}]}'
EMPTY='{"Subscriptions":[]}'
MIXED='{"Subscriptions":[{"Protocol":"email","Endpoint":"ops@example.com","SubscriptionArn":"arn:aws:sns:us-east-2:1:er-alerts:abc-123"},{"Protocol":"email","Endpoint":"second@example.com","SubscriptionArn":"PendingConfirmation"}]}'

run "a confirmed subscriber passes" 0 "$CONFIRMED"

# THE case this gate was written for. If this ever starts passing, the gate has
# stopped detecting the only incident it has ever had to detect.
run "the live 2026-11-17 shape (one pending, none confirmed) FAILS" 1 "$PENDING"

run "no subscriptions at all FAILS" 1 "$EMPTY"

# Deliberately a pass: the path already terminates at a human, and failing here
# would make adding a second destination turn the schedule red.
run "confirmed + pending passes" 0 "$MIXED"

run "a non-JSON document is a usage error, not a pass" 2 "not json at all"

# An ABSENT Subscriptions key must NOT read as "no subscribers". If AWS renames
# the field, a naive check reports the alarm path dead and sends someone after a
# problem that does not exist. Absent is a usage error; empty is a verdict.
run "a document with no Subscriptions key is a usage error" 2 '{"Nope":true}'
run "Subscriptions present but not an array is a usage error" 2 '{"Subscriptions":"nope"}'

grep_out "the failure names the remedy (#1107)" "$PENDING" "#1107"
grep_out "the failure is loud about what it means" "$PENDING" "THE ALARM PATH IS DEAD"
grep_out "a pending subscriber is reported even when passing" "$MIXED" "still pending"

# Endpoints reach a public repository's CI log, so they must be masked.
grep_out "the endpoint local part is redacted" "$PENDING" 'd\*\*\*@gmail.com'

if printf '%s' "$(bash "$CHECK" <<<'' 2>&1 || true)" | grep -q "usage"; then
	pass=$((pass + 1))
	printf 'ok   %s\n' "no argument is a usage error"
else
	fail=$((fail + 1))
	printf 'FAIL %s\n' "no argument is a usage error"
fi

# A missing file must not be mistaken for a healthy path.
out="$(bash "$CHECK" "$TMP/does-not-exist.json" 2>&1)"
if [ $? -ne 0 ] && printf '%s' "$out" | grep -q "no such file"; then
	pass=$((pass + 1))
	printf 'ok   %s\n' "a missing file is refused"
else
	fail=$((fail + 1))
	printf 'FAIL %s\n' "a missing file is refused"
fi

printf '\nalert-path-check-test: %d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
