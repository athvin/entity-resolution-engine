# The application and both MinIO services use this image (S7.3). MinIO's
# registries and binary downloads no longer serve the pinned community releases.
# Build those releases from verified source archives for the host architecture.
#
# This is also the er-pipeline runner image (infrastructure.md S11): the
# dispatcher launches each run as `python -m erserver.runner '<payload json>'`
# -- as a forked subprocess today and as a Kubernetes Job's container command
# (S6.2) -- so the venv carries the server package (erserver) alongside the
# engine. Only the package and its runtime dependencies: no server tests, and
# the engine's own lockfile stays authoritative for every package it pins.
#
# Python and uv are pinned by tag; the Go build image is pinned by digest.
# The source commits and archive checksums are recorded in er.versions (S2.1).
#
# The build context is the repository root (`docker build -f docker/Dockerfile .`
# here, `context: ..` in compose), so every COPY path below is repo-root-relative.

# ---- storage-builder ----
FROM golang:1.24.6-bookworm@sha256:ab1d1823abb55a9504d2e3e003b75b36dbeb1cbcc4c92593d85a84ee46becc6c AS storage-builder
ENV CGO_ENABLED=0 GOTOOLCHAIN=local GOFLAGS=-mod=readonly
WORKDIR /src/minio
ADD --checksum=sha256:8819e3e7817e46b7b3798f8f200ead208562e571563c2e040352378031abe9f2 https://codeload.github.com/minio/minio/tar.gz/07c3a429bfed433e49018cb0f78a52145d4bedeb /tmp/minio.tar.gz
RUN --mount=type=cache,target=/go/pkg/mod --mount=type=cache,target=/root/.cache/go-build \
    tar -xzf /tmp/minio.tar.gz --strip-components=1 && \
    go build -trimpath -buildvcs=false -tags kqueue \
      -ldflags="-s -w -X github.com/minio/minio/cmd.Version=2025-09-07T16:13:09Z -X github.com/minio/minio/cmd.ReleaseTag=RELEASE.2025-09-07T16-13-09Z -X github.com/minio/minio/cmd.CommitID=07c3a429bfed433e49018cb0f78a52145d4bedeb -X github.com/minio/minio/cmd.ShortCommitID=07c3a429bfed -X github.com/minio/minio/cmd.CopyrightYear=2025" \
      -o /out/minio . && /out/minio --version

WORKDIR /src/mc
ADD --checksum=sha256:95cd293c7119f16921a6dc515a1fb74a2227f19fd994b9c8b770a154e802ac44 https://codeload.github.com/minio/mc/tar.gz/7394ce0dd2a80935aded936b09fa12cbb3cb8096 /tmp/mc.tar.gz
RUN --mount=type=cache,target=/go/pkg/mod --mount=type=cache,target=/root/.cache/go-build \
    tar -xzf /tmp/mc.tar.gz --strip-components=1 && \
    go build -trimpath -buildvcs=false -tags kqueue \
      -ldflags="-s -w -X github.com/minio/mc/cmd.Version=2025-08-13T08:35:41Z -X github.com/minio/mc/cmd.ReleaseTag=RELEASE.2025-08-13T08-35-41Z -X github.com/minio/mc/cmd.CommitID=7394ce0dd2a80935aded936b09fa12cbb3cb8096 -X github.com/minio/mc/cmd.ShortCommitID=7394ce0dd2a8 -X github.com/minio/mc/cmd.CopyrightYear=2025" \
      -o /out/mc . && /out/mc --version

# ---- builder ----
FROM python:3.12-slim AS builder
COPY --from=ghcr.io/astral-sh/uv:0.11.3 /uv /bin/uv
WORKDIR /app

# LICENSE travels with the manifests because pyproject.toml declares
# `license = { file = "LICENSE" }`: hatchling reads that file when it generates the
# project metadata, so the root project sync below cannot install `er` without it.
# The server manifests ride the same layer: the server is a standalone uv project
# (server/README.md) that consumes `er` as an editable path dependency on the
# repository root, so both manifests and both lockfiles are build inputs here,
# exactly as in docker/Dockerfile.api.
COPY pyproject.toml uv.lock LICENSE ./
COPY server/pyproject.toml server/uv.lock server/

# Dependencies only -- neither src tree is in the context yet, and these are the
# expensive layers Docker should cache across source edits. Two locks, one venv,
# and the ROOT lock wins: the first sync seeds /app/.venv with erserver's
# dependencies at the server lock's pins, then the second re-asserts the engine's
# own lock over every package it names -- S2.1 makes that lock the engine's
# closed set, and this image is where the integration suite runs, so the shared
# packages the two locks resolve differently must land at the S2.1 pins.
# `--inexact` keeps the server-only packages (fastapi, uvicorn, httpx, ...) that
# the root lock does not know about. The dev dependency group is installed on
# purpose and MUST NOT be excluded: the `pipeline` service's command is `pytest`,
# so pytest, pytest-xdist and hypothesis have to be in the runtime image, and
# tests/unit/test_dockerfile_contract.py fails if the flag that would drop them
# appears anywhere in this file (S7.3). The server's test extra stays out for the
# same reason in reverse: the suite's tools come from the root dev group.
RUN UV_PROJECT_ENVIRONMENT=/app/.venv uv sync --frozen --project server --no-install-project --no-install-package er
RUN uv sync --frozen --inexact --no-install-project

