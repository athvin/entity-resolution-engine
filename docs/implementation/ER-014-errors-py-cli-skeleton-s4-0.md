---
id: ER-014
title: "errors.py + CLI skeleton: S4.0 contract, exit codes 0/1/2/3/10, NoOpStage vs NotImplementedStage, run-all chain, --json, dbt_vars(cfg)"
milestone: M1
status: in_progress
kind: code
size: L
gates: fast
depends_on: ["ER-011", "ER-013"]
spec_refs: ["s4", "s4-0", "s4-7", "s5-2", "s6", "s6-1", "s12"]
gap_refs: ["M19", "M2", "M26"]
provides: ["src/er/errors.py::ExitCode", "src/er/errors.py::ErrorClass", "src/er/errors.py::ErError", "src/er/errors.py::StageFailure", "src/er/errors.py::PreconditionFailure", "src/er/errors.py::ConfigError", "src/er/errors.py::NothingToDo", "src/er/errors.py::ERROR_CLASS_TO_EXIT", "src/er/errors.py::RETRYABLE", "src/er/errors.py::exit_code_for", "src/er/cli.py::app", "src/er/cli.py::main", "src/er/cli.py::GlobalOptions", "src/er/cli.py::Stage", "src/er/cli.py::NoOpStage", "src/er/cli.py::NotImplementedStage", "src/er/cli.py::run_all_chain", "src/er/cli.py::emit_stage_line", "src/er/cli.py::dbt_vars", "src/er/cli.py::COMMANDS"]
consumes: ["src/er/config/loader.py::load_config", "src/er/config/loader.py::ConfigValidationError", "src/er/config/hashing.py::config_hash", "src/er/entities/ids.py::UlidFactory"]
owns: ["src/er/errors.py", "src/er/cli.py", "tests/unit/test_cli_contract.py"]
protected_paths: []
extra_paths: ["pyproject.toml"]
attempts: 1
verify: "uv run pytest tests/unit/test_cli_contract.py -q && uv run mypy --strict src/er/cli.py src/er/errors.py"
branch: "ticket/ER-014-errors-py-cli-skeleton-s4-0"
commit: ""
spec_sha: "28d8d8e366a7b49b"
updated_at: "2026-08-14T23:32:04Z"
session: 829df09b-beae-49e4-ba6c-4057f841b536
---
## Description

Ship the S4.0 CLI contract as a real, testable skeleton: every command in the table with its flags and defaults, the uniform exit codes 0/1/2/3/10, the global `--config`/`--run-id`/`--json` options, and the `run-all` stage chains for both modes. The gap report's M19 is that the CLI is called the orchestration contract and never specified; M2 adds that a stage must distinguish "nothing to do" from "failed", which is exactly what the `NoOpStage` vs `NotImplementedStage` split encodes so the M1 exit gate cannot be satisfied by a stage that merely prints. `errors.py` carries the S4.7 taxonomy and the class-to-exit-code mapping every later stage raises through.

## Scope

### In scope

- `errors.py`: `ExitCode` (0/1/2/3/10), `ErrorClass` (transient_io, lock_conflict, precondition, config, contradiction, non_convergence, data), the class->exit and class->retryable maps, and `exit_code_for(exc)`
- Typer app exposing exactly the S4.0 command set including the `lake maintain` / `lake reset` sub-group and the `assert` / `review` sub-verbs, each with the flags and defaults of the S4.0 table
- Global options on every command: `--config PATH` (default `$ER_CONFIG`), `--run-id ULID` (default minted), `--json`
- Config validated at process start; an invalid document exits 2 before any connection is attempted
- `Stage` protocol, `NoOpStage` (does nothing, exits 10) and `NotImplementedStage` (exits 1 with `stage not implemented: <stage>`)
- `run_all_chain(mode, skip_ingest)` returning the ordered stage list for `incremental` and `full`, and the propagation rule: 10 never aborts, the first exit other than 0/10 propagates
- `emit_stage_line`: exactly one JSON line per stage on stderr; `--json` switches stdout to JSONL and never moves the stderr line
- `dbt_vars(cfg, run_id)` returning the `{std_version, survivorship_version, run_id}` mapping
- Console-script entry point `er = er.cli:main` in `pyproject.toml`

### Out of scope

- Any lake connection, DDL, or `runs`/`run_stages` persistence — ER-023 owns `RunContext` and the counter payload and extends `emit_stage_line` rather than adding a second emitter
- The tenant advisory lock and `--resume` (ER-024), the config-drift guard of `er run-all --mode incremental` (ER-034)
- Invoking dbt as a subprocess — ER-033 owns `dbt_runner.py` and MUST import `dbt_vars` rather than rebuild the mapping
- Implementing any stage body (ingest, standardize, match, reconcile, assemble, train, correct, doctor, init)

## Design decisions applied

