---
id: ER-035
title: "S5.1 schema evolution: idempotent DDL, additive ALTER, breaking-change exit 3, version-bump full rebuild + rebuild_reason, bounded time travel, doctor drift check"
milestone: M1
status: in_progress
kind: code
size: M
gates: full
depends_on: ["ER-019", "ER-022", "ER-034"]
spec_refs: ["s4-0", "s4-2", "s4-7", "s5", "s5-0", "s5-1", "s5-2", "s8-1", "s8-3"]
gap_refs: ["M16", "M2", "M26", "B2", "NEW-S5.1"]
provides: ["src/er/lake/ddl.py::plan_evolution", "src/er/lake/ddl.py::preflight_schema", "src/er/lake/ddl.py::SchemaBreakingError", "src/er/lake/ddl.py::ERR_SCHEMA_BREAKING", "src/er/versions.py::rebuild_reason_for", "src/er/versions.py::run_is_inc_accountable", "check:doctor.schema_drift"]
consumes: ["src/er/lake/ddl.py::apply", "src/er/lake/model.py::TableSpec", "src/er/lake/model.py::REGISTRY", "src/er/lake/ducklake.py::connect", "src/er/versions.py::check_mode_preconditions", "src/er/versions.py::RunFingerprint", "src/er/cli.py::doctor", "src/er/cli.py::app", "src/er/errors.py", "RunContext", "relation:runs", "relation:run_stages", "tests/conftest.py::lake_conn", "tests/conftest.py::er_env"]
owns: ["tests/integration/test_schema_evolution.py", "tests/unit/test_schema_evolution.py"]
protected_paths: []
extra_paths: ["src/er/lake/ddl.py", "src/er/versions.py", "src/er/cli.py", "src/er/doctor.py"]
attempts: 1
verify: "bash scripts/ci/itest.sh tests/integration/test_schema_evolution.py -q && uv run pytest tests/unit/test_schema_evolution.py -q"
branch: "ticket/ER-035-s5-1-schema-evolution-idempotent-ddl"
commit: ""
spec_sha: "2abcfe433c322f74"
updated_at: "2026-08-15T21:25:33Z"
session: cd78362e-44af-4243-957e-9a4e433a4b4d
---
## Description

Implement S5.1 end to end for the `ddl.py`-owned relations: `er init` stays an idempotent no-op on an initialised lake, the only permitted change to an existing relation is `ALTER TABLE … ADD COLUMN <name> <type>` with no NOT NULL and no DEFAULT, and any breaking difference aborts `er init` and every stage preflight with exit `3` and the named `ERR_SCHEMA_BREAKING` message. Add the version-bump arm — a `std_version` or `survivorship_version` change records `runs.rebuild_reason` and runs the affected stages non-incrementally — and the `er doctor` drift check. The evolution *plan* is a pure function over two registries so the breaking/additive classification is unit-testable without a lake.

## Scope

### In scope

- `plan_evolution(live_columns, spec) -> EvolutionPlan` classifying each difference as `noop`, `add_column` or `breaking` (drop, rename, type narrowing/change, removed `accepted_values` member).
- Emitting additive `ALTER TABLE … ADD COLUMN` with no NOT NULL and no DEFAULT, leaving pre-existing rows NULL in the new column.
- `preflight_schema(conn)` called by `er init` and by every stage's entry path, aborting with exit `3` and the literal message `ERR_SCHEMA_BREAKING: <relation>.<column>: <live_type> -> <declared_type>` before any snapshot is committed.
- `rebuild_reason_for(prior, current)` returning `std_version_bump` / `survivorship_version_bump` / `correction_pass` / `operator` / NULL, written to `runs.rebuild_reason`, and the rule that a bump runs `er standardize` without `--changed-only` and `er assemble` without `--touched-only`.
- `run_is_inc_accountable(run_id)` — False for a run with non-NULL `rebuild_reason` — the predicate T-INC-2 (M4) will use to exclude planned rebuilds from its accounting.
- One `er doctor` check row per `ddl.py`-owned relation comparing live columns against the registry.
- Bounded time travel: a snapshot captured before an additive ALTER reads back with an explicit projection, and the reference version is read from `run_stages`, never written as a literal.

### Out of scope

- dbt-owned relations: they evolve through their enforced contract and a violation fails `dbt build` with exit `1` (S5.1); this ticket issues no DDL against them and does not change `on_schema_change`.
- Any automatic destructive migration, column backfill policy beyond leaving NULL, or migration script — the operator path is explicit and out of scope.
- The drift guard on the run fingerprint itself (`config_hash`/`model_version`) — ER-034 owns it; this ticket only adds the `rebuild_reason` and non-incremental-flag consequences.
- Time travel across a breaking migration: it is not supported and no test may rely on it.