COPY src/ src/
COPY server/src/ server/src/
# Now install the projects themselves into /app/.venv. `uv sync` installs `er`
# editable, which is why the runtime stage still copies both src trees. erserver
# arrives via `uv pip install --no-deps` rather than a server-project sync: a
# sync against server/uv.lock here would re-pin the packages both locks share to
# the server lock's resolution, drifting the engine off its own S2.1 lock --
# --no-deps installs exactly one package into an already-correct environment.
RUN uv sync --frozen --inexact
RUN uv pip install --python /app/.venv/bin/python --no-deps --editable ./server

# Bake DuckDB extensions at BUILD time so no container ever downloads one at
# runtime. This is the precondition for `autoinstall_known_extensions = false` in
# every S4.0b connection, and therefore for an integration suite that never
# reaches the network mid-test (B1).
ENV DUCKDB_EXTENSION_DIRECTORY=/opt/duckdb_extensions
RUN /app/.venv/bin/python -c "\
import duckdb, os; \
c = duckdb.connect(config={'extension_directory': os.environ['DUCKDB_EXTENSION_DIRECTORY']}); \
[c.execute(f'INSTALL {e}') for e in ('ducklake','postgres','httpfs')]"

# ---- runtime ----
FROM python:3.12-slim AS runtime
ENV PATH=/app/.venv/bin:$PATH \
    PYTHONDONTWRITEBYTECODE=1 \
    DBT_PROFILES_DIR=/app/dbt/profiles
# Every process in this image runs as this non-root user (infrastructure.md S5.6):
# pytest in the compose `pipeline` service, the MinIO server in `objectstore`,
# `mc` in `objectstore-init`, and `er` itself. uid/gid 1001 deliberately matches
# the GitHub Actions runner that owns the ../artifacts bind mount, so the junit
# XML and dbt logs stay writable from inside the container on a Linux host.
# --create-home because mc writes ~/.mc and MinIO reads ~/.minio.
RUN groupadd --gid 1001 er && \
    useradd --uid 1001 --gid er --create-home --shell /usr/sbin/nologin er
COPY --from=builder /app/.venv /app/.venv
COPY --from=builder /opt/duckdb_extensions /opt/duckdb_extensions
COPY --from=storage-builder /out/minio /out/mc /usr/local/bin/
COPY --from=storage-builder /src/minio/LICENSE /src/minio/NOTICE /src/minio/CREDITS /usr/share/licenses/minio/
COPY --from=storage-builder /src/mc/LICENSE /src/mc/NOTICE /src/mc/CREDITS /usr/share/licenses/mc/
WORKDIR /app
COPY src/ src/
# erserver is installed editable from /app/server, so its src tree must ship
# exactly like src/ does: read-only at run time, root-owned on purpose.
COPY server/src/ server/src/
# dbt writes target/, logs/, .tmp/ and profiles/.user.yml under dbt/ at run time,
# so this tree must belong to the runtime user. Everything else COPYed into the
# image is read-only at run time and stays root-owned on purpose.
COPY --chown=er:er dbt/ dbt/
# Standalone CLI commands need dbt_utils in each fresh container. Resolve the
# pinned packages during the build, independently of the developer's dbt cache.
# This runs as root (USER comes later), and a re-run of `dbt deps` at test time
# rmtrees dbt_packages — so the baked tree must belong to the runtime user.
RUN dbt deps --project-dir dbt && rm -rf dbt/logs dbt/profiles/.user.yml && \
    chown -R er:er /app/dbt
COPY configs/ configs/
COPY benchmarks/ benchmarks/
COPY fixtures/ fixtures/
COPY tests/ tests/
# pytest reads [tool.pytest.ini_options] from here: without it the in-container
# suite runs with unregistered markers ("slow" filtering by luck, not contract)
# and without the pythonpath the helpers import relies on locally.
COPY pyproject.toml ./
# The S7.3 listing omits scripts/, but the image is where check_extensions.py runs
# under `--network none` and where the S9.2 preflight calls benchmarks-adjacent
# helpers, so the runtime stage MUST carry it.
COPY scripts/ scripts/
# artifacts/ is the Compose bind-mount target (S7.4) and .bench is the benchmark
# scratch directory; both must exist before anything mounts or writes into them.
# .tmp and dbt/.tmp are DuckDB's CWD-relative spill directories (infrastructure.md
# S18.1), /data is the `objectstore` volume -- an anonymous volume inherits the
# image directory's ownership, which is what lets non-root MinIO write it -- and
# /app itself must belong to the user so pytest/hypothesis can create their
# caches beside the code.
RUN mkdir -p /app/artifacts /app/.bench /app/.tmp /app/dbt/.tmp /data && \
    chown er:er /app /app/artifacts /app/.bench /app/.tmp /app/dbt/.tmp /data
USER er
ENTRYPOINT []
CMD ["er", "--help"]
