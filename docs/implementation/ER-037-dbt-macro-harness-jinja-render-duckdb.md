---
id: ER-037
title: "dbt macro harness (Jinja render + duckdb + seed() stub) + lowercase_trim, null_semantics, email_norm"
milestone: M2
status: todo
kind: code
size: M
gates: fast
depends_on: ["ER-008", "ER-011", "ER-033"]
spec_refs: ["s3", "s4-2", "s5", "s6", "s6-1", "s8-1", "s8-4"]
gap_refs: ["M14", "M21", "M12"]
provides: ["tests/unit/dbt/harness.py::MacroHarness", "tests/unit/dbt/harness.py::render_macro", "tests/unit/dbt/harness.py::eval_macro", "tests/unit/dbt/harness.py::register_seed", "dbt/macros/std/lowercase_trim.sql::lowercase_trim", "dbt/macros/std/null_semantics.sql::null_semantics", "dbt/macros/std/null_semantics.sql::NULL_SENTINELS", "dbt/macros/std/email_norm.sql::email_norm"]
consumes: ["src/er/dbt_runner.py::render_dbt_vars", "src/er/config/schema.py::Config", "src/er/config/loader.py::load_config", "dbt/dbt_project.yml", "dbt/profiles/profiles.yml::mem"]
owns: ["tests/unit/dbt/harness.py", "tests/unit/dbt/conftest.py", "tests/unit/dbt/test_macro_harness.py", "tests/unit/dbt/test_lowercase_trim.py", "tests/unit/dbt/test_null_semantics.py", "tests/unit/dbt/test_email_norm.py", "dbt/macros/std/lowercase_trim.sql", "dbt/macros/std/null_semantics.sql", "dbt/macros/std/email_norm.sql"]
protected_paths: []
extra_paths: []
attempts: 0
verify: "uv run pytest tests/unit/dbt/test_macro_harness.py tests/unit/dbt/test_lowercase_trim.py tests/unit/dbt/test_null_semantics.py tests/unit/dbt/test_email_norm.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

Build the unit-layer harness that renders a dbt macro through Jinja and executes the resulting SQL against a bare in-process DuckDB — the substrate S8.1's Unit row requires for normalizer testing — and ship the first three S4.2 standardization macros on top of it. `var()`, `ref()`, `source()` and `seed()` are stubbed so a macro can be exercised with no warehouse, no lake attach and no dbt subprocess. Every later macro ticket (ER-038, ER-039, ER-040) is written against this harness, so its contract is fixed here.

## Scope

### In scope

- `MacroHarness`: Jinja environment loading `dbt/macros/**`, bound `var()` reading a supplied vars dict, `ref()`/`source()` returning registered relation names, `seed()` resolving a registered CSV into an in-process DuckDB table.
- `render_macro(name, *args, vars=…) -> str`, `eval_macro(name, value, vars=…) -> rows`, and `register_seed(name, rows)`.
- `lowercase_trim(col)`: NFC-normalize, trim, lower; empty string → NULL.
- `null_semantics(col)`: the sentinel vocabulary `''`, `'NULL'`, `'N/A'`, `'-'`, `'unknown'` → NULL, declared once as `NULL_SENTINELS`.
- `email_norm(col)`: `lowercase_trim`, plus-addressing stripped only when `standardization.email_strip_plus_addressing`, then every address in `standardization.email_placeholders` nulled; emits the two aliases `email` and `email_valid`.
- Property-based coverage per S8.4: idempotence, casing invariance, NULL distinct from empty string, Unicode/NFC ranges.

### Out of scope

- `phone_e164` (ER-038), `name_norm` / `name_variants` (ER-039), `address_parse` (ER-040), `parse_date` — later tickets.
- Any `stg_*` model, `int_std_records`, or a dbt run against the `lake` target; no model ships before ER-042.
- Reading standardization settings from anywhere but the `standardization` var `render_dbt_vars` produces — the config is the single source of truth.
- Placeholder-email nulling inside `null_semantics`: S4.2 assigns that to `email_norm` and to no other macro.

## Design decisions applied

