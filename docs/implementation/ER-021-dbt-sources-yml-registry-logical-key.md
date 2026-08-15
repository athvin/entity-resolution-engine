---
id: ER-021
title: "dbt sources.yml from the registry + logical-key/accepted_values tests + canonical-pair singular test + T-KEY-1"
milestone: M1
status: todo
kind: code
size: M
gates: full
depends_on: ["ER-008", "ER-019", "ER-020"]
spec_refs: ["s5", "s5-0", "s8-1", "s8-3", "s12"]
gap_refs: ["B2", "M1", "M3", "M6", "M20"]
provides: ["src/er/lake/dbt_sources.py::render_sources_yml", "src/er/lake/dbt_sources.py::SOURCE_NAME", "dbt/models/sources.yml", "dbt/tests/assert_canonical_pair_ordering.sql", "dbt/tests/assert_single_active_model.sql", "dbt-selector:tag:keys", "tests/integration/test_keys.py::test_ddl_owned_duplicate_key_fails_dbt_test", "tests/integration/test_logical_keys.py"]
consumes: ["src/er/lake/model.py::TABLES", "src/er/lake/model.py::TableSpec", "src/er/lake/ddl.py::apply_ddl", "src/er/cli.py::app", "src/er/lake/ducklake.py::connect", "dbt/dbt_project.yml", "dbt/profiles/profiles.yml", "dbt/packages.yml", "tests/conftest.py::lake_ns", "tests/conftest.py::lake_conn", "tests/conftest.py::er_env"]
owns: ["src/er/lake/dbt_sources.py", "dbt/models/sources.yml", "dbt/tests/assert_canonical_pair_ordering.sql", "dbt/tests/assert_single_active_model.sql", "tests/unit/test_dbt_sources_parity.py", "tests/integration/test_logical_keys.py", "tests/integration/test_keys.py"]
protected_paths: []
extra_paths: ["dbt/dbt_project.yml", "dbt/packages.yml"]
attempts: 1
verify: "bash scripts/ci/itest.sh tests/integration/test_logical_keys.py -q && uv run pytest tests/unit/test_dbt_sources_parity.py -q"
branch: ""
commit: ""
spec_sha: "2abcfe433c322f74"
updated_at: "2026-08-15T15:03:26Z"
session: 1edf5b0f-ea7d-4c46-82fc-ef17d631e62e
---
## Description

DuckLake enforces NOT NULL only (S5.0), so every logical key in S5.0 is a test or it is nothing. This ticket declares the fourteen `ddl.py`-owned relations to dbt as **sources** in `dbt/models/sources.yml`, rendered from the S5 TableSpec registry so the registry stays the single authority, and attaches the S5.0 logical-key tests (`unique`, `dbt_utils.unique_combination_of_columns`, including the filtered variants), an `accepted_values` test for every `∈ {…}` domain in the S5 DDL, and the singular canonical-pair-ordering test over the four pair relations. All key/domain tests carry the `keys` tag, because T-KEY-1a's gate is `dbt test --select tag:keys` and a selector that matches nothing exits 0 and proves nothing (S12 M1). T-KEY-1a is the executable form of B2.

## Scope

### In scope

- `src/er/lake/dbt_sources.py::render_sources_yml(specs) -> str`: a pure renderer producing the whole `sources.yml` document from the registry (relation, column names, `not_null` for every NOT NULL column, `accepted_values` for every enum domain, the S5.0 logical key test with its `where:` filter where the key is filtered).
- The committed `dbt/models/sources.yml` — one source named `lake` with `database: lake`, `schema: main`, one table per `ddl.py`-owned relation.
- `dbt/tests/assert_canonical_pair_ordering.sql`: a singular test asserting `rec_a_key < rec_b_key` over `match_scores`, `assertions`, `review_queue` (where `subject_type='pair'`) and `cut_edges`, unioned, tagged `keys`.
- `dbt/tests/assert_single_active_model.sql`: at most one `model_registry` row with `status='active'` (S5.0), tagged `keys`.
- A `relationships` test `entity_membership.entity_id -> entities.entity_id` tagged `refs` (not `keys`).
- `tests/unit/test_dbt_sources_parity.py`: byte parity between the rendered document and the committed file, plus registry-coverage assertions.
- `tests/integration/test_logical_keys.py`: the selector is non-empty and green on a clean namespaced lake; each key/domain/ordering test is individually falsified by a hand-inserted violating row.
- `tests/integration/test_keys.py::test_ddl_owned_duplicate_key_fails_dbt_test` (T-KEY-1a), including the `CREATE TABLE … PRIMARY KEY` raise that documents B2 in executable form.
- Whatever `dbt_project.yml` config the `keys`/`refs` tags and the `dbt/tests/` path require.

### Out of scope

