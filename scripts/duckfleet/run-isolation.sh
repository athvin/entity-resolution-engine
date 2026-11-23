#!/usr/bin/env bash
# An isolated test stack, never the existing developer fleet.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PYTHON="${PYTHON:-python3}"
"$PYTHON" -c 'import sys; assert sys.version_info >= (3, 11), "Python >= 3.11 required"'
scratch="$(mktemp -d "${TMPDIR:-/tmp}/er-duckfleet-isolation-source.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT
revision=778dbe023203570f55e1265a91d4ec26885b250a
git init -q "$scratch/upstream"
git -C "$scratch/upstream" remote add origin https://github.com/athvin/duck-fleet.git
git -C "$scratch/upstream" fetch --depth 1 origin "$revision"
git -C "$scratch/upstream" checkout --detach FETCH_HEAD
"$PYTHON" -m venv "$scratch/venv"
"$scratch/venv/bin/pip" install --requirement "$ROOT/scripts/duckfleet/requirements.txt"
"$scratch/venv/bin/python" "$ROOT/scripts/duckfleet/isolation.py" \
  --source "$scratch/upstream" --output "${1:-$ROOT/artifacts/duckfleet/isolation.json}"
