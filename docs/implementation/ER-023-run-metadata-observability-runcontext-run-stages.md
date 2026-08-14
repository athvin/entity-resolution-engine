---
id: ER-023
title: "Run metadata + Observability: RunContext, run_stages ten-counter vocabulary, one JSON line per stage, stdout purity, er_touched_entities accessor"
milestone: M1
status: todo
kind: code
size: M
gates: full
depends_on: ["ER-014", "ER-019", "ER-020"]
spec_refs: ["s4", "s4-0", "s4-6", "s5", "s5-2", "s8-1"]
gap_refs: ["M2", "M10", "NEW-Observability"]
provides: ["src/er/obs/runctx.py::RunContext", "src/er/obs/runctx.py::StageRun", "src/er/obs/counters.py::PROMOTED_COUNTERS", "src/er/obs/counters.py::StageCounters", "src/er/obs/logging.py::emit_stage_record", "src/er/obs/logging.py::STAGE_RECORD_KEYS", "src/er/obs/touched.py::write_touched_entities", "src/er/obs/touched.py::read_touched_entities", "src/er/obs/touched.py::DISPOSITIONS"]
consumes: ["src/er/cli.py::app", "src/er/errors.py::ExitCode", "src/er/config/hashing.py::config_hash", "src/er/config/schema.py::ErConfig", "src/er/entities/ids.py::UlidFactory", "src/er/lake/ducklake.py::connect", "src/er/lake/model.py::TABLES", "src/er/lake/ddl.py::apply_ddl", "tests/conftest.py::lake_ns", "tests/conftest.py::lake_conn"]
owns: ["src/er/obs/__init__.py", "src/er/obs/runctx.py", "src/er/obs/counters.py", "src/er/obs/logging.py", "src/er/obs/touched.py", "tests/unit/obs/__init__.py", "tests/unit/obs/test_counters.py", "tests/unit/obs/test_stage_log_line.py", "tests/unit/obs/test_stdout_purity.py", "tests/integration/test_run_metadata.py"]
protected_paths: ["tests/unit/test_cli_contract.py"]
extra_paths: ["src/er/cli.py"]
attempts: 0
verify: "bash scripts/ci/itest.sh tests/integration/test_run_metadata.py -q && uv run pytest tests/unit/obs -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

`run_id` is referenced by six relations and, until this ticket, has no referent (M2). This ticket gives every CLI invocation a `runs` row and every stage a `run_stages` row written on entry (`status='running'`) and updated on exit with the snapshot **range**, the eleven promoted counters, the free-form `counters` JSON payload and `error_class`/`error_detail`, and emits the same record as exactly one JSON line on stderr keyed by `run_id` (S5.2). It also ships the `er_touched_entities` accessor that S4.6's assembly and S11's coherence hook both read. Closes M2, the touched-set half of M10 and the NEW-Observability section.

## Scope

### In scope