Closes M19 (CLI contract), M2 (a run has a `--run-id` threaded by `run-all`; "nothing to do" is exit 10 and is not a failure) and the M26 arm that makes config the single source of dbt vars. Easy to miss: (1) exit 3 is reserved for the five named precondition failures of S4.0 — a not-yet-implemented stage is exit 1, never 3 and never 10, because a 10 would silently satisfy the M1 exit gate; (2) `--json` changes stdout only, and stdout stays reserved for command output while the telemetry line goes to stderr (S5.2); (3) `run-all` mints ONE `run_id` and threads it to every child stage, and it NEVER trains — `er train` is always a separate invocation (S4.0); (4) `--source`/`--path` are required unless `--skip-ingest` is set, and supplying neither exits 2 before any stage runs; (5) modules that predate `errors.py` (`config.loader`, `lake.env`) surface their status by exposing an int `code` attribute, and `exit_code_for` must honour it — that is the stated convention, not an accident; (6) `er correct` chains `match --mode full --new-tf-snapshot` -> `reconcile` -> `assemble` and is the only caller allowed to pass `--new-tf-snapshot` (D4).

## Acceptance criteria

- [ ] AC1: The command tree of `app` equals exactly {init, doctor, ingest, standardize, train, match, reconcile, assemble, run-all, correct, assert, review, lake maintain, lake reset}, and every command accepts `--config`, `--run-id` and `--json`.
- [ ] AC2: `ERROR_CLASS_TO_EXIT` maps transient_io->1, lock_conflict->3, precondition->3, config->2, contradiction->1, non_convergence->1, data->1, and `RETRYABLE` is true for exactly {transient_io, lock_conflict}.
- [ ] AC3: Invoking any command with a config that fails validation exits 2, prints the offending JSON pointer, and reads no `ER_CATALOG_DSN`/`ER_S3_*` variable (asserted with those variables unset).
- [ ] AC4: `er run-all --mode incremental` with neither `--source`/`--path` nor `--skip-ingest` exits 2 and runs zero stages; with `--skip-ingest` the executed chain is exactly [standardize --changed-only, match --mode incremental, reconcile, assemble --touched-only] in that order, and `--mode full` yields the same stages without `--changed-only`/`--touched-only`.
- [ ] AC5: With all chain stages as `NoOpStage`, `er run-all` exits 0 and each stage emitted exit 10; replacing the second stage with one raising `StageFailure` makes `run-all` exit 1 and the third and fourth stages do not execute.
- [ ] AC6: A `NotImplementedStage` exits 1 with `stage not implemented: <stage>` on stderr and never 10 or 0; no command wired into the `run-all` chain is a `NotImplementedStage`.
- [ ] AC7: Every stage emits exactly one JSON line on stderr carrying the same `run_id`; with `--run-id 01J...` supplied, all four chain stages report that literal id; without it, `run-all` mints exactly one id and all four share it.
- [ ] AC8: With `--json`, every stdout line parses as JSON and the stderr line count is unchanged from the non-`--json` run; `dbt_vars(cfg, run_id)` returns exactly the keys {std_version, survivorship_version, run_id} with values taken from `versions:` and the run id.

## Tests

- tests/unit/test_cli_contract.py::test_command_tree_matches_s4_0
- tests/unit/test_cli_contract.py::test_error_class_exit_and_retryable_maps
- tests/unit/test_cli_contract.py::test_invalid_config_exits_2_before_touching_lake_env
- tests/unit/test_cli_contract.py::test_run_all_requires_source_or_skip_ingest
- tests/unit/test_cli_contract.py::test_run_all_chain_order_per_mode
- tests/unit/test_cli_contract.py::test_ten_does_not_abort_chain_and_first_failure_propagates
- tests/unit/test_cli_contract.py::test_not_implemented_stage_is_exit_1
- tests/unit/test_cli_contract.py::test_run_id_is_minted_once_and_threaded
- tests/unit/test_cli_contract.py::test_json_flag_switches_stdout_only
- tests/unit/test_cli_contract.py::test_dbt_vars_key_set

## Verification

```bash
uv run pytest tests/unit/test_cli_contract.py -q && uv run mypy --strict src/er/cli.py src/er/errors.py
uv run python -c "import subprocess,sys; sys.exit(subprocess.run(['er','--help']).returncode)"
uv run ruff check src/er/cli.py src/er/errors.py tests/unit/test_cli_contract.py
```

## Definition of Done

- Every S4.0 command, flag and default present; exit codes 0/1/2/3/10 uniform across commands
- `NoOpStage` (10) and `NotImplementedStage` (1) distinct and tested; no `NotImplementedStage` in the `run-all` chain
- One stderr JSON line per stage; `--json` affects stdout only
- `dbt_vars` is the single builder of the dbt var mapping; console script `er` installed
- Verify command passes; `mypy --strict src/er/cli.py src/er/errors.py` clean
