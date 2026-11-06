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

# NO `sudo`, AND NO CHOWN. The named volumes arrive writable because the
# Dockerfile pre-creates every mount point owned by the remote user and Docker's
# copy-up hands that ownership to a fresh volume. See the Dockerfile's
# mount-point stanza; scripts/ci/devcontainer-smoke.sh asserts it held.
#
# The version this replaced ran `sudo chown … || true`, which failed in
# `devcontainers/ci` with `sudo: a password is required` and reported success
# anyway, over a container whose build directory the developer could not write.

# ASSERT THE CARGO_HOME MOUNT ACTUALLY LANDED ON CARGO_HOME.
#
# devcontainer.json sets CARGO_HOME and separately mounts a volume at
# `$CARGO_HOME/registry`, but `mounts` does not expand `${containerEnv:…}` — so
# the path is written out literally in two places that cannot read each other.
# Were they to disagree, the mount would shadow an unrelated empty directory and
# cargo would silently re-download the whole registry on every rebuild, which
# reads as slow networking rather than a broken mount.
expected="/home/vscode/.cargo"
actual="${CARGO_HOME:-}"
if [ "$actual" != "$expected" ]; then
	echo "devcontainer-postcreate: CARGO_HOME is '${actual:-<unset>}' but devcontainer.json mounts '$expected/registry'." >&2
	echo "devcontainer-postcreate: make containerEnv.CARGO_HOME and the mount agree." >&2
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
