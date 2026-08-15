#!/usr/bin/env bash
# Prove the Compose substrate starts, is usable, and always tears itself down.
#
# This is the cheapest end-to-end check of the six B1 defects: MinIO credentials that
# satisfy the 3/8-character minimums and reach the init container, the full ER_*
# environment on `pipeline`, the artifacts bind mount, the fixed project name with a
# locally built image, and `run --rm` rather than `up`. It
# runs before the integration suite because a substrate that never came up produces a
# pytest failure whose message is about the lake rather than about the stack (S7.4).
#
# `run --rm` throughout: the abort-on-exit flag S7.4 forbids is named in no file under
# docker/ or scripts/ci/, and tests/unit/test_compose_contract.py greps for it.
#
# Three phases, in order:
#   1. render the Compose model, reset the stack and build the image;
#   2. `run --rm pipeline` a probe that writes through the artifacts bind mount --
#      reaching it at all proves `catalog` went healthy and `objectstore-init`
#      created the bucket, because those are the conditions `pipeline` gates on;
#   3. `run --rm pipeline` a deliberately failing command, and assert both that its
#      status propagates and that `down -v --remove-orphans` leaves no container
#      behind -- the failure path is the one that silently leaked containers before.
#
# Usage: scripts/ci/compose_smoke.sh
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
COMPOSE=(docker compose -f "${REPO_ROOT}/docker/compose.yaml" --profile test)

# Any status the failure phase could plausibly collide with would make the assertion
# vacuous; 7 is not produced by pytest, by Compose, or by a shell builtin failure.
readonly FAILING_STATUS=7
PROBE="${REPO_ROOT}/artifacts/compose_smoke.ok"

die() {
    printf 'compose_smoke: %s\n' "$*" >&2
    exit 1
}

teardown() {
    "${COMPOSE[@]}" down -v --remove-orphans >/dev/null 2>&1
}
trap teardown EXIT

# ---- phase 1: render and reset ----
mkdir -p "${REPO_ROOT}/artifacts" || die "cannot create ${REPO_ROOT}/artifacts"
rm -f "${PROBE}"

"${COMPOSE[@]}" config --quiet || die "docker/compose.yaml does not render"
teardown
"${COMPOSE[@]}" build pipeline || die "docker compose build pipeline failed"

# ---- phase 2: the substrate is usable ----
"${COMPOSE[@]}" run --rm pipeline sh -c 'printf ok > /app/artifacts/compose_smoke.ok' ||
    die "run --rm pipeline failed; the substrate did not come up"
[[ -s ${PROBE} ]] ||
    die "nothing reached ${PROBE}; the ../artifacts bind mount is not wired to the host"
rm -f "${PROBE}"

# ---- phase 3: a failing command is reported as such, and leaves nothing behind ----
"${COMPOSE[@]}" run --rm pipeline sh -c "exit ${FAILING_STATUS}"
status=$?
[[ ${status} -eq ${FAILING_STATUS} ]] ||
    die "run --rm returned ${status} for a command that exited ${FAILING_STATUS}"

"${COMPOSE[@]}" down -v --remove-orphans || die "down -v --remove-orphans failed"
remaining="$("${COMPOSE[@]}" ps --all --quiet)"
[[ -z ${remaining} ]] || die "containers survived teardown: ${remaining}"

printf 'compose_smoke: ok\n'
