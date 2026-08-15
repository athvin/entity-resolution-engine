---
id: ER-048
title: "Splink settings builder: level-token table, NullLevel-first/ElseLevel-last, variant_match, dob_same_year_month + birth_date_precision eligibility, tf mapping"
milestone: M2
status: done
kind: code
size: L
gates: fast
depends_on: ["ER-039", "ER-046"]
spec_refs: ["s4-3-1", "s4-3", "s4-2", "s6", "s6-1", "s5", "s5-0"]
gap_refs: ["M13", "MINOR-birth_date_precision"]
provides: ["src/er/matching/model.py::build_settings", "src/er/matching/model.py::LEVEL_TOKENS", "src/er/matching/model.py::settings_json", "tests/unit/matching/test_settings_builder.py"]
consumes: ["src/er/matching/model.py::blocking_rules_from_config", "src/er/config/schema.py::Config", "src/er/errors.py::ErConfigError", "dbt/seeds/nickname_variants.csv", "configs/test.yaml"]
owns: ["tests/unit/matching/test_settings_builder.py"]
protected_paths: ["src/er/config/schema.py", "dbt/macros/std/name_norm.sql"]
extra_paths: ["src/er/matching/model.py"]
attempts: 1
verify: "uv run pytest tests/unit/matching/test_settings_builder.py -q"
branch: "ticket/ER-048-splink-settings-builder-level-token-table"
commit: "0973a8c2a6e49a1ac0efd70730a482c66db4528e"
spec_sha: "2abcfe433c322f74"
updated_at: "2026-08-15T07:02:16Z"
session: c42092ac-b8ac-4ffa-b4bf-2567b6bbba00
---
## Description

Build the Splink 4 settings from the validated config: `record_key` as `unique_id_column_name`, the blocking rules from `blocking_rules_from_config`, and one comparison per `comparisons:` key whose levels are the S4.3.1 token→construct mapping with `NullLevel` ALWAYS first and `ElseLevel()` ALWAYS last. It pins the two tokens that had no Splink primitive — `variant_match` as an array-intersect level over `name_variants` (correct only because of S4.2's element-0 symmetry guarantee) and `dob_same_year_month` as the literal `date_trunc('month', …)` CustomLevel — and maps `tf: true` to `term_frequency_adjustments=True` on exact-match levels only. Per S4.2 the DOB precision is computed inside `parse_date` and never persisted, so the builder resolves the `birth_date_precision` minor by consuming no such column: a year-only DOB is NULL and `NullLevel` handles eligibility.

## Scope

### In scope

- `build_settings(cfg)` returning a Splink `SettingsCreator` plus `settings_json(cfg)` for the serialised form
- The six-token mapping table of S4.3.1, emitted as one construct per token
- NullLevel-first / ElseLevel-last emission for every comparison, regardless of what the config `levels` list contains
- `tf: true` → `.configure(term_frequency_adjustments=True)` on the exact-match level of the named column only
- Rejection of an unknown level token as a config error (exit 2) with `comparisons.unknown_level`
- Unit tests over the rendered settings dict — no Splink inference, no database

### Out of scope

- Anything that opens a DuckDB connection or constructs a `Linker` / `DuckDBAPI` (ER-049)
- TF materialisation, `tf_lookup` and `register_term_frequency_lookup` (ER-053) — this ticket only sets the flag
- Training call sequence and the committed fixture model (ER-054…ER-056)
- Adding a `birth_date_precision` column anywhere
- Editing `name_norm` or the nickname seed to make `variant_match` symmetric — the guarantee already exists

## Design decisions applied

Closes M13 (half the level vocabulary had no construct and no SQL, and no comparison had an else level) and the MINOR `birth_date_precision` finding, resolved in S4.2 by deletion: `parse_date` emits only `birth_date`, a year-precision parse yields NULL, and no comparison level, blocking key, survivorship rule or golden column reads a precision column. Easy to miss: without `ElseLevel()` Splink emits a `CASE … END` with no `ELSE`, giving gamma NULL and a NULL match weight for any pair matching nothing — that is the defect this ticket exists to prevent; the `null` token is stripped from `levels` during config normalisation (S6.1) so the builder must emit `NullLevel` unconditionally rather than on demand; `tf` applies only to exact-match levels; `phonetic` is deleted from the spec and must appear nowhere; all thresholds are probabilities and a Splink call taking a weight gets `log2(p/(1-p))`.

## Acceptance criteria

- [ ] AC1: For `configs/test.yaml`, the rendered settings dict has `unique_id_column_name == 'record_key'`, `link_type == 'dedupe_only'`, and its blocking rules are element-wise equal (by generated SQL) to `blocking_rules_from_config(cfg)[1]` in the same order.
- [ ] AC2: Every one of the six comparisons has a first level whose SQL is a NULL test on its own column and a last level that is the else level; removing the else-level emission makes the test fail.
- [ ] AC3: Token SQL is exact: `username_exact` renders `split_part(email_l,'@',1) = split_part(email_r,'@',1)`; `dob_same_year_month` renders `date_trunc('month', birth_date_l) = date_trunc('month', birth_date_r)`; `variant_match` renders an array-intersect level on `name_variants` with `min_intersection=1`; `jaro_winkler:0.90` renders a Jaro-Winkler level on the named column at threshold 0.9.
- [ ] AC4: `term_frequency_adjustments` is true on the exact-match level of `given_name`, `family_name` and `email` and on no other level and no other column (asserted by scanning every level of every comparison).
- [ ] AC5: A config that lists `null` in `levels` and the same config with `null` removed produce byte-identical `json.dumps(settings, sort_keys=True)` output.
- [ ] AC6: An unknown level token (`soundex`) raises the config error carrying `comparisons.unknown_level` and names the token, before any Splink object is constructed.
- [ ] AC7: The rendered settings reference no column named `birth_date_precision`, and the string `phonetic` appears nowhere in `src/er/matching/model.py`.
- [ ] AC8: Two calls to `settings_json(cfg)` in one process produce byte-identical output, and the builder opens no database connection.

## Tests

- tests/unit/matching/test_settings_builder.py::test_unique_id_and_blocking_rules_come_from_the_generator
- tests/unit/matching/test_settings_builder.py::test_null_level_first_and_else_level_last_on_every_comparison
- tests/unit/matching/test_settings_builder.py::test_level_token_sql_is_exact
- tests/unit/matching/test_settings_builder.py::test_tf_flag_only_on_exact_levels_of_tf_columns
- tests/unit/matching/test_settings_builder.py::test_null_token_is_normalisation_invariant
- tests/unit/matching/test_settings_builder.py::test_unknown_level_token_rejected
- tests/unit/matching/test_settings_builder.py::test_no_birth_date_precision_and_no_phonetic
- tests/unit/matching/test_settings_builder.py::test_settings_json_is_deterministic

## Verification

```bash
uv run pytest tests/unit/matching/test_settings_builder.py -q
uv run mypy --strict src/er/matching
uv run pytest tests/unit/matching/test_blocking_generator.py -q
bash scripts/gates.sh
```

## Definition of Done

- All acceptance criteria met and the verify command passes
- All six S4.3.1 tokens implemented; no seventh token exists and `phonetic` is absent
- `NullLevel` first / `ElseLevel` last is unconditional and covered by a test that fails when either is removed
- No `birth_date_precision` column is introduced in any model, contract or settings path
- `mypy --strict src/er/matching` clean
- Committed on main with the board updated
