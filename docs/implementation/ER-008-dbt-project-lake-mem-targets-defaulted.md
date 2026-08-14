---
id: ER-008
title: "dbt project: lake/mem targets, defaulted env_var, ER_LAKE_METADATA_SCHEMA, threads:1, pinned dbt_utils, +contract:{enforced:true} + on_schema_change defaults"
milestone: M1
status: todo
kind: code
size: M
gates: fast
depends_on: ["ER-003"]
spec_refs: ["s3", "s4-0b", "s4-2", "s5", "s5-0", "s5-1", "s6", "s7-3", "s8-1", "s9-1", "s12"]
gap_refs: ["M23", "M4", "M16"]
provides: ["dbt/dbt_project.yml", "dbt/packages.yml", "dbt/package-lock.yml", "dbt/profiles/profiles.yml::er.outputs.lake", "dbt/profiles/profiles.yml::er.outputs.mem", "dbt-target:mem", "dbt-target:lake", "tests/unit/test_dbt_profiles.py"]
consumes: ["pyproject.toml", "uv.lock", "DesignDoc.md::s4-0b", "dir:tests/unit/"]
owns: ["dbt/dbt_project.yml", "dbt/packages.yml", "dbt/package-lock.yml", "dbt/profiles/profiles.yml", "dbt/models/.gitkeep", "dbt/macros/.gitkeep", "dbt/seeds/.gitkeep", "dbt/tests/.gitkeep", "tests/unit/test_dbt_profiles.py"]
protected_paths: ["DesignDoc.md"]
extra_paths: []
attempts: 0
verify: "uv run dbt deps --project-dir dbt && uv run dbt parse --project-dir dbt --profiles-dir dbt/profiles --target mem && uv run dbt compile --project-dir dbt --profiles-dir dbt/profiles --target mem && uv run pytest tests/unit/test_dbt_profiles.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

Stand up the dbt project M1's own exit gate needs: `dbt_project.yml`, `packages.yml` pinning `dbt_utils` (the source of `unique_combination_of_columns`), and `profiles/profiles.yml` carrying the two targets S9.1 requires — `lake`, whose `settings:`/`secrets:`/`attach:` keys must equal the S4.0b SQL block field for field, and `mem` (`:memory:`, no attach) so `dbt parse` and `dbt compile` run on a bare service-less runner. Every `env_var()` call carries a default, which is the direct fix for M23's static job that could not parse. Project-level `+contract: {enforced: true}` and `+on_schema_change: sync_all_columns` are set here so that no future model can silently omit them (M4, M16).

## Scope

### In scope

- `dbt/dbt_project.yml`: project/profile name, model/seed/macro/test paths, `vars: {std_version, survivorship_version}` as FALLBACKS only, `models: +contract: {enforced: true}` and `+on_schema_change: sync_all_columns`
- `dbt/packages.yml` with an exact `dbt_utils` version plus the committed `dbt/package-lock.yml`
- `dbt/profiles/profiles.yml`: `er` profile with `lake` and `mem` outputs, `threads: 1` on both, every `env_var()` defaulted
- Directory scaffolding (`models/`, `macros/`, `seeds/`, `tests/`) with `.gitkeep` so `dbt parse` resolves its paths
- `tests/unit/test_dbt_profiles.py`: YAML-level assertions of the S4.0b parity, the defaults, and `threads: 1`

### Out of scope

- Any dbt model, macro, seed or singular test — M1 ships no dbt model (S12); staging arrives in M2
- `dbt/models/sources.yml` declaring the `ddl.py`-owned relations with `tag:keys` tests (ER-021)
- `src/er/dbt_runner.py` and the `--vars` rendering the CLI passes (ER-033)
- Running `dbt build` or anything against the `lake` target — that needs Compose and a lake (ER-016/ER-019)

## Design decisions applied

Implements M23 (two targets, defaulted `env_var`, static job cannot connect), M4 (dbt-owned relations carry an enforced contract) and M16 (`on_schema_change` default of `ignore` silently swallows columns). Three constraints. (1) The `lake` target's five `settings:` keys, the `er_s3` secret's six option values, and `attach:`'s `path`/`alias`/`data_path`/`metadata_schema` MUST equal the S4.0b statement block field for field — that is the one place the Python and dbt renderings can drift. (2) `threads: 1` is dbt's own concurrency and is a different key from `settings.threads`, which is DuckDB's and comes from `ER_DUCKDB_THREADS`; both must be present and must not be conflated. (3) Defaults are deliberately empty or inert (`''`, `path`, `false`, `us-east-1`) because a *wrong* default would attach a different lake — `er init`'s DATA_PATH immutability check exists to catch exactly that.

