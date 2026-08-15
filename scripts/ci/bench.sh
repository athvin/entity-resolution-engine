#!/usr/bin/env bash
# The itest.sh shape under the `bench` profile (S9.2, S10).
#
# Same substrate, same teardown discipline, same exit-status propagation; only the
# profile, the service and the artifact subdirectory differ. With no arguments the
# `benchmark` service runs its own command -- `benchmarks/report.py --run --scale
# ${BENCH_SCALE:-smoke}` -- so the scale is chosen by exporting BENCH_SCALE, which is
# what the S9.2 preflight does after it reads the scale's row from S10.2. Arguments,
# when given, replace that command.
#
# Usage: scripts/ci/bench.sh [command args...]       # BENCH_SCALE=<scale> to choose a scale
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
COMPOSE=(docker compose -f "${REPO_ROOT}/docker/compose.yaml" --profile bench)

die() {
    printf 'bench: %s\n' "$*" >&2
    exit 1
}

teardown() {
    "${COMPOSE[@]}" down -v --remove-orphans >/dev/null 2>&1
}
trap teardown EXIT

# `artifacts/bench/` is where the service writes latest.json, so it has to exist on
# the host side of the bind mount before the container opens the file (M24).
mkdir -p "${REPO_ROOT}/artifacts/bench" || die "cannot create ${REPO_ROOT}/artifacts/bench"

teardown

"${COMPOSE[@]}" build benchmark || die "docker compose build benchmark failed"

"${COMPOSE[@]}" run --rm benchmark "$@"
status=$?

exit "${status}"
