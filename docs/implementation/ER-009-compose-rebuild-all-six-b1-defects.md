---
id: ER-009
title: "Compose rebuild (all six B1 defects) + itest.sh/bench.sh/compose_smoke.sh + first real integration test"
milestone: M1
status: todo
kind: code
size: M
gates: full
depends_on: ["ER-007", "ER-008"]
spec_refs: ["s2-1", "s4-0b", "s7", "s7-1", "s7-2", "s7-4", "s8-1", "s9-1", "s10-2"]
gap_refs: ["B1", "M22", "M24"]
provides: ["docker/compose.yaml", "compose-service:catalog", "compose-service:objectstore", "compose-service:objectstore-init", "compose-service:pipeline", "compose-service:benchmark", "compose-anchor:x-er-env", "compose-anchor:x-er-image", "scripts/ci/itest.sh", "scripts/ci/bench.sh", "scripts/ci/compose_smoke.sh", "tests/unit/test_compose_contract.py", "tests/integration/test_compose_substrate.py"]
consumes: ["docker/Dockerfile", "image:er-pipeline", "src/er/versions.py::IMAGE_PINS", "dbt/profiles/profiles.yml::er.outputs.lake", "dir:artifacts/", "dir:scripts/ci/"]
owns: ["docker/compose.yaml", "scripts/ci/itest.sh", "scripts/ci/bench.sh", "scripts/ci/compose_smoke.sh", "tests/unit/test_compose_contract.py", "tests/integration/test_compose_substrate.py"]
protected_paths: ["DesignDoc.md", "docker/Dockerfile"]
extra_paths: []
attempts: 1
verify: "bash scripts/ci/compose_smoke.sh && uv run pytest tests/unit/test_compose_contract.py -q && bash scripts/ci/itest.sh tests/integration/test_compose_substrate.py -q"
branch: ""
commit: ""
spec_sha: "28d8d8e366a7b49b"
updated_at: "2026-08-15T00:27:36Z"
session: d37325e0-1c8d-41e3-8bab-ea7da0e77386
---
## Description

Rebuild the Compose substrate so it actually starts and so what it produces reaches CI — all six B1 defects at once: MinIO credentials that satisfy the 3/8-character minimum and are propagated to the `mc` init container, the full `ER_S3_*`/`ER_LAKE_*`/`ER_CONFIG` environment on `pipeline` and `benchmark`, the `../artifacts:/app/artifacts` bind mount, the fixed project name `er` plus `image: er-pipeline:ci` and `pull_policy: never`, and `run --rm` instead of `up --abort-on-container-exit` (S7.4). It ships the three CI entry scripts (`itest.sh`, `bench.sh`, `compose_smoke.sh`, each trapping `down -v --remove-orphans`) and the first real integration test, which proves the catalog, the bucket, the credentials and the artifacts mount from inside the pipeline container.

## Scope

### In scope

- `docker/compose.yaml`: top-level `name: er`, the `x-er-env` and `x-er-image` anchors, services `catalog` (healthchecked), `objectstore` (no healthcheck, by design), `objectstore-init` (retrying `mc alias set` → `mc ready` → `mc mb -p local/lake`), `pipeline` (profile `test`) and `benchmark` (profile `bench`), both with the artifacts bind mount and the `deploy.resources.limits` envelope
- `scripts/ci/itest.sh` — build/consume `er-pipeline:ci`, `mkdir -p artifacts`, `run --rm pipeline pytest <args> --junitxml=/app/artifacts/junit.xml`, teardown on exit via trap, propagate the pytest exit code
- `scripts/ci/bench.sh` — the same under `--profile bench`
- `scripts/ci/compose_smoke.sh` — bring the substrate up through `run --rm pipeline`, assert it is usable, tear down
- `tests/unit/test_compose_contract.py` — assertions against `docker compose config` output
- `tests/integration/test_compose_substrate.py` — catalog round-trip, S3 round-trip, artifacts writability, environment completeness

### Out of scope

- The `catalog-init` service and `er init` (ER-020 adds the service and gates `pipeline` on `service_completed_successfully`); until then `pipeline` gates on `catalog` healthy and `objectstore-init` completed
- Any DuckLake `ATTACH`, secret helper or snapshot code in `src/er/lake/` (ER-015/ER-016)
- `tests/conftest.py` and the session-namespaced lake fixture (ER-018) — `test_compose_substrate.py` must stand alone
- `.github/workflows/ci.yaml` (ER-010) and the per-scale benchmark envelope table (M5)

## Design decisions applied

