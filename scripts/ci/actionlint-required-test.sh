#!/usr/bin/env bash
# Self-test for actionlint.sh's ACTIONLINT_REQUIRED branch.
#
# WHY THIS ONE BRANCH GETS A TEST. It is the fix for a gate that reported
# success for months while running nothing: `taiki-e/install-action` could never
# install actionlint, `continue-on-error: true` hid that, and actionlint.sh's
# "binary not on PATH" path printed a line and exited 0. Absence was
# indistinguishable from a clean lint. The branch that now distinguishes them is
# the one thing standing between this repo and that state again, so it is
# asserted in BOTH directions rather than trusted.
#
# The binary is hidden by running actionlint.sh with a PATH that cannot reach
# it, which is how CI's "it failed to install" looks from inside the script.
# Nothing here installs, uninstalls or downloads anything.
#
# Run: bash scripts/ci/actionlint-required-test.sh
# Exit: 0 all cases passed · 1 a case failed
set -uo pipefail

cd "$(dirname "$0")/../.." || exit 2

GATE=scripts/ci/actionlint.sh
pass=0
fail=0

# A PATH holding only the core utilities the script needs, and NOT actionlint.
# /usr/bin and /bin carry grep, find, sort, awk, head, printf; neither carries
# actionlint on a GitHub runner or a developer machine.
NOLINT_PATH=/usr/bin:/bin

check() {
	local name=$1 want=$2 got=$3 out=$4
	if [ "$got" = "$want" ]; then
		pass=$((pass + 1))
		printf 'ok   %s\n' "$name"
	else
		fail=$((fail + 1))
		printf 'FAIL %s\n       want exit: %s\n        got exit: %s\n        output: %s\n' \
			"$name" "$want" "$got" "$out"
	fi
}

# --- REQUIRED and absent must FAIL -----------------------------------------
# The case that matters. If this passes-as-zero, the gate is back to its old
# behaviour and nothing else in this file is worth reading.
out=$(PATH="$NOLINT_PATH" ACTIONLINT_REQUIRED=1 bash "$GATE" 2>&1)
got=$?
check 'required + absent exits non-zero' 1 "$got" "$out"

# And it must say WHY, not just fail. A bare non-zero here sends the reader to
# the pin check, which is the half that was working all along.
if printf '%s' "$out" | grep -q 'REQUIRED but not on PATH'; then
	pass=$((pass + 1))
	printf 'ok   required + absent names the real cause\n'
else
	fail=$((fail + 1))
	printf 'FAIL required + absent should name the cause\n        output: %s\n' "$out"
fi

# The pin check must still have run and reported. The linting half failing must
# not swallow the half that works.
if printf '%s' "$out" | grep -qE '`uses:` references across .* workflow files'; then
	pass=$((pass + 1))
	printf 'ok   the pin check still runs and reports when linting is absent\n'
else
	fail=$((fail + 1))
	printf 'FAIL the pin check should still report\n        output: %s\n' "$out"
fi

# --- NOT required and absent must PASS -------------------------------------
# A developer with no actionlint installed still gets a usable pin check. If
# this direction breaks, the fix has made the script unrunnable locally, and
# people stop running it — the same end state by a different route.
out=$(PATH="$NOLINT_PATH" bash "$GATE" 2>&1)
got=$?
check 'absent without the flag still exits 0' 0 "$got" "$out"

if printf '%s' "$out" | grep -q 'workflow linting skipped'; then
	pass=$((pass + 1))
	printf 'ok   the local skip still says it skipped\n'
else
	fail=$((fail + 1))
	printf 'FAIL the local skip should say so\n        output: %s\n' "$out"
fi

# --- REQUIRED and present must behave exactly as unset ---------------------
# Only runnable where actionlint is genuinely installed. Skipped rather than
# faked: a stub named `actionlint` would assert the stub, not the linter.
if command -v actionlint >/dev/null 2>&1; then
	out=$(ACTIONLINT_REQUIRED=1 bash "$GATE" 2>&1)
	got=$?
	check 'required + present behaves like a normal run' 0 "$got" "$out"
else
	printf 'skip required + present — no actionlint on PATH here\n'
fi

# --- A suite that asserted nothing is not a passing suite ------------------
if [ "$((pass + fail))" -lt 5 ]; then
	printf 'FAIL the suite ran %s cases; it should run at least 5.\n' "$((pass + fail))"
	fail=$((fail + 1))
fi

printf '\nactionlint-required-test: %s passed, %s failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ] || exit 1
