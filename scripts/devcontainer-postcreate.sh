#!/usr/bin/env bash
# devcontainer.json's `postCreateCommand` — runs ONCE, when the container is
# created.
#
# It lives here rather than in `.devcontainer/` because the repository rule is
# that every tracked shell script sits under `scripts/`, enforced by
# `scripts/ci/scripts-home-check.sh` in `make check` and in rust.yml's ungated
# `workflow pins (S9)` job. A `.devcontainer/postcreate.sh` would fail the gate.
#
# WHAT IT DOES NOT DO: build anything. A postCreate that ran `make check` would
# make first open the ~40 minutes the published prebuild exists to avoid, and
# would do it on a machine with none of CI's sccache.
#
# Run (inside the container): bash scripts/devcontainer-postcreate.sh
set -euo pipefail

cd "$(dirname "$0")/.."

echo "devcontainer-postcreate: preparing the named volumes"

# A fresh named volume mounts root-owned, so every one of the four in
# devcontainer.json needs handing to `vscode` once. Idempotent: on a rebuild
# that reuses the volumes this is a no-op that costs a stat.
#
# `|| true` on each: a mount point that is missing (someone opened the folder
# without the mounts, or a path moved) must not abort the whole script before
# the assertions below, which are what would explain the problem.
for d in "${CARGO_HOME:-/usr/local/cargo}/registry" rust/target frontend/node_modules "$HOME/.duckdb"; do
	[ -d "$d" ] || continue
	sudo chown -R "$(id -u):$(id -g)" "$d" || true
done

# ASSERT THE CARGO_HOME MOUNT ACTUALLY LANDED ON CARGO_HOME.
#
# devcontainer.json has to name this path literally — `mounts` does not expand
# `${containerEnv:…}` — so it carries `/usr/local/cargo/registry` as a constant.
# If a base-image change moved CARGO_HOME, that mount would shadow an unrelated
# empty directory and cargo would silently re-download the whole registry on
# every rebuild, which looks like slow networking rather than a broken mount.
expected="/usr/local/cargo"
actual="${CARGO_HOME:-}"
if [ -n "$actual" ] && [ "$actual" != "$expected" ]; then
	echo "devcontainer-postcreate: CARGO_HOME is '$actual' but devcontainer.json mounts '$expected/registry'." >&2
	echo "devcontainer-postcreate: update the mount in .devcontainer/devcontainer.json to match." >&2
	exit 1
fi

# corepack resolves pnpm from frontend/package.json's `packageManager` field,
# which is how `make frontend` invokes it (`corepack pnpm …`). Enabling it is
# per-container state, so it belongs here rather than in the image.
echo "devcontainer-postcreate: enabling corepack"
corepack enable

# The toolchain assertions are the same ones CI runs on the built image, so a
# developer whose container is subtly wrong gets the identical message rather
# than a mysterious gate failure twenty minutes in.
bash scripts/ci/devcontainer-smoke.sh

cat <<'EOF'

devcontainer-postcreate: ready.

  make check        the backend PR gate (first run is a full cold build)
  make check-e2e    + DuckLake and the Postgres control plane (the sidecar
                    cluster is already running; ER_TEST_PG_DSN is preset)
  make frontend     the frontend PR gate, Playwright tier included

See docs/devcontainer.md. Visual baselines compare on Linux only, so this
container is the right place to regenerate them (`make frontend-baselines`).
EOF
