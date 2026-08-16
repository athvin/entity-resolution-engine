---
id: ER-043
title: "int_std_records: record_key, content_hash, greatest-ingested_at current-row rule, tombstone exclusion, contract, phone_valid/email_valid"
milestone: M2
status: todo
kind: code
size: M
gates: full
depends_on: ["ER-013", "ER-032", "ER-042"]
spec_refs: ["s4-2", "s5", "s5-0", "s4-1", "s4-1-1", "s12-1", "s8-1"]
gap_refs: ["M15", "M16", "M1", "D6", "D7"]
provides: ["relation:int_std_records", "dbt/models/intermediate/int_std_records.sql", "dbt/models/intermediate/schema.yml", "dbt/tests/assert_record_key_no_colon.sql", "dbt/tests/assert_one_current_std_row_per_record.sql", "dbt-selector:intermediate", "tests/integration/scenarios/test_int_std_records.py"]
consumes: ["relation:stg_crm", "relation:stg_billing", "relation:stg_webforms", "src/er/entities/ids.py::record_key", "src/er/dbt_runner.py::run_dbt", "relation:raw_records", "src/er/ingest/landing.py::TOMBSTONE_CONTENT_HASH", "tests/conftest.py::lake_ns"]
owns: ["dbt/models/intermediate/int_std_records.sql", "dbt/models/intermediate/schema.yml", "dbt/tests/assert_record_key_no_colon.sql", "dbt/tests/assert_one_current_std_row_per_record.sql", "tests/integration/scenarios/test_int_std_records.py"]
protected_paths: ["dbt/models/staging/stg_crm.sql", "dbt/models/staging/stg_billing.sql", "dbt/models/staging/stg_webforms.sql", "fixtures/static/base_10/base/crm.csv", "fixtures/static/base_10/base/billing.csv", "fixtures/static/base_10/base/webforms.csv"]
extra_paths: []
attempts: 0
verify: "bash scripts/ci/itest.sh tests/integration/scenarios/test_int_std_records.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

Build `int_std_records`, the single current-state standardized relation: it unions the three staging models, materializes `record_key`, `content_hash`, `std_version` and `updated_at_source`, and enforces the S4.2 supersession rule — exactly one current row per `(source_system, source_record_id)`, taken from the `raw_records` version with the greatest `ingested_at` (ties broken by `ingest_batch_id DESC`) — while excluding rows whose winning version is a tombstone (S4.1.1). It carries `phone_valid` alongside `email_valid` so the `validated` survivorship rule has an input for both attributes (M11), and declares an enforced contract matching S5 column-for-column. This is the relation Splink's `unique_id_column_name` points at and the one every later stage reads.

## Scope

### In scope

- `int_std_records.sql` with the S4.2 `delete+insert` incremental config on `unique_key=['source_system','source_record_id']` and `on_schema_change='append_new_columns'`
- The greatest-`ingested_at` current-row selection with the `ingest_batch_id DESC` tiebreak, and exclusion of rows whose winning version has `is_deleted = true`
- `record_key := source_system || ':' || source_record_id`, `content_hash`, `std_version` from the CLI `--vars` override, `updated_at_source`
- `dbt/models/intermediate/schema.yml`: enforced contract with the S5 column list, `unique` on `record_key`, `unique_combination_of_columns` on `(source_system, source_record_id)`, both tagged `keys`
- Two singular tests: `source_record_id` contains no `':'`, and one current row per record
- `tombstones_excluded` available as a stage counter input (row count of excluded keys)
- An integration scenario covering base, supersession and tombstone cases on the namespaced lake

### Out of scope

- `int_blocking_keys` (ER-047) — this ticket adds no blocking key
- `table_content_hash` / T-STD-1 (ER-044)
- Edge invalidation when a `content_hash` changes (ER-082) — this ticket only makes the *current row* correct
- The tombstone ingest arm itself (`--full-refresh-keys`, sentinel hash, resurrection) — ER-032 owns it; this ticket consumes its output
- `golden_*` marts and survivorship

## Design decisions applied

