---
id: ER-018
title: "Test harness A: session-namespaced lake fixture (lake_ns, lake_conn, er_env), METADATA_SCHEMA er_test_{ns}, DATA_PATH s3://lake/test/{ns}/, drop-schema teardown"
milestone: M1
status: in_progress
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
attempts: 3
verify: "bash scripts/ci/itest.sh tests/integration/test_harness_namespace.py -q"
branch: "ticket/ER-018-test-harness-session-namespaced-lake-fixture"
commit: ""
spec_sha: "28d8d8e366a7b49b"
updated_at: "2026-08-15T01:34:01Z"
session: ccfd1b99-9588-4cf8-a0e1-67796a4e82d2
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

## Blocker log

### Attempt 2 — environment (2026-08-15T01:30:20Z)

- **Failing command:** `git switch main && git merge --ff-only ticket/ER-018-test-harness-session-namespaced-lake-fixture`
- **Assertion / contradiction:** fatal: Not possible to fast-forward, aborting. -- the ticket branch already existed from attempt 1 at 31dc421, so step 2's 'git switch -c' failed with 'a branch named ticket/ER-018-... already exists' and work proceeded on that stale tip, one commit behind main's 'bd188ef board(ER-018): claim attempt 2'. main and the branch now carry conflicting board.py edits to the same frontmatter lines (main: in_progress/attempts 2/session d730f949; branch: done/attempts 1/session 4a1ce021, because 'board.py complete' ran against the stale checkout). 'board.py audit' on main reports 'DRIFT ER-018: left in_progress'. The implementation itself is FINISHED and fully green: commit 1cc86c53b7834e9ba64d0d5bd94c5f1284664e46 passed the full gate ladder (integration 28 passed, verify 6 passed, receipt .loop/receipts/ER-018-1.json).
- **Smallest change that would unblock:** Delete the stale branch and replay the finished commit onto a correctly-based one, then re-run: (1) git branch -m ticket/ER-018-test-harness-session-namespaced-lake-fixture ER-018-attempt2-keep ; (2) board.py claim ER-018 ; (3) git switch -c ticket/ER-018-test-harness-session-namespaced-lake-fixture ; (4) git checkout ER-018-attempt2-keep -- tests/conftest.py tests/integration/conftest.py tests/integration/test_harness_namespace.py (the three files are complete and need no edits) ; (5) rewrite .loop/change-plan.json for ER-018 and plan-check, then gates.sh, commit, complete, merge. Separately: S8.1 step 4's first two statements do not exist in the pinned duckdb==1.5.5 ducklake extension and need a spec amendment -- replace 'CALL lake.expire_snapshots(older_than => now())' and 'CALL lake.cleanup_old_files(cleanup_all => true)' with 'CALL ducklake_expire_snapshots($ER_LAKE_ALIAS, older_than => now())' and 'CALL ducklake_cleanup_old_files($ER_LAKE_ALIAS, cleanup_all => true)', since DuckDB reads 'lake.' there as a schema qualifier, not the attached catalog.
- **Log:** `.loop/logs/ER-018.attempt-2.log`
