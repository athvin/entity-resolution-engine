---
id: ER-042
title: "stg_crm/stg_billing/stg_webforms + incremental config + staging schema.yml"
milestone: M2
status: done
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
attempts: 1
verify: "bash scripts/ci/itest.sh tests/integration/scenarios/test_staging.py -q && uv run pytest tests/unit/dbt/test_incremental_configs.py -q"
branch: "ticket/ER-042-stg-crm-stg-billing-stg-webforms"
commit: "4ea6c7ca73cef93d6f5abdb99b93e876fc281073"
spec_sha: "2e62460d9f41a842"
updated_at: "2026-08-16T01:43:15Z"
session: 4c399469-59da-4c70-b761-42f05a94d032
---
## Description

Ship the three hand-written staging models (S4.2) that project `raw_records.payload` into the canonical `stg_<source>` shape S5 declares, applying the S4.2 standardization macros and reading their column mapping from the `sources` var the CLI passes. Each model carries the normative incremental configuration — `materialized='incremental'`, `incremental_strategy='append'`, `on_schema_change='append_new_columns'` and the literal `ingest_batch_id not in (select distinct ingest_batch_id from {{ this }})` predicate — which is what makes `er standardize` idempotent (M16). `dbt/models/staging/schema.yml` declares `contract: {enforced: true}` plus the S5.0 logical-key test, so S5 stays the review-time authority for these three relations.

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

Closes M14 (per-source column mapping is config, models are hand-written for v1's three sources) and M16 (every incremental model's strategy, predicate and `on_schema_change`). Easy to miss: `on_schema_change` MUST be `append_new_columns` — the dbt-duckdb default of `ignore` silently swallows the columns a `std_version` bump adds; `append` with the *not-in-distinct-batch* predicate, never a `>` watermark, is what stops a re-delivered batch duplicating rows; `name_variants` is `LIST(VARCHAR)` and NOT NULL with the normalized `given_name` as element 0 (the S4.2 symmetry guarantee `variant_match` depends on); no model declares `indexes` and no dbt model uses `incremental_strategy='merge'` (S4.2); `persona_id` is stripped by the fixture loader and MUST NOT appear as a column; dbt runs as a subprocess with `threads: 1` and no Python DuckDB connection spans it (S4.0b).

## Acceptance criteria

- [ ] AC1: After ingesting `base_10`'s three CSVs and running `dbt build --select staging --target lake`, each `stg_*` relation holds exactly the row count of its source CSV, the three together hold 23 rows, and no relation carries a `persona_id` column.
- [ ] AC2: Each `stg_*` relation's column names, order and types read back from the lake equal the S5 `stg_<source>` listing exactly, and deleting or retyping a column in `schema.yml` makes `dbt build` fail with a contract violation (asserted by a negative arm on a scratch copy).
- [ ] AC3: `name_variants` is non-NULL on all 23 rows and `name_variants[1]` (DuckDB 1-based) equals `given_name` on every row.
- [ ] AC4: The billing rows parsed under `%m/%d/%Y` and the crm/webforms rows parsed under `%Y-%m-%d` yield the same `birth_date` DATE for the same persona, and no `birth_date` is non-NULL for a year-only source value.
- [ ] AC5: Re-running `er standardize` over the same `ingest_batch_id` appends zero rows: the three `stg_*` row counts and the full non-`VOLATILE_COLUMNS` content are unchanged, and the compiled model contains the literal predicate `ingest_batch_id not in (select distinct ingest_batch_id from {{ this }})`.
- [ ] AC6: The unit test asserts, for every file under `dbt/models/`, that an incremental model declares `on_schema_change='append_new_columns'`, that `stg_*` use `incremental_strategy='append'`, that no model declares `indexes`, and that no model uses `incremental_strategy='merge'`.
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

## Blocker log
### Resolution of attempt 1 (applied by the board owner, 2026-08-16)

Attempt 1 was correct and the citation checks out. dbt-core 1.12.2 rejects
`on_schema_change: sync_all_columns` on a contract-enforced incremental model, in
`dbt/artifacts/resources/v1/config.py`:

```python
if (self.contract.enforced and self.materialized == "incremental"
        and self.on_schema_change not in ("append_new_columns", "fail")):
    raise ValidationError(...)
```

S4.2 mandated `sync_all_columns` while S5.0 mandates `contract: {enforced: true}` on every dbt-owned
model, so the two could never coexist. **The spec has been amended to `append_new_columns`**
throughout (S4.2, its two config examples, the S7 marts config, S5.1), along with
`dbt/dbt_project.yml` and the four other tickets that stated the old value (ER-008, ER-043, ER-047,
ER-088).

Nothing is lost. S5.1's stated purpose is that *"an additive column introduced by a `std_version`
bump propagates instead of being silently dropped"* — exactly what `append_new_columns` does. The
only behaviour given up is silent syncing of column REMOVALS and type changes, which under an
enforced contract should fail loudly rather than be applied.

For the next attempt: write `on_schema_change='append_new_columns'`; the project-level default in
`dbt/dbt_project.yml` already carries the corrected value.


### Attempt 1 — spec_contradiction (2026-08-16T00:00:42Z)

- **Failing command:** `uv run dbt parse --project-dir dbt --profiles-dir dbt/profiles --target mem`
- **Assertion / contradiction:** Parsing Error at path []: Invalid value for on_schema_change: append_new_columns. Models materialized as incremental with contracts enabled must set on_schema_change to 'append_new_columns' or 'fail'. S4.2 requires on_schema_change='append_new_columns' on every incremental model AND contract: {enforced: true} on every dbt-owned model (S4.2, S5.0); dbt-core==1.12.2 (S2.1) refuses that pairing in dbt/artifacts/resources/v1/config.py:150-157, validating the MERGED model config, so no spelling of the two values escapes it. The staging models are the first contract-enforced incremental models on the board; ER-043, ER-047 and ER-088 state the same value and hit the same wall.
- **Smallest change that would unblock:** Land a kind: spec-amendment ticket replacing append_new_columns with append_new_columns in DesignDoc.md S4.2 (the two code blocks at lines 495/506 and the MUST sentence at line 509), S4.6 line 793 and S5.1 line 1158, adding the reason: every model there is contract-enforced, and the half of append_new_columns dbt refuses (dropping and retyping columns) is the half S5.1 already classifies as breaking, so the permitted value is also the correct one; append_new_columns propagates the additive std_version-bump column that S4.2 and S5.1 say the setting exists for, while 'fail' would make a legitimate additive S5.1 evolution a hard error. Then the forced mechanical edits: tests/unit/test_dbt_profiles.py:196 (ER-008's committed test, which is why ER-042 cannot resolve this itself), ER-008 AC6 + Scope, ER-042 Description + AC6, ER-043 Scope + AC7, ER-047 Scope, ER-088 design item (5) + DoD. Verified: with that one token changed and nothing else, dbt parse and dbt compile --target mem both exit 0 on the branch's tree.
- **Log:** `.loop/logs/ER-042.attempt-1.log`
