#!/usr/bin/env bash
# Runs INSIDE the freshly built devcontainer: is every tool `make check` and
# `make frontend` reach for actually present, at the version this repo pins?
#
# WHY THIS AND NOT `make check` ITSELF. The gate's own acceptance is that
# `make check` runs green in the container, and it does — measured on a
# developer machine and recorded on #584. Putting it in CI would be a second
# ~40-minute cold backend build on a workflow that is not a required check,
# paid on every `.devcontainer/` PR. What CI needs to catch is narrower and
# cheaper: the image lost a tool. An upstream base-image change dropping
# rustfmt, or a Node feature that resolves to 20, breaks `make check` at the
# developer's first run with nothing having gone red — and that is exactly the
# "abandoned instead of used" failure the prebuild exists to avoid.
#
# A missing tool is a FAILURE, never a warning: the whole point of a pinned dev
# image is that the toolchain question is settled before anyone opens it.
#
# Run (inside the container): bash scripts/ci/devcontainer-smoke.sh
set -uo pipefail

cd "$(dirname "$0")/../.."

fail=0
checked=0

# `want` is a prefix match on the tool's own version output, not an equality
# test, so a patch bump does not fail the gate. An empty `want` only asserts
# presence — right for mold and psql, where this repo pins no version.
check() {
	local tool="$1" want="$2" got
	checked=$((checked + 1))
	if ! command -v "$tool" > /dev/null 2>&1; then
		echo "devcontainer-smoke: $tool is MISSING from the image" >&2
		fail=1
		return
	fi
	got="$("$tool" --version 2>&1 | head -n 1)"
	if [ -n "$want" ] && [ "${got#*"$want"}" = "$got" ]; then
		echo "devcontainer-smoke: $tool is '$got', expected to contain '$want'" >&2
		fail=1
		return
	fi
	echo "devcontainer-smoke: ok  $tool  $got"
}

# The backend gate: fmt, lint, doc, entities-check and test are all cargo.
check cargo ""
check rustc ""
check rustfmt ""
# `cargo clippy --version` rather than a `clippy` binary — `cargo-clippy` exists
# but is not what the Makefile invokes.
checked=$((checked + 1))
if cargo clippy --version > /dev/null 2>&1; then
	echo "devcontainer-smoke: ok  cargo clippy  $(cargo clippy --version 2>&1 | head -n 1)"
else
	echo "devcontainer-smoke: cargo clippy is MISSING (rustup component add clippy)" >&2
	fail=1
fi

# mold is a CORRECTNESS requirement on aarch64, not a speed one — see the
# .devcontainer/Dockerfile header. Without it the bundled-DuckDB link fails
# with `undefined reference to duckdb_register_static_extensions`.
check mold ""

# The frontend gate: corepack resolves pnpm from frontend/package.json's
# `packageManager`, so only node's major is pinned here.
check node "v22."
check corepack ""

# `make check-e2e`'s client half. The server is started by
# scripts/devcontainer-poststart.sh.
check psql ""

# The gate's own shell tools. `skills-check.sh` is awk/sed/grep throughout, and
# a BusyBox applet standing in for GNU awk is a failure this would otherwise
# surface as a mysteriously empty generated block.
check awk ""
check make ""
check git ""

# RUSTFLAGS must carry mold, or an aarch64 build links with GNU ld and fails in
# a way that reads as a dependency bug. Asserted rather than assumed because it
# is set in devcontainer.json's `containerEnv`, which a local override can drop.
checked=$((checked + 1))
case "${RUSTFLAGS:-}" in
*fuse-ld=mold*)
	echo "devcontainer-smoke: ok  RUSTFLAGS  ${RUSTFLAGS}"
	;;
*)
	echo "devcontainer-smoke: RUSTFLAGS is '${RUSTFLAGS:-}' — it must select mold (-C link-arg=-fuse-ld=mold)" >&2
	fail=1
	;;
esac

# EVERY NAMED VOLUME MUST BE WRITABLE, and this is the assertion that replaced
# a `sudo chown`. The Dockerfile pre-creates each mount point owned by the
# remote user so Docker's copy-up hands a fresh volume over writable; if that
# ever stops holding — a base image moving CARGO_HOME, a mount added to
# devcontainer.json without its directory added to the Dockerfile — the symptom
# is otherwise a permission error forty minutes into a developer's first build.
#
# This runs in CI against the real mounts, which is the point: the previous
# arrangement failed in CI and said nothing, because the chown that failed
# carried `|| true`.
# The `-` fallbacks exist only so a bare `docker run` of the image can be
# smoke-tested; devcontainer.json sets both variables, and postCreate asserts
# CARGO_HOME matches the mount.
for d in "${CARGO_HOME:-$HOME/.cargo}/registry" \
	"${CARGO_TARGET_DIR:-$HOME/.cache/er/cargo-target}" \
	"$HOME/.duckdb"; do
	checked=$((checked + 1))
	if [ ! -d "$d" ]; then
		echo "devcontainer-smoke: $d does not exist — the Dockerfile must pre-create every volume mount point" >&2
		fail=1
	elif [ ! -w "$d" ]; then
		echo "devcontainer-smoke: $d is not writable by $(id -un) — Docker's copy-up did not carry ownership" >&2
		fail=1
	else
		echo "devcontainer-smoke: ok  writable  $d"
	fi
done

# POSITIVE CONTROL: a run that asserted nothing is a broken harness, not a
# clean image. Same zero-matches-is-failure rule as actionlint.sh and
# scripts-home-check.sh.
if [ "$checked" -lt 10 ]; then
	echo "devcontainer-smoke: only $checked assertions ran — the harness is broken, not the image" >&2
	exit 2
fi

if [ "$fail" -ne 0 ]; then
	echo "devcontainer-smoke: the image is missing something make check needs; fix .devcontainer/Dockerfile" >&2
	exit 1
fi

echo "devcontainer-smoke: ok (${checked} assertions, every tool present)"
