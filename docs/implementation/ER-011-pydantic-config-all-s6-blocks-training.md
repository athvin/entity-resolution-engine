---
id: ER-011
title: "Pydantic config: all S6 blocks (training/storage/versions/generator/clustering/coherence/correction_pass/sources.columns) + config_hash"
milestone: M1
status: blocked
kind: code
size: L
gates: fast
depends_on: ["ER-001", "ER-003"]
spec_refs: ["s6", "s6-1", "s5-2", "s4-0", "s4-3-2"]
gap_refs: ["M26", "M8", "M14", "B5"]
provides: ["src/er/config/schema.py::Config", "src/er/config/schema.py::Thresholds", "src/er/config/schema.py::Standardization", "src/er/config/schema.py::SourceSpec", "src/er/config/schema.py::BlockingRule", "src/er/config/schema.py::ComparisonSpec", "src/er/config/schema.py::Training", "src/er/config/schema.py::TrainingEm", "src/er/config/schema.py::Storage", "src/er/config/schema.py::Versions", "src/er/config/schema.py::Generator", "src/er/config/schema.py::Clustering", "src/er/config/schema.py::Coherence", "src/er/config/schema.py::CorrectionPass", "src/er/config/schema.py::CONFIG_BLOCKS", "src/er/config/schema.py::normalize", "src/er/config/loader.py::load_config", "src/er/config/loader.py::ConfigValidationError", "src/er/config/hashing.py::config_hash", "src/er/config/hashing.py::canonical_document", "configs/default.yaml", "configs/test.yaml"]
consumes: ["src/er/__init__.py", "pyproject.toml::pydantic==2.13.4"]
owns: ["src/er/config/schema.py", "src/er/config/loader.py", "src/er/config/hashing.py", "configs/default.yaml", "configs/test.yaml", "tests/unit/test_config_schema.py"]
protected_paths: []
extra_paths: ["src/er/config/__init__.py"]
attempts: 1
verify: "uv run pytest tests/unit/test_config_schema.py -q && uv run mypy --strict src/er/config"
branch: "ticket/ER-011-pydantic-config-all-s6-blocks-training"
commit: ""
spec_sha: "2e60a757351fe5ce"
updated_at: "2026-08-14T23:01:29Z"
session: 2816104e-7a5e-4c7d-8ce1-d94e54c2aa99
---
## Description

Build the Pydantic model tree for the whole S6 configuration document — all fourteen blocks, not the seven of v1.0 — plus the loader and the normative `config_hash`. S6 is the single source of truth for every tunable the pipeline reads, and S4.0 requires the document to be validated at process start with exit 2 before any lake connection is opened. `config_hash` is defined normatively in S5.2 and is written to `runs.config_hash`, `model_registry.config_hash` and every stage log line, so it must be computed over the canonicalised, *normalized* validated document rather than over the YAML text. This ticket also authors `configs/default.yaml` and `configs/test.yaml`, the file S8 fixtures and CI consume verbatim.

## Scope

### In scope

- Pydantic v2 models for `tenant`, `thresholds`, `standardization`, `sources`, `blocking`, `comparisons`, `survivorship`, `training`, `storage`, `versions`, `generator`, `clustering`, `coherence`, `correction_pass`
- `sources.<name>`: `adapter`, `priority_rank`, `record_id_column`, `updated_at_column`, `date_format`, `columns` (canonical attribute -> source column)
- Single-block validators V9, V10, V11, V12, V13, V14, V15, V16 of S6.1, each raising the S6.1 failure message key
- `load_config(path | $ER_CONFIG)`: read YAML, validate, normalize, return `Config`; failure raises `ConfigValidationError` carrying the offending JSON pointer and `code = 2`
- S6.1 post-validation normalization: drop the `null` token from every `comparisons[*].levels` list, append `record_key ASC` as the terminal element of every survivorship chain
- `config_hash`: SHA-256 hex over the normalized validated document dumped with keys sorted, no aliases, UTF-8, compact separators
- `configs/default.yaml` (reference) and `configs/test.yaml` (`tenant: test`, equal to the S6 listing)

