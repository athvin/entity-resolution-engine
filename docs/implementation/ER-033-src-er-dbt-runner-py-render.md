---
id: ER-033
title: "src/er/dbt_runner.py: render_dbt_vars(cfg) + run_dbt(); config is the single source of truth, dbt_project.yml holds only fallbacks"
milestone: M1
status: in_progress
kind: code
size: S
gates: fast
depends_on: ["ER-008", "ER-011", "ER-014"]
spec_refs: ["s4-0b", "s4-2", "s4-6", "s6", "s9-1"]
gap_refs: ["M26", "M16"]
provides: ["src/er/dbt_runner.py::render_dbt_vars", "src/er/dbt_runner.py::run_dbt", "src/er/dbt_runner.py::DbtResult", "src/er/dbt_runner.py::DBT_PROJECT_DIR", "src/er/dbt_runner.py::DBT_PROFILES_DIR"]
consumes: ["src/er/config/schema.py::Config", "src/er/config/loader.py::load_config", "src/er/cli.py::dbt_vars", "src/er/cli.py::app", "src/er/errors.py", "dbt/dbt_project.yml", "dbt/profiles/profiles.yml::lake", "dbt/profiles/profiles.yml::mem"]
owns: ["src/er/dbt_runner.py", "tests/unit/test_dbt_runner.py"]
protected_paths: []
extra_paths: ["src/er/cli.py", "dbt/dbt_project.yml"]
attempts: 1
verify: "uv run pytest tests/unit/test_dbt_runner.py -q"
branch: "ticket/ER-033-src-er-dbt-runner-py-render"
commit: ""
spec_sha: "28d8d8e366a7b49b"
updated_at: "2026-08-15T01:55:44Z"
session: 9eb28383-adeb-4b4c-b590-a57f035007f4
---
## Description

Ship the single subprocess wrapper every dbt-backed stage invokes (S3, S4.0b). `render_dbt_vars(cfg)` builds the `--vars` payload from the Pydantic-validated config so that the config document — not `dbt_project.yml` — is the single source of truth for `std_version`, `survivorship_version`, the `sources` column mapping and the `standardization` block (S6, S4.2); `dbt_project.yml` keeps the same keys only as fallbacks for a bare `dbt parse`. `run_dbt()` owns argv construction, target selection, log capture, exit-code mapping and the S4.0b rule that no Python DuckDB connection spans a dbt invocation.

## Scope

### In scope

- `render_dbt_vars(cfg, run_id, extra=None) -> dict` producing `std_version`, `survivorship_version`, `run_id`, `sources` and `standardization`, merged with an explicit `extra` mapping for payloads later stages add (e.g. the S4.2 blocking payload).
- Rejection of an `extra` key that would shadow a config-sourced var.
- `run_dbt(command, select=None, vars=None, target='lake', full_refresh=False, close_conn=None, reopen_conn=None) -> DbtResult` with `DbtResult(exit_code, models, snapshot_start, snapshot_end, log_path)`.
- Argv construction: `dbt <command> --project-dir dbt --profiles-dir dbt/profiles --target <t> [--select …] --vars '<json>' [--full-refresh]`.
- Ordering guarantee: the Python connection is closed before the subprocess is spawned and reopened after it exits.
- Mapping a non-zero dbt exit onto the `src/er/errors.py` error whose exit code is `1`, with captured stdout+stderr written under `artifacts/`.
- Parity test between the `vars:` keys declared in `dbt/dbt_project.yml` and the keys `render_dbt_vars` produces.

### Out of scope

- Any dbt model, macro or seed — no model ships before M2.
- Running dbt against the `lake` target in this ticket's tests: the verify is unit-layer and must not need Docker, the catalog or the object store.
- Snapshot-range *capture* against a real lake (the probe callables are injected; the integration wiring lands with the first dbt-backed stage in M2).
- Selection strings for `er standardize --changed-only` / `er assemble --touched-only`; this ticket provides the mechanism, the stages choose the selectors.

## Design decisions applied

