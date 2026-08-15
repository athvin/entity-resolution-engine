---
id: ER-036
title: "M1 exit gate: er init && er doctor && er run-all on an empty lake, tagged M1-EXIT-<k> parity with S12"
milestone: M1
status: in_progress
kind: code
size: S
gates: full
depends_on: ["ER-010", "ER-022", "ER-023", "ER-026", "ER-031"]
spec_refs: ["s4-0", "s4-0b", "s5", "s5-2", "s8-1", "s8-3", "s12"]
gap_refs: ["MINOR-milestones", "M19", "M2"]
provides: ["tests/integration/test_m1_exit.py::M1_EXIT_CLAUSES", "gate:M1-EXIT"]
consumes: ["src/er/cli.py::app", "src/er/cli.py::doctor", "src/er/lake/ddl.py::apply", "RunContext", "relation:runs", "relation:run_stages", "relation:ingest_batches", "tests/conftest.py::lake_ns", "tests/conftest.py::lake_conn", "tests/conftest.py::er_env", ".github/workflows/ci.yaml", "src/er/ingest/landing.py::ingest_delivery"]
owns: ["tests/integration/test_m1_exit.py"]
protected_paths: ["tests/integration/test_doctor.py", "tests/integration/test_logical_keys.py", "tests/integration/test_concurrency.py", "tests/integration/test_cli_contract.py"]
extra_paths: []
attempts: 1
verify: "bash scripts/ci/itest.sh tests/integration/test_m1_exit.py -q"
branch: "ticket/ER-036-m1-exit-gate-er-init-er"
commit: ""
spec_sha: "2abcfe433c322f74"
updated_at: "2026-08-15T22:39:06Z"
session: 7f236e11-ecaf-4251-b9c7-d5d01a0a4081
---
## Description

Encode S12's M1 exit criteria as one executable gate: on a fresh namespace, `er init && er doctor && er run-all --mode incremental --skip-ingest` must behave exactly as the M1 row of S12 states, clause by clause. Each clause is a separately tagged assertion `M1-EXIT-<k>` and a parity test asserts the tag set matches the clause list and that each clause's quoted text is verbatim in S12 — so a spec edit that changes a criterion fails the gate instead of silently drifting from it. This is the milestone's falsifiable exit, not a summary of the tickets that preceded it.

## Scope

### In scope

- `M1_EXIT_CLAUSES: dict[str, str]` mapping each `M1-EXIT-<k>` id to the literal clause text quoted from the S12 M1 exit-criteria cell.
- One tagged test per clause covering: `er init` exits 0 on an empty namespace; a second `er init` is an idempotent no-op exiting 0; `er doctor` exits 0; `er run-all --mode incremental --skip-ingest` exits 0 with every stage returning 10; exactly one `runs` row with `status='succeeded'`; exactly four `run_stages` rows (`standardize, match, reconcile, assemble`) each carrying a snapshot range; omitting `--source`/`--path` without `--skip-ingest` is rejected with exit 2 before any stage runs; no `ingest_batches` row is written; zero relations matching `__splink__%` exist in `lake`; a second concurrent `er run-all` exits 3.
- `test_m1_exit_tag_parity`: every clause id appears exactly once as a tag in the module, and every clause string is a substring of the S12 M1 row read from `DesignDoc.md`.
- Re-running the three named M1 scenario tests (T-DOCTOR-1, T-KEY-1a, T-CONC-1) as part of the gate's verification.

### Out of scope

- Any new production code: this ticket is the gate, and a failure is fixed in the owning ticket, not by weakening the assertion.
- Running the chain with a real ingest delivery — S12 pins `--skip-ingest` for this criterion, and no `ingest_batches` row is written or asserted.
- Asserting snapshot *counts*: a stage commits a range, and a no-op run may legitimately commit empty snapshots.
- Modifying `tests/integration/test_doctor.py`, `test_logical_keys.py`, `test_concurrency.py` or `test_cli_contract.py`.
- The S12 clause 'the integration job green end to end', which is the CI job's own criterion (ER-010) and is a DoD item here rather than a tagged assertion.

