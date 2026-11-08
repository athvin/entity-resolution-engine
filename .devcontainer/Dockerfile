# syntax=docker/dockerfile:1
# The DEVELOPMENT container: enough toolchain to run `make check`, `make
# check-e2e` and `make frontend` green. Aimed at a collaborator on Windows, for
# whom reproducing rustup-at-the-pinned-channel + mold + DuckDB extensions +
# corepack pnpm + a Postgres natively is not a one-afternoon job, and for whom
# getting it subtly wrong produces failures that look like code bugs (#584).
#
# NOT the production image. `rust/Dockerfile` builds that (the chart's
# `er-api`), and the two differ deliberately — see the cmake note below, which
# is the one difference that could otherwise hide a broken release build.
#
# The build context is `.devcontainer/` (devcontainer.json sets no `context`,
# so it defaults to this folder) and NOTHING is COPYed in. That is deliberate:
# the repo root carries a 132 GB `rust/target`, and a context of `..` would
# send it to the daemon on every build. The workspace arrives as a mount at
# runtime instead.
FROM mcr.microsoft.com/devcontainers/rust:1-bookworm

# Everything below runs as root; devcontainer.json switches back to `vscode`.
USER root

# mold is a CORRECTNESS requirement here, not the speed optimisation it is in
# CI — which is why this is a plain `apt-get install` and not the retry-and-fall-
# back dance `scripts/ci/install-mold.sh` performs. On aarch64-linux, which is
# what every Apple-silicon and Windows-on-ARM host produces, GNU ld fails the
# link with `undefined reference to duckdb_register_static_extensions`:
# libduckdb-sys's whole-archive extension-autoregister object lands AFTER the
# rlib group on the command line and single-pass ld never extracts the defining
# member from the bundled libduckdb. mold resolves symbols globally regardless
# of archive order. Same reasoning and same fix as rust/Dockerfile's chef stage;
# devcontainer.json's `containerEnv` is what selects it via RUSTFLAGS.
#
# A FALLBACK WOULD BE WRONG HERE. CI can afford to drop mold and link slower,
# because its runners are amd64 where either linker works. Dropping it in this
# image on an ARM host does not produce a slow build, it produces a build that
# cannot link at all — so a missing mold must fail at image build time, where
# there is a Dockerfile to look at, rather than at the developer's first
# `make check`.
RUN apt-get update \
	&& apt-get install -y --no-install-recommends mold \
	&& rm -rf /var/lib/apt/lists/*

# DELIBERATELY ABSENT: cmake.
#
# The aws crates in this workspace are pinned `default-features = false`
# precisely so `aws-lc-sys` — which needs cmake — never enters the dependency
# graph, and `rust/Dockerfile` does not install cmake either. #584 notes the
# devcontainer "may install cmake freely"; it must not, and the reason is the
# second half of that same note. A dev image carrying cmake would happily build
# a graph the release image cannot, so an accidental `default-features`
# relaxation would pass `make check` in here and fail `images.yml` after merge.
# Leaving cmake out makes that failure local and immediate. Add it here only in
# the same change that adds it there.
#
# Nothing in the current graph needs it: there is no `openssl-sys` and no
# `aws-lc-sys` in rust/Cargo.lock, and libduckdb-sys and ring both build
# through `cc`, not cmake.

# Postgres 16 for `make check-e2e`, from PGDG rather than Debian's 15, so the
# major matches the `postgres:16-alpine` service `rust.yml` runs the
# control-plane tier against. #584 asked for this to be picked explicitly
# rather than left half-configured, and the generous option is the right one:
# `make check-e2e` is the only gate that exercises the Postgres job queue, and
# a Windows collaborator who cannot run it cannot touch the control plane.
#
# Only the BINARIES are installed here. The cluster itself is created at first
# start by scripts/devcontainer-poststart.sh, in $HOME, owned by whoever is
# running — see that script's header. Debian's package auto-creates a `main`
# cluster owned by `postgres` that would need root to start, so it is dropped:
# one cluster, no privilege question, and nothing listening on 5432 except the
# one this repo's DSN points at.
RUN install -d -m 0755 /usr/share/postgresql-common/pgdg \
	&& curl -fsSL -o /usr/share/postgresql-common/pgdg/apt.postgresql.org.asc \
		https://www.postgresql.org/media/keys/ACCC4CF8.asc \
	&& echo "deb [signed-by=/usr/share/postgresql-common/pgdg/apt.postgresql.org.asc] https://apt.postgresql.org/pub/repos/apt bookworm-pgdg main" \
		> /etc/apt/sources.list.d/pgdg.list \
	&& apt-get update \
	&& apt-get install -y --no-install-recommends postgresql-16 postgresql-client-16 \
	&& pg_dropcluster 16 main --stop \
	&& rm -rf /var/lib/apt/lists/*

# rustfmt and clippy explicitly. The base image ships both, and `make check`
# runs `cargo fmt --check` and `cargo clippy` as its first two targets — so an
# upstream image that stopped shipping one should fail HERE, with a Dockerfile
# to point at, rather than at a developer's first gate run. The channel itself
# comes from rust/rust-toolchain.toml (`stable`) and is deliberately not named
# again; see scripts/ci/install-rust.sh for the same argument in CI.
RUN rustup component add rustfmt clippy

# THE MOUNT POINTS FOR devcontainer.json's NAMED VOLUMES, pre-created and owned
# by `vscode`. This is what makes the whole container privilege-free.
#
# A fresh named volume mounts root-owned, so the usual recipe is a postCreate
# `sudo chown`. That recipe is what broke in `devcontainers/ci`
# (`sudo: a password is required`), and it broke SILENTLY because the chown
# carried `|| true`.
#
# Docker's copy-up removes the need for it: when an empty named volume is
# mounted over a directory that exists in the image, the volume's root inherits
# that directory's uid, gid and mode. So creating them here, owned by the remote
# user, means every volume arrives writable and nothing ever needs root.
#
# A volume target must EXIST IN THIS IMAGE for copy-up to apply at all, which
# is why the cargo build directory is created here rather than mounted at
# `rust/target`: a workspace-relative path does not exist at build time, because
# the workspace folder name is not known until `up`. devcontainer.json reaches
# it through CARGO_TARGET_DIR instead.
#
# THE OWNERSHIP IS NOT UNIFORM ACROSS THE THREE, AND IT MUST NOT BE, because
# `devcontainers/ci` REWRITES the remote user's uid and gid to match the runner
# (1000 -> 1001) and then chowns only $HOME. Measured directly:
#
#   copy-up owner BEFORE rewrite:  /usr/local=1000:1000   $HOME=1000:1000
#   owner AFTER rewrite:           /usr/local=1000:1000   $HOME=1001:1001
#     /usr/local: NOT writable        $HOME: writable
#
# So the two under $HOME are owned by the user and the chown follows them. The
# cargo registry cannot be: CARGO_HOME is pinned to /usr/local/cargo by an
# UNCONDITIONAL `export` in the base image's /etc/bash.bashrc, which the CLI's
# `userEnvProbe` re-applies over `containerEnv` and which every interactive
# shell re-applies again — so relocating it under $HOME does not work, it only
# looks like it does until something reads the variable.
#
# The base image already answers this for the parent directory: /usr/local/cargo
# is mode 2777 with group `rustlang`, deliberately permissive so that ANY uid can
# write it. `registry` does not exist there, so it is created the same way rather
# than narrowed to 0755 — narrowing it to the build-time uid is precisely the bug
# this comment exists to prevent, and it was shipped once.
#
# scripts/ci/devcontainer-smoke.sh asserts all three are writable, and runs in CI
# against the real mounts after the rewrite — which is how this was caught, rather
# than it surfacing as a permission error forty minutes into someone's first build.
RUN install -d -o vscode -g vscode \
	/home/vscode/.duckdb \
	/home/vscode/.cache/er/cargo-target \
	&& install -d -m 2777 -g rustlang /usr/local/cargo/registry

USER vscode
