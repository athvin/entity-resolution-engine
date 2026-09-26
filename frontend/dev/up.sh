#!/usr/bin/env bash
# Boot the frontend dev stack: containers (Postgres x2 + MinIO), erweb migrations,
# then the erserver API (:8000) and dispatcher as host processes. Blocks until
# Ctrl+C, which stops the host processes; containers stay up (make frontend-dev-reset
# tears everything down, volumes included).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
COMPOSE=(docker compose -f "${SCRIPT_DIR}/compose.yaml")

source "${SCRIPT_DIR}/env.sh"

mkdir -p "${STATE_DIR}/configs" "${STATE_DIR}/drop" "${STATE_DIR}/ext" "${STATE_DIR}/logs"

echo "==> starting containers"
"${COMPOSE[@]}" up -d controlplane-db catalog-db objectstore

echo "==> waiting for postgres"
for service in controlplane-db catalog-db; do
  for _ in $(seq 1 60); do
    "${COMPOSE[@]}" exec -T "$service" pg_isready -U postgres >/dev/null 2>&1 && break
    sleep 1
  done
done

echo "==> waiting for minio"
for _ in $(seq 1 60); do
  curl -sf http://localhost:9000/minio/health/live >/dev/null 2>&1 && break
  sleep 1
done

echo "==> creating bucket"
"${COMPOSE[@]}" run --rm objectstore-init

echo "==> applying erweb migrations"
(cd "${REPO_ROOT}/frontend" && corepack pnpm db:migrate)

echo "==> starting erserver api (:8000; logs: frontend/dev/.state/logs/)"
cd "${REPO_ROOT}"
uv run --project server uvicorn --factory erserver.api:create_app --port 8000 \
  >"${STATE_DIR}/logs/api.log" 2>&1 &
API_PID=$!
DISPATCHER_PID=""

cleanup() {
  echo "==> stopping erserver processes"
  kill ${API_PID} ${DISPATCHER_PID} 2>/dev/null || true
  wait ${API_PID} ${DISPATCHER_PID} 2>/dev/null || true
}
trap cleanup EXIT INT TERM

# Generous: the first `uv run` of a session can spend tens of seconds resolving
# the server environment before uvicorn ever binds.
for _ in $(seq 1 90); do
  curl -sf http://localhost:8000/healthz >/dev/null 2>&1 && break
  sleep 1
done
curl -sf http://localhost:8000/healthz >/dev/null || {
  echo "erserver api did not become healthy; see ${STATE_DIR}/logs/api.log" >&2
  exit 1
}

# Strictly after the API is healthy: on a first boot both processes run
# ensure_schema, and the dispatcher's long-lived leader connection blocks the
# API's DDL indefinitely when they start together.
echo "==> starting dispatcher"
uv run --project server python -m erserver.dispatcher \
  >"${STATE_DIR}/logs/dispatcher.log" 2>&1 &
DISPATCHER_PID=$!

echo "==> dev stack ready"
echo "    api:        http://localhost:8000 (operator token: \$ERSERVER_OPERATOR_TOKEN)"
echo "    seed:       make frontend-seed"
echo "    web:        cd frontend && corepack pnpm dev"
echo "    Ctrl+C stops the erserver processes; containers keep running."
wait