### Out of scope

- Cross-field and referential validators V1–V8 (threshold ordering, survivorship keyset vs `GOLDEN_SURVIVABLE_COLUMNS`, `validated`↔`_valid`, unknown-column/level/duplicate-key checks) — ER-012 owns them
- Rendering dbt `--vars` from the config (ER-033) and CLI flag wiring / exit-code mapping (ER-014)
- Reading any `ER_*` environment variable other than `$ER_CONFIG`; secrets and paths are not in the YAML (S6 environment-variables paragraph)
- Blocking-rule generation, Splink settings construction, or any consumption of the `training:` block

## Design decisions applied

Closes M26 (config surface), M8 (`training:` block Pydantic-rejects if incomplete), M14 (`sources.<name>.columns` + `record_id_column`/`updated_at_column`/`date_format`) and the B5 arm that puts `clustering.max_iterations` / `cut_protect_probability` in config. Easy to miss: (1) `training.u_seed` is REQUIRED with no default (V10) — a defaulted seed makes T-TRAIN-1's byte-equality claim unreproducible; (2) `config_hash` is NOT a config field (S6) and is computed only after S6.1 normalization, so two documents differing solely by a redundant `null` token hash identically; (3) normalization MUST run last, after every validator, because ER-012's V6/V8 inspect the pre-normalized `levels` lists; implement it as an explicit `normalize()` step invoked by the loader, not as a field validator; (4) `storage.model_uri_prefix` already ends in `/` and already names the tenant (V14) — never interpolate the tenant a second time (S4.3.2); (5) the validator split with ER-012 is normative for this ticket: implement V9–V16 here and none of V1–V8; (6) S6.1 says each validator has a test in `tests/unit/test_config.py` — the board splits that file in two (`test_config_schema.py` here, `test_config_validators.py` in ER-012) and the union satisfies the requirement; (7) exceptions raised by this package expose `code = 2` so the CLI (ER-014) maps them to the S4.0 config exit status without a type import.

## Acceptance criteria

- [ ] AC1: `set(Config.model_fields)` equals exactly {tenant, thresholds, standardization, sources, blocking, comparisons, survivorship, training, storage, versions, generator, clustering, coherence, correction_pass}, and an unknown top-level key in the YAML raises `ConfigValidationError` (models are `extra='forbid'`).
- [ ] AC2: `load_config('configs/test.yaml')` succeeds and the parsed YAML of `configs/test.yaml` equals the parsed YAML of the fenced block under the S6 anchor in `DesignDoc.md`, key for key and value for value.
- [ ] AC3: A document omitting `training.u_seed` raises `ConfigValidationError` whose error path is `('training','u_seed')` and whose message key is `training.u_seed.required`; the same document with `u_seed: 20260101` loads.
- [ ] AC4: Each of V9, V11, V12, V13, V14, V15, V16 has a failing document whose raised error carries the literal S6.1 failure message key (`training.em_blocking_rules.min_items`, `sources.columns.incomplete`, `clustering.bounds`, `versions.required`, `storage.uri`, `correction_pass.cadence`, `standardization.invalid`).
- [ ] AC5: After `load_config`, no `comparisons[*].levels` list contains the token `null`, and every `survivorship[*]` chain ends with the literal element `record_key ASC`.
- [ ] AC6: `config_hash` is unchanged when `configs/test.yaml` is re-serialised with mapping keys reordered, comments added and block scalars restyled, and is unchanged when a redundant `null` token is added to a `levels` list; changing `thresholds.auto_merge` from 0.95 to 0.94 changes it.
- [ ] AC7: `uv run mypy --strict src/er/config` exits 0 and no public attribute of any model is annotated `Any`.

## Tests

