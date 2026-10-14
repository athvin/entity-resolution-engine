# syntax=docker/dockerfile:1
# The Rust control-plane image (the chart's `er-api`): the HTTP server
# (er-server), the in-process job dispatcher (er-dispatcher), and the pipeline
# CLI (er) in one image. The api Deployment runs `er-server`; the dispatcher
# Deployment overrides CMD to `er-dispatcher`; the db-migrate Job runs
# `er-server migrate`. Build context = rust/ (the cargo workspace root).
FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
# Release-build only the deployed binaries. DuckDB is statically linked
# (duckdb "bundled" feature); libduckdb-sys fetches the prebuilt lib at build
# time, so the build stage needs network egress.
RUN cargo build --release --bin er-server --bin er-dispatcher --bin er
RUN strip target/release/er-server target/release/er-dispatcher target/release/er || true

FROM debian:bookworm-slim AS runtime
# ca-certificates for outbound HTTPS (webhook delivery + DuckDB extension fetch).
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/er-server /usr/local/bin/er-server
COPY --from=build /src/target/release/er-dispatcher /usr/local/bin/er-dispatcher
COPY --from=build /src/target/release/er /usr/local/bin/er
# The chart sets the api container port to 8000; default the bind to match.
ENV ER_SERVER_ADDR=0.0.0.0:8000
EXPOSE 8000
# uid 1001 == the `er` user the chart's securityContext pins (runAsUser 1001).
# WORKDIR /app so DuckDB's CWD-relative spill lands in the chart's /app/.tmp mount.
RUN useradd --system --uid 1001 --create-home --home-dir /app er
WORKDIR /app
USER 1001
CMD ["er-server"]

# NOTE: the DuckLake/DuckDB extension is autoloaded at runtime (needs egress to
# the DuckDB extension repo, or a pre-baked extension dir on the duckdb-tmp /
# EFS volume the chart mounts). Baking the extension into the image is a
# follow-up once the cluster's egress policy is settled.
