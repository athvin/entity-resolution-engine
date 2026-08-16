---
id: ER-049
title: "Splink scratch isolation: splink_api(), assert_no_splink_relations_in_lake, thread/insertion-order pinning, leak test in its own namespace"
milestone: M2
status: in_progress
kind: code
size: M
gates: full
depends_on: ["ER-016", "ER-018", "ER-048"]
spec_refs: ["s4-0b", "s4-3-4", "s8-1", "s8-3"]
gap_refs: ["M17"]
provides: ["src/er/matching/api.py::splink_api", "src/er/matching/api.py::assert_no_splink_relations_in_lake", "src/er/matching/api.py::SPLINK_SCRATCH_SCHEMA", "tests/integration/test_splink_isolation.py"]
consumes: ["src/er/lake/ducklake.py::connect", "src/er/matching/model.py::build_settings", "tests/conftest.py::lake_ns", "tests/conftest.py::sub_namespace", "src/er/errors.py"]
owns: ["src/er/matching/api.py", "tests/integration/test_splink_isolation.py"]
protected_paths: []
extra_paths: ["src/er/lake/ducklake.py"]
attempts: 2
verify: "bash scripts/ci/itest.sh tests/integration/test_splink_isolation.py -q"
branch: "ticket/ER-049-splink-scratch-isolation-splink-api-assert"
commit: ""
spec_sha: "2e62460d9f41a842"
updated_at: "2026-08-16T04:21:26Z"
session: 88a7c0d7-65eb-4b19-81f3-88a30d26c09b
---
## Description

Provide the one construction every Splink call site uses — `splink_api(conn)` returning `DuckDBAPI(connection=conn, output_schema='splink_scratch')` over a connection whose primary database is `:memory:` with DuckLake attached as `lake` (S4.0b) — plus `assert_no_splink_relations_in_lake(conn)`, the executable form of the guarantee that no `__splink__` relation ever lands in the lake (M17). The API also pins the determinism-relevant settings on the connection it hands Splink (`threads` from `ER_DUCKDB_THREADS`, insertion order preserved), so repeated runs produce identical output ordering. The leak test runs in its own sub-namespace and proves both directions: the intermediates exist in the in-memory database and zero of them reach `lake`.

## Scope

### In scope

- `src/er/matching/api.py`: `splink_api()`, `SPLINK_SCRATCH_SCHEMA`, `assert_no_splink_relations_in_lake()`
- Creation of the `splink_scratch` schema in the in-memory primary database if absent, never in `lake`
- `SET threads` / insertion-order pinning applied on the connection Splink receives
- An integration leak test in its own sub-namespace: build a small local temp frame, run one Splink prediction through `build_settings`, assert zero `__splink__%` relations in `lake` and a non-zero count in the in-memory database
- A negative arm proving the assertion has teeth

### Out of scope

- Persisting scored pairs to `match_scores` — the single `MERGE INTO` write is ER-058/ER-065
- TF registration (`register_term_frequency_lookup`) — ER-053
- Wiring the assertion into `er doctor` or the T-INV-1 finalizer (ER-022 exists; ER-072 arms the finalizer) — this ticket only exports the callable
- Reading `lake.main.int_std_records` as the leak-test input: this ticket does not depend on the intermediate models and must build its own local frame
- Changing the connection factory's statement sequence in `lake/ducklake.py` beyond exposing the scratch schema

## Design decisions applied

Closes M17 (Splink's unqualified `CREATE TABLE`s would otherwise become Postgres catalog writes plus Parquet objects plus one snapshot per statement, corrupting the snapshot history T-SNAP-1 depends on). Easy to miss: the guarantee comes from the *primary* database being `:memory:` — `output_schema` alone is not sufficient if the lake is the default catalog, so the assertion must be able to fail; input frames must be read fully qualified (`lake.main.…`) or materialized into local temp tables first; the label-propagation loop is bound by the same rule (S4.0b), so this API is the only place a Splink-facing connection is constructed; the leak test must use its own namespace so a teardown failure cannot mask a leak into another test's lake.

## Acceptance criteria

- [ ] AC1: `splink_api(conn)` returns a `DuckDBAPI` whose `output_schema` is `splink_scratch`, and after the call a schema named `splink_scratch` exists in the in-memory primary database while `lake` has no schema of that name (checked via `duckdb_schemas()` filtered on `database_name`).
- [ ] AC2: After running one Splink prediction over a 20-row local temp frame using `build_settings(cfg)`, `assert_no_splink_relations_in_lake(conn)` passes while the same `__splink__%` pattern matches at least one relation in the in-memory database — both counts are asserted, so a no-op run cannot pass vacuously.
- [ ] AC3: The lake's current snapshot version is unchanged across the whole prediction (no snapshot is committed by Splink), read before and after from the snapshot helper.
- [ ] AC4: A deliberately misconstructed API (DuckLake as the default catalog / `output_schema` omitted) causes `assert_no_splink_relations_in_lake` to raise, asserted with `pytest.raises` — the guard is proven to have teeth.
- [ ] AC5: On the connection Splink receives, `current_setting('threads')` equals `$ER_DUCKDB_THREADS` and insertion order is preserved; running the same prediction twice yields identical row order and identical probabilities.
- [ ] AC6: The test executes in its own sub-namespace and, after teardown, that namespace's catalog metadata schema is gone and no relation matching `__splink__%` exists in any other namespace used by the session.

## Tests

- tests/integration/test_splink_isolation.py::test_scratch_schema_is_in_memory_not_lake
- tests/integration/test_splink_isolation.py::test_prediction_leaves_zero_splink_relations_in_lake
- tests/integration/test_splink_isolation.py::test_prediction_commits_no_lake_snapshot
- tests/integration/test_splink_isolation.py::test_assertion_fails_on_misconfigured_api
- tests/integration/test_splink_isolation.py::test_threads_and_insertion_order_pinned
- tests/integration/test_splink_isolation.py::test_runs_in_own_sub_namespace_and_tears_down

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_splink_isolation.py -q
uv run mypy --strict src/er/matching
uv run pytest tests/unit/matching/test_settings_builder.py -q
bash scripts/gates.sh
```

## Definition of Done

- All acceptance criteria met and the verify command passes
- `splink_api()` is the only construction of `DuckDBAPI` in the repository (grep-asserted or reviewed)
- `assert_no_splink_relations_in_lake` is exported for reuse by `er doctor` and the T-INV-1 finalizer
- The negative arm demonstrably fails without the isolation, so the test is not vacuous
- `mypy --strict src/er/matching` clean
- Committed on main with the board updated
