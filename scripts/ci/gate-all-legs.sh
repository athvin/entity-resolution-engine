#!/usr/bin/env bash
# The matrix-aggregate gate step: every leg must have succeeded.
#
# A matrix job's per-leg names can never be required status checks, so each
# sharded tier hangs a single aggregate gate job off its legs and THAT is the
# required context (rust.yml's `test-gate` and `ducklake-gate`; infra.yml and
# frontend.yml carry the same shape). The gate job must run with
# `!cancelled()` — a gate skipped because its `needs` failed would read to
# branch protection as satisfied, the exact false-green it exists to prevent.
#
# Input: GATE_LEGS, one `name=result` line per upstream job, where result is
# the `needs.<job>.result` aggregate: `success` only if every leg succeeded,
# `failure` if any failed, `skipped` if the whole job was.
#
# Run (CI):
#   env:
#     GATE_LEGS: |
#       test-shard=${{ needs.test-shard.result }}
#   run: bash scripts/ci/gate-all-legs.sh
set -uo pipefail

if [ -z "${GATE_LEGS:-}" ]; then
	echo "::error::GATE_LEGS is empty — a gate that checks nothing is a false green" >&2
	exit 2
fi

fail=0
while IFS='=' read -r name result; do
	[ -n "$name" ] || continue
	echo "$name: $result"
	case "$result" in
	success) ;;
	skipped)
		# Defensive only: the legs skip exactly when detect says
		# code != 'true', and then the gate job is itself skipped. A leg
		# reported skipped alongside a running gate means detect gated it.
		echo "$name reported skipped alongside a running gate"
		;;
	*)
		echo "::error::leg '$name' did not succeed (result: $result)"
		fail=1
		;;
	esac
done <<<"$GATE_LEGS"

if [ "$fail" -ne 0 ]; then
	exit 1
fi
echo "all legs succeeded"
