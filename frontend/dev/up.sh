#!/usr/bin/env bash
# Boot the frontend dev stack: the control-plane Postgres container, the erweb
# migrations, then er-server (:8000) as a host process. Blocks until Ctrl+C,
# which stops er-server; the container stays up (`make frontend-dev-reset`
# tears everything down, volumes included).
#
# One host process, not two. `ER_DISPATCH_IN_PROCESS=1` makes the er-server
# binary drain the job queue in a background task, which is the merged `web-api`
# topology the Helm chart ships. Two consequences worth knowing:
#
#   * The old API-strictly-before-dispatcher dance is gone. It existed because
#     the Python API and dispatcher raced `ensure_schema` on a first boot and
#     the dispatcher's long-lived leader connection blocked the API's DDL
#     forever. `er_server::pg::ensure_schema` now opens with
#     `pg_advisory_xact_lock(hashtext('erserver.ensure_schema'), 0)`, so
#     concurrent ensures are safe — and in-process there is only one anyway.
#
#   * The half-up state run.sh used to detect — a live API with no dispatcher,
#     so every run sits in `queued` forever — is no longer reachable. If the
#     API answers, the queue is being drained by the same process.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
COMPOSE=(docker compose -f "${SCRIPT_DIR}/compose.yaml")

source "${SCRIPT_DIR}/env.sh"

mkdir -p "${CONFIG_DIR}" "${DROP_DIR}" "${LAKE_DIR}" "${STATE_DIR}/logs"

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

cleanup() {
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

echo "==> dev stack ready"
echo "    api:   http://localhost:8000 (operator token: \$ER_OPERATOR_TOKEN)"
echo "    seed:  make frontend-seed"
echo "    web:   cd frontend && corepack pnpm dev"
echo "    Ctrl+C stops er-server; the container keeps running."
wait
