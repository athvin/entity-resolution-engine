#!/usr/bin/env bash
# Boot the frontend dev stack: the control-plane Postgres container, the erweb
# migrations, then er-server (:8000) and er-dispatcher as host processes. Blocks
# until Ctrl+C, which stops both; the container stays up
# (`make frontend-dev-reset` tears everything down, volumes included).
#
# TWO host processes: er-server and er-dispatcher. That is the topology the Helm
# chart ships — a separate dispatcher Deployment beside the API — and the only
# one that survives Phase 4 of the batch-orchestration design, which deletes
# `ER_DISPATCH_IN_PROCESS` along with the legacy in-process arm. Local dev
# should not be the last place a retired topology lives on.
#
# Two consequences worth knowing:
#
#   * No API-before-dispatcher ordering is required, and that is a property
#     rather than an assumption. The Python stack needed the dance because its
#     API and dispatcher raced `ensure_schema` on a first boot and the
#     dispatcher's long-lived leader connection blocked the API's DDL forever.
#     `er_server::pg::ensure_schema` now opens with
#     `pg_advisory_xact_lock(hashtext('erserver.ensure_schema'), 0)`, so
#     concurrent ensures serialise instead of deadlocking.
#
#   * The half-up state is REACHABLE AGAIN, so it is checked again. With the
#     dispatcher in-process, a live API implied a draining queue and the check
#     was deleted as unreachable; with two processes a dispatcher can die and
#     leave every job in `queued` forever while the API still answers. This
#     script asserts the dispatcher survived boot, below.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
COMPOSE=(docker compose -f "${REPO_ROOT}/frontend/dev/compose.yaml")

source "${SCRIPT_DIR}/env.sh"

mkdir -p "${CONFIG_DIR}" "${DROP_DIR}" "${LAKE_DIR}" "${ER_PROVISION_ROOT}" "${STATE_DIR}/logs"

echo "==> starting the control-plane container"
"${COMPOSE[@]}" up -d controlplane-db

echo "==> waiting for postgres"
for _ in $(seq 1 60); do
    "${COMPOSE[@]}" exec -T controlplane-db pg_isready -U postgres >/dev/null 2>&1 && break
    sleep 1
done
"${COMPOSE[@]}" exec -T controlplane-db pg_isready -U postgres >/dev/null 2>&1 ||
    { echo "controlplane-db did not become ready" >&2; exit 1; }

echo "==> applying erweb migrations"
(cd "${REPO_ROOT}/frontend" && corepack pnpm db:migrate)

# `er-server migrate` ahead of the API, so the control-plane schema and the
# DDL-ledger event triggers exist before anything serves. The API would ensure
# the schema on its own at boot, but doing it here means a migration failure is
# reported as a migration failure rather than as an unhealthy API.
echo "==> applying the control-plane schema"
(cd "${REPO_ROOT}/rust" && cargo run -q -p er-server --bin er-server -- migrate)

echo "==> starting er-server (:8000; logs: frontend/dev/.state/logs/)"
cd "${REPO_ROOT}/rust"
cargo run -q -p er-server --bin er-server >"${STATE_DIR}/logs/api.log" 2>&1 &
API_PID=$!

# Its own process, mirroring the chart's separate dispatcher Deployment. Started
# without waiting for the API: see the header on why no ordering is needed. It
# reads the same `ER_PG_DSN` env.sh already exports for er-server.
echo "==> starting er-dispatcher"
cargo run -q -p er-server --bin er-dispatcher >"${STATE_DIR}/logs/dispatcher.log" 2>&1 &
DISPATCHER_PID=$!

cleanup() {
    echo "==> stopping er-dispatcher"
    kill ${DISPATCHER_PID} 2>/dev/null || true
    wait ${DISPATCHER_PID} 2>/dev/null || true
    echo "==> stopping er-server"
    kill ${API_PID} 2>/dev/null || true
    wait ${API_PID} 2>/dev/null || true
}
trap cleanup EXIT INT TERM

# Generous: a cold `cargo run` has to build er-server and its bundled DuckDB,
# which is minutes on a first boot and seconds afterwards.
for _ in $(seq 1 600); do
    curl -sf http://localhost:8000/healthz >/dev/null 2>&1 && break
    kill -0 ${API_PID} 2>/dev/null ||
        { echo "er-server exited; see ${STATE_DIR}/logs/api.log" >&2; exit 1; }
    sleep 1
done
curl -sf http://localhost:8000/healthz >/dev/null || {
    echo "er-server did not become healthy; see ${STATE_DIR}/logs/api.log" >&2
    exit 1
}

# The half-up state is REACHABLE AGAIN, so it is checked again.
#
# With the dispatcher in-process, "the API answers" implied "the queue is being
# drained" and this check was deleted as unreachable. Two processes bring the
# state back: a live API whose dispatcher died leaves every job sitting in
# `queued` forever, and nothing about the API says so. That silence is the
# original bug `run.sh` was written to name, so it is named here rather than
# rediscovered.
#
# The dispatcher is a daemon with no port, so liveness is the process plus the
# absence of an early exit. It shares the API's generous window because it pays
# the same cold `cargo run`.
if ! kill -0 ${DISPATCHER_PID} 2>/dev/null; then
    echo "er-dispatcher exited during boot; see ${STATE_DIR}/logs/dispatcher.log" >&2
    tail -20 "${STATE_DIR}/logs/dispatcher.log" >&2 2>/dev/null || true
    exit 1
fi

echo "==> dev stack ready"
echo "    api:   http://localhost:8000 (operator token: \$ER_OPERATOR_TOKEN)"
echo "    seed:  make frontend-seed"
echo "    web:   cd frontend && corepack pnpm dev"
echo "    Ctrl+C stops er-server and er-dispatcher; the container keeps running."
wait
