---
id: ER-016
title: "lake/ducklake.py S4.0b: memory primary catalog, ATTACH lake, S3 secret, splink_scratch, thread/memory pinning, snapshot helpers, extension-dir fallback"
milestone: M1
status: todo
kind: code
size: M
gates: full
depends_on: ["ER-007", "ER-009", "ER-011", "ER-015"]
spec_refs: ["s4", "s4-0b", "s5-2", "s7-1", "s7-2", "s7-3", "s10-3"]
gap_refs: ["B1", "M17", "M22", "M24"]
provides: ["src/er/lake/ducklake.py::connect", "src/er/lake/ducklake.py::attach_statements", "src/er/lake/ducklake.py::sql_literal", "src/er/lake/ducklake.py::env_literal", "src/er/lake/ducklake.py::ducklake_uri", "src/er/lake/ducklake.py::current_snapshot", "src/er/lake/ducklake.py::snapshot_range", "src/er/lake/ducklake.py::detach", "src/er/lake/ducklake.py::LAKE_ALIAS", "src/er/lake/ducklake.py::SPLINK_OUTPUT_SCHEMA", "src/er/lake/ducklake.py::EXTENSION_DIRECTORY_DEFAULT", "tests/integration/test_connection_model.py"]
consumes: ["src/er/lake/env.py::require_env", "src/er/lake/env.py::require_int_env", "src/er/lake/env.py::require_bool_env", "src/er/lake/catalog.py::catalog_connect", "src/er/lake/objectstore.py::ObjectStore", "src/er/config/schema.py::Config", "docker/Dockerfile::/opt/duckdb_extensions", "scripts/ci/itest.sh"]
owns: ["src/er/lake/ducklake.py", "tests/integration/test_connection_model.py"]
protected_paths: ["docker/Dockerfile", "docker/compose.yaml"]
extra_paths: []
attempts: 0
verify: "bash scripts/ci/itest.sh tests/integration/test_connection_model.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

Implement the single connection factory every Python stage opens, emitting the S4.0b statement block verbatim and in order: extension directory and autoload settings, the three extensions, the pinned `threads`/`memory_limit`, the `er_s3` secret and the `ATTACH` of the lake as `lake` over a `:memory:` primary database. S4.0b is normative because the engine forces it — `getenv()` does not exist on a Python connection and `ATTACH` takes only a bare string literal — so every value is substituted in Python through one escaping helper and no shell-style `$VAR` may survive into the rendered text. The ticket also lands the `splink_scratch` output schema that keeps `__splink__` intermediates out of DuckLake (M17) and the snapshot helpers `run_stages` records ranges with.

## Scope

### In scope

- `connect()` context manager: renders and executes the S4.0b block statement-for-statement, in order, then yields the connection and DETACHes/closes on exit
- `sql_literal` / `env_literal`: single-quoted with embedded quotes doubled for string positions; validated and unquoted for the integer (`ER_DUCKDB_THREADS`) and boolean (`ER_S3_USE_SSL`) positions
- `ducklake_uri(dsn)` assembling the full `ducklake:postgres:<dsn>` literal in Python
- `SET threads` / `SET memory_limit` applied on every connection from `ER_DUCKDB_THREADS` / `ER_DUCKDB_MEMORY_LIMIT`
- `splink_scratch` schema created in the in-memory primary database and exposed as `SPLINK_OUTPUT_SCHEMA`
- `current_snapshot(conn)` and the `snapshot_range` context manager yielding `(snapshot_start, snapshot_end)`
- Extension-directory fallback: `/opt/duckdb_extensions` by default, overridable by `ER_DUCKDB_EXTENSION_DIR` for a non-image developer run, with autoinstall/autoload left disabled either way

### Out of scope

- Creating any relation — `ddl.py` (ER-019) and `er init` (ER-020) own DDL; this factory only attaches
- The dbt-side rendering of the same contract (`profiles.yml` keys) — ER-008 owns it; this ticket must not edit the profile
- The advisory lock, `run_stages` persistence, and `er doctor`'s assertion list (ER-024, ER-023, ER-022)
- Constructing a Splink `DuckDBAPI` or registering TF tables (ER-049, ER-053)

## Design decisions applied

