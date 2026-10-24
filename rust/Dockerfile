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
FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
# DuckDB is compiled from sources vendored inside the `libduckdb-sys` crate (the
# `bundled` feature untars them and builds with cc), so the only network the
# build stage needs is crates.io — it does NOT fetch a prebuilt library.
RUN cargo build --release --bin er-server
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
RUN ./target/release/er-server install-extensions --dir /opt/duckdb/extensions

FROM debian:bookworm-slim AS runtime
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
CMD ["er-server"]
