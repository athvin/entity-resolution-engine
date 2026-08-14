---
id: ER-022
title: "er doctor with the exact M25 assertion list + T-DOCTOR-1 + wire as first integration step"
milestone: M1
status: todo
kind: code
size: M
gates: full
depends_on: ["ER-004", "ER-010", "ER-016", "ER-020"]
spec_refs: ["s2-1", "s4-0", "s4-0b", "s7-1", "s8-1", "s8-3", "s9-1"]
gap_refs: ["M25", "M17", "B1"]
provides: ["src/er/doctor.py::DoctorCheck", "src/er/doctor.py::CHECKS", "src/er/doctor.py::run_checks", "src/er/doctor.py::RUNTIME_CHECK_NAMES", "cli:er doctor", "tests/integration/test_doctor.py::test_doctor_passes"]
consumes: ["src/er/versions.py::PINS", "src/er/cli.py::app", "src/er/errors.py::ExitCode", "src/er/lake/ducklake.py::connect", "src/er/lake/catalog.py::catalog_connection", "src/er/lake/objectstore.py::S3Client", "tests/conftest.py::lake_ns", "tests/conftest.py::er_env", ".github/workflows/ci.yaml", "scripts/ci/itest.sh"]
owns: ["src/er/doctor.py", "tests/unit/test_doctor_checks.py", "tests/integration/test_doctor.py"]
protected_paths: []
extra_paths: ["src/er/cli.py", ".github/workflows/ci.yaml", "tests/unit/test_ci_workflow.py", "scripts/ci/itest.sh"]
attempts: 0
verify: "bash scripts/ci/itest.sh tests/integration/test_doctor.py -q && uv run pytest tests/unit/test_ci_workflow.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

S2.1 pins are only real if something asserts them at runtime; `er doctor` is that something and S13 names it as the mitigation for version skew. This ticket implements `er doctor` as a table of named checks covering (a) every S2.1 row whose *Asserted by* cell names `er doctor` and (b) the six T-DOCTOR-1 runtime assertions, printing one line per check (`name, expected, actual, verdict`) on stdout and exiting `1` if any check fails. It then wires `er doctor` as the first step of the integration job (S9.1) so a broken substrate fails before the suite rather than inside it. Closes M25, the `__splink__` arm of M17, and the doctor half of B1.

## Scope

### In scope

- `CHECKS`: an ordered table of `DoctorCheck(name, expected, probe)` built by iterating `PINS` (S2.1) plus the six named runtime checks.
- Version checks: interpreter `sys.version_info[:2] == (3, 12)`, `splink.__version__`, `duckdb.__version__`, `dbt.version.__version__`, the dbt-duckdb adapter version, the three extension commit hashes from `duckdb_extensions()`, `uv --version`, `ruff`, `mypy`, `pytest`, `actionlint --version`, `typer`/`pydantic`/`python-ulid`.
- The six runtime assertions: `SELECT * FROM lake.snapshots()` succeeds; `duckdb_extensions()` reports all three extensions loaded; the dbt target database equals `$ER_LAKE_ALIAS`; a write/read round-trip to `DATA_PATH`; a catalog round-trip returning `server_version`; zero relations matching `__splink__%` in `lake`.
- Exit `0` when every check passes, `1` when any fails; every check runs and prints even after an earlier failure.
- `er doctor --json` switches stdout to JSONL, one object per check.
- Wiring: `.github/workflows/ci.yaml`'s integration job runs `er doctor` as the step immediately after the substrate reset and before the pytest step, and `tests/unit/test_ci_workflow.py` is extended to assert that ordering by step index.

### Out of scope

- Asserting the Compose image **digests** literally: S2.1 assigns the catalog and object-store rows to behavioural probes (`server_version`, bucket round-trip); digest/`uv.lock` parity is `scripts/lint_spec.py`'s duty (S9.1) and is not re-implemented here.
- Schema drift detection between `ddl.py` and the live catalog (ER-035).
- Any lake mutation: `er doctor` writes no table and creates no relation other than the transient DATA_PATH round-trip object it deletes.
- Adding or changing a pin — a pin change is an S2.1 edit and therefore a spec ticket.

