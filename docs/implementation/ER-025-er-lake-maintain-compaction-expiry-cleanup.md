---
id: ER-025
title: "er lake maintain: compaction, expiry, cleanup, run_stages retention guard"
milestone: M1
status: done
kind: code
size: M
gates: full
depends_on: ["ER-023", "ER-024"]
spec_refs: ["s3", "s4-0", "s4-0b", "s4-7", "s5"]
gap_refs: ["MINOR-lake-maint", "M2", "M24"]
provides: ["src/er/lake/maintain.py::maintain", "src/er/lake/maintain.py::retention_cutoff", "src/er/lake/maintain.py::MaintainResult", "cli:er lake maintain"]
consumes: ["src/er/obs/runctx.py::RunContext", "src/er/lake/catalog.py::tenant_lock", "src/er/lake/ducklake.py::connect", "src/er/lake/objectstore.py::S3Client", "src/er/cli.py::app", "src/er/errors.py::ErrorClass", "tests/conftest.py::lake_ns", "tests/conftest.py::lake_conn"]
owns: ["src/er/lake/maintain.py", "tests/unit/test_retention_cutoff.py", "tests/integration/test_lake_maintain.py"]
protected_paths: ["tests/unit/test_cli_contract.py"]
extra_paths: ["src/er/cli.py"]
attempts: 1
verify: "bash scripts/ci/itest.sh tests/integration/test_lake_maintain.py -q"
branch: "ticket/ER-025-er-lake-maintain-compaction-expiry-cleanup"
commit: "79c65b37ee5d8b4c65bc62b4970c0813b7e8fdb4"
spec_sha: "2abcfe433c322f74"
updated_at: "2026-08-15T21:11:46Z"
session: 221e5e98-7c99-41b9-9ad6-08764503975a
---
## Description

A lake that commits a snapshot range per stage accumulates small files and snapshot history without bound, and a reused local volume grows forever (MINOR-lake-maint). This ticket implements `er lake maintain --retain-days N` (default 7) as the ordered sequence `merge_adjacent_files` → `expire_snapshots` → `cleanup_old_files` (S3), taken under the same tenant advisory lock every other writer takes so maintenance can never run concurrently with a pipeline stage (S4.7). The retention guard is the load-bearing part: expiry never reaps a snapshot referenced by a `run_stages` row inside the retention window, which is what keeps time travel — the only recovery tool S4.7 offers — usable.

## Scope

### In scope

- `maintain(conn, retain_days)` executing the three DuckLake calls in the S3 order and returning `MaintainResult(files_merged, snapshots_expired, files_deleted)`.
- `retention_cutoff(now, retain_days, referenced)`: a pure function returning the effective expiry cutoff as the minimum of `now - retain_days` and the oldest snapshot referenced by a `run_stages` row whose run started inside the window.
- CLI wiring per S4.0: `--retain-days N` (default 7), exit `0`, `3` when the lock is not acquired, `1` on maintenance failure; stdout line `files_merged, snapshots_expired, files_deleted`.
- Run metadata: `runs.mode='maintain'` and exactly one `run_stages` row with `stage='maintain'` carrying a snapshot range and `duration_ms`.
- Object-store reclamation observed through the `DATA_PATH` prefix listing.

### Out of scope

- `er lake reset` (ER-020) — a different verb with a different guard.
- The session teardown's `expire_snapshots`/`cleanup_old_files` calls in `tests/conftest.py` (ER-018/ER-026); this ticket does not change teardown.
- Any scheduling: `er lake maintain` is operator- or CI-invoked; no cron, no timer, no automatic invocation from `run-all`.
- Snapshot rollback or history rewriting of any kind (S4.7 non-goal).

## Design decisions applied

MINOR-lake-maint + M2 + M24. Constraints: (1) the order is normative — merging before expiry, expiry before cleanup; cleaning before expiry deletes nothing and expiring before merging leaves the small files it was meant to compact. (2) The retention guard is not "retain N days of snapshots": it is "never reap a snapshot a `run_stages` row inside the window still points at", so the cutoff is a `min()` over both, and `retention_cutoff` is a pure function so the boundary is unit-testable without a lake. (3) `er lake maintain` is a **writer**: it takes the S4.0b advisory lock (exit `3` on conflict) and is recorded like one, with `runs.mode='maintain'` and `run_stages.stage='maintain'` — both values already exist in the S5 enums. (4) There is no exit `10` for this command in the S4.0 table: an idempotent second run exits `0` with zero counts.

## Acceptance criteria

- [ ] AC1: `er lake maintain` on a lake carrying several snapshots calls `merge_adjacent_files`, then `expire_snapshots`, then `cleanup_old_files`, in that order (asserted from the recorded call sequence), exits 0 and prints the three counts.
- [ ] AC2: With the tenant advisory lock held by another session, `er lake maintain` exits 3 with the S4.7 lock message and commits no snapshot.
- [ ] AC3: A successful run writes `runs.mode='maintain'` and exactly one `run_stages` row with `stage='maintain'`, non-null `snapshot_start`/`snapshot_end` and non-null `duration_ms`.
- [ ] AC4: `retention_cutoff` is a total function of its arguments: a snapshot referenced by a run started `retain_days - 1` days ago is never expirable; one referenced only by a run started `retain_days + 1` days ago is; the exact `retain_days` boundary is retained.
- [ ] AC5: After `er lake maintain --retain-days 7`, every snapshot id recorded in a `run_stages` row of a run started within 7 days is still readable: `SELECT * FROM lake.main.raw_records AT (VERSION => :snap)` succeeds for each, with `:snap` captured at runtime from `run_stages`.
- [ ] AC6: The `DATA_PATH` prefix object count strictly decreases when at least one out-of-window snapshot was expired, and is unchanged when none was.
- [ ] AC7: Running `er lake maintain` twice in a row exits 0 both times, and the second reports `snapshots_expired=0` and `files_deleted=0`.

## Tests

- tests/unit/test_retention_cutoff.py::test_referenced_snapshot_inside_window_is_never_expirable
- tests/unit/test_retention_cutoff.py::test_retain_days_boundary_is_inclusive
- tests/unit/test_retention_cutoff.py::test_cutoff_is_min_of_window_and_referenced
- tests/integration/test_lake_maintain.py::test_calls_run_in_specified_order
- tests/integration/test_lake_maintain.py::test_lock_conflict_exits_3
- tests/integration/test_lake_maintain.py::test_writes_maintain_run_and_stage_rows
- tests/integration/test_lake_maintain.py::test_referenced_snapshots_remain_time_travelable
- tests/integration/test_lake_maintain.py::test_second_invocation_is_a_zero_count_noop

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_lake_maintain.py -q
uv run pytest tests/unit/test_retention_cutoff.py -q
uv run mypy --strict src/er/lake/maintain.py
```

## Definition of Done

- All acceptance criteria demonstrated by the listed node ids
- Three maintenance calls issued in the S3 order and counted
- Retention guard implemented as a pure, unit-tested cutoff function
- Command takes the tenant lock and is recorded as `runs.mode='maintain'` + one `run_stages` row
- No absolute snapshot version appears in any test; all captured at runtime from `run_stages`
- ruff + `mypy --strict src/er` clean; verify command passes
