---
id: ER-017
title: "lake/model.py TableSpec registry (all relations, owners, logical keys, enums incl. edge_cut/retired/member_removed, seq/details_hash, cut_edges, full runs/run_stages column set) — pure"
milestone: M1
status: done
kind: code
size: M
gates: fast
depends_on: ["ER-006", "ER-013"]
spec_refs: ["s3", "s5", "s5-0", "s5-2", "s4-4-2", "s4-5-3", "s4-5-4", "s12-1"]
gap_refs: ["B2", "M2", "M3", "M4", "M6", "M10", "M15", "M20", "MINOR-event_id", "MINOR-golden_display", "D3", "D4", "D5", "D7", "D8"]
provides: ["src/er/lake/model.py::Owner", "src/er/lake/model.py::Column", "src/er/lake/model.py::TableSpec", "src/er/lake/model.py::REGISTRY", "src/er/lake/model.py::DDL_OWNED", "src/er/lake/model.py::DBT_OWNED", "src/er/lake/model.py::PAIR_RELATIONS", "src/er/lake/model.py::create_table_sql", "src/er/lake/model.py::logical_key", "src/er/lake/model.py::PROMOTED_COUNTERS", "src/er/lake/model.py::EVENT_TYPES", "src/er/lake/model.py::ENTITY_STATUSES", "src/er/lake/model.py::ASSERTION_KINDS", "src/er/lake/model.py::REVIEW_SUBJECT_TYPES", "src/er/lake/model.py::REVIEW_REASONS", "src/er/lake/model.py::REVIEW_STATUSES", "src/er/lake/model.py::MODEL_STATUSES", "src/er/lake/model.py::RUN_MODES", "src/er/lake/model.py::RUN_STAGES", "src/er/lake/model.py::RUN_STATUSES", "src/er/lake/model.py::REBUILD_REASONS", "src/er/lake/model.py::DISPOSITIONS"]
consumes: ["src/er/lake/columns.py::VOLATILE_COLUMNS", "src/er/lake/columns.py::GOLDEN_SURVIVABLE_COLUMNS", "src/er/lake/columns.py::STD_RECORD_COLUMNS", "src/er/entities/ids.py::record_key", "src/er/entities/ids.py::canonicalize_pair"]
owns: ["src/er/lake/model.py", "tests/unit/test_ddl_registry.py"]
protected_paths: ["src/er/lake/columns.py", "DesignDoc.md"]
extra_paths: []
attempts: 1
verify: "uv run pytest tests/unit/test_ddl_registry.py -q && uv run mypy --strict src/er/lake/model.py"
branch: "ticket/ER-017-lake-model-py-tablespec-registry-all"
commit: "6fa7b81fc4ad180640a7141c04199f10b9132ab8"
spec_sha: "28d8d8e366a7b49b"
updated_at: "2026-08-15T01:54:33Z"
session: 54b7ebbc-9561-4cef-9574-23f6aac094bb
---
## Description

Encode S5 and S5.0 as one pure, importable registry: every relation, its owner (`ddl.py` or dbt), its typed column list in DDL order, its logical key, and every `in {…}` domain the DDL comments declare. B2 is the blocker that DuckLake enforces `NOT NULL` only, so keys are logical and the registry is where they live; M4 is that two systems claimed the same relations, so a relation appearing under both owners must be impossible by construction. Because the registry is pure it lands before anything persists, and `ddl.py` (ER-019), the dbt `sources.yml` generator (ER-021) and every later stage read column lists from it instead of restating them.

## Scope

### In scope

- `TableSpec` with name, owner, ordered `Column` list (name, type, nullable), logical key(s) including the filtered ones, and declared enum domains
- All fourteen `ddl.py`-owned relations of S5 with their exact column lists and types, including `entity_events.seq`/`details_hash`, `cut_edges` in full, `review_queue.reason`, `ingest_batches.resurrected_count`/`full_refresh_keys`, `er_touched_entities`, and the complete `runs` / `run_stages` column sets
- All six dbt-owned relations (`stg_crm`, `stg_billing`, `stg_webforms`, `int_std_records`, `int_blocking_keys`, `golden_records`, `golden_lineage`, `golden_display`) declared as typed column lists with owner `DBT` for ownership checks and contract generation
- Enum vocabularies: event types incl. `edge_cut` and `member_removed`, entity statuses incl. `retired`, run modes incl. `reset`/`correction_pass`/`stage`, review reasons incl. `never_unsatisfiable`/`coherence`, dispositions, rebuild reasons, model statuses, assertion kinds
- `create_table_sql(spec)`: pure `CREATE TABLE IF NOT EXISTS` text for `ddl.py`-owned specs only
- `PROMOTED_COUNTERS`: the closed eleven-name list of typed `run_stages` counter columns
- `PAIR_RELATIONS`: the four relations carrying the `rec_a_key < rec_b_key` invariant

### Out of scope

- Executing any statement, opening any connection, or applying/evolving schema — ER-019 owns `apply`/`evolve` and ER-020 owns `er init`
- Emitting dbt `schema.yml` / `sources.yml` or any dbt test (ER-021)
- The Splink model-artifact lifecycle that S3 also annotates on `lake/model.py` (params_path allocation, active/superseded pointer, `tf_snapshot_id`) — it lands with `er train` and must not be pre-built here
- Re-listing `VOLATILE_COLUMNS`, `GOLDEN_SURVIVABLE_COLUMNS` or the std-record column contract; they are imported from `columns.py`

## Design decisions applied

