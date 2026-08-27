---
id: ER-088
title: "golden_records literal typed schema (addr_* expanded) + golden_lineage (6-token vocabulary, address composite) + contract + config parity"
milestone: M4
status: done
kind: code
size: L
gates: full
depends_on: ["ER-006", "ER-012", "ER-074", "ER-087"]
spec_refs: ["s4-2", "s4-6", "s5", "s5-0", "s5-1", "s6", "s6-1"]
gap_refs: ["B4", "M4", "M16"]
provides: ["relation:golden_records", "relation:golden_lineage", "dbt/models/marts/golden_records.sql", "dbt/models/marts/golden_lineage.sql", "src/er/lake/columns.py::GOLDEN_LINEAGE_ATTRIBUTES", "src/er/lake/columns.py::ADDRESS_COMPOSITE_COLUMNS", "tests/unit/config/test_golden_schema_parity.py::test_survivorship_keyset_equals_golden_survivable_columns"]
consumes: ["src/er/lake/columns.py::GOLDEN_SURVIVABLE_COLUMNS", "dbt/macros/survivorship/survivorship.sql::survivorship_order_by", "dbt/macros/survivorship/survivorship.sql::survivorship_decision", "relation:int_std_records", "relation:entity_membership", "relation:entities", "src/er/dbt_runner.py::render_dbt_vars", "src/er/config/schema.py::VersionsConfig"]
owns: ["dbt/models/marts/golden_records.sql", "dbt/models/marts/golden_lineage.sql", "tests/integration/test_golden_models.py", "tests/unit/config/test_golden_schema_parity.py"]
protected_paths: []
extra_paths: ["dbt/models/schema.yml", "src/er/lake/columns.py", "tests/unit/test_columns.py"]
attempts: 1
verify: "bash scripts/ci/itest.sh tests/integration/test_golden_models.py -q && uv run pytest tests/unit/config/test_golden_schema_parity.py -q"
branch: "ticket/ER-088-golden-records-literal-typed-schema-addr"
commit: "c4915d2686d0f3806c88c40c132df6abba6a3263"
spec_sha: "7467bdacba1bd84c"
updated_at: "2026-08-27T18:51:09Z"
session: c379438e-8c95-4be8-8f5f-3832d8ceafaa
---
## Description

B4 called `golden_records` a schema placeholder: the pipeline's terminal output had no column list, so the mart could not be written, the expected files could not be authored and T-GOLD-1 had nothing to assert. S5 now carries the literal typed list with `address` expanded to six `addr_*` columns. This ticket builds the two marts against that contract, emits `golden_lineage` over the closed six-token attribute vocabulary with the address treated as one composite decision, and lands the config-parity unit test that keeps the `survivorship:` key set and `GOLDEN_SURVIVABLE_COLUMNS` from drifting apart.

## Scope

### In scope

- `golden_records.sql`: per-attribute window over the entity's `int_std_records` member rows, rank-1 per the ER-087 chains, contract-enforced against the S5 column list and types
- `golden_lineage.sql`: one row per `(entity_id, attribute)` over the six-token vocabulary, carrying the winning `record_key`, `source_system`, `source_record_id`, the deciding `rule`, `survivorship_version` and `assembled_at`
- The address composite rule: all six `addr_*` values come from the single record named by the `attribute='address'` lineage row
- `schema.yml` contract + data tests (`unique` on `entity_id`, `unique_combination_of_columns` on `(entity_id, attribute)`, `accepted_values` on `attribute` and `rule`, active-entity relationship tests)
- `GOLDEN_LINEAGE_ATTRIBUTES` / `ADDRESS_COMPOSITE_COLUMNS` in `columns.py` and the two-directional parity test

### Out of scope

- `golden_display` (ER-089)
- Touched-only assembly, `er_touched_entities`, the reap step and `assembled_at` for untouched entities (ER-092)
- The committed `base_10` expected golden/lineage values (ER-090) and T-GOLD-1 itself (ER-091)
- Re-deriving the survivorship ORDER BY fragments — ER-087 owns them

## Design decisions applied

