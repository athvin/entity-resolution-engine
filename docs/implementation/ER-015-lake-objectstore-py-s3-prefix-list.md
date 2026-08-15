---
id: ER-015
title: "lake/objectstore.py (S3 prefix list/delete) + lake/catalog.py (direct psycopg catalog connection)"
milestone: M1
status: in_progress
kind: code
size: S
gates: full
depends_on: ["ER-003", "ER-011"]
spec_refs: ["s4-0", "s4-0b", "s6-1", "s7-1", "s8-1"]
gap_refs: ["B1", "M22", "M19"]
provides: ["src/er/lake/env.py::require_env", "src/er/lake/env.py::require_int_env", "src/er/lake/env.py::require_bool_env", "src/er/lake/env.py::MissingEnvError", "src/er/lake/objectstore.py::ObjectStore", "src/er/lake/objectstore.py::ObjectStore.from_env", "src/er/lake/objectstore.py::ObjectStore.put_bytes", "src/er/lake/objectstore.py::ObjectStore.get_bytes", "src/er/lake/objectstore.py::ObjectStore.list_prefix", "src/er/lake/objectstore.py::ObjectStore.delete_prefix", "src/er/lake/objectstore.py::PrefixGuardError", "src/er/lake/catalog.py::catalog_connect", "src/er/lake/catalog.py::server_version", "src/er/lake/catalog.py::metadata_schema_exists", "src/er/lake/catalog.py::drop_metadata_schema", "src/er/lake/catalog.py::read_data_path", "src/er/lake/catalog.py::advisory_lock", "src/er/lake/catalog.py::try_advisory_lock", "src/er/lake/catalog.py::advisory_lock_key", "tests/integration/test_objectstore_catalog.py"]
consumes: ["src/er/config/schema.py::Config", "scripts/ci/itest.sh", "docker/compose.yaml::x-er-env"]
owns: ["src/er/lake/env.py", "src/er/lake/objectstore.py", "src/er/lake/catalog.py", "tests/integration/test_objectstore_catalog.py"]
protected_paths: ["docker/compose.yaml"]
extra_paths: ["pyproject.toml", "uv.lock", "src/er/lake/__init__.py"]
attempts: 1
verify: "bash scripts/ci/itest.sh tests/integration/test_objectstore_catalog.py -q"
branch: "ticket/ER-015-lake-objectstore-py-s3-prefix-list"
commit: ""
spec_sha: "28d8d8e366a7b49b"
updated_at: "2026-08-15T00:40:55Z"
session: a0405b5f-5796-474d-a04b-84c780fc646d
---
## Description

Provide the two direct clients the lake needs outside DuckDB: an S3 client for `DATA_PATH` prefix listing/deletion and model-artifact round-trips, and a direct Postgres catalog connection for the metadata schema and the tenant advisory lock. B1 left the pipeline with no S3 credentials path at all, M22's namespaced harness cannot reclaim a namespace without prefix deletion plus `DROP SCHEMA`, and S4.0's `er lake reset` and S4.0b's single-writer lock have no other implementation route. Also lands the one `ER_*` reader (`require_env`) that raises the S4.0b `ERR_ENV_MISSING: <name>` failure, so no module invents a second env convention.

## Scope

### In scope

- `require_env` / `require_int_env` / `require_bool_env`: read from `os.environ`, raise `MissingEnvError('ERR_ENV_MISSING: <name>')` with `code = 2` when absent or empty, validate integer/boolean positions
- `ObjectStore.from_env()` built from `ER_S3_ENDPOINT` (host:port, no scheme), `ER_S3_ACCESS_KEY_ID`, `ER_S3_SECRET_ACCESS_KEY`, `ER_S3_REGION`, `ER_S3_URL_STYLE`, `ER_S3_USE_SSL`
- `put_bytes` / `get_bytes` / `list_prefix` / `delete_prefix` over `s3://` URIs, with pagination beyond 1000 keys and a guard refusing a bucket-root or empty prefix
- `catalog_connect(dsn)` context manager over `$ER_CATALOG_DSN` (`postgresql://` accepted verbatim), plus `server_version`, `metadata_schema_exists`, `drop_metadata_schema` (idempotent `DROP SCHEMA ... CASCADE`), `read_data_path`
- `advisory_lock(tenant)` / `try_advisory_lock(tenant)` on a session-scoped connection with `advisory_lock_key(tenant)` a deterministic bigint; released on context exit including on exception
- An integration test that provisions and reclaims its own throwaway namespace

### Out of scope

- Any DuckDB connection, ATTACH, or extension load — ER-016 owns `ducklake.py` and its SQL-literal rendering helper
- Wiring the advisory lock onto CLI commands and the exit-3 refusal message (ER-024)
- `er lake reset` itself (ER-020) and lake maintenance (ER-025)
- The session-namespaced pytest fixture (ER-018) — this ticket's test must not depend on it

