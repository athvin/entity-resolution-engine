---
id: ER-019
title: "lake/ddl.py: apply/evolve/ownership guard over the registry"
milestone: M1
status: in_progress
kind: code
size: M
gates: full
depends_on: ["ER-017", "ER-018"]
spec_refs: ["s3", "s5", "s5-0", "s5-1"]
gap_refs: ["B2", "M4", "M15"]
provides: ["src/er/lake/ddl.py::apply", "src/er/lake/ddl.py::evolve", "src/er/lake/ddl.py::diff", "src/er/lake/ddl.py::live_columns", "src/er/lake/ddl.py::assert_ddl_owned", "src/er/lake/ddl.py::SchemaBreakingError", "src/er/lake/ddl.py::OwnershipViolation", "src/er/lake/ddl.py::RelationAction", "tests/integration/test_ddl_apply.py"]
consumes: ["src/er/lake/model.py::REGISTRY", "src/er/lake/model.py::DDL_OWNED", "src/er/lake/model.py::DBT_OWNED", "src/er/lake/model.py::create_table_sql", "src/er/lake/ducklake.py::connect", "tests/conftest.py::lake_conn", "tests/conftest.py::er_env", "scripts/ci/itest.sh"]
owns: ["src/er/lake/ddl.py", "tests/integration/test_ddl_apply.py"]
protected_paths: ["src/er/lake/model.py", "tests/conftest.py"]
extra_paths: []
attempts: 1
verify: "bash scripts/ci/itest.sh tests/integration/test_ddl_apply.py -q"
branch: "ticket/ER-019-lake-ddl-py-apply-evolve-ownership"
commit: ""
spec_sha: "2abcfe433c322f74"
updated_at: "2026-08-15T13:12:56Z"
session: d2cee831-7040-46ad-b16f-53881432d5a3
---
## Description

Apply the TableSpec registry to a live namespace: idempotent `CREATE TABLE IF NOT EXISTS` for the `ddl.py`-owned relations only, additive `ALTER TABLE … ADD COLUMN` evolution, and the ownership guard that makes S5.0's two-owner rule executable rather than aspirational. B2 is that the v1.0 DDL could not execute at all against DuckLake and M4 is that two systems claimed the same relations; S5.1 adds that a breaking difference between the registry and the live catalog must abort with exit 3 and the literal `ERR_SCHEMA_BREAKING` message rather than migrate destructively. This is the module `er init` (ER-020) and every stage preflight call.

## Scope

### In scope

- `apply(conn)`: emit `create_table_sql` for every `ddl.py`-owned spec, returning `(relation, 'created'|'exists')` per relation
- `diff(conn)` / `live_columns(conn, relation)`: compare the live catalog against the registry, classifying each difference as additive or breaking
- `evolve(conn)`: issue `ALTER TABLE … ADD COLUMN <name> <type>` for additive differences, with no `NOT NULL` and no `DEFAULT`
- `SchemaBreakingError` with the literal message `ERR_SCHEMA_BREAKING: <relation>.<column>: <live_type> -> <declared_type>` and `code = 3`, raised before any statement executes
- `assert_ddl_owned(relation)` / `OwnershipViolation`: refuse to emit DDL for a dbt-owned relation

### Out of scope

- The `er init` / `er lake reset` commands, their stdout format and the `DATA_PATH` immutability check (ER-020)
- `runs` / `run_stages` persistence and the stage preflight wiring (ER-023, ER-035)
- dbt-owned relations: never create, alter or drop them; their contracts are ER-008/ER-021's
- Backfilling a newly added column with data (the owning stage does it under S5.1)

## Design decisions applied

Closes B2, M4 and the M15 arm that `raw_records`' deletion columns exist from M1. Easy to miss: (1) `apply` touches the fourteen `ddl.py`-owned relations ONLY — the dbt-owned relations are created by the first `dbt run` and `ddl.py` never issues DDL against them (S5.0, S3 layout rule); (2) the only permitted change to an existing relation is `ADD COLUMN` with no `NOT NULL` and no `DEFAULT`, because DuckLake permits neither on an added column; (3) dropping, renaming, narrowing or re-typing a column — and a live column the registry no longer declares — are all breaking, abort with exit 3 and commit nothing; (4) every statement is derived from the registry; `ddl.py` must not restate a column list, which is why `src/er/lake/model.py` is protected here; (5) running `apply` on an initialised lake is a no-op that exits 0 (S5.1); (6) tests assert relation state, never snapshot counts.

## Acceptance criteria

- [ ] AC1: On an empty namespace, `apply(conn)` returns action `created` for exactly the fourteen `ddl.py`-owned relations, each is then queryable with `count(*) == 0`, and zero dbt-owned relations exist.
- [ ] AC2: A second `apply(conn)` returns `exists` for all fourteen, executes no `CREATE`, and leaves every column set and row count unchanged.
- [ ] AC3: With a hand-made `int_std_records` carrying a deliberately wrong column set, `apply(conn)` exits 0 and leaves that relation byte-identical; calling the emitter directly for a dbt-owned spec raises `OwnershipViolation`.
- [ ] AC4: With one nullable registry column removed from a live relation, `evolve(conn)` issues exactly one `ALTER TABLE … ADD COLUMN <name> <type>` containing neither `NOT NULL` nor `DEFAULT`; the column exists afterwards and is NULL on the pre-existing rows.
- [ ] AC5: With a live column typed `VARCHAR` where the registry declares `BIGINT`, `evolve(conn)` raises `SchemaBreakingError` with exactly `ERR_SCHEMA_BREAKING: <relation>.<column>: VARCHAR -> BIGINT`, `code == 3`, and no `ALTER` is executed; a live column the registry does not declare raises the same error class.
- [ ] AC6: Every statement `apply` executes contains only `NOT NULL` as a constraint — captured statement text contains no PRIMARY KEY, UNIQUE, FOREIGN KEY, CHECK, DEFAULT or CREATE INDEX.

## Tests

- tests/integration/test_ddl_apply.py::test_apply_creates_the_fourteen_ddl_owned_relations
- tests/integration/test_ddl_apply.py::test_apply_is_idempotent
- tests/integration/test_ddl_apply.py::test_apply_never_touches_dbt_owned_relations
- tests/integration/test_ddl_apply.py::test_evolve_adds_nullable_column_only
- tests/integration/test_ddl_apply.py::test_breaking_type_change_raises_err_schema_breaking
- tests/integration/test_ddl_apply.py::test_undeclared_live_column_is_breaking
- tests/integration/test_ddl_apply.py::test_emitted_statements_declare_only_not_null

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_ddl_apply.py -q
uv run mypy --strict src/er/lake/ddl.py
uv run ruff check src/er/lake/ddl.py tests/integration/test_ddl_apply.py
```

## Definition of Done

- `apply` idempotent over exactly the fourteen `ddl.py`-owned relations; dbt-owned relations untouched
- `evolve` additive-only; breaking differences raise the literal `ERR_SCHEMA_BREAKING` message with code 3 and commit nothing
- Ownership guard rejects a dbt-owned spec
- Every statement derived from the registry; `model.py` and `tests/conftest.py` unmodified
- Verify command passes; mypy --strict clean