- tests/unit/test_config_schema.py::test_fourteen_blocks_exact_and_extra_forbidden
- tests/unit/test_config_schema.py::test_test_yaml_matches_spec_s6_block
- tests/unit/test_config_schema.py::test_u_seed_is_required_with_error_path
- tests/unit/test_config_schema.py::test_single_block_validator_message_keys
- tests/unit/test_config_schema.py::test_normalization_drops_null_token_and_appends_record_key
- tests/unit/test_config_schema.py::test_config_hash_is_stable_under_reserialisation
- tests/unit/test_config_schema.py::test_config_hash_changes_on_value_change
- tests/unit/test_config_schema.py::test_loader_raises_with_code_2

## Verification

```bash
uv run pytest tests/unit/test_config_schema.py -q && uv run mypy --strict src/er/config
uv run ruff check src/er/config tests/unit/test_config_schema.py
uv run ruff format --check src/er/config tests/unit/test_config_schema.py
```

## Definition of Done

- All fourteen S6 blocks modelled; `set(Config.model_fields)` test green
- V9–V16 implemented with the exact S6.1 failure message keys; V1–V8 deliberately absent
- `configs/default.yaml` and `configs/test.yaml` committed and loading
- `config_hash` computed post-normalization over the canonicalised document (sorted keys, no aliases, UTF-8, compact separators)
- Verify command passes; `mypy --strict src/er/config` clean; ruff check and format clean
- No `ER_*` variable other than `$ER_CONFIG` is read by this package

## Blocker log

### Attempt 1 — spec_contradiction (2026-08-14T23:01:29Z)

- **Failing command:** `uv run mypy --strict src/er/config`
- **Assertion / contradiction:** S6 mandates a YAML config document and S4.0 validates it at process start, but S2.1's pin table is closed by its own final rule ('Adding a dependency means adding a row here') and names no YAML parser. PyYAML 6.0.3 is in uv.lock only transitively (via dbt-core/dbt-common), is not declared in pyproject.toml, and ships no py.typed; mypy 2.3.0 bundles no third-party stubs. So 'import yaml' yields: src/er/config/loader.py:14: error: Library stubs not installed for "yaml" [import-untyped] -- Found 1 error in 1 file. That is the only error. ER-011 AC7 requires 'uv run mypy --strict src/er/config' to exit 0, so AC7, S6 and S2.1's closing rule cannot all hold. types-PyYAML is absent from uv.lock and the permitted command set has no 'uv add'/'uv lock', so it cannot be installed from inside an iteration; and the remaining routes are all forbidden (a mypy per-module ignore_missing_imports override loosens the type gate; a hand-written src/yaml.pyi works but types safe_load as Any, which is the same suppression wearing a stub's clothes).
- **Smallest change that would unblock:** Add one row to the S2.1 table via a kind: spec-amendment ticket, then let ER-003's pyproject.toml/uv.lock gain the pins. Proposed row, to sit directly after the 'typer / pydantic / python-ulid' row: '| PyYAML | `PyYAML==6.0.3`, `types-PyYAML==6.0.12.20250915` | The S6 configuration document is YAML; it arrives transitively through dbt-core, but S6 and S4.0 depend on it directly, so it is rowed for the same reason dbt-adapters and dbt-common are. The stub distribution is what makes `mypy --strict src/er/config` (S9.1) checkable at all, since PyYAML ships no py.typed | `er doctor`: `yaml.__version__`; `uv.lock` |'. Then add `pyyaml==6.0.3` to pyproject.toml [project].dependencies and `types-PyYAML==6.0.12.20250915` to [dependency-groups].dev, re-lock, and unblock ER-011. Confirm the types-PyYAML version resolves; any current release works, only the S2.1 row and uv.lock must agree. The plan at .loop/change-plan.json already passes plan-check and the loader skeleton is on branch ticket/ER-011-pydantic-config-all-s6-blocks-training at ad9287b.
- **Log:** `.loop/logs/ER-011.attempt-1.log`
