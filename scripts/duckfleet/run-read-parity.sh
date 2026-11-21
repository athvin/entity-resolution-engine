#!/usr/bin/env bash
# Build the pinned transport fix and prove the generated Rust read in isolation.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
scratch="$(mktemp -d "${TMPDIR:-/tmp}/er-duckfleet-read-source.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT
revision=ecaa09ce8df21f14dcd550a347ea3460055b4930
image="er-duckfleet-read-${revision:0:12}:proof"
git init -q "$scratch/upstream"
git -C "$scratch/upstream" remote add origin https://github.com/athvin/duck-fleet.git
git -C "$scratch/upstream" fetch --depth 1 origin "$revision"
git -C "$scratch/upstream" checkout --detach FETCH_HEAD
docker build --target runtime --tag "$image" --file "$scratch/upstream/docker/server.Dockerfile" "$scratch/upstream"
python3 "$ROOT/scripts/duckfleet/read-parity.py" --source "$scratch/upstream" \
  --image "$image" --output "${1:-$ROOT/artifacts/duckfleet/read-parity.json}"
