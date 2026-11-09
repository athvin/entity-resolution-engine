# syntax=docker/dockerfile:1
# The Rust control-plane image (the chart's `er-api`). ONE binary serves all
# three entry points: the HTTP server (`er-server`), the standalone job
# dispatcher (`er-dispatcher`) and the pipeline CLI (`er`), dispatched on
# argv[0] — see the module header of crates/er-server/src/main.rs. `er` and
# `er-dispatcher` are symlinks to it.
#
# Each of the three used to be its own binary, and because `duckdb` is pinned
# with the `bundled` feature every one of them statically linked its own copy of
# DuckDB. The image paid for DuckDB three times and the chart only ever ran
# `er-server` (plus `er-server migrate` / `migrate-tenants`); the dispatcher runs
# in-process in the merged web-api pod and the pipeline runs as a linked library,
# so neither other binary was ever executed in a pod.
#
# Build context = rust/ (the cargo workspace root). Nothing here may read a path
# above it: `entities/` lives beside the workspace at the repo root and is NOT in
# this context. The er-entity-gen `no_embedded_asset_escapes_the_cargo_workspace`
# gate enforces that, after a generated module embedding the entity YAML by
# relative path broke every image build while `make check` stayed green.
# THE DEPENDENCY LAYER IS SPLIT OUT, and the reason is a measurement: this
# file used to be `COPY . .` followed by one `cargo build`, so ANY change under
# rust/ — a one-line handler edit — invalidated that layer and recompiled all
# 408 crates, bundled DuckDB's C++ included. Run 41524220825 measured it at
# 58m9s on every rust-touching merge to main, with deploy-dev waiting behind it.
#
# cargo-chef splits it in two. `prepare` reduces the workspace to a recipe
# (manifests + lockfile, no sources); `cook` builds the dependency graph from
# that recipe against stubbed workspace crates. Sources then arrive in a LATER
# layer, so a source-only change reuses the cooked dependencies and recompiles
# only this workspace's 33 crates. The recipe changes when Cargo.toml or
# Cargo.lock does, which is exactly when the dependency build genuinely must
# re-run. The cook layer is what the ECR registry cache (images.yml
# `cache-to: type=registry`) now carries between runs.
#
# `--bin er-server` on BOTH cook and build, deliberately: cargo resolves
# features per selected target, so cooking the whole workspace would unify a
# different feature set than the final build asks for, and every crate whose
# features differed would compile a second time — handing back the saving.
# CI overrides this named context with verified archives; standalone builds
# retain the pinned-download fallback. Tool archives are not BuildKit secrets
# (which have a 500 KiB size limit).
FROM scratch AS ci-tools
COPY .dockerignore /placeholder