Implements D6 (`record_key` as the canonical scalar identity, `':'` banned in `source_record_id`) and D7 (append-only `raw_records`; the current std row is the greatest-`ingested_at` version), and closes M15 (supersession had no rule), M16 (incremental config) and M1 (record identity). Easy to miss: the tiebreak is `ingest_batch_id DESC` — ULIDs are time-ordered, so `ASC` would let the *older* batch win; tombstone exclusion is by the *winning* version's `is_deleted`, not by any version having it, which is exactly what makes resurrection work with no special case (S4.1.1); `delete+insert` on `(source_system, source_record_id)` is correct even though the relation has one row per record because the same strategy must hold for `int_blocking_keys`; `phone_valid` exists solely as the `validated` input for `phone_e164` (S6.1 V4) and is NOT a `golden_records` column.

## Acceptance criteria

- [ ] AC1: After ingesting and standardizing `base_10`, `int_std_records` holds exactly 23 rows, `record_key` equals `source_system || ':' || source_record_id` on every row, and `dbt build --select int_std_records` exits 0 with the contract enforced.
- [ ] AC2: Given two `raw_records` versions of one key with different `ingested_at`, `int_std_records` holds exactly one row for that key carrying the greater-`ingested_at` version's `content_hash`; when the two `ingested_at` values are equal, the row from the lexically greater `ingest_batch_id` wins.
- [ ] AC3: A key whose current version has `is_deleted = true` produces zero `int_std_records` rows, while a key that was tombstoned and then re-delivered with an ordinary content version is present again with the new `content_hash`.
- [ ] AC4: The relation's columns, order and types read back from the lake equal the S5 `int_std_records` listing exactly, including `name_variants LIST(VARCHAR) NOT NULL`, `email_valid BOOLEAN` and `phone_valid BOOLEAN`.
- [ ] AC5: Inserting a staged row whose `source_record_id` contains `':'` makes `dbt test --select tag:keys` exit non-zero and name `int_std_records`; so does inserting a second current row for one `(source_system, source_record_id)`.
- [ ] AC6: Every row with a non-NULL `email` has a non-NULL `email_valid`, every row with a non-NULL `phone_e164` has a non-NULL `phone_valid`, and the two `test@test.com` records have `email IS NULL`.
- [ ] AC7: Re-running `er standardize` with no new deliveries leaves the row count at 23 and every non-`VOLATILE_COLUMNS` value unchanged, and the compiled model declares `incremental_strategy='delete+insert'` with `unique_key=['source_system','source_record_id']` and `on_schema_change='append_new_columns'`.

## Tests

- tests/integration/scenarios/test_int_std_records.py::test_base_10_yields_23_current_rows_with_record_key
- tests/integration/scenarios/test_int_std_records.py::test_greatest_ingested_at_wins_with_batch_id_tiebreak
- tests/integration/scenarios/test_int_std_records.py::test_tombstoned_key_excluded_and_resurrected_key_returns
- tests/integration/scenarios/test_int_std_records.py::test_contract_matches_s5_column_list
- tests/integration/scenarios/test_int_std_records.py::test_colon_and_duplicate_key_fail_tag_keys
- tests/integration/scenarios/test_int_std_records.py::test_phone_and_email_valid_populated
- tests/integration/scenarios/test_int_std_records.py::test_restandardize_is_row_stable

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/scenarios/test_int_std_records.py -q
uv run dbt parse --project-dir dbt --profiles-dir dbt/profiles --target mem
uv run pytest tests/unit/dbt/test_incremental_configs.py -q
bash scripts/gates.sh
```

## Definition of Done

- All acceptance criteria met and the verify command passes
- Contract in `dbt/models/intermediate/schema.yml` matches S5 column-for-column and type-for-type
- Both singular tests are tagged `keys` and demonstrably fail on their negative arm
- No staging model or fixture CSV was modified
- T-KEY-1b (`tests/integration/test_keys.py::test_dbt_owned_duplicate_key_fails_dbt_test`) is now runnable against `int_std_records`
- Committed on main with the board updated
