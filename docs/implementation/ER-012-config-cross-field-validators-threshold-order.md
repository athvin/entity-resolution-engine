---
id: ER-012
title: "Config cross-field validators: threshold order, survivorship↔GOLDEN_SURVIVABLE_COLUMNS equality, validated↔_valid, referential blocking/comparison checks"
milestone: M1
status: todo
kind: code
size: M
gates: fast
depends_on: ["ER-006", "ER-011"]
spec_refs: ["s6-1", "s5-0", "s5", "s4-3", "s4-3-1", "s4-6"]
gap_refs: ["M26", "B4", "M11", "M12", "M13"]
provides: ["src/er/config/validators.py::validate_cross_fields", "src/er/config/validators.py::FAILURE_KEYS", "src/er/config/validators.py::SURVIVORSHIP_RULES", "src/er/config/validators.py::COMPARISON_LEVEL_TOKENS", "src/er/config/validators.py::expand_survivorship_keys", "src/er/config/validators.py::CrossFieldError"]
consumes: ["src/er/config/schema.py::Config", "src/er/config/loader.py::load_config", "src/er/config/loader.py::ConfigValidationError", "src/er/lake/columns.py::GOLDEN_SURVIVABLE_COLUMNS", "src/er/lake/columns.py::STD_RECORD_COLUMNS"]
owns: ["src/er/config/validators.py", "tests/unit/test_config_validators.py"]
protected_paths: ["src/er/lake/columns.py"]
extra_paths: ["src/er/config/schema.py", "src/er/config/loader.py"]
attempts: 0
verify: "uv run pytest tests/unit/test_config_validators.py -q && uv run mypy --strict src/er/config"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

Add the S6.1 cross-field and referential validators that no single block can check by itself: the threshold ordering, the set-equality between the `survivorship:` key set and `GOLDEN_SURVIVABLE_COLUMNS`, the `validated`-rule-to-`_valid`-column dependency, and the referential checks that every blocking expression and comparison key names a real `int_std_records` column with a legal level token. S5.0 makes the survivorship/golden equality a two-directional invariant so a `golden_records` column without a rule — or a rule without a column — fails at config-validation time instead of producing a silently NULL golden attribute. Every validator raises the literal failure message key of the S6.1 table so a failure names the rule rather than merely failing.

## Scope

### In scope

- V1 `0 < thresholds.review_low < thresholds.auto_merge <= 1` -> `thresholds.ordering`
- V2 survivorship key set, with `address` expanded to the six `addr_*` columns, set-equal to `GOLDEN_SURVIVABLE_COLUMNS` in both directions -> `survivorship.keyset`
- V3 every rule token in every chain in {source_priority, recency, frequency, completeness, validated} -> `survivorship.unknown_rule`
- V4 a chain containing `validated` requires a matching `<attr>_valid` column on `int_std_records` (only `email`/`phone_e164` qualify) -> `survivorship.validated_missing_column`
- V5 every chain contains at least one rule able to separate two records from the same source -> `survivorship.not_separating`
- V6 every column referenced by `blocking[].expr` and every `comparisons` key exists in the S5 `int_std_records` column list; `variant_match` additionally requires `name_variants` -> `columns.unknown`
- V7 `blocking[].key_type` uniqueness -> `blocking.duplicate_key_type`
- V8 every level token is `exact`, `jaro_winkler:<T>` with `0 < T <= 1`, `null`, `username_exact`, `variant_match` or `dob_same_year_month` -> `comparisons.unknown_level`
- Wiring: `load_config` runs these validators after per-block validation and before S6.1 normalization

### Out of scope

- The single-block validators V9–V16 (ER-011) — do not re-implement or move them
- Re-listing `GOLDEN_SURVIVABLE_COLUMNS` or the `int_std_records` column list locally; both are imported from `src/er/lake/columns.py`
- Generating blocking rules or Splink comparison levels from the validated config (ER-046, ER-048)
- Changing `config_hash`, the normalization step, or the YAML documents

## Design decisions applied

