---
id: ER-042
title: "stg_crm/stg_billing/stg_webforms + incremental config + staging schema.yml"
milestone: M2
status: todo
kind: code
size: M
gates: full
depends_on: ["ER-031", "ER-033", "ER-041"]
spec_refs: ["s4-2", "s5", "s5-0", "s6", "s4-0b", "s8-1"]
gap_refs: ["M14", "M16"]
provides: ["relation:stg_crm", "relation:stg_billing", "relation:stg_webforms", "dbt/models/staging/stg_crm.sql", "dbt/models/staging/stg_billing.sql", "dbt/models/staging/stg_webforms.sql", "dbt/models/staging/schema.yml", "dbt-selector:staging", "tests/integration/scenarios/test_staging.py", "tests/unit/dbt/test_incremental_configs.py"]
consumes: ["relation:raw_records", "src/er/dbt_runner.py::run_dbt", "src/er/dbt_runner.py::render_dbt_vars", "fixtures/static/base_10/base/crm.csv", "fixtures/static/base_10/base/billing.csv", "fixtures/static/base_10/base/webforms.csv", "dbt/macros/std/lowercase_trim.sql", "dbt/macros/std/null_semantics.sql", "dbt/macros/std/email_norm.sql", "dbt/macros/std/phone_e164.sql", "dbt/macros/std/name_norm.sql", "dbt/macros/std/address_parse.sql", "tests/conftest.py::lake_ns"]
owns: ["dbt/models/staging/stg_crm.sql", "dbt/models/staging/stg_billing.sql", "dbt/models/staging/stg_webforms.sql", "dbt/models/staging/schema.yml", "tests/integration/scenarios/test_staging.py", "tests/unit/dbt/test_incremental_configs.py"]
protected_paths: ["fixtures/static/base_10/base/crm.csv", "fixtures/static/base_10/base/billing.csv", "fixtures/static/base_10/base/webforms.csv"]
extra_paths: ["dbt/dbt_project.yml"]
attempts: 0
verify: "bash scripts/ci/itest.sh tests/integration/scenarios/test_staging.py -q && uv run pytest tests/unit/dbt/test_incremental_configs.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

Ship the three hand-written staging models (S4.2) that project `raw_records.payload` into the canonical `stg_<source>` shape S5 declares, applying the S4.2 standardization macros and reading their column mapping from the `sources` var the CLI passes. Each model carries the normative incremental configuration — `materialized='incremental'`, `incremental_strategy='append'`, `on_schema_change='sync_all_columns'` and the literal `ingest_batch_id not in (select distinct ingest_batch_id from {{ this }})` predicate — which is what makes `er standardize` idempotent (M16). `dbt/models/staging/schema.yml` declares `contract: {enforced: true}` plus the S5.0 logical-key test, so S5 stays the review-time authority for these three relations.

## Scope

### In scope

- `stg_crm.sql`, `stg_billing.sql`, `stg_webforms.sql`: source-column → canonical-column projection driven by the `sources` var, macros applied per S4.2
- The S4.2 incremental config block and `is_incremental()` predicate, identical on all three
- `dbt/models/staging/schema.yml`: enforced contract with the exact S5 `stg_<source>` column list and types, `unique_combination_of_columns` on `(source_system, source_record_id, content_hash)` tagged `keys`, `not_null` on the NOT NULL columns
- An integration scenario test that ingests `base_10` and builds `--select staging` against the namespaced lake
- A unit test asserting the incremental-config contract across every dbt model file (strategy, `on_schema_change`, no `indexes`, no `merge`)

### Out of scope

- `int_std_records` / `int_blocking_keys` and anything in `dbt/models/intermediate/` (ER-043, ER-047)
- The standardization macros themselves (ER-037…ER-040) — this ticket calls them, it does not edit them
- `er standardize`'s `--changed-only` selection logic and `--full-refresh` rebuild path beyond invoking `run_dbt`
- Tombstone exclusion — tombstoned rows are staged like any other `raw_records` version; exclusion happens in `int_std_records` (ER-043)