## Design decisions applied

M25 + T-DOCTOR-1. Four constraints an implementer will otherwise get wrong: (1) exit code is `1`, not `3` — a pin mismatch is a check failure under the S4.0 table and `3` is reserved for the five named precondition failures (S2.1 states this explicitly). (2) The `postgres` extension MUST be asserted under its **registered** name `postgres_scanner`; a check filtered on `postgres` silently finds no row and passes. (3) `er doctor` writes **no** `runs`/`run_stages` row and takes **no** advisory lock: `run_stages.stage` has no `doctor` value in the S5 enum and doctor is not a mutating command. (4) No Python DuckDB connection may span a dbt subprocess (S4.0b), so the connection is closed before `dbt debug`/the adapter probe and reopened after. A component with no S2.1 row is not asserted — the check set is derived from `PINS`, never hand-listed.

## Acceptance criteria

- [ ] AC1: On a healthy stack `er doctor` exits 0 and prints exactly `len(CHECKS)` check lines, each carrying name, expected, actual and verdict; a unit test asserts the check-name set equals the set derived from `PINS` (rows asserted by `er doctor`) plus `RUNTIME_CHECK_NAMES`, in both directions.
- [ ] AC2: Forcing one pin to a wrong value (e.g. patching `PINS['splink']` to `4.0.15`) makes `er doctor` exit 1 with that line's verdict `fail`, while every other line still prints with verdict `pass`.
- [ ] AC3: The postgres-extension check queries `duckdb_extensions()` for the row named `postgres_scanner`; a unit test asserts the check's expected name is `postgres_scanner` and that filtering on `postgres` returns zero rows against the pinned engine.
- [ ] AC4: Each of the six runtime assertions is independently falsifiable: pointing `ER_LAKE_DATA_PATH` at a non-existent bucket makes only the DATA_PATH round-trip check fail and the process exit 1.
- [ ] AC5: Creating a relation named `__splink__probe` in `lake.main` makes the `no __splink__ relations` check fail (exit 1); dropping it restores exit 0.
- [ ] AC6: After `er doctor` completes, `runs` and `run_stages` hold zero rows for that invocation and no advisory lock is held (a concurrent `er doctor` also exits 0).
- [ ] AC7: `tests/unit/test_ci_workflow.py` asserts the integration job's step order: checkout/build/reset, then `er doctor`, then the pytest step — reordering the workflow makes it fail.

## Tests

- tests/unit/test_doctor_checks.py::test_check_set_equals_s2_1_rows_plus_runtime_names
- tests/unit/test_doctor_checks.py::test_postgres_extension_asserted_as_postgres_scanner
- tests/unit/test_doctor_checks.py::test_failed_check_exits_1_and_still_prints_all_lines
- tests/integration/test_doctor.py::test_doctor_passes
- tests/integration/test_doctor.py::test_bad_data_path_fails_only_the_roundtrip_check
- tests/integration/test_doctor.py::test_splink_relation_in_lake_fails_doctor
- tests/integration/test_doctor.py::test_doctor_writes_no_run_rows
- tests/unit/test_ci_workflow.py::test_doctor_is_first_integration_step

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_doctor.py -q && uv run pytest tests/unit/test_ci_workflow.py -q
uv run pytest tests/unit/test_doctor_checks.py -q
uv run mypy --strict src/er/doctor.py
```

## Definition of Done

- All acceptance criteria demonstrated by the listed node ids
- Check table derived from `PINS`; no hand-maintained duplicate of S2.1 inside `doctor.py`
- Six T-DOCTOR-1 runtime checks present and individually falsifiable
- `er doctor` wired as the first integration-job step in `ci.yaml`, asserted by `tests/unit/test_ci_workflow.py`
- Exit code is 1 on any failure; zero lake mutations
- ruff + `mypy --strict src/er` clean; verify command passes