Closes M26/B4 (V2 makes the golden column set and the survivorship key set one fact), M11 (`validated` has an input only where `<attr>_valid` exists; a chain that cannot separate same-source records is rejected), M12 (V6/V7 make the blocking payload referentially sound before it reaches the single generator) and M13 (V8 pins the level-token vocabulary; `phonetic` is deleted from the spec and MUST be rejected). Easy to miss: (1) V2 is checked in BOTH directions after expanding `address` to the six `addr_*` columns — the expansion is the only place `address` is composite, and `email_valid`/`phone_valid` are deliberately NOT members of `GOLDEN_SURVIVABLE_COLUMNS`; (2) the column vocabulary and the survivable column tuple are imported from `src/er/lake/columns.py` (ER-006), never restated — `columns.py` is protected here; (3) V8 must accept the `null` token even though S6.1 normalization later removes it, because validators run before normalization; (4) V5 rejects a chain consisting solely of `source_priority`, since the mandatory terminal `record_key ASC` would then decide every contest (S4.6); (5) failure keys are literal strings from the S6.1 table and each validator's test asserts the key, not merely that validation failed (S8.4); (6) validation MUST open no lake connection — S6 requires exit 2 before any connection exists.

## Acceptance criteria

- [ ] AC1: Each of V1–V8 has a document that fails it, and the raised error's message key equals the literal S6.1 value (`thresholds.ordering`, `survivorship.keyset`, `survivorship.unknown_rule`, `survivorship.validated_missing_column`, `survivorship.not_separating`, `columns.unknown`, `blocking.duplicate_key_type`, `comparisons.unknown_level`).
- [ ] AC2: `review_low = auto_merge = 0.95` is rejected with `thresholds.ordering`; `review_low = 0.0` is rejected; `auto_merge = 1.0` with `review_low = 0.6` is accepted.
- [ ] AC3: Removing `address` from `survivorship:` is rejected with `survivorship.keyset`, and adding a key `email_valid` is also rejected with `survivorship.keyset`; the expansion of the accepted key set equals `set(GOLDEN_SURVIVABLE_COLUMNS)` exactly.
- [ ] AC4: A `validated` rule on `given_name` is rejected with `survivorship.validated_missing_column` while `email` and `phone_e164` chains containing `validated` are accepted.
- [ ] AC5: `survivorship.given_name = [source_priority]` is rejected with `survivorship.not_separating`.
- [ ] AC6: A blocking `expr` referencing `nickname` is rejected with `columns.unknown`; a duplicated `key_type` with `blocking.duplicate_key_type`; the level tokens `phonetic`, `jaro_winkler:1.5` and `jaro_winkler:0` are each rejected with `comparisons.unknown_level`.
- [ ] AC7: `configs/test.yaml` and `configs/default.yaml` pass all sixteen validators, and loading an invalid document in a fresh subprocess leaves `duckdb` absent from `sys.modules`.
- [ ] AC8: `uv run mypy --strict src/er/config` exits 0.

## Tests

- tests/unit/test_config_validators.py::test_v1_threshold_ordering_key
- tests/unit/test_config_validators.py::test_v2_survivorship_keyset_is_bidirectional
- tests/unit/test_config_validators.py::test_v3_unknown_survivorship_rule_key
- tests/unit/test_config_validators.py::test_v4_validated_requires_valid_column
- tests/unit/test_config_validators.py::test_v5_chain_must_separate_same_source_records
- tests/unit/test_config_validators.py::test_v6_unknown_column_in_blocking_and_comparisons
- tests/unit/test_config_validators.py::test_v7_duplicate_key_type
- tests/unit/test_config_validators.py::test_v8_unknown_level_token_rejects_phonetic
- tests/unit/test_config_validators.py::test_shipped_configs_pass_all_validators
- tests/unit/test_config_validators.py::test_validation_opens_no_lake_connection

## Verification

```bash
uv run pytest tests/unit/test_config_validators.py -q && uv run mypy --strict src/er/config
uv run pytest tests/unit/test_config_schema.py -q
uv run ruff check src/er/config tests/unit/test_config_validators.py
```

## Definition of Done

- V1–V8 implemented with the literal S6.1 failure message keys and one test each
- V2 asserted in both directions against imported `GOLDEN_SURVIVABLE_COLUMNS`; `columns.py` unmodified
- Validators wired into `load_config` before S6.1 normalization; ER-011's tests still green
- Verify command passes; `mypy --strict src/er/config` clean
- No lake connection, no duckdb import on the validation path
