---
id: ER-095
title: "T-SNAP-1: runtime snapshot capture from run_stages, golden time travel"
milestone: M4
status: todo
kind: code
size: S
gates: full
depends_on: ["ER-092", "ER-093"]
spec_refs: ["s4", "s4-6", "s4-7", "s5-1", "s5-2", "s8-1", "s8-2-1", "s8-3"]
gap_refs: ["M2", "M22"]
provides: ["tests/integration/test_snapshot_time_travel.py::test_time_travel_to_pre_incremental_golden", "tests/helpers/snapshots.py::snapshot_end_for", "tests/helpers/snapshots.py::snapshot_range_for"]
consumes: ["relation:run_stages", "relation:runs", "src/er/golden/assemble.py::assemble", "tests/helpers/compare.py::assert_golden_equal", "tests/helpers/expected.py::load_scenario", "fixtures/static/base_10/expected/base/golden.csv", "tests/conftest.py::lake_ns", "src/er/lake/columns.py::GOLDEN_SURVIVABLE_COLUMNS", "scripts/ci/itest.sh"]
owns: ["tests/integration/test_snapshot_time_travel.py", "tests/helpers/snapshots.py"]
protected_paths: ["fixtures/static/base_10/expected/base/golden.csv"]
extra_paths: []
attempts: 0
verify: "bash scripts/ci/itest.sh tests/integration/test_snapshot_time_travel.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

T-SNAP-1 proves that the snapshot RANGE recorded in `run_stages` is a usable time-travel handle, which is the recovery story S4.7 rests on and the reason S4's preamble forbids asserting snapshot counts. The test reads `run_stages.snapshot_end` for the pre-incremental `assemble` stage at runtime, reads `golden_records AT (VERSION => :snap)` with an explicit column projection, and compares it against `expected/base/golden.csv` while asserting the CURRENT golden state differs — so a pipeline that never advanced the lake cannot pass. No absolute snapshot version appears anywhere in the test.

## Scope

### In scope

- `tests/helpers/snapshots.py`: `snapshot_end_for(conn, run_id, stage)` and `snapshot_range_for(conn, run_id, stage)`, both reading `run_stages` at runtime
- An integration test running `base_10` base phase, then the `incremental_batch` batch phase, then time-travelling to the base assemble's `snapshot_end`
- Explicit column projection over `GOLDEN_SURVIVABLE_COLUMNS` plus `entity_id` and `survivorship_version` (S5.1 forbids relying on `SELECT *` across additive changes)
- A sensitivity assertion that the current `golden_records` does NOT equal the base expectation

### Out of scope

- Snapshot expiry and retention (`er lake maintain`, ER-025)
- Any assertion on the NUMBER of snapshots a stage commits — the range is the unit, counts are not assertable
- Time travel across a breaking schema change (unsupported by S5.1)
- Rollback: there is none; reading a prior state is the supported operation

## Design decisions applied

Closes M2 (the pre-incremental snapshot id now has a persisted referent and T-SNAP-1 is writable) and M22 (every snapshot-dependent test captures its reference id at runtime and no test names an absolute version). Easy to miss: `assembled_at` is a `VOLATILE_COLUMNS` member and is dropped by `assert_golden_equal`, so the time-travelled comparison is about survived VALUES, not stamps; the untouched-entity rule of S4.6 means most rows are identical in both snapshots, which is precisely why the sensitivity assertion must target an entity the incremental batch touched. Follows the `SPEC_TEST_IDS` convention from ER-092.

## Acceptance criteria

- [ ] AC1: `bash scripts/ci/itest.sh tests/integration/test_snapshot_time_travel.py -q` exits 0.
- [ ] AC2: The snapshot version used by the test is obtained from `run_stages` for the `(run_id, stage='assemble')` of the base run, is non-NULL and greater than zero; the module source contains no integer literal used as a snapshot version (asserted by scanning the module's own source for a `VERSION =>` literal).
- [ ] AC3: `SELECT <explicit columns> FROM lake.main.golden_records AT (VERSION => :snap)` returns the base-phase golden state and `assert_golden_equal` against `fixtures/static/base_10/expected/base/golden.csv` passes.
- [ ] AC4: The CURRENT `golden_records` does not satisfy that same comparison after the incremental batch — the test asserts the failure, proving the time travel is doing the work.
- [ ] AC5: `snapshot_range_for` returns `(snapshot_start, snapshot_end)` with `snapshot_start <= snapshot_end` for every stage of the base run, and raises a named error for an unknown `(run_id, stage)` pair.
- [ ] AC6: The test asserts no snapshot count anywhere and passes unchanged when a stage commits additional snapshots (verified by re-running the base phase, which is a no-op that may commit empty snapshots).

## Tests

- tests/integration/test_snapshot_time_travel.py::test_time_travel_to_pre_incremental_golden
- tests/integration/test_snapshot_time_travel.py::test_current_state_differs_from_travelled_state
- tests/unit/test_snapshot_helpers.py::test_snapshot_range_rejects_unknown_stage

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_snapshot_time_travel.py -q
uv run pytest tests/unit/test_snapshot_helpers.py -q
uv run mypy --strict tests/helpers
```

## Definition of Done

- T-SNAP-1 green with the reference snapshot captured at runtime from `run_stages`.
- No absolute snapshot version literal anywhere in the test module; the snapshot-literal collection guard stays green.
- The time-travel query projects columns explicitly rather than `SELECT *`.
- The sensitivity arm asserts the current state fails the same comparison.
- ruff + `mypy --strict tests/helpers` clean; gate receipt recorded and `board.py complete` run.
