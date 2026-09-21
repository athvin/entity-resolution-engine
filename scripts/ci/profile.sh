#!/usr/bin/env bash
# Isolated profiling campaign; never tears down the developer's normal `er` stack.
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SESSION="$(date -u +%Y%m%dt%H%M%Sz)-${RANDOM}"
PROJECT="er-profile-${SESSION}"
OUT="${REPO_ROOT}/artifacts/profile/${SESSION}"
mkdir -p "$OUT"
COMPOSE=(docker compose -p "$PROJECT" -f "${REPO_ROOT}/docker/compose.yaml" --profile bench)
STATS_PID=""
cleanup() {
  if [[ -n "$STATS_PID" ]]; then
    kill "$STATS_PID" 2>/dev/null || true
    wait "$STATS_PID" 2>/dev/null || true
  fi
  "${COMPOSE[@]}" logs --no-color >"$OUT/services.log" 2>&1 || true
  "${COMPOSE[@]}" down -v --remove-orphans >/dev/null 2>&1 || true
  printf 'Profiling artifacts: %s\n' "$OUT"
}
trap cleanup EXIT
docker info >/dev/null
"${COMPOSE[@]}" build benchmark
IMAGE_DIGEST=$(docker image inspect er-pipeline:ci --format '{{.Id}}')
GIT_SHA=$(git -C "$REPO_ROOT" rev-parse HEAD)
git -C "$REPO_ROOT" status --short >"$OUT/source-status.txt"
(
  while true; do
    printf '{"sample_time":"%s"}\n' "$(date -u +%FT%TZ)"
    ids=$(docker ps -q --filter "label=com.docker.compose.project=$PROJECT")
    if [[ -n "$ids" ]]; then
      docker stats --no-stream --format '{{json .}}' $ids 2>/dev/null || true
    fi
    sleep 1
  done
) >"$OUT/services-resources.jsonl" &
STATS_PID=$!
"${COMPOSE[@]}" run --rm -e "ER_GIT_SHA=$GIT_SHA" -e "ER_IMAGE_DIGEST=$IMAGE_DIGEST" benchmark python benchmarks/profile_pipeline.py \
  --out "/app/artifacts/profile/${SESSION}" "$@" 2>&1 | tee "$OUT/campaign.log"