FROM rust:1.99.0-bookworm@sha256:114c7a4425406451c2866b6aafe69fe29b1b298832db1277d411ac73c82d04d6 AS chef
# Keep prepare, cook and build on the compiler pinned by the base image.
# COPY brings in rust-toolchain.toml (channel=stable); without this override
# it can switch the final build to a newer compiler and rebuild dependencies.
ENV RUSTUP_TOOLCHAIN=1.99.0
WORKDIR /src
# mold is the LINKER FIX, not an optimisation: on aarch64-linux, GNU ld fails
# this link with `undefined reference to duckdb_register_static_extensions` —
# libduckdb-sys's whole-archive extension-autoregister object lands AFTER the
# rlib group on the command line, and single-pass ld never extracts the
# defining member from the bundled libduckdb. mold resolves symbols globally
# regardless of archive order (amd64 happens to link either way, and CI's
# `test` job has linked with mold for weeks), so one unconditional linker keeps
# the two architectures' builds identical. Observed 2026-11-02 building the
# kind-probe image on an arm64 host.
RUN apt-get update && apt-get install -y --no-install-recommends mold \
    && rm -rf /var/lib/apt/lists/*
# Pinned, like every other third-party tool here. This compiles cargo-chef
# itself, which is why it sits in the shared base stage: the layer is reused by
# both stages below and only rebuilt when this version changes.
ARG TARGETARCH
RUN --mount=type=bind,from=ci-tools,target=/ci-tools \
    case "$TARGETARCH" in \
      amd64) arch=x86_64; digest=aa73c7270c997894038c0d8f0d297865f57569e1687883322bc431713d4bb9dd ;; \
      arm64) arch=aarch64; digest=3e47fc4a71b1ddd1b4294a0de9d34498922fdf41e3cd0bda5df3954e3d6deb9d ;; \
      *) exit 1 ;; \
    esac \
    && if [ -f "/ci-tools/$digest.tar.gz" ]; then cp "/ci-tools/$digest.tar.gz" /tmp/sccache.tar.gz; \
       else curl -fSL --connect-timeout 10 --max-time 60 --retry 3 --retry-all-errors \
      "https://github.com/mozilla/sccache/releases/download/v0.19.0/sccache-v0.19.0-${arch}-unknown-linux-musl.tar.gz" -o /tmp/sccache.tar.gz; fi \
    && echo "$digest  /tmp/sccache.tar.gz" | sha256sum -c - \
    && tar xzf /tmp/sccache.tar.gz -C /tmp \
    && install "/tmp/sccache-v0.19.0-${arch}-unknown-linux-musl/sccache" /usr/local/bin/sccache \
    && rm -rf /tmp/sccache*
RUN cargo install cargo-chef --locked --version 0.1.78

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
# `.cargo/config.toml` BEFORE the cook, not with the sources. It carries the
# CXXFLAGS that stamp libduckdb-sys's release version, and CXXFLAGS is part of
# that crate's fingerprint: cooking without it would build DuckDB once here and
# then AGAIN after the sources arrived with the config — the single most
# expensive crate in the graph, compiled twice, for no gain.
COPY .cargo .cargo
COPY --from=planner /src/recipe.json recipe.json
# DuckDB is compiled from sources vendored inside the `libduckdb-sys` crate (the
# `bundled` feature untars them and builds with cc), so the only network this
# stage needs is crates.io — it does NOT fetch a prebuilt library. This is the
# step that takes the time, and the step the layer cache now spares.
RUN --mount=type=secret,id=gha_url,env=ACTIONS_RESULTS_URL \
    --mount=type=secret,id=gha_token,env=ACTIONS_RUNTIME_TOKEN \
    --mount=type=cache,target=/root/.cache/sccache \
    if [ -n "${ACTIONS_RUNTIME_TOKEN:-}" ]; then export SCCACHE_GHA_ENABLED=on ACTIONS_CACHE_SERVICE_V2=on; fi; \
    RUSTC_WRAPPER=sccache RUSTFLAGS="-C link-arg=-fuse-ld=mold" cargo chef cook --release --bin er-server --recipe-path recipe.json \
    && sccache --show-stats \
    && sccache --stop-server
COPY . .
RUN --mount=type=secret,id=gha_url,env=ACTIONS_RESULTS_URL \
    --mount=type=secret,id=gha_token,env=ACTIONS_RUNTIME_TOKEN \
    --mount=type=cache,target=/root/.cache/sccache \
    if [ -n "${ACTIONS_RUNTIME_TOKEN:-}" ]; then export SCCACHE_GHA_ENABLED=on ACTIONS_CACHE_SERVICE_V2=on; fi; \
    RUSTC_WRAPPER=sccache RUSTFLAGS="-C link-arg=-fuse-ld=mold" cargo build --release --bin er-server \
    && sccache --show-stats \
    && sccache --stop-server
RUN strip target/release/er-server || true

# Bake the DuckDB extensions, using the binary just built.
#
# This is what removes a per-run network fetch. `duckdb` is pinned with the plain
# `bundled` feature — without `parquet` or `json` — and the bundled build defines
# DUCKDB_EXTENSION_AUTOINSTALL_DEFAULT=1, so only `core_functions` is statically
# linked and a cold process downloaded ducklake, parquet, json and icu from
# extensions.duckdb.org before it could read its first row. On a scale-to-zero
# runner node, with an empty ~/.duckdb, that was every single run.
#
# The install runs through the same statically-linked DuckDB that will read the
# directory at runtime, because extension files are keyed by exact version and
# platform; a directory built by some other duckdb would be silently ignored and
# the fetch would quietly come back. See `install_extensions` in
# crates/er-server/src/main.rs. This step needs egress to the extension repo —
# at build time, which is the point.
#
# This step also depends on `.cargo/config.toml` being in the build context: it
# carries the release version label `libduckdb-sys`'s bundled build omits, and
# without it the engine reports a `-dev` version for which no extension binary
# exists, so every INSTALL here 404s and the image build fails. `.dockerignore`
# must keep allowing it.
RUN ./target/release/er-server install-extensions --dir /opt/duckdb/extensions

FROM debian:bookworm-slim AS runtime
ADD --checksum=sha256:90a6f1a7978a1b1b096dca6c57511ca48d3b8cf052f8e704baf358f34ea2f21b https://truststore.pki.rds.amazonaws.com/global/global-bundle.pem /etc/ssl/rds/global-bundle.pem
# Set the file mode separately: ADD --chmod also changes created parent directories.
RUN chmod 644 /etc/ssl/rds/global-bundle.pem
# ca-certificates for outbound HTTPS: webhook delivery and the SES API. The
# DuckDB extension fetch is deliberately no longer in this list.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/er-server /usr/local/bin/er-server
# The other two entry points, as names rather than copies.
RUN ln -s er-server /usr/local/bin/er \
    && ln -s er-server /usr/local/bin/er-dispatcher
COPY --from=build /opt/duckdb/extensions /opt/duckdb/extensions
# Point DuckDB at the baked directory. Nothing outside the image sets this, so a
# local `cargo run` and the test suites keep DuckDB's default
# ~/.duckdb/extensions and its autoinstall behaviour — a developer should not
# have to bake extensions to run the engine. Setting it also turns
# `autoinstall_known_extensions` off (see SessionOptions::with_baked_extensions),
# so a missing extension fails loudly here instead of silently downloading.
ENV ER_DUCKDB_EXTENSION_DIR=/opt/duckdb/extensions
# The chart sets the api container port to 8000; default the bind to match.
ENV ER_SERVER_ADDR=0.0.0.0:8000
# Panic backtraces on, per-error backtraces off: RUST_BACKTRACE covers a real
# panic, while RUST_LIB_BACKTRACE=0 stops `anyhow` capturing a backtrace for
# every Error it constructs — the dispatcher builds those on expected failure
# paths, so capturing there is pure cost. Set here rather than in the chart so
# it applies however the image is invoked.
ENV RUST_BACKTRACE=1 \
    RUST_LIB_BACKTRACE=0
EXPOSE 8000
# uid 1001 == the `er` user the chart's securityContext pins (runAsUser 1001).
# WORKDIR /app so DuckDB's CWD-relative spill lands in the chart's /app/.tmp mount.
RUN useradd --system --uid 1001 --create-home --home-dir /app er
WORKDIR /app
USER 1001
RUN test -r /etc/ssl/rds/global-bundle.pem
CMD ["er-server"]
