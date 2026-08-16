---
id: ER-047
title: "int_blocking_keys macro-generated from the payload + contract + accepted_values"
milestone: M2
status: in_progress
kind: code
size: S
gates: full
depends_on: ["ER-033", "ER-043", "ER-044", "ER-046"]
spec_refs: ["s4-2", "s5", "s5-0", "s6", "s8-1"]
gap_refs: ["M12", "M16", "M4"]
provides: ["relation:int_blocking_keys", "dbt/models/intermediate/int_blocking_keys.sql", "dbt/macros/blocking/int_blocking_keys_union.sql", "tests/integration/scenarios/test_blocking_keys.py"]
consumes: ["src/er/matching/model.py::blocking_rules_from_config", "src/er/matching/model.py::BLOCKING_DBT_VAR", "src/er/dbt_runner.py::render_dbt_vars", "src/er/dbt_runner.py::run_dbt", "relation:int_std_records", "src/er/lake/hashing.py::table_content_hash", "tests/conftest.py::lake_ns"]
owns: ["dbt/models/intermediate/int_blocking_keys.sql", "dbt/macros/blocking/int_blocking_keys_union.sql", "tests/integration/scenarios/test_blocking_keys.py"]
protected_paths: ["dbt/models/intermediate/int_std_records.sql", "src/er/matching/model.py", "fixtures/static/base_10/base/crm.csv", "fixtures/static/base_10/base/billing.csv", "fixtures/static/base_10/base/webforms.csv"]
extra_paths: ["dbt/models/intermediate/schema.yml"]
attempts: 1
verify: "bash scripts/ci/itest.sh tests/integration/scenarios/test_blocking_keys.py -q"
branch: "ticket/ER-047-int-blocking-keys-macro-generated-payload"
commit: ""
spec_sha: "2e62460d9f41a842"
updated_at: "2026-08-16T06:31:09Z"
session: d99dc4ff-62e9-4219-bd48-a8baa9fee813
---
## Description

Materialize `int_blocking_keys` as a macro-generated `UNION ALL` over the blocking payload the CLI passes as a dbt var (S4.2): one branch per `key_type`, each emitting `(key_type, key_value, record_key, source_system, source_record_id)` from `int_std_records` under the normative NULL/empty predicate. The model file contains no hand-written key logic — adding a `blocking:` entry to the config changes the compiled SQL and the emitted key set with no model edit, which is what removes the drift M12 identified. The relation declares an enforced contract with the S5 column list plus an `accepted_values` test on `key_type` bound to the config's key-type set.

## Scope

### In scope

- `dbt/macros/blocking/int_blocking_keys_union.sql`: renders one branch per payload entry using the S4.2 template verbatim
- `dbt/models/intermediate/int_blocking_keys.sql`: the S4.2 `delete+insert` incremental config on `unique_key=['source_system','source_record_id']`, `on_schema_change='append_new_columns'`, calling the macro
- `schema.yml` additions: enforced contract, `unique_combination_of_columns` on `(key_type, key_value, record_key)` tagged `keys`, `accepted_values` on `key_type`, `not_null` on all five columns
- An integration scenario over `base_10` asserting key-type coverage, the NULL/empty policy and the trap-derived key expectations
- `blocking_rows` / `blocking_keys_by_type` available as standardize-stage counter inputs

### Out of scope

- `blocking_rules_from_config` itself (ER-046) — this ticket only consumes its payload
- T-BLK-1 parity against Splink's blocked pair set (ER-057)
- Using `int_blocking_keys` as a scoring input — S4.3.4 forbids it; it is a rule source, a touched-subgraph driver and a benchmark metric
- Editing `int_std_records` or the fixture to make a key appear

## Design decisions applied

Closes M12 (macro generation direction), M16 (incremental config for a relation with many rows per record) and M4 (the relation is dbt-owned with an enforced contract, and `ddl.py` never touches it). Easy to miss: `delete+insert` on `(source_system, source_record_id)` is correct here even though the relation holds many rows per record — the strategy deletes every row for a touched record before re-inserting its full key set, which is exactly the required behaviour when a record's standardized values change; the branch SQL must match the S4.2 template character-for-character including the `where <expr> is not null and <expr> <> ''` clause, because T-BLK-1 later compares this table's `DISTINCT` canonicalised pair set to Splink's; the `accepted_values` domain is derived from the config, not hard-coded.

## Acceptance criteria

- [ ] AC1: After standardizing `base_10`, `int_blocking_keys`'s distinct `key_type` set equals exactly `{email_exact, phone_exact, name_postal, dob_name}` and `dbt build --select int_blocking_keys` exits 0 with the contract enforced.
- [ ] AC2: Zero rows have a NULL or empty-string `key_value` (`select count(*) ... where key_value is null or key_value = ''` returns 0).
- [ ] AC3: The four records with an empty email and the two `test@test.com` records emit no `email_exact` row, and their `record_key`s appear under no `email_exact` key.
- [ ] AC4: The drifted-phone persona's three records each emit a `phone_exact` row whose `key_value` is `+14155550132`, so the three block together.
- [ ] AC5: The compiled model SQL contains exactly one `UNION ALL` branch per payload entry and each branch is byte-equal to the S4.2 template with `<key_type>` and `<expr>` substituted; appending a fifth `blocking:` entry to a copied config and recompiling adds exactly one branch and one `key_type` with no edit to `int_blocking_keys.sql`.
- [ ] AC6: Re-running `er standardize` leaves the row count and the full key set unchanged; re-standardizing after one record's standardized values change removes all of that record's prior rows and inserts its new full key set, with every other record's rows untouched.
- [ ] AC7: `dbt test --select tag:keys` fails after a duplicate `(key_type, key_value, record_key)` row is inserted, and the `accepted_values` test fails after a row with an unconfigured `key_type` is inserted.

## Tests

- tests/integration/scenarios/test_blocking_keys.py::test_key_type_set_equals_config
- tests/integration/scenarios/test_blocking_keys.py::test_no_null_or_empty_key_values
- tests/integration/scenarios/test_blocking_keys.py::test_missing_and_placeholder_emails_emit_no_key
- tests/integration/scenarios/test_blocking_keys.py::test_drifted_phones_share_one_key
- tests/integration/scenarios/test_blocking_keys.py::test_compiled_sql_is_macro_generated_per_payload_entry
- tests/integration/scenarios/test_blocking_keys.py::test_delete_insert_replaces_full_key_set_for_touched_record
- tests/integration/scenarios/test_blocking_keys.py::test_duplicate_and_unknown_key_type_fail_dbt_tests

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/scenarios/test_blocking_keys.py -q
uv run dbt parse --project-dir dbt --profiles-dir dbt/profiles --target mem
uv run pytest tests/unit/dbt/test_incremental_configs.py -q
bash scripts/gates.sh
```

## Definition of Done

- All acceptance criteria met and the verify command passes
- `int_blocking_keys.sql` contains no literal key expression — every branch comes from the macro and the dbt var
- Contract in `schema.yml` matches S5's `int_blocking_keys` column list and the `key_type` `accepted_values` domain is derived from the config
- `int_std_records` and `src/er/matching/model.py` unmodified
- Committed on main with the board updated