Implements B1 in full, M22 (a substrate a local `up` and a fresh CI runner reach identically, with `down -v --remove-orphans` before and after) and M24 (the artifacts mount without which `report.py --compare` reads a file that cannot exist). Four constraints. (1) `up --abort-on-container-exit` MUST NOT appear anywhere: the one-shot init containers make its exit code a teardown signal (S7.4). (2) `objectstore` MUST NOT declare a healthcheck — the server image ships no `mc` and no HTTP client, so a probe can never pass and everything gated on `service_healthy` hangs; readiness is `objectstore-init` completing. (3) `ER_S3_ENDPOINT` is `objectstore:9000`, host:port with no scheme — DuckDB's httpfs rejects a URL. (4) No new Python dependency may be introduced for the substrate test: S2.1 is a closed set, so the catalog check goes through DuckDB's `postgres` extension and the S3 check through `httpfs` with a `CREATE SECRET` built from the environment, both already baked into the image.

## Acceptance criteria

- [ ] AC1: `bash scripts/ci/compose_smoke.sh` exits 0 from a clean state and exits 0 again when run a second time consecutively; it tears the stack down with `down -v --remove-orphans` even when the inner command fails (asserted by forcing a failing inner command and checking no container remains).
- [ ] AC2: `docker compose -f docker/compose.yaml config` renders project name `er`, and `tests/unit/test_compose_contract.py` asserts `pipeline` and `benchmark` each carry every environment variable of the S6.1 list with `ER_S3_ENDPOINT=objectstore:9000` (no scheme), `ER_LAKE_DATA_PATH=s3://lake/er/`, `ER_LAKE_ALIAS=lake`, `ER_LAKE_METADATA_SCHEMA=er_main` and `ER_CONFIG=/app/configs/test.yaml`.
- [ ] AC3: MinIO credentials are `erminio`/`erminiopassword` on both `objectstore` and `objectstore-init`, satisfying the 3/8-character minimums, and after the smoke run the bucket `lake` exists (the integration test lists it through httpfs).
- [ ] AC4: The three service images carry `@sha256:` digests byte-equal to `src/er/versions.py::IMAGE_PINS`, and `pipeline`/`benchmark` resolve to `image: er-pipeline:ci` with `pull_policy: never`; the compose contract test also asserts the string `--abort-on-container-exit` appears in no file under `docker/` or `scripts/ci/`.
- [ ] AC5: `bash scripts/ci/itest.sh tests/integration/test_compose_substrate.py -q` exits 0 and leaves a non-empty `artifacts/junit.xml` on the host; `bash scripts/ci/itest.sh tests/integration/test_compose_substrate.py::test_does_not_exist -q` exits non-zero — the script propagates the pytest status rather than a teardown status.
- [ ] AC6: `tests/integration/test_compose_substrate.py` asserts, from inside the pipeline container: a catalog round-trip through DuckDB's postgres extension returns a `server_version`; a write-then-read round-trip to `s3://lake/smoke/<uuid>` through httpfs with a secret built from `ER_S3_*` succeeds; `/app/artifacts` is writable; every `ER_*` variable of S6.1 is set and non-empty.
- [ ] AC7: The test imports nothing from `src/er` beyond nothing at all — it runs before the lake layer exists — and performs no `ATTACH` of DuckLake (asserted by inspection in review and by the absence of `ducklake:` in the test source).
- [ ] AC8: `deploy.resources.limits` on `pipeline` and `benchmark` is `cpus: ${ER_CPU_LIMIT:-2}` / `memory: ${ER_MEM_LIMIT:-6g}` with `ER_DUCKDB_THREADS` and `ER_DUCKDB_MEMORY_LIMIT` present in `x-er-env`, matching the S7.1 defaults.

## Tests

- tests/unit/test_compose_contract.py::test_project_name_image_and_pull_policy
- tests/unit/test_compose_contract.py::test_pipeline_and_benchmark_carry_the_full_env
- tests/unit/test_compose_contract.py::test_image_digests_match_versions_module
- tests/unit/test_compose_contract.py::test_no_abort_on_container_exit_anywhere
- tests/unit/test_compose_contract.py::test_artifacts_bind_mount_and_resource_envelope
- tests/integration/test_compose_substrate.py::test_catalog_round_trip
- tests/integration/test_compose_substrate.py::test_objectstore_round_trip
- tests/integration/test_compose_substrate.py::test_artifacts_mount_is_writable
- tests/integration/test_compose_substrate.py::test_required_env_is_complete

## Verification

```bash
bash scripts/ci/compose_smoke.sh && uv run pytest tests/unit/test_compose_contract.py -q && bash scripts/ci/itest.sh tests/integration/test_compose_substrate.py -q
docker compose -f docker/compose.yaml --profile test down -v --remove-orphans
docker compose -f docker/compose.yaml config >/dev/null
```

## Definition of Done

- Acceptance criteria met
- Verify command passes
- All six B1 defects fixed and each covered by an assertion
- `run --rm`, never `up --abort-on-container-exit`
- No healthcheck on `objectstore`; readiness proven by `objectstore-init`
- `artifacts/junit.xml` reaches the host after itest.sh
- Teardown runs on failure paths too (trap)
- No new Python dependency introduced
- Committed on main
