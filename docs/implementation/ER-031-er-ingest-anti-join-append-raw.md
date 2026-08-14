---
id: ER-031
title: "er ingest: anti-join append to raw_records, ingest_batches manifest, counters, T-IDEM-1"
milestone: M1
status: todo
kind: code
size: M
gates: full
depends_on: ["ER-023", "ER-026", "ER-029", "ER-030"]
spec_refs: ["s4", "s4-0", "s4-1", "s4-1-1", "s5", "s5-0", "s5-2", "s8-1"]
gap_refs: ["D7", "B2", "M2", "M14", "M15"]
provides: ["src/er/ingest/landing.py::ingest_delivery", "src/er/ingest/landing.py::IngestManifest", "src/er/ingest/landing.py::content_hash", "src/er/ingest/landing.py::anti_join_append", "src/er/ingest/landing.py::write_ingest_batch", "src/er/cli.py::ingest", "relation:raw_records", "relation:ingest_batches", "counters:ingest"]
consumes: ["src/er/ingest/hashing.py::content_hash", "src/er/ingest/sources.py::SourceAdapter", "src/er/ingest/sources.py::read_delivery", "src/er/config/schema.py::Config", "src/er/config/loader.py::load_config", "src/er/lake/ducklake.py::connect", "src/er/lake/model.py::REGISTRY", "src/er/lake/ddl.py::apply", "src/er/cli.py::app", "src/er/errors.py", "RunContext", "relation:runs", "relation:run_stages", "tests/conftest.py::lake_conn", "tests/conftest.py::er_env", "src/er/entities/ids.py::record_key"]
owns: ["src/er/ingest/landing.py", "tests/integration/test_ingest_idempotence.py", "tests/unit/test_landing.py"]
protected_paths: []
extra_paths: ["src/er/cli.py"]
attempts: 0
verify: "bash scripts/ci/itest.sh tests/integration/test_ingest_idempotence.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

Implement the `er ingest` stage: read a delivery through the S4.1 source adapters, compute `content_hash` per S4.1, and append to `raw_records` as an anti-join against the logical key `(source_system, source_record_id, content_hash)` — never an upsert (D7, S4.1). Persist exactly one `ingest_batches` manifest row per invocation with the five counts S4.1/S5 name, and write the S4.1.1 counter payload into the stage's `run_stages` row. This is the first stage that writes real rows to the lake, so it is also where the S4 preamble's idempotency contract (re-execution leaves the lake logically unchanged and exits `10`) becomes executable.

## Scope

### In scope

- `src/er/ingest/landing.py`: `ingest_delivery(cfg, source, path, conn, run_ctx) -> IngestManifest` performing read → hash → anti-join append → manifest write.
- Anti-join append on `(source_system, source_record_id, content_hash)` as a single write statement; a re-delivered identical file appends zero rows.
- Classification of every delivered key into `new_count` / `changed_count` / `unchanged_count` exactly as S4.1 defines them.
- One `ingest_batches` row per invocation carrying all eleven S5 columns, with `tombstone_count=0`, `resurrected_count=0`, `full_refresh_keys=false` (the deletion arm is ER-032).
- `raw_records.is_deleted=false` and `deleted_at` NULL on every row this stage writes; `payload` is the full source row verbatim as JSON.
- Wiring the real command into the existing CLI ingest slot: `--source` (required), `--path` (required), exit codes `0` / `10` / `2` / `1` per the S4.0 row, and the stdout manifest line `source, files, new, changed, unchanged, tombstoned, resurrected, ingest_batch_id`.
- `run_stages` counters for the stage: `rows_in`, `rows_out`, `duration_ms` as typed columns and the S4.1.1 `counters` JSON payload.
- The ingest half of idempotence in `tests/integration/test_ingest_idempotence.py`.

### Out of scope

- Tombstones, `--full-refresh-keys`, the empty-delivery guard and resurrection — ER-032.
- Any assertion that reads `int_std_records`, `match_scores`, `entity_membership` or `golden_*`; those relations do not exist in M1. T-IDEM-1a (M2) and T-IDEM-1 (M4) are separate arms with their own node ids in `tests/integration/test_idempotence.py` and are not created here.
- A second `content_hash` implementation: ER-029 owns the algorithm and its committed golden vector.
- Source-file discovery, CSV/Parquet parsing and `sources.columns` projection — ER-030.
- Schema evolution / breaking-change preflight — ER-035.

## Design decisions applied