## Design decisions applied

Closes B1's credentials/`DATA_PATH` arm, M22 (namespace teardown is only possible with prefix delete + `DROP SCHEMA`) and the M19 arm that gives `er lake reset` an implementation. Easy to miss: (1) `ER_S3_ENDPOINT` is `host:port` with NO scheme (S7.1) — the client, not the caller, adds `http`/`https` from `ER_S3_USE_SSL`; (2) `delete_prefix` MUST refuse anything shallower than a namespace prefix (empty, `s3://lake`, `s3://lake/`) — harness teardown and `er lake reset` both call it and a bucket-wide delete is unrecoverable; (3) the advisory lock is keyed on `tenant` and taken on `$ER_CATALOG_DSN` for the process lifetime (S4.0b) — it must live on its own long-lived connection, not on a pooled or per-statement one, or the lock silently releases; (4) `postgresql://` DSNs need no translation; (5) `MissingEnvError` exposes `code = 2` so `errors.exit_code_for` (ER-014) maps it without an import cycle; (6) this ticket runs before the harness exists, so its test derives its own `er_test_<ulid>` schema and `s3://lake/test/<ulid>/` prefix and reclaims both in a `finally`; (7) if `psycopg` is not already in the pinned runtime set, add it to `pyproject.toml`/`uv.lock` AND add the matching S2.1 row (`Asserted by: er doctor; uv.lock`) — a dependency with no S2.1 row is not asserted by `er doctor` (S2.1 rules). Those three files are the only permitted edits outside `owns`.

## Acceptance criteria

- [ ] AC1: `ObjectStore.from_env()` round-trips: `put_bytes` then `get_bytes` returns identical bytes; `list_prefix` returns exactly the written key; after `delete_prefix`, `list_prefix` returns an empty list.
- [ ] AC2: `delete_prefix` removes all 1005 objects written under one prefix (list pagination beyond the 1000-key page is handled), and raises `PrefixGuardError` for `''`, `s3://lake` and `s3://lake/` without deleting anything.
- [ ] AC3: `server_version()` returns the running catalog's version string starting with `16`; `drop_metadata_schema(s)` returns True the first time and False (no error) the second, and afterwards `metadata_schema_exists(s)` is False.
- [ ] AC4: With `advisory_lock('test')` held on one connection, `try_advisory_lock('test')` on a second connection returns False and `try_advisory_lock('other')` returns True; after the first context exits — including via a raised exception — `try_advisory_lock('test')` returns True. `advisory_lock_key` is stable per tenant and differs across tenants.
- [ ] AC5: With `ER_S3_ACCESS_KEY_ID` unset, `ObjectStore.from_env()` raises `MissingEnvError` whose message is exactly `ERR_ENV_MISSING: ER_S3_ACCESS_KEY_ID` and whose `code` is 2; `require_int_env('ER_DUCKDB_THREADS')` raises on the value `2; DROP`.
- [ ] AC6: Neither `objectstore.py` nor `catalog.py` imports `duckdb` (asserted on the module import graph), and the integration test leaves no schema and no object behind (verified at teardown from a second connection).

## Tests

- tests/integration/test_objectstore_catalog.py::test_prefix_round_trip
- tests/integration/test_objectstore_catalog.py::test_delete_prefix_paginates_beyond_1000
- tests/integration/test_objectstore_catalog.py::test_delete_prefix_refuses_bucket_root
- tests/integration/test_objectstore_catalog.py::test_catalog_server_version
- tests/integration/test_objectstore_catalog.py::test_drop_metadata_schema_is_idempotent
- tests/integration/test_objectstore_catalog.py::test_advisory_lock_is_exclusive_per_tenant_and_released_on_error
- tests/integration/test_objectstore_catalog.py::test_missing_env_raises_err_env_missing
- tests/integration/test_objectstore_catalog.py::test_modules_do_not_import_duckdb

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_objectstore_catalog.py -q
uv run mypy --strict src/er/lake/env.py src/er/lake/objectstore.py src/er/lake/catalog.py
uv run ruff check src/er/lake tests/integration/test_objectstore_catalog.py
```

## Definition of Done

- S3 prefix list/put/get/delete with pagination and a root-prefix guard
- Catalog connect, `server_version`, schema existence/drop, `read_data_path`, tenant advisory lock with guaranteed release
- `require_env` raising the literal `ERR_ENV_MISSING: <name>` with `code = 2`
- Integration test provisions and reclaims its own namespace; no reliance on the ER-018 fixture
- If `psycopg` was added: `pyproject.toml`, `uv.lock` and the S2.1 row all updated together
- Verify command passes; mypy --strict clean on the three modules
