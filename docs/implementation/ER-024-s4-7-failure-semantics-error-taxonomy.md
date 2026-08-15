---
id: ER-024
title: "S4.7 failure semantics: error taxonomy, --resume <run_id>, tenant advisory lock on every mutating command (exit 3), tenancy statement enforcement"
milestone: M1
status: blocked
kind: code
size: M
gates: full
depends_on: ["ER-015", "ER-023"]
spec_refs: ["s1", "s4-0", "s4-0b", "s4-7", "s5", "s5-2", "s8-3"]
gap_refs: ["M2", "M19", "M22", "MINOR-tenancy", "NEW-S4.7"]
provides: ["src/er/errors.py::ErrorClass", "src/er/errors.py::classify", "src/er/errors.py::ERROR_CLASS_TABLE", "src/er/lake/catalog.py::tenant_lock", "src/er/lake/catalog.py::LOCK_HELD_MESSAGE", "src/er/resume.py::resume_plan", "src/er/resume.py::ResumePlan", "cli:er run-all --resume", "tests/integration/test_cli_contract.py::test_second_concurrent_run_exits_3"]
consumes: ["src/er/obs/runctx.py::RunContext", "src/er/obs/logging.py::emit_stage_record", "src/er/cli.py::app", "src/er/errors.py::ExitCode", "src/er/lake/catalog.py::catalog_connection", "src/er/config/hashing.py::config_hash", "src/er/lake/model.py::TABLES", "tests/conftest.py::lake_ns", "tests/conftest.py::er_env"]
owns: ["src/er/resume.py", "tests/unit/test_locking.py", "tests/unit/test_resume_plan.py", "tests/integration/test_failure_resume.py", "tests/integration/test_concurrency.py", "tests/integration/test_cli_contract.py"]
protected_paths: ["tests/unit/test_cli_contract.py"]
extra_paths: ["src/er/cli.py", "src/er/errors.py", "src/er/lake/catalog.py"]
attempts: 0
verify: "bash scripts/ci/itest.sh tests/integration/test_failure_resume.py tests/integration/test_concurrency.py -q && uv run pytest tests/unit/test_locking.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-15T15:00:12Z"
---
## Description

S4.7 is the recovery contract: every failure is classified into `run_stages.error_class`, a mid-run failure leaves stages `1..k-1` committed, and `er run-all --resume <run_id>` restarts from the first non-`succeeded` stage with the original `run_id` and `config_hash`. This ticket implements the seven-class taxonomy with its exit codes and retryability, the Postgres advisory lock keyed on `tenant` that makes v1's single-writer model enforceable (S4.0b), and the namespace-only tenancy statement of S1. T-CONC-1 — a second concurrent run exits 3 without writing a `runs` row — is an M1 exit criterion.

## Scope

### In scope

- `ErrorClass` with exactly the seven S4.7 classes (`transient_io`, `lock_conflict`, `precondition`, `config`, `contradiction`, `non_convergence`, `data`), each carrying its exit code and retryable flag; `classify()` maps a raised exception to one.
- `run_stages.error_class` is written with the class name and `error_detail` with the message; the stderr JSON line carries both.
- `tenant_lock(dsn, tenant)`: a Postgres advisory lock over `$ER_CATALOG_DSN` keyed on the config `tenant`, acquired before the first stage, held for the process lifetime and released in a `finally` including on failure; failure to acquire exits `3` with the literal message `writer lock held for tenant <t> by run <run_id>` and writes nothing — not even a `runs` row.
- Every mutating command takes the lock: `init`, `ingest`, `standardize`, `train`, `match`, `reconcile`, `assemble`, `run-all`, `correct`, `assert add|remove|load`, `review resolve`, `lake maintain`, `lake reset`. `doctor` and `review list` do not.
- `resume_plan(rows, on_disk_config_hash)`: a pure function returning the first non-`succeeded` stage plus the run's pinned `run_id`/`config_hash`/`model_version`, raising the `config` class (exit 2) on a config-hash mismatch and the `precondition` class (exit 3) when the run is already `succeeded`.
- `er run-all --resume <run_id>` wiring: re-executes the failed stage in full, never mid-stage, and preserves `(run_id, stage)` uniqueness.
- Tenancy enforcement: a test asserting no relation in the registry carries a `tenant` column except `runs.tenant`.

### Out of scope

- Automatic retry loops and snapshot rollback — both are explicit non-goals of S4.7; time travel over the recorded snapshot range is the only recovery tool.
- The config-drift guard on `(config_hash, model_version, std_version)` and `--allow-escalate` (ER-034) — `--resume`'s own config-hash refusal is in scope, the mode guard is not.
- `er lake maintain`'s implementation (ER-025); this ticket only makes the lock available to it.
- Changing the `runs`/`run_stages` writing mechanics from ER-023.

## Design decisions applied