## Design decisions applied

Implements gap entries M16, M2, M26, B2 and NEW-S5.1. Constraints: (1) The two abort codes differ and both are asserted in the same test — `er init` and stage preflight exit `3` (S4.7 `precondition` class) while `er doctor` exits `1`, because a doctor check failure is a check failure under the S4.0 table, not a precondition failure. (2) DuckLake permits neither NOT NULL nor DEFAULT on an added column (S5.1), so an additive column is always nullable; the registry's NOT NULL declarations apply only at CREATE time. (3) A breaking change aborts *before* any snapshot is committed — assert the lake snapshot version is identical before and after the failed invocation. (4) A version-bump run is explicitly outside T-INC-2's accounting; ship `run_is_inc_accountable` now so ER-092 does not re-derive the rule. (5) Time travel is supported across additive changes only, and a query against a pre-ALTER snapshot MUST project explicitly rather than `SELECT *`. (6) The escalation mechanics (`--allow-escalate`, exit `3` on drift) belong to ER-034; this ticket consumes that decision and adds only the rebuild bookkeeping.

## Acceptance criteria

- [ ] AC1: Adding a nullable column to a `ddl.py`-owned `TableSpec` and re-running `er init` on an initialised lake exits `0`, issues exactly one `ALTER TABLE … ADD COLUMN` with neither `NOT NULL` nor `DEFAULT` in the emitted statement, and pre-existing rows read back with the new column NULL.
- [ ] AC2: Running `er init` twice on an initialised, unchanged lake exits `0` both times, reports `exists` for every relation on the second run, and issues zero DDL statements (live column sets identical before and after).
- [ ] AC3: Narrowing a declared column type in the registry makes `er init` exit `3` with the literal message `ERR_SCHEMA_BREAKING: <relation>.<column>: <live_type> -> <declared_type>`, and the lake's snapshot version is unchanged across the failed invocation; the same registry state makes a stage's preflight exit `3` before it writes its `run_stages` row.
- [ ] AC4: Against the same drifted registry, `er doctor` exits `1` (not `3`) and prints a failing check row naming the relation and column — the two codes are asserted distinct in one test.
- [ ] AC5: A run whose `versions.std_version` differs from the last successful run's records `runs.rebuild_reason='std_version_bump'` and invokes `er standardize` without `--changed-only`; a `survivorship_version` change records `survivorship_version_bump` and invokes `er assemble` without `--touched-only`.
- [ ] AC6: `run_is_inc_accountable(run_id)` returns False for a run with non-NULL `rebuild_reason` and True for a run with NULL `rebuild_reason`.
- [ ] AC7: After an additive ALTER, selecting the pre-ALTER column list `FROM lake.main.<relation> AT (VERSION => :snap)` — with `:snap` read at runtime from `run_stages.snapshot_end` — returns the pre-ALTER rows, and no absolute snapshot version appears anywhere in the test.
- [ ] AC8: `plan_evolution` classifies drop / rename / type-change / removed-domain-member as breaking and added-column as additive, over a table of hand-built registry pairs, with no lake connection.

## Tests

- tests/unit/test_schema_evolution.py::test_plan_evolution_classifies_additive_and_breaking
- tests/unit/test_schema_evolution.py::test_added_column_ddl_has_no_not_null_and_no_default
- tests/integration/test_schema_evolution.py::test_init_is_idempotent_and_issues_no_ddl
- tests/integration/test_schema_evolution.py::test_additive_alter_applies_and_backfills_null
- tests/integration/test_schema_evolution.py::test_breaking_change_exits_3_with_named_message_and_no_snapshot
- tests/integration/test_schema_evolution.py::test_doctor_reports_drift_and_exits_1
- tests/integration/test_schema_evolution.py::test_version_bump_sets_rebuild_reason_and_runs_non_incrementally
- tests/integration/test_schema_evolution.py::test_time_travel_across_an_additive_change

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_schema_evolution.py -q && uv run pytest tests/unit/test_schema_evolution.py -q
bash scripts/ci/itest.sh tests/integration/test_init.py tests/integration/test_doctor.py -q
uv run mypy --strict src/er
uv run ruff check . && uv run ruff format --check .
```

## Definition of Done

- All acceptance criteria have a named passing test
- Verify command passes; `er init` and `er doctor` integration tests still pass
- Breaking changes abort before any snapshot is committed, with the literal `ERR_SCHEMA_BREAKING` message
- No DDL is issued against a dbt-owned relation by this ticket
- `run_is_inc_accountable` and `rebuild_reason_for` are exported for M4
- No test references an absolute snapshot version
- mypy --strict and ruff clean
