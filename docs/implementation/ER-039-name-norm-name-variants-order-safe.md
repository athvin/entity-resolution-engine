---
id: ER-039
title: "name_norm + name_variants (order-safe SYMMETRY construction) + nickname_variants.csv seed (12 pairs incl. robert/bob/bobby)"
milestone: M2
status: todo
kind: code
size: M
gates: fast
depends_on: ["ER-037"]
spec_refs: ["s3", "s4-2", "s4-3-1", "s5", "s8-2", "s8-3", "s8-4"]
gap_refs: ["M13"]
provides: ["dbt/macros/std/name_norm.sql::name_norm", "dbt/macros/std/name_variants.sql::name_variants", "dbt/seeds/nickname_variants.csv"]
consumes: ["tests/unit/dbt/harness.py::MacroHarness", "tests/unit/dbt/harness.py::eval_macro", "tests/unit/dbt/harness.py::register_seed", "dbt/macros/std/lowercase_trim.sql::lowercase_trim", "dbt/macros/std/null_semantics.sql::null_semantics"]
owns: ["dbt/macros/std/name_norm.sql", "dbt/macros/std/name_variants.sql", "dbt/seeds/nickname_variants.csv", "tests/unit/dbt/test_name_norm.py", "tests/unit/dbt/test_name_variants.py"]
protected_paths: []
extra_paths: []
attempts: 0
verify: "uv run pytest tests/unit/dbt/test_name_norm.py tests/unit/dbt/test_name_variants.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

Ship `name_norm` (NFC + lowercase + trim, punctuation strip, diacritic fold) and `name_variants`, the `LIST(VARCHAR) NOT NULL` column S5 declares and S4.3.1's `variant_match` (`ArrayIntersectLevel`, `min_intersection=1`) consumes. The array construction must be order-safe: the normalized `given_name` is ALWAYS element 0 of its own array on every record, which is the symmetry guarantee that makes `variant_match` orientation-independent and T-MATCH-SYM provable rather than empirical. Ship the `nickname_variants.csv` seed with twelve canonical pairs including robert/bob and robert/bobby, the `base_10` nickname trap.

## Scope

### In scope

- `name_norm(col)`: NFC-normalize, trim, lower, strip punctuation, fold diacritics, collapse internal whitespace; empty result → NULL.
- `name_variants(col)`: builds the array as the normalized name at element 0 followed by the sorted, de-duplicated one-hop seed closure of that name.
- `dbt/seeds/nickname_variants.csv`: exactly 12 rows, canonical `variant_a < variant_b`, lowercase ASCII, sorted, no duplicates, including `(bob, robert)` and `(bobby, robert)`.
- Symmetric use of the seed at query time: a row `(a, b)` contributes `b` to `a`'s variants and `a` to `b`'s.
- Seed lint assertions inside `tests/unit/dbt/test_name_variants.py` (the verify command runs only the two named files).

### Out of scope

- The Splink comparison level `variant_match` itself and the settings builder — ER-048.
- T-MATCH-SYM (an integration test in M3) — this ticket proves the *precondition* at unit level.
- Transitive closure of the nickname graph, phonetic keys, or a `phonetic` comparison level (deleted from the spec).
- Family-name variant expansion: only `given_name` carries `name_variants`.
- Any `stg_*` model wiring — ER-042.

## Design decisions applied

Implements gap entry M13. Constraints an implementer will otherwise get wrong. (1) **S4.2's macro row reads as one macro emitting three columns; it is two.** `name_norm(col)` is a scalar expression used for both `given_name` and `family_name`; `name_variants(col)` builds the array, and the symmetry construction lives there. (2) `name_variants` is `LIST(VARCHAR) NOT NULL` in S5, so it is never SQL NULL — it is the empty list exactly when `name_norm(col)` is NULL, and otherwise its element 0 is exactly `name_norm(col)`. (3) The tail is sorted and de-duplicated so two records with the same given name produce byte-identical arrays — a determinism requirement, since `int_std_records` feeds `std_hash` in T-STD-1. (4) One-hop closure is sufficient and transitive closure is forbidden: `bob → {bob, robert}` and `bobby → {bobby, robert}` already intersect on `robert`, so `ArrayIntersectLevel(min_intersection=1)` fires without inflating the graph. (5) Punctuation handling must be pinned, not left to taste: `-` and `_` become a single space, `.` `'` `’` `,` are removed, then whitespace is collapsed — so `O'Brien` and `OBrien` normalize alike, as do `Mary-Jane` and `Mary Jane`.

## Acceptance criteria

- [ ] AC1: `name_norm` maps `"  José-María O'Brien "` to `'jose maria obrien'`, maps `'MARY-JANE'` and `'Mary Jane'` to the same value, and maps `'  '`, `''` and each `null_semantics` sentinel to NULL.
- [ ] AC2: `name_norm` is idempotent and casing-invariant over hypothesis-generated Unicode names, including NFD input that must fold identically to its NFC spelling.
- [ ] AC3: For every non-NULL input, `name_variants(col)[1]` (DuckDB lists are 1-indexed) equals `name_norm(col)` — asserted over every seed member and over hypothesis-generated names; this is the S4.2 symmetry guarantee.
- [ ] AC4: `name_variants` is never SQL NULL and is the empty list exactly when `name_norm(col)` is NULL.
- [ ] AC5: `list_has_any(name_variants('Robert'), name_variants('Bob'))` is true in both orientations, as is `Robert`/`Bobby`, while `list_has_any(name_variants('Robert'), name_variants('Susan'))` is false.
- [ ] AC6: For every row `(a, b)` of the seed, `b ∈ name_variants(a)` and `a ∈ name_variants(b)`.
- [ ] AC7: `dbt/seeds/nickname_variants.csv` has exactly 12 data rows, is sorted ascending, contains no duplicate pair, has every value lowercase ASCII, satisfies `variant_a < variant_b` on every row, and contains `(bob, robert)` and `(bobby, robert)`.
- [ ] AC8: The array tail is sorted ascending and de-duplicated, so two records with the same given name yield byte-identical `name_variants` values.

## Tests

- tests/unit/dbt/test_name_norm.py::test_punctuation_diacritics_and_whitespace_rules
- tests/unit/dbt/test_name_norm.py::test_idempotent_and_casing_invariant
- tests/unit/dbt/test_name_norm.py::test_blank_and_sentinel_inputs_are_null
- tests/unit/dbt/test_name_variants.py::test_normalized_name_is_element_zero
- tests/unit/dbt/test_name_variants.py::test_never_null_and_empty_list_when_name_is_null
- tests/unit/dbt/test_name_variants.py::test_robert_bob_bobby_intersect_in_both_orientations
- tests/unit/dbt/test_name_variants.py::test_seed_closure_is_symmetric
- tests/unit/dbt/test_name_variants.py::test_nickname_seed_lint
- tests/unit/dbt/test_name_variants.py::test_tail_is_sorted_deduped_and_byte_stable

## Verification

```bash
uv run pytest tests/unit/dbt/test_name_norm.py tests/unit/dbt/test_name_variants.py -q
uv run dbt parse --project-dir dbt --profiles-dir dbt/profiles --target mem
uv run ruff check . && uv run ruff format --check .
```

## Definition of Done

- All acceptance criteria have a named passing test
- Verify command passes on a bare runner
- Element 0 of `name_variants` is the record's own normalized given name on every non-NULL input
- `nickname_variants.csv` passes the seed lint inside `test_name_variants.py`
- No transitive closure is computed and no phonetic key is introduced
- `dbt parse --target mem` still succeeds
- ruff clean