Implements D7 (append-only version history) and gap entries B2, M2, M14. Constraints that are easy to miss: (1) S4.1 names the hash function `er.ingest.landing.content_hash(row, columns)` while S3 places the module at `src/er/ingest/hashing.py` — resolve by re-exporting ER-029's function from `landing.py`; there must remain exactly one implementation and one golden vector. (2) The write is an anti-join append, never an upsert and never a MERGE; a corrected record ADDS a row. (3) `ingest_batches` is written on every accepted invocation including a no-op one — the exit code is `10` but the manifest row still exists, because T-IDEM-1a asserts on the *second* row's counts. (4) The five count names on `ingest_batches`, the five counter names in `run_stages.counters` and the five stdout labels are the same five numbers under three spellings; do not invent a sixth counter. (5) `counters` must be the union of the S4.1.1 per-stage names and the promoted counters this stage writes (S5.2 completeness rule). (6) A standalone `er ingest` mints its own run: `runs.mode='stage'` with exactly one `run_stages` row naming `ingest`. (7) The advisory lock (S4.0b) is taken like any other writer; failure to acquire is exit `3`.

## Acceptance criteria

- [ ] AC1: Given a `crm` delivery of N distinct keys against an empty lake, `er ingest --source crm --path <dir>` exits `0`, appends exactly N `raw_records` rows all with `is_deleted=false` and `deleted_at` NULL, and writes exactly one `ingest_batches` row with `new_count=N, changed_count=0, unchanged_count=0, tombstone_count=0, resurrected_count=0, full_refresh_keys=false`.
- [ ] AC2: Re-running the identical command over the same directory exits `10`, appends zero `raw_records` rows, leaves every non-`VOLATILE_COLUMNS` value of the existing rows unchanged, and writes a second `ingest_batches` row with `new_count=0 AND changed_count=0 AND unchanged_count=N AND tombstone_count=0`.
- [ ] AC3: Re-delivering one already-ingested key with one changed mapped source column appends exactly one row: `raw_records` then holds two rows for that `(source_system, source_record_id)` with different `content_hash` values and both prior and new versions present, and the manifest reports `new_count=0, changed_count=1, unchanged_count=N-1`.
- [ ] AC4: For every appended row, the stored `content_hash` equals `er.ingest.hashing.content_hash(row, cfg.sources[source].columns)` recomputed in the test from the source CSV, and `er.ingest.landing.content_hash` is the same object as `er.ingest.hashing.content_hash` (identity assertion, so no second implementation can exist).
- [ ] AC5: `er ingest --source nope --path <dir>` exits `2` before any lake connection is opened, and a delivery directory containing no parsable file exits `10` with zero appended rows.
- [ ] AC6: The stage's `run_stages` row has `stage='ingest'`, `status='succeeded'`, a non-NULL `(snapshot_start, snapshot_end)` range, `rows_in` = source rows read, `rows_out` = rows appended, and a `counters` JSON object whose key set is exactly `{files, new_count, changed_count, unchanged_count, tombstone_count, resurrected_count, ingest_batch_id, duration_ms, rows_in, rows_out}`.
- [ ] AC7: A standalone invocation writes exactly one `runs` row with `mode='stage'` and one `run_stages` row; stdout contains only the manifest line and stderr contains exactly one JSON line keyed by that `run_id`.

## Tests

- tests/integration/test_ingest_idempotence.py::test_first_delivery_appends_and_writes_manifest
- tests/integration/test_ingest_idempotence.py::test_reingesting_a_delivery_appends_no_rows_and_exits_10
- tests/integration/test_ingest_idempotence.py::test_changed_content_hash_appends_a_version_row
- tests/integration/test_ingest_idempotence.py::test_run_stages_counters_and_stdout_manifest
- tests/unit/test_landing.py::test_unknown_source_exits_2_before_connecting
- tests/unit/test_landing.py::test_anti_join_key_is_source_system_source_record_id_content_hash
- tests/unit/test_landing.py::test_landing_content_hash_is_the_hashing_module_function

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_ingest_idempotence.py -q
uv run pytest tests/unit/test_landing.py -q
uv run mypy --strict src/er
uv run ruff check . && uv run ruff format --check .
```

## Definition of Done

- All acceptance criteria have a named passing test
- `bash scripts/ci/itest.sh tests/integration/test_ingest_idempotence.py -q` passes and fails on `main` before the change
- `raw_records` is written only by an anti-join append; no UPDATE or MERGE statement targets it
- Exactly one `content_hash` implementation in the repository (identity assertion green)
- `ingest_batches` row written on every accepted invocation, including the `10` no-op
- `counters` payload satisfies the S5.2 completeness rule
- mypy --strict and ruff clean; `python3 scripts/lint_board.py docs/implementation` exits 0