- Any dbt **model**: no `stg_*`, `int_*` or `golden_*` model and no `dbt/models/schema.yml` model contract ships here (the first dbt-owned relation does not exist until M2 — S12).
- T-KEY-1b (the dbt-owned arm) — it needs `int_std_records`.
- The `source_record_id` `':'` singular test and the `record_key` materialisation test: both are asserted on `int_std_records`, which does not exist yet (ER-043).
- `src/er/dbt_runner.py` (ER-033): this ticket's integration test invokes `dbt` as a plain subprocess, closing the Python DuckDB connection first per S4.0b.
- Editing the TableSpec registry to add columns or relations.

## Design decisions applied

Closes B2 (logical keys are tests), M1 (canonical pair ordering as the precondition that makes assertion application correct), M3 (one membership row per record), M6 and M20 (the filtered uniqueness on `assertions` and `review_queue`). Three constraints that are easy to miss: (1) S12 M1 speaks of "a `dbt/models/schema.yml` declaring the `ddl.py`-owned relations as sources" — this ticket puts them in `dbt/models/sources.yml` instead, because dbt resolves any `.yml` under `models/` and `schema.yml` is reserved for the dbt-owned **model** contracts of M2+; the board title is normative on the filename. (2) `sources.yml` MUST be generated from the registry and checked by a parity unit test, never hand-maintained — a hand-edited file is a second authority and S5/S5.0 is the review-time authority. (3) The `keys` tag must cover the uniqueness, `accepted_values` and pair-ordering tests, since T-KEY-1a's whole gate is `dbt test --select tag:keys`. `ddl.py` never issues DDL against a dbt-owned relation and this ticket never declares one as a source.

## Acceptance criteria

- [ ] AC1: `uv run pytest tests/unit/test_dbt_sources_parity.py -q` passes, and mutating any column name, NOT NULL flag or enum domain in the committed `dbt/models/sources.yml` makes it fail, because the test renders from `TABLES` and asserts byte equality with the committed file.
- [ ] AC2: The set of tables declared in `dbt/models/sources.yml` equals `{spec.name for spec in TABLES if spec.owner == 'ddl'}` — fourteen relations — and contains no dbt-owned relation; asserted in both directions.
- [ ] AC3: On a namespaced lake after `er init`, `dbt test --select tag:keys --target lake` exits 0 and reports a non-zero test count that equals the number of key/domain tests the renderer emitted.
- [ ] AC4: Inserting a second `raw_records` row with an identical `(source_system, source_record_id, content_hash)` makes `dbt test --select tag:keys` exit non-zero with `raw_records` named in the output (T-KEY-1a).
- [ ] AC5: `CREATE TABLE lake.main.t_pk (a VARCHAR PRIMARY KEY)` raises against the attached lake, and the test asserts on the raised error rather than skipping.
- [ ] AC6: Inserting `('b:2','a:1', …)` into `match_scores` — and the equivalent into `assertions`, `review_queue` with `subject_type='pair'`, and `cut_edges` — makes `assert_canonical_pair_ordering` fail naming that relation; the canonicalised row passes.
- [ ] AC7: Every `∈ {…}` domain in the S5 DDL has an `accepted_values` test: writing `entities.status='bogus'`, `run_stages.stage='doctor'` or `er_touched_entities.disposition='keep'` each makes `dbt test --select tag:keys` exit non-zero.

## Tests

- tests/unit/test_dbt_sources_parity.py::test_rendered_document_equals_committed_file
- tests/unit/test_dbt_sources_parity.py::test_declared_tables_equal_ddl_owned_registry
- tests/unit/test_dbt_sources_parity.py::test_every_enum_domain_has_accepted_values_test
- tests/unit/test_dbt_sources_parity.py::test_every_s5_0_logical_key_has_a_tagged_test
- tests/integration/test_logical_keys.py::test_keys_selector_is_non_empty_and_green
- tests/integration/test_logical_keys.py::test_pair_ordering_violation_fails_singular_test
- tests/integration/test_logical_keys.py::test_accepted_values_rejects_unknown_domain_value
- tests/integration/test_logical_keys.py::test_filtered_uniqueness_on_active_assertions
- tests/integration/test_keys.py::test_ddl_owned_duplicate_key_fails_dbt_test

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_logical_keys.py -q && uv run pytest tests/unit/test_dbt_sources_parity.py -q
bash scripts/ci/itest.sh tests/integration/test_keys.py::test_ddl_owned_duplicate_key_fails_dbt_test -q
uv run dbt deps --project-dir dbt && uv run dbt parse --project-dir dbt --profiles-dir dbt/profiles --target mem
uv run mypy --strict src/er/lake/dbt_sources.py
```

## Definition of Done

- All acceptance criteria demonstrated by the listed node ids
- `dbt/models/sources.yml` is generated output, committed, and byte-equal to `render_sources_yml(TABLES)`
- Every S5.0 logical key of a `ddl.py`-owned relation has exactly one test, tagged `keys`
- No dbt model file and no model contract added
- `dbt parse --target mem` green on a bare runner (no services)
- ruff + `mypy --strict src/er` clean; verify command passes; board entry updated with provides