- `RunContext`: context manager that mints or accepts a `run_id`, writes the `runs` row (tenant, mode, `config_hash`, `std_version`, `survivorship_version`, `code_version` from `git describe --always --dirty`, `started_at`), captures `snapshot_start` before the first stage and `snapshot_end` after the last, and stamps `status`/`ended_at` on exit including on failure.
- `RunContext.stage(name)`: writes the `run_stages` row on entry with dense 1-based `seq` per `run_id`, captures `snapshot_start`/`snapshot_end` around the stage body, and updates status, timings, `rows_in`/`rows_out`, the promoted counters, `counters` JSON and error fields on exit.
- `PROMOTED_COUNTERS`: the closed set of typed `run_stages` counter columns (S5.2); a stage may add names only to the `counters` JSON, never as a column.
- The S5.2 completeness rule: the persisted `counters` payload is the union of the stage's S4-declared per-stage names and every promoted counter it wrote.
- `emit_stage_record`: exactly one JSON line per stage on **stderr**, key set equal to `STAGE_RECORD_KEYS` (the S5.2 example's keys, in that order), including on failure with `status='failed'`.
- stdout purity: the human summary (or JSONL under `--json`) goes to stdout and the stderr line never moves.
- `write_touched_entities` / `read_touched_entities` over `er_touched_entities(run_id, entity_id, disposition, created_at)` with `disposition ∈ {rebuild, retire}`, idempotent on `(run_id, entity_id)`.
- Wiring into the `er run-all` chain and the standalone stage commands built in ER-014, including the not-yet-implemented no-op stages.

### Out of scope

- The S4.7 error taxonomy and the values of `error_class` (ER-024) — this ticket persists whatever class the raised error carries and leaves it non-null.
- The advisory lock and `--resume` (ER-024).
- Computing the touched set from `entity_events` and invoking the marts (ER-092) — only the accessor ships here.
- Any change to exit codes, flags or stdout formats defined by the S4.0 contract in ER-014.
- Snapshot **count** assertions anywhere: a stage commits a range, and a no-op run may legitimately commit empty snapshots (S4 preamble).

## Design decisions applied

M2 + NEW-Observability + D12. Constraints: (1) the promoted-counter set is the **eleven** typed columns S5.2 lists (`rows_in`, `rows_out`, `candidate_pairs`, `pairs_above_auto_merge`, `entities_created`, `entities_merged`, `entities_split`, `entities_retired`, `edges_cut`, `review_queue_added`, `duration_ms`); the board title's "ten-counter vocabulary" counts the ten a stage supplies, because `duration_ms` is derived by `RunContext` — ship eleven columns. (2) The record on stderr is *one line*, and stdout stays parseable by a caller who pipes it: `tests/unit/test_cli_contract.py` is protected precisely so the S4.0 stdout contract cannot be relaxed to make this easier. (3) A stage records a snapshot **range**, never a count, and a no-op stage legitimately produces an empty range. (4) `run_stages` is keyed `(run_id, seq)` and `(run_id, stage)` — a resumed or re-executed stage updates its row rather than appending a second one. (5) `src/er/obs/` is a new package not drawn in the S3 tree; the three normative S3 layout rules are unaffected.

## Acceptance criteria

- [ ] AC1: After `er run-all --mode incremental --skip-ingest` on an empty initialised lake, exactly one `runs` row exists with `status='succeeded'`, non-null `config_hash` and `code_version`, and `run_stages` holds four rows (`standardize`, `match`, `reconcile`, `assemble`) with `seq` densely 1..4 and `snapshot_end >= snapshot_start` on every row.
- [ ] AC2: Capturing stderr of a single stage yields exactly one line that `json.loads` parses, whose key set equals `STAGE_RECORD_KEYS` with no missing and no extra key, and whose `run_id` equals the `runs.run_id`.
- [ ] AC3: In human mode no stdout line parses as JSON; under `--json` every stdout line parses and stderr still carries exactly one line per stage — asserted by counting lines on both streams for the same run.
- [ ] AC4: `PROMOTED_COUNTERS` equals the eleven typed counter columns of `run_stages` in the registry (asserted in both directions), and attempting to write an unlisted counter name as a column raises, while writing it into `counters` JSON succeeds.
- [ ] AC5: For every stage in the chain, `set(json.loads(run_stages.counters))` contains every promoted counter the stage wrote non-null plus every per-stage name that stage declares — a stage that omits one fails the completeness assertion.
- [ ] AC6: A stage whose body raises leaves its own `run_stages` row with `status='failed'`, non-null `error_class`, `error_detail` and `ended_at`, every preceding row `succeeded`, and `runs.status='failed'`; re-executing that stage updates the same row (still one row per `(run_id, stage)`).
- [ ] AC7: `write_touched_entities(conn, run_id, [(e, 'rebuild')])` followed by a second write of the same `(run_id, entity_id)` leaves exactly one row; `read_touched_entities` returns only rows for the requested `run_id`; a disposition outside `{rebuild, retire}` is rejected before any write.

## Tests

- tests/unit/obs/test_counters.py::test_promoted_counters_equal_run_stages_typed_columns
- tests/unit/obs/test_counters.py::test_unknown_counter_goes_to_json_not_column
- tests/unit/obs/test_counters.py::test_counters_payload_is_union_of_declared_and_promoted
- tests/unit/obs/test_stage_log_line.py::test_exactly_one_json_line_with_exact_key_set
- tests/unit/obs/test_stage_log_line.py::test_failed_stage_line_carries_error_class
- tests/unit/obs/test_stdout_purity.py::test_human_mode_stdout_is_not_json
- tests/unit/obs/test_stdout_purity.py::test_json_mode_moves_stdout_only
- tests/integration/test_run_metadata.py::test_run_all_writes_one_run_and_four_stage_rows
- tests/integration/test_run_metadata.py::test_stage_records_snapshot_range
- tests/integration/test_run_metadata.py::test_failed_stage_marks_run_failed
- tests/integration/test_run_metadata.py::test_touched_entities_roundtrip_is_idempotent

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_run_metadata.py -q && uv run pytest tests/unit/obs -q
uv run pytest tests/unit/test_cli_contract.py -q
uv run mypy --strict src/er/obs
```

## Definition of Done

- All acceptance criteria demonstrated by the listed node ids
- Every stage in the `er run-all` chain, including no-op stubs, writes exactly one `run_stages` row with a snapshot range
- Exactly one stderr JSON line per stage; stdout contract unchanged and `tests/unit/test_cli_contract.py` unmodified and green
- Eleven promoted counters as typed columns; free-form names only in `counters` JSON
- `er_touched_entities` accessor exported for ER-092 and ER-104
- ruff + `mypy --strict src/er` clean; verify command passes
