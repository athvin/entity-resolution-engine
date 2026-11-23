#!/usr/bin/env bash
# Own every resource this command creates; no existing compose project is used.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PYTHON="${PYTHON:-python3}"
"$PYTHON" -c 'import sys; assert sys.version_info >= (3, 11), "Python >= 3.11 required"'
revision=778dbe023203570f55e1265a91d4ec26885b250a
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
# The client runs in Linux on the Compose network, never on the host. Scores
# are compared bit-for-bit and libm differs across operating systems: a macOS
# baseline differed from the Linux workers by one ULP in match_probability,
# which is a platform difference, not a DuckFleet one. The image is pinned by
# digest so the baseline's glibc cannot drift between runs.
baseline_image=python:3.12-slim@sha256:5e2bdb1b0d542d5039cffb8181c65930093949194a65c365dfb1b0fe13a4fa5b
mkdir -p "$(dirname "$output")"
docker run --rm --network "${project}_default" --env DUCKFLEET_TOKEN \
  --volume "$ROOT/scripts/duckfleet:/er/scripts/duckfleet:ro" \
  --volume "$ROOT/rust/.cargo:/er/rust/.cargo:ro" \
  --volume "$ROOT/rust/fixtures/golden/fake_1000:/er/rust/fixtures/golden/fake_1000:ro" \
  --volume "$(cd "$(dirname "$output")" && pwd):/out" \
  "$baseline_image" sh -euc '
    pip install --quiet --requirement /er/scripts/duckfleet/requirements.txt
    python -m unittest discover -s /er/scripts/duckfleet -p "test_*.py"
    python /er/scripts/duckfleet/compatibility.py --url http://server:8080 \
      --duckfleet-revision "$0" --output "/out/$1" --disposable-compose
  ' "$revision" "$(basename "$output")"
