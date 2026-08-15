---
id: ER-034
title: "Mode preconditions + T-CFG-1: refuse incremental on config_hash/model_version/std_version/survivorship_version drift; --allow-escalate"
milestone: M1
status: in_progress
kind: code
size: S
gates: full
depends_on: ["ER-023"]
spec_refs: ["s4-0", "s4-7", "s5", "s5-1", "s5-2", "s8-3"]
gap_refs: ["M2", "M18", "M26"]
provides: ["src/er/versions.py::RunFingerprint", "src/er/versions.py::check_mode_preconditions", "src/er/versions.py::ModeDecision", "src/er/versions.py::last_successful_run", "src/er/cli.py::run-all --allow-escalate"]
consumes: ["src/er/config/hashing.py::config_hash", "src/er/config/schema.py::Config", "src/er/cli.py::app", "src/er/errors.py", "RunContext", "relation:runs", "relation:run_stages", "tests/conftest.py::lake_conn", "tests/conftest.py::er_env"]
owns: ["tests/integration/test_mode_guard.py", "tests/unit/test_mode_preconditions.py"]
protected_paths: []
extra_paths: ["src/er/versions.py", "src/er/cli.py", "tests/integration/test_cli_contract.py"]
attempts: 1
verify: "bash scripts/ci/itest.sh tests/integration/test_mode_guard.py -q && uv run pytest tests/unit/test_mode_preconditions.py -q"
branch: "ticket/ER-034-mode-preconditions-t-cfg-1-refuse"
commit: ""
spec_sha: "2abcfe433c322f74"
updated_at: "2026-08-15T19:13:40Z"
session: b46c088b-88b9-41a4-b808-8ca7e9a75bef
---
## Description

Implement the S4.0 config-drift guard: `er run-all --mode incremental` refuses to proceed with exit `3` when the run fingerprint `(config_hash, model_version, std_version, survivorship_version)` differs from the last successful run for this tenant, and `--allow-escalate` promotes the run to `--mode full` instead of failing. The guard is the mechanism S5.1 relies on to catch a planned version bump and the one T-CFG-1 asserts. The comparison itself is a pure function so the drift matrix is unit-testable without a lake.

## Scope

### In scope

- `RunFingerprint` over the four fields, all of which are already columns of `runs` (S5).
- `last_successful_run(conn, tenant)` selecting the most recent `runs` row with `status='succeeded'`, ordered by `run_id DESC` (ULIDs are time-ordered).
- `check_mode_preconditions(prior, current, mode, allow_escalate) -> ModeDecision` — pure, no I/O — returning proceed-incremental, refuse (exit `3`, named message) or escalate-to-full.
- Wiring the decision into `er run-all` ahead of the first stage, and the promotion of the chain to the S4.0 `--mode full` shape when escalating.
- The named refusal message identifying which of the four fields drifted and both values.
- The S8.3-pinned node `tests/integration/test_cli_contract.py::test_incremental_refuses_on_config_drift` (T-CFG-1), plus the broader drift matrix in `tests/integration/test_mode_guard.py`.

### Out of scope

- Setting `runs.rebuild_reason` and forcing the non-incremental stage flags for a `std_version` / `survivorship_version` bump — ER-035 owns that arm.
- Breaking-schema detection and the `ERR_SCHEMA_BREAKING` preflight — ER-035.
- The `--resume` precondition (a resumed run refusing on a config-hash change) — ER-024 owns it; this guard must not duplicate or contradict it.
- Any change to the advisory-lock or concurrency behaviour.

## Design decisions applied

Implements gap entries M2, M18, M26. Constraints: (1) S4.0 names three fields (`config_hash`, `model_version`, `std_version`); S5.1 adds `survivorship_version` by stating that a version bump is *exactly* the drift this guard catches, and `runs` carries all four as columns — the fingerprint is the four-tuple. (2) The guard runs after config validation and after the advisory lock is acquired but **before** the first stage, and a refusal writes no `runs` row and no `run_stages` row and commits no snapshot, matching the S4.7 `precondition` class (exit `3`, not retryable). (3) Only `status='succeeded'` runs are compared against; a failed run never becomes the baseline. (4) With no prior successful run the guard cannot fire — a first run is not drift. (5) `--mode full` never consults the guard. (6) The escalated run is recorded with `runs.mode='full'` and executes the S4.0 full chain (standardize without `--changed-only`, match `--mode full`, assemble without `--touched-only`).

## Acceptance criteria

- [ ] AC1: With one prior `status='succeeded'` `runs` row, mutating `thresholds.auto_merge` in the config and invoking `er run-all --mode incremental --skip-ingest` exits `3`, prints a message naming `config_hash` and both values, writes no new `runs` row and no `run_stages` row, and leaves the lake snapshot version unchanged.
- [ ] AC2: The same drift with `--allow-escalate` exits `0` and writes a `runs` row with `mode='full'`; the `run_stages` rows for that run show the full-mode chain (no `--changed-only`, `match --mode full`, no `--touched-only`).
- [ ] AC3: Each of the four fields drifting alone produces a refusal, and no combination of unchanged fields produces one — asserted as a parametrised matrix over the pure `check_mode_preconditions`.
- [ ] AC4: With an empty `runs` table, `er run-all --mode incremental --skip-ingest` exits `0` (a first run cannot drift), and with only `status='failed'` prior rows the guard still does not fire.
- [ ] AC5: Mutating all four fields and invoking `er run-all --mode full --skip-ingest` exits `0`: the guard is not consulted in full mode.
- [ ] AC6: `check_mode_preconditions` performs no I/O — the unit test calls it with plain dataclasses and no lake, catalog or filesystem access, and `tests/unit/test_mode_preconditions.py` runs on a bare runner.
- [ ] AC7: `tests/integration/test_cli_contract.py::test_incremental_refuses_on_config_drift` collects and passes, asserting exit `3` with the named message and then success under `--allow-escalate`.

## Tests

- tests/integration/test_mode_guard.py::test_config_hash_drift_refuses_with_exit_3
- tests/integration/test_mode_guard.py::test_allow_escalate_promotes_to_full_mode
- tests/integration/test_mode_guard.py::test_first_run_and_failed_prior_runs_do_not_trip_the_guard
- tests/integration/test_mode_guard.py::test_full_mode_never_consults_the_guard
- tests/integration/test_cli_contract.py::test_incremental_refuses_on_config_drift
- tests/unit/test_mode_preconditions.py::test_drift_matrix_over_the_four_fields
- tests/unit/test_mode_preconditions.py::test_decision_function_is_pure

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_mode_guard.py -q && uv run pytest tests/unit/test_mode_preconditions.py -q
bash scripts/ci/itest.sh tests/integration/test_cli_contract.py -q
uv run mypy --strict src/er
uv run ruff check . && uv run ruff format --check .
```

## Definition of Done

- All acceptance criteria have a named passing test
- Verify command passes; T-CFG-1's pinned node id collects and passes
- A refused run leaves `runs`, `run_stages` and the snapshot version untouched
- The fingerprint is the four-tuple and is read from `runs` columns, not recomputed from a second source
- `check_mode_preconditions` is pure and unit-tested without a lake
- mypy --strict and ruff clean