NEW-S4.7 + M2 + M19 + M22 + MINOR-tenancy. Constraints: (1) `lock_conflict` exits `3`, and the refusal happens **before** any write — a `runs` row written by the refused process would corrupt T-CONC-1 and the run ledger. (2) The lock lives on the catalog DSN and is keyed on `tenant`, which names the namespace pair (`$ER_LAKE_METADATA_SCHEMA`, `$ER_LAKE_DATA_PATH`) — not on a row-level discriminator; no table has a `tenant` column except `runs`. (3) `--resume` re-executes the failed stage **in full**; it never resumes mid-stage and never mints a new `run_id`. (4) Release must be in a `finally` so a crashed writer does not wedge the tenant — the session-scoped Postgres connection dying releases it, and the test proves a failed run is followed by a successful acquire. (5) T-CONC-1's node id is pinned by S8.3 at `tests/integration/test_cli_contract.py::test_second_concurrent_run_exits_3`; this ticket creates that file, and ER-034 later adds T-CFG-1 to it.

## Acceptance criteria

- [ ] AC1: `ERROR_CLASS_TABLE` reproduces the S4.7 table exactly: a table-driven test asserts the exit code and retryable flag of all seven classes, and an unclassified exception maps to `data` with exit 1.
- [ ] AC2: Two `er run-all` invocations against the same tenant overlap in time: the second exits 3, prints `writer lock held for tenant test by run <run_id>` and adds zero rows to `runs` and `run_stages` (T-CONC-1).
- [ ] AC3: The lock is released on failure: after a run that exits non-zero mid-stage, a subsequent run against the same tenant acquires the lock and completes, with no manual unlock step.
- [ ] AC4: With the advisory lock held by an external session, every command in the mutating list exits 3 while `er doctor` and `er review list` exit 0 — asserted per command, not in aggregate.
- [ ] AC5: `er run-all --resume <run_id>` on a run whose `match` stage failed re-executes from `match`: afterwards `(run_id, stage)` is still unique per stage, the run keeps its original `run_id` and `config_hash`, and the `started_at` of each already-succeeded stage is byte-unchanged.
- [ ] AC6: `--resume` on a run whose `status='succeeded'` exits 3; `--resume` after editing `configs/test.yaml` so its `config_hash` differs exits 2 with the mismatch named.
- [ ] AC7: A failed run leaves stages `1..k-1` with `status='succeeded'` and their snapshot ranges recorded, stage `k` with `status='failed'` and a populated `error_class`/`error_detail`, and `runs.status='failed'`.
- [ ] AC8: No relation in the registry declares a `tenant` column other than `runs.tenant`.

## Tests

- tests/unit/test_locking.py::test_error_class_table_matches_s4_7
- tests/unit/test_locking.py::test_lock_key_derives_from_tenant
- tests/unit/test_locking.py::test_lock_released_in_finally_on_exception
- tests/unit/test_resume_plan.py::test_first_non_succeeded_stage_is_selected
- tests/unit/test_resume_plan.py::test_config_hash_mismatch_is_exit_2
- tests/unit/test_resume_plan.py::test_already_succeeded_run_is_exit_3
- tests/integration/test_concurrency.py::test_mutating_commands_require_the_lock
- tests/integration/test_concurrency.py::test_lock_is_reacquirable_after_failure
- tests/integration/test_cli_contract.py::test_second_concurrent_run_exits_3
- tests/integration/test_failure_resume.py::test_failed_stage_leaves_prefix_committed
- tests/integration/test_failure_resume.py::test_resume_restarts_from_failed_stage_without_duplicating_rows
- tests/integration/test_failure_resume.py::test_no_tenant_column_outside_runs

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_failure_resume.py tests/integration/test_concurrency.py -q && uv run pytest tests/unit/test_locking.py -q
bash scripts/ci/itest.sh tests/integration/test_cli_contract.py::test_second_concurrent_run_exits_3 -q
uv run pytest tests/unit/test_resume_plan.py -q
uv run mypy --strict src/er/resume.py src/er/errors.py
```

## Definition of Done

- All acceptance criteria demonstrated by the listed node ids
- Seven error classes with S4.7's exit codes and retryability, persisted to `run_stages.error_class`
- Advisory lock on every mutating command, released in a `finally`, refusing before any write with the literal message
- T-CONC-1 green at its S8.3 node id and referenced by the M1 exit gate
- `--resume` re-executes whole stages only and refuses on config drift (2) or a succeeded run (3)
- No retry loop and no rollback path added anywhere
- ruff + `mypy --strict src/er` clean; verify command passes

## Blocker log

### Attempt 0 — gate_failed (2026-08-15T15:00:12Z)

- **Failing command:** `bash scripts/gates.sh --scope full --no-cache (driver re-verification on main)`
- **Assertion / contradiction:** The ticket was marked done, but an independent full-ladder run on merged main failed. See /Users/athvin/github.com/athvin/entity-resolution-engine/.loop/runs/abc08182-6ae5-45da-9564-e634d85fe45c/reverify-ER-024.log
- **Smallest change that would unblock:** Inspect branch loop-quarantine/ER-024, fix the failing gate, then unblock the ticket.
- **Log:** `/Users/athvin/github.com/athvin/entity-resolution-engine/.loop/runs/abc08182-6ae5-45da-9564-e634d85fe45c/reverify-ER-024.log`