## Acceptance criteria

- [ ] AC1: `uv run dbt deps --project-dir dbt` resolves `dbt_utils` at the exact version pinned in `packages.yml` (no range) and writes `dbt/package-lock.yml`, which is committed.
- [ ] AC2: `env -u ER_CATALOG_DSN -u ER_S3_ENDPOINT -u ER_S3_ACCESS_KEY_ID -u ER_S3_SECRET_ACCESS_KEY -u ER_LAKE_DATA_PATH -u ER_LAKE_METADATA_SCHEMA uv run dbt parse --project-dir dbt --profiles-dir dbt/profiles --target mem` exits 0, proving every `env_var()` has a default.
- [ ] AC3: `uv run dbt compile --project-dir dbt --profiles-dir dbt/profiles --target mem` exits 0 and the `mem` output has `path: ':memory:'` with no `attach:` and no `extensions:` key.
- [ ] AC4: `tests/unit/test_dbt_profiles.py` loads `profiles.yml` as YAML and asserts the `lake` output matches S4.0b: `settings` has exactly `extension_directory=/opt/duckdb_extensions`, `autoinstall_known_extensions=false`, `autoload_known_extensions=false`, `threads` from `ER_DUCKDB_THREADS`, `memory_limit` from `ER_DUCKDB_MEMORY_LIMIT`; `secrets[0]` is named `er_s3` of type `s3` with the six option keys; `attach[0]` is `ducklake:postgres:{{ env_var('ER_CATALOG_DSN', '') }}` with alias `lake` and options `data_path` and `metadata_schema`.
- [ ] AC5: Both outputs declare `threads: 1`, and the test asserts it is distinct from `settings.threads` (which is env-derived).
- [ ] AC6: `dbt_project.yml` sets `+contract: {enforced: true}` and `+on_schema_change: 'sync_all_columns'` at the `models:` root, and declares `vars` for `std_version` and `survivorship_version` — the test asserts a comment or config note marking them fallbacks, since the CLI's `--vars` always wins (S6).
- [ ] AC7: `uv run dbt ls --project-dir dbt --profiles-dir dbt/profiles --target mem --resource-type model` lists zero models: M1 ships none.
- [ ] AC8: Every `env_var(` occurrence in `profiles.yml` has a second argument — asserted by a regex test, so a future edit that drops a default fails on the unit layer rather than in the static CI job.
- [ ] AC9: `git ls-files dbt` returns at least one entry, so `docker/Dockerfile` (ER-007) can COPY `dbt/` in a fresh clone. Moved here from ER-003 AC5, which could not satisfy it: this ticket owns every path under `dbt/`.

## Tests

- tests/unit/test_dbt_profiles.py::test_lake_target_matches_s4_0b_field_for_field
- tests/unit/test_dbt_profiles.py::test_mem_target_has_no_attach_or_extensions
- tests/unit/test_dbt_profiles.py::test_every_env_var_call_has_a_default
- tests/unit/test_dbt_profiles.py::test_threads_is_one_on_both_targets
- tests/unit/test_dbt_profiles.py::test_project_sets_contract_and_on_schema_change
- tests/unit/test_dbt_profiles.py::test_no_models_ship_in_m1

## Verification

```bash
uv run dbt deps --project-dir dbt && uv run dbt parse --project-dir dbt --profiles-dir dbt/profiles --target mem && uv run dbt compile --project-dir dbt --profiles-dir dbt/profiles --target mem && uv run pytest tests/unit/test_dbt_profiles.py -q
env -u ER_CATALOG_DSN -u ER_S3_ENDPOINT -u ER_LAKE_DATA_PATH uv run dbt parse --project-dir dbt --profiles-dir dbt/profiles --target mem
bash scripts/gates.sh --list | grep '^dbt'
```

## Definition of Done

- Acceptance criteria met
- Verify command passes
- `dbt parse` proven to work with every ER_* variable unset
- `lake` target asserted equal to the S4.0b block field for field
- `dbt_utils` pinned exactly and `package-lock.yml` committed
- No dbt model shipped
- DesignDoc.md unmodified
- Committed on main