## Design decisions applied

Closes M14 (per-source column mapping is config, models are hand-written for v1's three sources) and M16 (every incremental model's strategy, predicate and `on_schema_change`). Easy to miss: `on_schema_change` MUST be `sync_all_columns` — the dbt-duckdb default of `ignore` silently swallows the columns a `std_version` bump adds; `append` with the *not-in-distinct-batch* predicate, never a `>` watermark, is what stops a re-delivered batch duplicating rows; `name_variants` is `LIST(VARCHAR)` and NOT NULL with the normalized `given_name` as element 0 (the S4.2 symmetry guarantee `variant_match` depends on); no model declares `indexes` and no dbt model uses `incremental_strategy='merge'` (S4.2); `persona_id` is stripped by the fixture loader and MUST NOT appear as a column; dbt runs as a subprocess with `threads: 1` and no Python DuckDB connection spans it (S4.0b).

## Acceptance criteria

- [ ] AC1: After ingesting `base_10`'s three CSVs and running `dbt build --select staging --target lake`, each `stg_*` relation holds exactly the row count of its source CSV, the three together hold 23 rows, and no relation carries a `persona_id` column.
- [ ] AC2: Each `stg_*` relation's column names, order and types read back from the lake equal the S5 `stg_<source>` listing exactly, and deleting or retyping a column in `schema.yml` makes `dbt build` fail with a contract violation (asserted by a negative arm on a scratch copy).
- [ ] AC3: `name_variants` is non-NULL on all 23 rows and `name_variants[1]` (DuckDB 1-based) equals `given_name` on every row.
- [ ] AC4: The billing rows parsed under `%m/%d/%Y` and the crm/webforms rows parsed under `%Y-%m-%d` yield the same `birth_date` DATE for the same persona, and no `birth_date` is non-NULL for a year-only source value.
- [ ] AC5: Re-running `er standardize` over the same `ingest_batch_id` appends zero rows: the three `stg_*` row counts and the full non-`VOLATILE_COLUMNS` content are unchanged, and the compiled model contains the literal predicate `ingest_batch_id not in (select distinct ingest_batch_id from {{ this }})`.
- [ ] AC6: The unit test asserts, for every file under `dbt/models/`, that an incremental model declares `on_schema_change='sync_all_columns'`, that `stg_*` use `incremental_strategy='append'`, that no model declares `indexes`, and that no model uses `incremental_strategy='merge'`.
- [ ] AC7: `dbt test --select tag:keys` is non-zero after a duplicate `(source_system, source_record_id, content_hash)` row is inserted into a `stg_*` relation and names that relation.

## Tests

- tests/integration/scenarios/test_staging.py::test_base_10_stages_23_rows_with_contract_shape
- tests/integration/scenarios/test_staging.py::test_name_variants_symmetry_and_date_formats
- tests/integration/scenarios/test_staging.py::test_restandardize_appends_no_rows
- tests/integration/scenarios/test_staging.py::test_duplicate_key_fails_tag_keys
- tests/unit/dbt/test_incremental_configs.py::test_every_incremental_model_syncs_all_columns
- tests/unit/dbt/test_incremental_configs.py::test_staging_uses_append_with_batch_predicate
- tests/unit/dbt/test_incremental_configs.py::test_no_model_declares_indexes_or_merge

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/scenarios/test_staging.py -q
uv run pytest tests/unit/dbt/test_incremental_configs.py -q
uv run dbt parse --project-dir dbt --profiles-dir dbt/profiles --target mem
uv run dbt compile --project-dir dbt --profiles-dir dbt/profiles --target mem
bash scripts/gates.sh
```

## Definition of Done

- All acceptance criteria met and both verify commands pass
- `contract: {enforced: true}` on all three staging models with the S5 column list column-for-column
- Incremental config identical on all three models and asserted by the unit test
- No fixture CSV under `fixtures/static/base_10/base/` was modified
- `dbt parse` and `dbt compile --target mem` green on a bare runner (no services)
- Committed on main with the board updated
