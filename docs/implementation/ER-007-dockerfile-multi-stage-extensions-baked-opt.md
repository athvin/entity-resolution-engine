---
id: ER-007
title: "Dockerfile: multi-stage, extensions baked to /opt/duckdb_extensions, autoinstall off, check_extensions.py"
milestone: M1
status: in_progress
kind: code
size: M
gates: full
depends_on: ["ER-003", "ER-004"]
spec_refs: ["s2-1", "s3", "s4-0b", "s7-1", "s7-3", "s9-1"]
gap_refs: ["B1", "M25"]
provides: ["docker/Dockerfile (stages: builder, runtime)", "image:er-pipeline (built from docker/Dockerfile, context = repo root)", "path:/opt/duckdb_extensions (baked ducklake/postgres/httpfs)", "scripts/check_extensions.py::main", ".dockerignore", "tests/unit/test_dockerfile_contract.py"]
consumes: ["pyproject.toml", "uv.lock", "src/er/versions.py::EXTENSION_PINS", "src/er/versions.py::PINS", "dir:configs/", "dir:fixtures/static/", "dir:benchmarks/baselines/", "dir:tests/integration/"]
owns: ["docker/Dockerfile", ".dockerignore", "scripts/check_extensions.py", "tests/unit/test_dockerfile_contract.py"]
protected_paths: ["DesignDoc.md", "pyproject.toml", "uv.lock"]
extra_paths: []
attempts: 1
verify: "docker build -f docker/Dockerfile -t er-pipeline:verify . && docker run --rm --network none er-pipeline:verify python /app/scripts/check_extensions.py && uv run pytest tests/unit/test_dockerfile_contract.py -q"
branch: "ticket/ER-007-dockerfile-multi-stage-extensions-baked-opt"
commit: ""
spec_sha: "28d8d8e366a7b49b"
updated_at: "2026-08-15T00:05:57Z"
session: bc962b73-0b6b-4e2e-b1f6-630555548a4e
---
## Description

Build the single image every Compose service runs, per S7.3: a builder stage that syncs the frozen lockfile twice (dependencies first with `--no-install-project`, then the project) and installs the three DuckDB extensions into `/opt/duckdb_extensions`, and a runtime stage that copies the venv, the baked extension directory and the source tree. Baking at build time is what lets `autoinstall_known_extensions=false` hold at runtime (S4.0b), which is the fix for the half of B1 that has containers reaching the network mid-test. `scripts/check_extensions.py` is the executable proof: it runs inside the image with the network disabled and asserts each extension is installed, loaded, and at the commit hash `src/er/versions.py` pins.

## Scope

### In scope

- `docker/Dockerfile`: builder (`python:3.12-slim` + `ghcr.io/astral-sh/uv:0.11.3`, two `uv sync --frozen` invocations, `DUCKDB_EXTENSION_DIRECTORY=/opt/duckdb_extensions`, INSTALL of ducklake/postgres/httpfs) and runtime (venv on PATH, `DBT_PROFILES_DIR=/app/dbt/profiles`, copies of `src/`, `dbt/`, `configs/`, `benchmarks/`, `fixtures/`, `tests/`, `scripts/`, `mkdir -p /app/artifacts /app/.bench`)
- `scripts/check_extensions.py`: offline verification of the baked directory against `EXTENSION_PINS`
- `.dockerignore` excluding `.git`, `.venv`, `.loop`, `artifacts`, `dbt/target`, `dbt/dbt_packages`, caches
- `tests/unit/test_dockerfile_contract.py`: static assertions on the Dockerfile text that need no daemon

### Out of scope

- `docker/compose.yaml`, the service definitions, credentials and the artifacts bind mount (ER-009)
- `er doctor` (ER-022) — this ticket verifies extensions from a bare script, not through the CLI
- Any ATTACH of DuckLake, S3 secret creation or catalog access (ER-016)
- Changing pins: `pyproject.toml`/`uv.lock` are protected here; a version problem is an ER-003/ER-004 amendment

## Design decisions applied

