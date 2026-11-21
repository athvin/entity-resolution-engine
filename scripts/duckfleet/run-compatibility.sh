#!/usr/bin/env bash
# Own every resource this command creates; no existing compose project is used.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PYTHON="${PYTHON:-python3}"
"$PYTHON" -c 'import sys; assert sys.version_info >= (3, 11), "Python >= 3.11 required"'
revision=672b3e3e82b61122c9647dd5dc85d32a3cedf09d
scratch="$(mktemp -d "${TMPDIR:-/tmp}/er-duckfleet-compat.XXXXXX")"
project="er-duckfleet-compat-$("$PYTHON" -c 'import uuid; print(uuid.uuid4().hex[:12])')"
output="${1:-$ROOT/artifacts/duckfleet/compatibility.json}"
compose=(docker compose --project-name "$project" --file "$scratch/upstream/compose.yaml" --file "$scratch/override.yaml")
cleanup() {
  local result=$?
  trap - EXIT
  if [[ -f "$scratch/override.yaml" ]]; then
    "${compose[@]}" down --volumes --remove-orphans || result=1
  fi
  rm -rf "$scratch"
  exit "$result"
}
trap cleanup EXIT
# A specific commit, not whichever upstream main happens to mean on the day.
git init -q "$scratch/upstream"
git -C "$scratch/upstream" remote add origin https://github.com/athvin/duck-fleet.git
git -C "$scratch/upstream" fetch --depth 1 origin "$revision"
git -C "$scratch/upstream" checkout --detach FETCH_HEAD
cat > "$scratch/override.yaml" <<EOF
services:
  worker-small:
    image: $project:compat
  server:
    image: $project:compat
    pull_policy: never
EOF
export DUCKFLEET_WORKER_URLS=http://worker-small:8080
export DUCKFLEET_PROFILING=true
export DUCKFLEET_PORT=0
export DUCKFLEET_TOKEN=duckfleet-local-development-token
"${compose[@]}" up --detach --build server
address="$("${compose[@]}" port server 8080)"
"$PYTHON" - "$address" <<'READY'
import sys, time, urllib.request
for attempt in range(120):
    try:
        with urllib.request.urlopen('http://' + sys.argv[1] + '/healthz', timeout=2) as response:
            if response.status == 200:
                break
    except OSError:
        pass
    time.sleep(1)
else:
    raise SystemExit('disposable DuckFleet did not become healthy')
READY
"$PYTHON" -m venv "$scratch/venv"
"$scratch/venv/bin/pip" install --requirement "$ROOT/scripts/duckfleet/requirements.txt"
"$scratch/venv/bin/python" -m unittest discover -s "$ROOT/scripts/duckfleet" -p 'test_*.py'
"$scratch/venv/bin/python" "$ROOT/scripts/duckfleet/compatibility.py" \
  --url "http://$address" --duckfleet-revision "$revision" \
  --output "$output" --disposable-compose
