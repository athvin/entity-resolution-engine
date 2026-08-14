---
id: ER-089
title: "**NEW (split)** golden_display + presentation-only guard (no matching module reads it)"
milestone: M4
status: todo
kind: code
size: S
gates: full
depends_on: ["ER-088"]
spec_refs: ["s3", "s4-6", "s5", "s5-0", "s5-2"]
gap_refs: ["M4", "MINOR-golden_display"]
provides: ["relation:golden_display", "dbt/models/marts/golden_display.sql", "src/er/lake/columns.py::GOLDEN_MART_RELATIONS", "tests/unit/test_golden_display_isolation.py::test_no_matching_module_references_golden_display"]
consumes: ["relation:golden_records", "relation:golden_lineage", "src/er/lake/columns.py::ADDRESS_COMPOSITE_COLUMNS", "src/er/lake/columns.py::GOLDEN_SURVIVABLE_COLUMNS", "src/er/dbt_runner.py::run_dbt"]
owns: ["dbt/models/marts/golden_display.sql", "tests/integration/test_golden_display.py", "tests/unit/test_golden_display_isolation.py"]
protected_paths: []
extra_paths: ["dbt/models/schema.yml", "src/er/lake/columns.py"]
attempts: 0
verify: "bash scripts/ci/itest.sh tests/integration/test_golden_display.py -q && uv run pytest tests/unit/test_golden_display_isolation.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

`golden_display` was declared by S4.6 and present in neither S3 nor S5 in v1.0 (MINOR-golden_display, M4); it now exists as a dbt-owned relation whose defining property is negative — it is presentation casing only and MUST NEVER be read by the matching layer, or matching-layer data gets silently re-cased. This ticket ships the model and, more importantly, the guard that makes that negative property falsifiable: a static check that no matching, ingest or entity module and no staging/intermediate dbt model references it, plus the shared constant naming the three golden marts that ER-092 reaps in lockstep.

## Scope

### In scope

- `golden_display.sql`: one row per `entity_id` derived from `golden_records`, with the S5 column list (`display_name`, `display_email`, `display_phone`, `display_address`, `assembled_at`) and an enforced contract
- Pinned presentation transforms: proper-cased name, formatted phone, single-line composed address, email passed through unchanged
- `GOLDEN_MART_RELATIONS = ('golden_records', 'golden_lineage', 'golden_display')` in `columns.py`, the list ER-092's reap step consumes
- The isolation guard: a source scan plus a dbt-manifest check asserting `golden_display` has no downstream children and no matching-layer reader

### Out of scope

- The reap step itself and `er_touched_entities` (ER-092) — this ticket only provides the relation list it must use
- `golden_records` / `golden_lineage` (ER-088)
- Any locale, i18n or address-formatting configuration — the transforms are fixed in this model, not configurable in v1
- Adding a `survivorship_version` to `golden_display`: it carries none, and its provenance is read by joining `golden_records`/`golden_lineage` on `entity_id` (S4.6)

## Design decisions applied

Implements MINOR-golden_display and gap M4's missing relation. Constraints easy to miss: (1) the presentation transforms run on top of `golden_records` only — reading `int_std_records` here would re-introduce a second survivorship path; (2) `golden_display` carries no `survivorship_version` by design, so its schema must not gain one 'for symmetry'; (3) it is reaped alongside the other two marts for `disposition='retire'` (S4.6, S5.2) — an orphan display row is the row a consumer is most likely to read, which is why the shared constant lives here rather than being spelled out again in `assemble.py`; (4) the isolation guard must fail on a *new* reference, so it scans `src/er/matching/`, `src/er/entities/`, `src/er/ingest/` and `dbt/models/{staging,intermediate}/` for the literal relation name and asserts the dbt manifest gives the node zero children; (5) transforms are pinned for testability: `display_name` = title-cased `given_name` and `family_name` joined by a single space with NULL parts dropped; `display_phone` = `(NNN) NNN-NNNN` for a `+1` NANP number and the E.164 string verbatim otherwise; `display_address` = the six `addr_*` joined as `number street unit, city region postal` with empty parts and their separators dropped; `display_email` = `golden_records.email` verbatim.

## Acceptance criteria

- [ ] AC1: `dbt build --select golden_display --target lake` exits 0 and the relation has exactly the S5 columns and types, with `contract: {enforced: true}` and a `unique` test on `entity_id`
- [ ] AC2: `select count(*) from golden_display` equals `select count(*) from golden_records`, one row per entity, with no row whose `entity_id` is absent from `golden_records`
- [ ] AC3: For a `golden_records` row with `given_name='robert'`, `family_name='chen'`, `phone_e164='+14155550132'` and the six `addr_*` populated, `golden_display` emits `display_name='Robert Chen'`, `display_phone='(415) 555-0132'` and a `display_address` containing no empty separator runs; a row with NULL `given_name` yields a `display_name` with no leading space
- [ ] AC4: `display_email` is byte-equal to `golden_records.email` — the display layer never re-cases an email
- [ ] AC5: Rebuilding `golden_display` leaves `int_std_records`, `int_blocking_keys` and `match_scores` content-hash identical (matching-layer data untouched)
- [ ] AC6: `tests/unit/test_golden_display_isolation.py` fails when the literal `golden_display` is introduced into any file under `src/er/matching/`, `src/er/entities/`, `src/er/ingest/`, `dbt/models/staging/` or `dbt/models/intermediate/` (asserted with a synthetic positive case)
- [ ] AC7: The compiled dbt manifest reports zero children for the `golden_display` node
- [ ] AC8: `GOLDEN_MART_RELATIONS` names exactly the three golden relations, and a unit test asserts `golden_display` is among them

## Tests

- tests/integration/test_golden_display.py::test_display_schema_and_one_row_per_entity
- tests/integration/test_golden_display.py::test_display_transforms_are_pinned
- tests/integration/test_golden_display.py::test_matching_inputs_unchanged_by_display_rebuild
- tests/unit/test_golden_display_isolation.py::test_no_matching_module_references_golden_display
- tests/unit/test_golden_display_isolation.py::test_golden_display_has_no_downstream_dbt_children
- tests/unit/test_golden_display_isolation.py::test_golden_mart_relations_names_all_three

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_golden_display.py -q
uv run pytest tests/unit/test_golden_display_isolation.py -q
uv run dbt parse --project-dir dbt --profiles-dir dbt/profiles --target mem
```

## Definition of Done

- `golden_display` exists as a dbt-owned, contract-enforced relation matching S5 column-for-column, with no `survivorship_version`
- The four presentation transforms are pinned and each has a positive and a NULL-handling assertion
- The isolation guard is proven to fail on an introduced reference, not merely to pass today
- `GOLDEN_MART_RELATIONS` is exported for ER-092's reap step so the three-relation list exists once
- Both arms of the verify command pass and both failed before the change