Implements B1 (extensions installed at build time, autoinstall off at runtime) and M25 (base images pinned to the S2.1 python and uv rows, by tag — the `@sha256:` digest requirement covers the three Compose *service* images only). Three constraints an implementer will otherwise miss. (1) The runtime stage MUST also `COPY scripts/ scripts/`: the S7.3 listing omits it, but the verify command runs `/app/scripts/check_extensions.py` and later tickets run `benchmarks/`-adjacent helpers from the image. (2) `uv sync --frozen --no-dev` MUST NOT be used — the `pipeline` service's command is `pytest`, so pytest, pytest-xdist and hypothesis must be present (S7.3). (3) The build context is the repository root for both invocations (`docker build … .` here, `context: ..` in compose), so every `COPY` path is repo-root-relative and every directory it names must already exist in git — ER-003 created them.

## Acceptance criteria

- [ ] AC1: `docker build -f docker/Dockerfile -t er-pipeline:verify .` succeeds from the repository root on a clean checkout.
- [ ] AC2: `docker run --rm --network none er-pipeline:verify python /app/scripts/check_extensions.py` exits 0 — proving nothing is fetched at runtime — and prints one line per extension with its commit hash.
- [ ] AC3: The check is not vacuous: `docker run --rm --network none -e ER_DUCKDB_EXTENSION_DIRECTORY=/tmp/empty er-pipeline:verify python /app/scripts/check_extensions.py` exits non-zero, and so does a run whose expected hash is perturbed.
- [ ] AC4: `check_extensions.py` opens DuckDB with `extension_directory=/opt/duckdb_extensions`, `autoinstall_known_extensions=false` and `autoload_known_extensions=false`, LOADs all three extensions, and asserts `duckdb_extensions()` reports each `installed AND loaded` with `extension_version` equal to `src/er/versions.py::EXTENSION_PINS` — looking the postgres extension up under `postgres_scanner`.
- [ ] AC5: `docker run --rm er-pipeline:verify python -c "import pytest, hypothesis, splink, dbt.version, er; import sys; assert sys.version_info[:2]==(3,12)"` exits 0 (dev group present, interpreter matches the S2.1 Python row).
- [ ] AC6: `docker run --rm er-pipeline:verify sh -lc 'ls /app/src /app/dbt /app/configs /app/benchmarks /app/fixtures /app/tests /app/scripts /app/artifacts && printf "%s" "$DBT_PROFILES_DIR"'` exits 0 and prints `/app/dbt/profiles`.
- [ ] AC7: `tests/unit/test_dockerfile_contract.py` (no daemon required) asserts: exactly two stages named `builder` and `runtime`; the first sync carries `--no-install-project` and a second plain `uv sync --frozen` follows `COPY src/`; the string `--no-dev` appears nowhere; the two base image references equal the S2.1 python and uv pins; the runtime stage copies `/opt/duckdb_extensions` from the builder and copies `scripts/`.
- [ ] AC8: `.dockerignore` excludes `.git`, `.venv`, `.loop`, `artifacts`, `dbt/target` and `dbt/dbt_packages`, and `docker run --rm er-pipeline:verify sh -lc 'test ! -e /app/.git'` exits 0.

## Tests

- tests/unit/test_dockerfile_contract.py::test_two_stages_and_pinned_base_images
- tests/unit/test_dockerfile_contract.py::test_builder_syncs_twice_and_never_uses_no_dev
- tests/unit/test_dockerfile_contract.py::test_runtime_copies_extension_dir_and_scripts
- tests/unit/test_dockerfile_contract.py::test_dockerignore_excludes_build_noise
- scripts/check_extensions.py — executed in-image with `--network none` as the ticket's offline arm

## Verification

```bash
docker build -f docker/Dockerfile -t er-pipeline:verify . && docker run --rm --network none er-pipeline:verify python /app/scripts/check_extensions.py && uv run pytest tests/unit/test_dockerfile_contract.py -q
docker run --rm er-pipeline:verify python -c "import pytest, hypothesis, splink, er"
uv run ruff check scripts/check_extensions.py
```

## Definition of Done

- Acceptance criteria met
- Verify command passes
- Extensions baked at build time and verified with the network disabled
- Negative arm proves the extension check is non-vacuous
- Runtime stage copies `scripts/`; no `--no-dev` anywhere
- Base image tags equal the S2.1 python and uv pins
- pyproject.toml, uv.lock and DesignDoc.md unmodified
- Committed on main