Closes B2 (logical keys, no enforced constraints), M2 (`runs`/`run_stages` are the referent for every `run_id` and carry the snapshot range), M3 (D3: `entity_membership` is current state, one row per `(source_system, source_record_id)`), M4 (D14: exactly two owners, and a relation in both is a defect `lint_spec.py` fails on), M6/D5 (`cut_edges`, `edge_cut`, `never_unsatisfiable`), M10 (`er_touched_entities` with `disposition`), M15/D8 (`is_deleted`/`deleted_at`, so `member_removed` and `retired` are reachable), M20 (`review_queue` keyed with `reason` in the open-row tuple because one pair can be open for two independent reasons), MINOR-`event_id` (`event_id` + `seq` + `details_hash` idempotency key) and MINOR-`golden_display`. Easy to miss: (1) only `NOT NULL` may appear in any generated statement — no PRIMARY KEY, UNIQUE, FOREIGN KEY, CHECK, DEFAULT, ENUM, sequence, index, or fixed-size `ARRAY`; `LIST(VARCHAR)` and `JSON` are retained; (2) `match_scores`' logical key `(rec_a_key, rec_b_key, model_version, tf_snapshot_id)` is unfiltered — invalidation updates `is_active` in place and never adds a second row; (3) `run_stages` promoted counters are a CLOSED list and a stage may add JSON counter names but never a column; (4) `raw_records` carries no update column: it is append-only version history keyed with `content_hash` (D7); (5) the registry is the review-time authority's mirror — when it disagrees with S5, fix the registry, never `DesignDoc.md`.

## Acceptance criteria

- [ ] AC1: `set(DDL_OWNED)` equals exactly the fourteen `ddl.py`-owned relation names of S5, `set(DBT_OWNED)` the eight dbt-owned names, and their intersection is empty.
- [ ] AC2: For every `ddl.py`-owned relation, `create_table_sql(spec)` matches the corresponding `CREATE TABLE IF NOT EXISTS` block parsed out of the S5 section of `DesignDoc.md` column-for-column, type-for-type, in declared order, including `NOT NULL` placement; the same parse-and-compare holds for the dbt-owned typed column lists.
- [ ] AC3: No text produced by `create_table_sql` for any spec contains PRIMARY KEY, UNIQUE, FOREIGN KEY, CHECK, DEFAULT, ENUM, SEQUENCE, CREATE INDEX or a fixed-size `ARRAY` type.
- [ ] AC4: `logical_key(...)` equals the S5.0 ownership table for all twenty-two relations, including the filtered keys: `assertions (rec_a_key, rec_b_key) where active`, `review_queue (subject_type, rec_a_key, rec_b_key, entity_id, reason) where status='open'`, `cut_edges (rec_a_key, rec_b_key) where active`, `model_registry` at most one `status='active'`, and `entity_events` `event_id` plus idempotency key `(run_id, entity_id, event_type, details_hash)`.
- [ ] AC5: The enum domains equal exactly: `EVENT_TYPES == {created, member_added, member_removed, merged, split, retired, edge_cut}`, `ENTITY_STATUSES == {active, merged, retired}`, `RUN_MODES == {incremental, full, train, init, maintain, reset, correction_pass, stage}`, `RUN_STAGES == {init, ingest, standardize, train, match, reconcile, assemble, maintain, reset}`, `REVIEW_REASONS == {gray_band, never_unsatisfiable, coherence}`, `DISPOSITIONS == {rebuild, retire}`.
- [ ] AC6: `PROMOTED_COUNTERS` is exactly the eleven S5.2 names and each is a nullable `BIGINT` column of the `run_stages` spec; `PAIR_RELATIONS == {match_scores, assertions, review_queue, cut_edges}` and each declares `rec_a_key`/`rec_b_key`.
- [ ] AC7: `golden_records`' survivable columns — every column except `entity_id`, `survivorship_version`, `assembled_at` — equal the imported `GOLDEN_SURVIVABLE_COLUMNS` tuple in order, and the module re-lists neither that tuple nor `VOLATILE_COLUMNS`.
- [ ] AC8: Importing `src/er/lake/model.py` imports neither `duckdb` nor `psycopg`, and `uv run mypy --strict src/er/lake/model.py` exits 0.

## Tests

- tests/unit/test_ddl_registry.py::test_owner_partition_is_exact_and_disjoint
- tests/unit/test_ddl_registry.py::test_registry_matches_spec_s5_ddl
- tests/unit/test_ddl_registry.py::test_only_not_null_constraints_are_emitted
- tests/unit/test_ddl_registry.py::test_logical_keys_match_s5_0_table
- tests/unit/test_ddl_registry.py::test_enum_domains_are_exact
- tests/unit/test_ddl_registry.py::test_promoted_counters_and_pair_relations
- tests/unit/test_ddl_registry.py::test_golden_survivable_columns_are_imported_not_restated
- tests/unit/test_ddl_registry.py::test_registry_module_is_pure

## Verification

```bash
uv run pytest tests/unit/test_ddl_registry.py -q && uv run mypy --strict src/er/lake/model.py
uv run ruff check src/er/lake/model.py tests/unit/test_ddl_registry.py
```

## Definition of Done

- All twenty-two relations registered with owner, ordered typed columns, logical key and enum domains
- Generated DDL matches S5 exactly, parsed from `DesignDoc.md` by the test rather than hand-copied
- Only `NOT NULL` is ever emitted; no index, sequence, default or enum type
- `columns.py` imported, never restated; `DesignDoc.md` unmodified
- Module is pure (no duckdb/psycopg import, no I/O); verify command passes; mypy --strict clean
