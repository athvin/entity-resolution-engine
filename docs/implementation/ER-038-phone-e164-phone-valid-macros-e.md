---
id: ER-038
title: "phone_e164 + phone_valid macros with E.164 round-trip properties"
milestone: M2
status: in_progress
kind: code
size: S
gates: fast
depends_on: ["ER-037"]
spec_refs: ["s2-1", "s4-2", "s5", "s6", "s6-1", "s8-2", "s8-4"]
gap_refs: ["M11", "M14"]
provides: ["dbt/macros/std/phone_e164.sql::phone_e164"]
consumes: ["tests/unit/dbt/harness.py::MacroHarness", "tests/unit/dbt/harness.py::eval_macro", "dbt/macros/std/lowercase_trim.sql::lowercase_trim", "dbt/macros/std/null_semantics.sql::null_semantics", "src/er/dbt_runner.py::render_dbt_vars"]
owns: ["dbt/macros/std/phone_e164.sql", "tests/unit/dbt/test_phone_e164.py"]
protected_paths: []
extra_paths: []
attempts: 1
verify: "uv run pytest tests/unit/dbt/test_phone_e164.py -q"
branch: "ticket/ER-038-phone-e164-phone-valid-macros-e"
commit: ""
spec_sha: "28d8d8e366a7b49b"
updated_at: "2026-08-15T05:47:06Z"
session: 03061036-0d0e-4c61-80b4-368a0bd419f0
---
## Description

Ship the S4.2 `phone_e164(col)` macro: digits-only extraction, default region from `standardization.phone_default_region`, E.164 rendering, emitting the two columns `phone_e164` and `phone_valid` that S5 declares on `stg_*` and `int_std_records`. `phone_valid` is the input the `validated` survivorship rule needs for the phone attribute (S6.1 V4), and the drifted-phone trap in `base_10` — `(415) 555-0132`, `415-555-0132`, `+14155550132` all collapsing to one blocking key — is what this macro must make true.

## Scope

### In scope

- `phone_e164(col)` expanding to two aliased projections, `phone_e164` and `phone_valid`.
- Digits-only extraction after `lowercase_trim`/sentinel handling; NANP normalization for `phone_default_region = 'US'`: 10 digits → `+1` + digits, 11 digits leading `1` → `+1` + last 10.
- Inputs already in E.164 (`+` prefix): digits preserved verbatim, never re-prefixed.
- `phone_default_region` read from the `standardization` var, not hard-coded.
- Property-based E.164 round-trip and idempotence coverage per S8.4.

### Out of scope

- Adding a phone-parsing dependency such as `phonenumbers`: S2.1's rule is that adding a dependency means adding a row to the pin table, which is a spec amendment and is not in this ticket's scope.
- Full international parsing: v1 supports the NANP region only; every other configured region yields NULL/false rather than a guessed rendering.
- Extension normalisation (`x1234`), vanity numbers, and any `phone_*` column beyond the two S5 declares.
- The `base_10` fixture rows themselves — ER-041.

## Design decisions applied

Implements gap entries M11 and M14. Constraints: (1) **No new dependency.** The macro is pure SQL/Jinja over DuckDB string functions; a library-based parser would require an S2.1 pin row and an `er doctor` assertion. (2) Validity tri-state mirrors `email_norm`: `phone_valid` is NULL when the input carries no evidence (NULL, blank, or a `null_semantics` sentinel), `false` when a non-empty input cannot be rendered, `true` when it renders — and `phone_e164` is NULL whenever `phone_valid` is not `true`. (3) v1 supports `phone_default_region = 'US'`; V16 admits any ISO alpha-2, so a non-US region must degrade to NULL/false rather than emit a wrong `+1` number that would silently block two unrelated records together. (4) `phone_e164` is a blocking key expression (S6 `phone_exact`), so its output must be stable and canonical — no formatting characters, and no digits stripped from an E.164 input.

## Acceptance criteria

- [ ] AC1: All three `base_10` drift formats of one number — `(415) 555-0132`, `415-555-0132`, `+14155550132` — render `phone_e164 = '+14155550132'` with `phone_valid = true`.
- [ ] AC2: `1-415-555-0132`, `415.555.0132` and `  4155550132 ` also render `'+14155550132'`; idempotence `phone_e164(phone_e164(x)) == phone_e164(x)` holds over hypothesis-generated NANP numbers under all drift formats.
- [ ] AC3: NULL, `''`, `'   '` and each `null_semantics` sentinel yield `phone_e164` NULL and `phone_valid` NULL; a 7-digit local number `'555-0132'` and `'abc'` yield `phone_e164` NULL and `phone_valid` false.
- [ ] AC4: An already-E.164 non-NANP input `'+442071838750'` round-trips unchanged with `phone_valid = true` and is never prefixed with `+1`.
- [ ] AC5: Rendering with `standardization.phone_default_region = 'GB'` yields `phone_e164` NULL and `phone_valid` false for a bare 10-digit input — the region is read from the var and no `+1` is emitted for a non-US region.
- [ ] AC6: The rendered SQL contains no hard-coded region literal outside the region-keyed branch, so changing `phone_default_region` in the vars dict changes the result with no macro edit.
- [ ] AC7: `phone_e164(col)` expands to exactly two projections aliased `phone_e164` and `phone_valid`, matching the S5 column names.

## Tests

- tests/unit/dbt/test_phone_e164.py::test_drift_formats_collapse_to_one_e164_value
- tests/unit/dbt/test_phone_e164.py::test_e164_round_trip_and_idempotence_property
- tests/unit/dbt/test_phone_e164.py::test_null_blank_and_sentinel_inputs_yield_null_validity
- tests/unit/dbt/test_phone_e164.py::test_unrenderable_input_is_false_not_null
- tests/unit/dbt/test_phone_e164.py::test_existing_e164_is_preserved
- tests/unit/dbt/test_phone_e164.py::test_non_us_region_degrades_to_null
- tests/unit/dbt/test_phone_e164.py::test_emits_phone_e164_and_phone_valid_aliases

## Verification

```bash
uv run pytest tests/unit/dbt/test_phone_e164.py -q
uv run dbt parse --project-dir dbt --profiles-dir dbt/profiles --target mem
uv run ruff check . && uv run ruff format --check .
```

## Definition of Done

- All acceptance criteria have a named passing test
- Verify command passes on a bare runner
- No new Python or dbt package dependency introduced (no S2.1 row needed)
- `phone_valid` tri-state matches the `email_valid` convention from ER-037
- `dbt parse --target mem` still succeeds
- ruff clean