Closes B1's attach-sequence arm, M17 (`splink_scratch` and no `__splink__` relation in the lake), M22 (a namespace is exactly `(METADATA_SCHEMA, DATA_PATH)` and nothing else in the sequence varies, S7.2) and M24's requirement that `ER_DUCKDB_THREADS`/`ER_DUCKDB_MEMORY_LIMIT` be applied with `SET` on every connection because DuckDB reads neither cgroup. Easy to miss: (1) the DuckDB primary database is `:memory:` and DuckLake is NEVER the default catalog — every lake reference is `lake.main.<relation>`; (2) no `getenv()`, no `||` concatenation and no `?` parameter may appear in the block, and a bare `$NAME` in DuckDB SQL is a prepared-statement parameter marker while `'$NAME'` is the literal text — a block that mixed those forms would attach the wrong lake without erroring; (3) `ER_S3_ENDPOINT` is `host:port` with no scheme; (4) a missing or empty variable raises `ERR_ENV_MISSING: <name>` (exit 2) instead of emitting an empty literal, and it must raise BEFORE any statement executes; (5) `INSTALL` is a no-op against the baked directory and autoinstall/autoload stay false; (6) tests may assert snapshot ranges and time travel but MUST NOT assert snapshot counts (S4 preamble); (7) the ER-018 harness does not exist yet, so this test sets its own `ER_LAKE_METADATA_SCHEMA`/`ER_LAKE_DATA_PATH` namespace and reclaims it through `catalog.py`/`objectstore.py` in a `finally`.

## Acceptance criteria

- [ ] AC1: `attach_statements(env)` returns the S4.0b statements in exactly that order — extension_directory, autoinstall, autoload, INSTALL/LOAD ducklake, INSTALL/LOAD postgres, INSTALL/LOAD httpfs, SET threads, SET memory_limit, CREATE OR REPLACE SECRET er_s3, ATTACH IF NOT EXISTS — and the rendered text contains no `getenv(`, no `$`-prefixed identifier, no `||` and no `?`.
- [ ] AC2: A value containing a single quote (`ER_S3_SECRET_ACCESS_KEY=o'brien`) is rendered with the quote doubled and the connection still opens; `ER_DUCKDB_THREADS=2; DROP TABLE x` is rejected before any statement executes; unsetting `ER_LAKE_DATA_PATH` raises `ERR_ENV_MISSING: ER_LAKE_DATA_PATH` with zero statements executed.
- [ ] AC3: Inside `connect()`: `SELECT current_database()` returns the in-memory primary database (not `lake`), `SELECT * FROM lake.snapshots()` succeeds, `duckdb_extensions()` reports `ducklake`, `postgres_scanner` and `httpfs` loaded, and `current_setting('autoinstall_known_extensions')` and `current_setting('autoload_known_extensions')` are both false.
- [ ] AC4: `current_setting('threads')` equals `ER_DUCKDB_THREADS` and `current_setting('memory_limit')` equals `ER_DUCKDB_MEMORY_LIMIT` on every connection opened by the factory.
- [ ] AC5: Creating a table in `SPLINK_OUTPUT_SCHEMA` leaves zero rows in `duckdb_tables()` filtered on `database_name = 'lake'`, and zero relations matching `__splink__%` exist in `lake` after the connection closes.
- [ ] AC6: Around an insert wrapped in `snapshot_range`, the yielded `(start, end)` satisfies `end >= start`, the inserted rows are visible at `AT (VERSION => end)` and absent at `AT (VERSION => start)`; no test asserts a snapshot count.
- [ ] AC7: With `ER_DUCKDB_EXTENSION_DIR` set to a directory holding the three extensions, the rendered `SET extension_directory` equals that path and the connection opens; unset, it renders `/opt/duckdb_extensions`.
- [ ] AC8: After the `connect()` context exits, the connection is closed and a second `connect()` in the same process attaches the same namespace successfully.

## Tests

- tests/integration/test_connection_model.py::test_statement_block_matches_spec_order
- tests/integration/test_connection_model.py::test_no_shell_substitution_and_quotes_are_doubled
- tests/integration/test_connection_model.py::test_missing_env_raises_before_any_statement
- tests/integration/test_connection_model.py::test_primary_database_is_memory_and_lake_is_attached
- tests/integration/test_connection_model.py::test_extensions_loaded_with_autoinstall_disabled
- tests/integration/test_connection_model.py::test_threads_and_memory_limit_are_pinned
- tests/integration/test_connection_model.py::test_splink_scratch_stays_out_of_lake
- tests/integration/test_connection_model.py::test_snapshot_range_supports_time_travel
- tests/integration/test_connection_model.py::test_extension_directory_fallback
- tests/integration/test_connection_model.py::test_reconnect_after_detach

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_connection_model.py -q
uv run mypy --strict src/er/lake/ducklake.py
uv run ruff check src/er/lake/ducklake.py tests/integration/test_connection_model.py
```

## Definition of Done

- S4.0b block emitted verbatim and in order through one escaping helper; no shell-style substitution anywhere
- `:memory:` primary database, lake attached as `lake`, `splink_scratch` present and provably outside the lake
- `threads`/`memory_limit` set on every connection; extension directory fallback implemented
- Snapshot helpers return ranges usable for time travel; no snapshot-count assertion in any test
- Test provisions and reclaims its own namespace; Dockerfile and compose.yaml unmodified
- Verify command passes; mypy --strict clean
