# The single image every Compose service runs (S7.3): `pipeline`, `benchmark` and
# `catalog-init` all execute this build, and the k8s job image later will too.
#
# Both base images are pinned by tag, matching the S2.1 Python and `uv` rows (M25).
# The `@sha256:` digest requirement in S2.1 covers the three Compose *service*
# images, whose digests are literal in S7.1 -- not these two build stages.
#
# The build context is the repository root (`docker build -f docker/Dockerfile .`
# here, `context: ..` in compose), so every COPY path below is repo-root-relative.

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
WORKDIR /app
COPY src/ src/
COPY dbt/ dbt/
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
