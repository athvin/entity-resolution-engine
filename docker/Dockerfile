# The application and both MinIO services use this image (S7.3). MinIO's
# registries and binary downloads no longer serve the pinned community releases.
# Build those releases from verified source archives for the host architecture.
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

# LICENSE travels with the manifest because pyproject.toml declares
# `license = { file = "LICENSE" }`: hatchling reads that file when it generates the
# project metadata, so the second sync below cannot install `er` without it.
COPY pyproject.toml uv.lock LICENSE ./

# Dependencies only -- src/ is not in the context yet, and this is the expensive
# layer Docker should cache across source edits. The dev dependency group is
# installed on purpose and MUST NOT be excluded: the `pipeline` service's command
# is `pytest`, so pytest, pytest-xdist and hypothesis have to be in the runtime
# image, and tests/unit/test_dockerfile_contract.py fails if the flag that would
# drop them appears anywhere in this file (S7.3).
RUN uv sync --frozen --no-install-project

COPY src/ src/
# Now install the project itself into /app/.venv. `uv sync` installs it editable,
# which is why the runtime stage still copies src/.
RUN uv sync --frozen

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
COPY --from=builder /app/.venv /app/.venv
COPY --from=builder /opt/duckdb_extensions /opt/duckdb_extensions
COPY --from=storage-builder /out/minio /out/mc /usr/local/bin/
COPY --from=storage-builder /src/minio/LICENSE /src/minio/NOTICE /src/minio/CREDITS /usr/share/licenses/minio/
COPY --from=storage-builder /src/mc/LICENSE /src/mc/NOTICE /src/mc/CREDITS /usr/share/licenses/mc/
WORKDIR /app
COPY src/ src/
COPY dbt/ dbt/
# Standalone CLI commands need dbt_utils in each fresh container. Resolve the
# pinned packages during the build, independently of the developer's dbt cache.
RUN dbt deps --project-dir dbt && rm -rf dbt/logs dbt/profiles/.user.yml
COPY configs/ configs/
COPY benchmarks/ benchmarks/
COPY fixtures/ fixtures/
COPY tests/ tests/
# The S7.3 listing omits scripts/, but the image is where check_extensions.py runs
# under `--network none` and where the S9.2 preflight calls benchmarks-adjacent
# helpers, so the runtime stage MUST carry it.
COPY scripts/ scripts/
# artifacts/ is the Compose bind-mount target (S7.4) and .bench is the benchmark
# scratch directory; both must exist before anything mounts or writes into them.
RUN mkdir -p /app/artifacts /app/.bench
ENTRYPOINT []
CMD ["er", "--help"]