## Design decisions applied

Implements gap entries MINOR-milestones, M19 and M2. The authority for every clause is the S12 M1 row of DesignDoc v1.1: `--skip-ingest`, exit `0`, **exactly one** `runs` row, **exactly four** `run_stages` rows, and **no** `ingest_batches` row. Earlier board narrative describing five `run_stages` rows and a `0/0/0` ingest manifest predates the locked `run-all` chain and MUST NOT be implemented — if the two ever need reconciling, that is a spec amendment, not a test edit. Other constraints: a chain of `10`s is a successful no-op run and never aborts (S4.0), so the run's `status` is `succeeded`; the stub stages must actually write their `run_stages` rows with a snapshot range (printing to stdout does not satisfy the criterion); the second-writer refusal is the S4.0b advisory lock and the refused process writes no `runs` row.

## Acceptance criteria

- [ ] AC1: On a namespace with no schema, `er init` exits `0` and creates exactly the `ddl.py`-owned relation set; a second `er init` exits `0`, reports `exists` for every relation and changes no column set.
- [ ] AC2: `er doctor` exits `0` on that namespace with every check row reporting pass.
- [ ] AC3: `er run-all --mode incremental --skip-ingest` exits `0` and every stage's `run_stages` row records the stage having nothing to do, per the S4.0 rule that a chain of `10`s is a successful run.
- [ ] AC4: That invocation writes exactly one `runs` row with `status='succeeded'` and a non-NULL `config_hash`, and exactly four `run_stages` rows whose `stage` values are `{standardize, match, reconcile, assemble}`, each with non-NULL `snapshot_start` and `snapshot_end`.
- [ ] AC5: `er run-all --mode incremental` with neither `--source`/`--path` nor `--skip-ingest` exits `2` and writes no `runs` row and no `run_stages` row.
- [ ] AC6: Zero `ingest_batches` rows exist after the run, and zero relations matching `__splink__%` exist in `lake`.
- [ ] AC7: A second `er run-all` started against the same tenant while the first holds the lock exits `3` and writes no `runs` row.
- [ ] AC8: `test_m1_exit_tag_parity` fails if a clause id is missing, duplicated, or if its quoted text is not a verbatim substring of the S12 M1 exit-criteria cell in `DesignDoc.md`.

## Tests

- tests/integration/test_m1_exit.py::test_init_creates_ddl_owned_relations
- tests/integration/test_m1_exit.py::test_init_is_idempotent
- tests/integration/test_m1_exit.py::test_doctor_exits_zero
- tests/integration/test_m1_exit.py::test_run_all_skip_ingest_exits_zero_with_four_stage_rows
- tests/integration/test_m1_exit.py::test_missing_source_and_path_exits_2
- tests/integration/test_m1_exit.py::test_no_ingest_batches_row_and_no_splink_relations
- tests/integration/test_m1_exit.py::test_second_concurrent_run_exits_3
- tests/integration/test_m1_exit.py::test_m1_exit_tag_parity

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_m1_exit.py -q
bash scripts/ci/itest.sh tests/integration/test_doctor.py tests/integration/test_logical_keys.py tests/integration/test_concurrency.py -q
python3 scripts/lint_board.py docs/implementation
uv run ruff check . && uv run ruff format --check .
```

## Definition of Done

- All acceptance criteria have a named passing test
- `bash scripts/ci/itest.sh tests/integration/test_m1_exit.py -q` passes
- Every S12 M1 clause has exactly one `M1-EXIT-<k>` tag and the parity test is green
- T-DOCTOR-1, T-KEY-1a and T-CONC-1 pass unmodified
- No snapshot count is asserted anywhere in the gate
- The integration CI job runs the gate as part of `tests/integration` and is green
- No production source file changed by this ticket