Implements gap entries M14, M21 and M12 (the macro half). Constraints: (1) The division of labour in S4.2 is normative — `null_semantics` handles only the sentinel vocabulary and never sees an email address; `email_norm` alone nulls `standardization.email_placeholders`. `base_10`'s `test@test.com` trap is the executable proof and is asserted here. (2) The sentinel vocabulary is listed in S4.2 in mixed case; match case-insensitively on the trimmed value, otherwise `'null'` and `'n/a'` survive standardization. Define the vocabulary once. (3) `email_valid` is NULL exactly when `email` is NULL (no evidence), `true` when the value parses as a syntactically valid address and `false` when a non-empty value does not — a nulled placeholder must not present as `false` evidence to the `validated` survivorship rule, which orders `<attr>_valid DESC NULLS LAST`. (4) The harness must open no warehouse connection and must not shell out to dbt: S8.1 forbids `dbt compile` on the static layer and the unit layer runs service-lessly. (5) `email_norm` emits two aliased projections matching the S5 column names `email` and `email_valid`; it is used in a select list, not as a scalar.

## Acceptance criteria

- [ ] AC1: `MacroHarness` renders and executes any macro under `dbt/macros/` against an in-process `duckdb` connection with no ATTACH, no profile and no dbt subprocess; `test_harness_uses_no_warehouse_connection` fails if an `ATTACH` statement or a `subprocess` call is issued.
- [ ] AC2: `eval_macro('lowercase_trim', …)` maps both the NFC and NFD spellings of `'  ÅSA '` to `'åsa'`, maps `''` and `'   '` to SQL NULL, and satisfies `f(f(x)) == f(x)` and `f(x) == f(upper(x))` over hypothesis-generated Unicode strings.
- [ ] AC3: `null_semantics` returns NULL for each of `''`, `'NULL'`, `'null'`, `'N/A'`, `'n/a'`, `'-'`, `'unknown'`, `'UNKNOWN'` and returns the input unchanged for `'none'`, `'na'`, `'--'`, `'0'` and `'test@test.com'`.
- [ ] AC4: Under `configs/test.yaml` (`email_strip_plus_addressing: false`), `email_norm('Bob+news@Example.COM ')` yields `email='bob+news@example.com'` and `email_valid=true`; with the flag set to `true` the same input yields `email='bob@example.com'`.
- [ ] AC5: Every address in `standardization.email_placeholders` — including `test@test.com` — yields `email` NULL and `email_valid` NULL, and a non-empty unparsable value such as `'not-an-email'` yields `email` NULL and `email_valid` false.
- [ ] AC6: `email_norm` expands to exactly two projections aliased `email` and `email_valid`, and reads its settings from the `standardization` var (rendering with a different `email_placeholders` list changes the result with no macro edit).
- [ ] AC7: All three macros are idempotent and casing-invariant under hypothesis, and NULL and the empty string are distinguishable in every output.

## Tests

- tests/unit/dbt/test_macro_harness.py::test_renders_and_executes_a_macro_in_process
- tests/unit/dbt/test_macro_harness.py::test_harness_uses_no_warehouse_connection
- tests/unit/dbt/test_macro_harness.py::test_seed_and_ref_stubs_resolve
- tests/unit/dbt/test_lowercase_trim.py::test_nfc_trim_lower_and_empty_to_null
- tests/unit/dbt/test_lowercase_trim.py::test_idempotent_and_casing_invariant
- tests/unit/dbt/test_null_semantics.py::test_sentinel_vocabulary_maps_to_null
- tests/unit/dbt/test_null_semantics.py::test_non_sentinels_including_placeholder_email_pass_through
- tests/unit/dbt/test_email_norm.py::test_plus_addressing_honoured_in_both_settings
- tests/unit/dbt/test_email_norm.py::test_placeholders_null_email_and_email_valid
- tests/unit/dbt/test_email_norm.py::test_emits_email_and_email_valid_aliases

## Verification

```bash
uv run pytest tests/unit/dbt/test_macro_harness.py tests/unit/dbt/test_lowercase_trim.py tests/unit/dbt/test_null_semantics.py tests/unit/dbt/test_email_norm.py -q
uv run dbt parse --project-dir dbt --profiles-dir dbt/profiles --target mem
uv run dbt compile --project-dir dbt --profiles-dir dbt/profiles --target mem
uv run ruff check . && uv run ruff format --check .
```

## Definition of Done

- All acceptance criteria have a named passing test
- Verify command passes on a bare runner with no Docker and no services
- `dbt parse` and `dbt compile --target mem` still succeed with the three macros present
- Sentinel vocabulary defined exactly once; placeholder nulling lives only in `email_norm`
- Standardization settings are read from the `standardization` dbt var, never hard-coded
- ruff clean