Implements gaps B4, M4 and M16 for the marts. Constraints easy to miss: (1) the survivable column set is EXACTLY the eleven columns of `GOLDEN_SURVIVABLE_COLUMNS`; `email_valid`/`phone_valid` are inputs to the `validated` rule and are NOT columns of `golden_records`; (2) when `address` wins, all six `addr_*` values MUST come from ONE winning contributing record — never assembled field by field, which is the defect S4.6 explicitly names; (3) the lineage grid is complete: a row is emitted for every `(entity_id, attribute)` pair in the six-token vocabulary even when the winning value is NULL, so `(entity_id, attribute)` is a well-formed logical key and ER-090's expected file has a fixed shape; (4) `survivorship_version` comes from `versions.survivorship_version` in the config, passed as a dbt `--vars` override (S6) — `dbt_project.yml` holds only a fallback, and reading the fallback is a defect; (5) mart config is `incremental`, `delete+insert`, `unique_key='entity_id'`, `on_schema_change='append_new_columns'`, no `indexes`, no `merge` strategy (S4.2, S4.6); (6) these are dbt-owned relations — `ddl.py` must never issue DDL against them (S5.0).

## Acceptance criteria

- [ ] AC1: `dbt build --select golden_records golden_lineage --target lake` exits 0 and `information_schema` reports `golden_records` with exactly the S5 columns, in S5 order, with the S5 types, including all six expanded `addr_*` columns
- [ ] AC2: Changing one declared type in `schema.yml` to disagree with the model makes `dbt build` fail with a contract violation (asserted by an in-test perturbation), proving `contract: {enforced: true}` is live
- [ ] AC3: `golden_lineage` holds exactly `6 * count(distinct entity_id)` rows; `select distinct attribute` returns exactly `{email, phone_e164, given_name, family_name, address, birth_date}`; `select distinct rule` is a subset of the five rule names plus `tiebreak_deterministic`
- [ ] AC4: For every entity, joining `int_std_records` on the `record_key` of the `attribute='address'` lineage row reproduces all six `addr_*` values of that entity's `golden_records` row exactly (NULL compares equal)
- [ ] AC5: For every non-address attribute, the `golden_records` value equals the value on the record named by that attribute's lineage row
- [ ] AC6: `golden_records.survivorship_version` equals `versions.survivorship_version` from the config passed via `--vars`; changing that config value and rebuilding changes the column
- [ ] AC7: dbt tests pass: every `golden_records.entity_id` has `entities.status='active'`; every active entity with at least one member has exactly one `golden_records` row; `(entity_id, attribute)` is unique
- [ ] AC8: `tests/unit/config/test_golden_schema_parity.py` fails when a column is added to `GOLDEN_SURVIVABLE_COLUMNS` without a matching `survivorship:` key, and when a `survivorship:` key is added without a column — both directions

## Tests

- tests/integration/test_golden_models.py::test_golden_records_schema_is_literal_s5
- tests/integration/test_golden_models.py::test_contract_violation_fails_dbt_build
- tests/integration/test_golden_models.py::test_lineage_grid_is_complete_and_vocabulary_closed
- tests/integration/test_golden_models.py::test_address_columns_come_from_one_record
- tests/integration/test_golden_models.py::test_survivorship_version_comes_from_config_vars
- tests/integration/test_golden_models.py::test_mart_incremental_config_is_delete_insert
- tests/unit/config/test_golden_schema_parity.py::test_survivorship_keyset_equals_golden_survivable_columns
- tests/unit/config/test_golden_schema_parity.py::test_parity_fails_in_both_directions

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_golden_models.py -q
uv run pytest tests/unit/config/test_golden_schema_parity.py -q
uv run dbt parse --project-dir dbt --profiles-dir dbt/profiles --target mem
uv run mypy --strict src/er/lake/columns.py
```

## Definition of Done

- Both marts exist under `dbt/models/marts/` with `contract: {enforced: true}` and the S5 column list column-for-column and type-for-type
- The address composite rule is implemented as a single winning-record join, and asserted as such
- The lineage grid is complete (six rows per entity) and its attribute and rule vocabularies are closed by `accepted_values` tests
- `survivorship_version` is sourced from the config `--vars` override, not from `dbt_project.yml`
- Mart config is `delete+insert` on `entity_id` with `on_schema_change='append_new_columns'`; no `indexes`, no `merge`
- Both arms of the verify command pass and both failed before the change