Implements gap entries M26 and M16. Constraints: (1) ER-014's CLI skeleton already carries a `dbt_vars(cfg)` helper — move the single implementation here and make the CLI import it; exactly one function may produce the `--vars` payload. (2) S4.6 says the marts are invoked with **only** `--vars '{run_id: <ulid>}'`; that rule is about the *touched set* — the entity-id list must never be passed as a var (E2BIG at the 1m scale) — while S6 requires the version vars on every invocation. Encode the ban directly: the rendered payload must contain no list of entity ids and must stay far below the 128 KB argv limit. (3) `--full-refresh` is added only when explicitly requested for a planned S5.1 version-bump rebuild; the absence of `--changed-only` widens the *selection* and never implies a full refresh (S4.2). (4) `threads` is pinned to 1 in the profile, not on the command line. (5) No Python DuckDB connection may span the subprocess (S4.0b) — this is testable as a call-ordering assertion, and it is the reason `run_dbt` takes close/reopen callables rather than a connection.

## Acceptance criteria

- [ ] AC1: `render_dbt_vars(cfg, run_id='01J…')` for `configs/test.yaml` returns a mapping whose key set is exactly `{std_version, survivorship_version, run_id, sources, standardization}`, with `std_version`/`survivorship_version` equal to `cfg.versions.*` and `standardization` equal to the three S6 keys `email_strip_plus_addressing`, `email_placeholders`, `phone_default_region`.
- [ ] AC2: Every key declared under `vars:` in `dbt/dbt_project.yml` appears in the output of `render_dbt_vars`, so no dbt var is ever left to its fallback at runtime; adding a var to `dbt_project.yml` without a config source fails `tests/unit/test_dbt_runner.py::test_project_vars_are_all_overridden`.
- [ ] AC3: `render_dbt_vars(cfg, run_id, extra={'std_version': 'x'})` raises rather than shadowing a config-sourced var, while `extra={'blocking': […]}` merges.
- [ ] AC4: `json.dumps(render_dbt_vars(cfg, run_id))` is under 8192 bytes for `configs/test.yaml`, and no value in the payload is a list of 26-character ULIDs.
- [ ] AC5: Against a subprocess spy, `run_dbt('run', select='staging+ intermediate', target='lake')` invokes exactly `dbt run --project-dir dbt --profiles-dir dbt/profiles --target lake --select 'staging+ intermediate' --vars '<json>'` with no `--full-refresh`, and `run_dbt(..., full_refresh=True)` appends exactly one `--full-refresh`.
- [ ] AC6: The spy records `close_conn` called before the subprocess starts and `reopen_conn` called after it returns, in that order, on both the success and the non-zero-exit path.
- [ ] AC7: A simulated non-zero dbt exit writes the captured stdout+stderr under `artifacts/` and raises the `src/er/errors.py` error whose exit code is `1`; a zero exit returns a `DbtResult` with `exit_code == 0` and the parsed per-model results.

## Tests

- tests/unit/test_dbt_runner.py::test_render_dbt_vars_key_set_and_values
- tests/unit/test_dbt_runner.py::test_project_vars_are_all_overridden
- tests/unit/test_dbt_runner.py::test_extra_cannot_shadow_config_vars
- tests/unit/test_dbt_runner.py::test_vars_payload_never_carries_an_entity_id_list
- tests/unit/test_dbt_runner.py::test_argv_construction_and_full_refresh_is_explicit
- tests/unit/test_dbt_runner.py::test_connection_is_closed_before_and_reopened_after_dbt
- tests/unit/test_dbt_runner.py::test_non_zero_dbt_exit_maps_to_exit_code_1_and_captures_the_log

## Verification

```bash
uv run pytest tests/unit/test_dbt_runner.py -q
uv run dbt parse --project-dir dbt --profiles-dir dbt/profiles --target mem
uv run mypy --strict src/er/dbt_runner.py
uv run ruff check . && uv run ruff format --check .
```

## Definition of Done

- All acceptance criteria have a named passing test
- `uv run pytest tests/unit/test_dbt_runner.py -q` passes with no Docker, catalog or object store running
- Exactly one function produces the dbt `--vars` payload; `src/er/cli.py` imports it rather than duplicating it
- `dbt/dbt_project.yml` declares the same var keys as fallbacks only, and the parity test proves the override always wins
- `dbt parse --target mem` still succeeds
- mypy --strict and ruff clean
