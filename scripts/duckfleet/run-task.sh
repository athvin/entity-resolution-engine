#!/usr/bin/env bash
# Prove the account golden task against the exact qualified upstream source.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
scratch="$(mktemp -d "${TMPDIR:-/tmp}/er-duckfleet-task-source.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT
revision=ac894b551e6d802e4c4f88422881c6aaac6e4f4b
image="er-duckfleet-task-${revision:0:12}:proof"
git init -q "$scratch/upstream"
git -C "$scratch/upstream" remote add origin https://github.com/athvin/duck-fleet.git
git -C "$scratch/upstream" fetch --depth 1 origin "$revision"
git -C "$scratch/upstream" checkout --detach FETCH_HEAD
docker build --target runtime --tag "$image" --file "$scratch/upstream/docker/server.Dockerfile" "$scratch/upstream"
python3 "$ROOT/scripts/duckfleet/task.py" --source "$scratch/upstream" \
  --image "$image" --output "${1:-$ROOT/artifacts/duckfleet/task.json}"
