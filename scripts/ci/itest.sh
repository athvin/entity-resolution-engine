#!/usr/bin/env bash
# Run the integration suite on the Compose substrate and return pytest's own status.
#
# This is the `test` profile entry point for CI (S9.1) and for a developer who wants
# the same substrate locally (M22): it builds `er-pipeline:ci`, resets the stack,
# runs pytest inside the `pipeline` container and tears the stack down on every exit
# path -- success, failure, or interrupt -- via the trap below (S7.4).
#
# Two properties are load-bearing and are asserted by ER-009's acceptance criteria.
# The stack is driven with `run --rm`, never `up`, because the one-shot initialisers
# make an `up` exit code a teardown signal rather than a test result. And the exit
# status this script returns is pytest's, not `down`'s: a suite that failed must not
# be reported green because the teardown after it succeeded.
#
# Usage: scripts/ci/itest.sh [pytest args...]        # default: the whole suite
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
COMPOSE=(docker compose -f "${REPO_ROOT}/docker/compose.yaml" --profile test)

die() {
    printf 'itest: %s\n' "$*" >&2
    exit 1
}

teardown() {
    "${COMPOSE[@]}" down -v --remove-orphans >/dev/null 2>&1
}
trap teardown EXIT

pytest_args=("$@")
if [[ ${#pytest_args[@]} -eq 0 ]]; then
    pytest_args=(tests/integration)
fi

# The bind mount's host side must exist before the first `run`, or Docker creates it
# root-owned and the junit XML never reaches the runner's artifact upload (M24, S7.4).
mkdir -p "${REPO_ROOT}/artifacts" || die "cannot create ${REPO_ROOT}/artifacts"

# `down -v` before as well as after: a local run starts from the same empty state a
# fresh CI runner has, which is the whole point of M22.
teardown

"${COMPOSE[@]}" build pipeline || die "docker compose build pipeline failed"

"${COMPOSE[@]}" run --rm pipeline pytest "${pytest_args[@]}" \
    --junitxml=/app/artifacts/junit.xml --durations=20
status=$?

exit "${status}"
