---
id: ER-018
title: "Test harness A: session-namespaced lake fixture (lake_ns, lake_conn, er_env), METADATA_SCHEMA er_test_{ns}, DATA_PATH s3://lake/test/{ns}/, drop-schema teardown"
milestone: M1
status: todo
kind: code
size: M
gates: full
depends_on: ["ER-015", "ER-016"]
spec_refs: ["s1", "s4-0b", "s7-1", "s7-2", "s8-1"]
gap_refs: ["M22"]
provides: ["tests/conftest.py::lake_ns", "tests/conftest.py::er_env", "tests/conftest.py::lake_conn", "tests/conftest.py::object_store", "tests/conftest.py::catalog", "tests/integration/conftest.py", "tests/integration/test_harness_namespace.py"]
consumes: ["src/er/lake/ducklake.py::connect", "src/er/lake/ducklake.py::detach", "src/er/lake/catalog.py::catalog_connect", "src/er/lake/catalog.py::drop_metadata_schema", "src/er/lake/catalog.py::metadata_schema_exists", "src/er/lake/objectstore.py::ObjectStore", "src/er/lake/env.py::require_env", "scripts/ci/itest.sh"]
owns: ["tests/conftest.py", "tests/integration/conftest.py", "tests/integration/test_harness_namespace.py"]
protected_paths: ["docker/compose.yaml"]
extra_paths: []
attempts: 1
verify: "bash scripts/ci/itest.sh tests/integration/test_harness_namespace.py -q"
branch: ""
commit: ""
spec_sha: "28d8d8e366a7b49b"
updated_at: "2026-08-15T01:08:39Z"
session: 4a1ce021-5b63-4478-b4e0-26cdfbeec9cb
---
## Description

Implement the S8.1 test-isolation contract: a session-scoped fixture that mints a namespace, exports `ER_LAKE_METADATA_SCHEMA=er_test_<ns>` and `ER_LAKE_DATA_PATH=s3://lake/test/<ns>/`, hands every integration test its lake handle, and reclaims the namespace on teardown under `try/finally`. M22 is that "ephemeral DuckLake per test session" was asserted and never designed — with one shared catalog and one un-namespaced bucket, test ordering becomes load-bearing and every snapshot-dependent assertion is contaminated. Landing it here, immediately after the connection factory and before the first relation exists, is what stops the following eight integration suites from being written against a shared default namespace.

## Scope

### In scope

- `lake_ns` (session scope): lowercased Crockford ULID minted at session start, suffixed with `PYTEST_XDIST_WORKER` when that variable is set
- `er_env` (session scope): exports `ER_LAKE_METADATA_SCHEMA`, `ER_LAKE_DATA_PATH`, `ER_LAKE_ALIAS=lake`, leaving `ER_CATALOG_DSN` and every `ER_S3_*` variable exactly as Compose supplied them
- `lake_conn`: an attached DuckDB connection over that namespace, built through `ducklake.connect`
- `object_store` and `catalog` fixtures exposing the ER-015 clients bound to the same namespace
- Teardown: `CALL lake.expire_snapshots(older_than => now())`, `CALL lake.cleanup_old_files(cleanup_all => true)`, delete the `s3://lake/test/<ns>/` prefix, `DETACH lake`, `DROP SCHEMA er_test_<ns> CASCADE`, all inside `try/finally`
- `tests/integration/conftest.py` as the placeholder module later tickets extend (the T-INV-1 autouse finalizer lands there in M3)

### Out of scope

- Creating any relation, running `er init`, or applying DDL — the namespace is empty when this fixture yields; ER-019/ER-020 add the initialised-lake fixture on top
- Function-level isolation (DELETE from every `ddl.py`-owned relation, fixture reload), `--keep-lake`, and the `sub_namespace` factory for T-INC-1 — all ER-026
- Changing `docker/compose.yaml`: the namespace comes from the fixture, never from a second set of Compose environment variables
- Any scenario fixture loading or expected-file comparison (ER-027, ER-028)

## Design decisions applied

Closes M22. Easy to miss: (1) a namespace is exactly the pair `(METADATA_SCHEMA, DATA_PATH)` and nothing else in the S4.0b attach sequence may vary between namespaces (S7.2); (2) teardown MUST run under `try/finally` so a failing test still reclaims the namespace, and it must expire snapshots and clean up files BEFORE deleting the prefix and dropping the schema, or orphan Parquet is left in the bucket; (3) integration tests run single-process — `-n auto` is a unit-layer flag — but the `PYTEST_XDIST_WORKER` suffix is still required by S8.1 and must be implemented; (4) S8.1 step 3 runs `er init` against the namespace, which does not exist yet: this ticket deliberately yields an EMPTY namespace and ER-020 wires `er init` in, so nothing here may create tables; (5) tenancy is namespace-only (S1) — no relation gains a tenant column and no fixture writes one; (6) no test may reference an absolute snapshot version or assert a snapshot count (S8.1, S4 preamble).

## Acceptance criteria

- [ ] AC1: `lake_ns` matches `^[0-9a-hjkmnp-tv-z]{26}$`; with `PYTEST_XDIST_WORKER=gw3` exported it equals `f"{ulid}_gw3"`; two nested sessions produce different values.
- [ ] AC2: `er_env` sets `ER_LAKE_METADATA_SCHEMA == f'er_test_{ns}'`, `ER_LAKE_DATA_PATH == f's3://lake/test/{ns}/'` and `ER_LAKE_ALIAS == 'lake'`, and leaves `ER_CATALOG_DSN` and every `ER_S3_*` variable byte-identical to the values captured before the fixture ran.
- [ ] AC3: With `lake_conn`, `SELECT * FROM lake.snapshots()` succeeds and the catalog reports a schema literally named `er_test_<ns>`.
- [ ] AC4: Immediately after `lake_conn` yields, `duckdb_tables()` filtered on `database_name='lake'` returns zero rows — the fixture creates no relation.
- [ ] AC5: A nested pytest session that writes one table and one object leaves, after teardown, zero keys under `s3://lake/test/<ns>/` and no `er_test_<ns>` schema in the catalog (both asserted from the outer session).
- [ ] AC6: A nested pytest session whose test raises still tears down: the inner run exits non-zero and the outer assertions on prefix emptiness and schema absence both hold.

## Tests

- tests/integration/test_harness_namespace.py::test_ns_shape_and_xdist_suffix
- tests/integration/test_harness_namespace.py::test_env_exports_namespace_and_preserves_compose_vars
- tests/integration/test_harness_namespace.py::test_lake_conn_attaches_namespaced_schema
- tests/integration/test_harness_namespace.py::test_namespace_starts_empty_of_relations
- tests/integration/test_harness_namespace.py::test_teardown_drops_schema_and_prefix
- tests/integration/test_harness_namespace.py::test_teardown_runs_on_test_failure

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_harness_namespace.py -q
uv run ruff check tests/conftest.py tests/integration
```

## Definition of Done

- `lake_ns`, `er_env`, `lake_conn`, `object_store`, `catalog` fixtures implemented at session scope
- Namespace derived only from `METADATA_SCHEMA` + `DATA_PATH`; Compose-supplied variables untouched
- Teardown expires snapshots, cleans files, deletes the prefix, DETACHes and drops the schema, under `try/finally`
- Namespace yields empty — no relation created by the harness
- Verify command passes, including the deliberate-failure teardown test
