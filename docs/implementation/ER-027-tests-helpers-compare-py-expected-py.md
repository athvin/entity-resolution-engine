---
id: ER-027
title: "tests/helpers/compare.py + expected.py: assert_partition_equal, assert_ids_stable, assert_golden_equal, content_digest, label-space loader"
milestone: M1
status: done
kind: code
size: M
gates: fast
depends_on: ["ER-006", "ER-013"]
spec_refs: ["s5", "s5-0", "s8-2-1", "s8-3"]
gap_refs: ["M7", "B4", "M3"]
provides: ["tests/helpers/compare.py::assert_partition_equal", "tests/helpers/compare.py::assert_ids_stable", "tests/helpers/compare.py::assert_golden_equal", "tests/helpers/expected.py::load_expected", "tests/helpers/expected.py::label_map_from_membership", "tests/helpers/expected.py::content_digest", "tests/helpers/expected.py::NULL_TOKEN", "tests/helpers/expected.py::FLOAT_TOL", "tests/helpers/expected.py::sort_key"]
consumes: ["src/er/lake/columns.py::VOLATILE_COLUMNS", "src/er/lake/columns.py::GOLDEN_SURVIVABLE_COLUMNS", "src/er/entities/ids.py::record_key", "src/er/entities/ids.py::CountingIdFactory"]
owns: ["tests/helpers/__init__.py", "tests/helpers/compare.py", "tests/helpers/expected.py", "tests/unit/test_compare_helpers.py"]
protected_paths: []
extra_paths: ["pyproject.toml"]
attempts: 1
verify: "uv run pytest tests/unit/test_compare_helpers.py -q && uv run mypy --strict tests/helpers"
branch: "ticket/ER-027-tests-helpers-compare-py-expected-py"
commit: "4b78abc810ce49db9008d33dc7f7b02b6fe90853"
spec_sha: "28d8d8e366a7b49b"
updated_at: "2026-08-15T03:34:14Z"
session: ad0622cb-92ac-495c-8c42-3e14b7d453d8
---
## Description

Nine of the eleven scenario tests are dead on arrival without the ID-insensitive / ID-identical distinction: G3 compares two independently built universes whose minted ULIDs legitimately differ, while G2 asserts identifiers survive, so an ID-insensitive comparison would pass vacuously against a pipeline that re-mints every entity every run (S8.2.1). This ticket ships the three comparison helpers with exactly the S8.2.1 signatures, plus the expected-file loading layer: the symbolic `E1..En` label space, the `\N` null token, the byte-wise sort key, the 1e-9 absolute float tolerance and a `content_digest` over an explicit stable column list. Closes M7, the comparison half of B4 and the partition-comparison half of M3.

## Scope

### In scope

- `compare.py` exposing **exactly** `assert_partition_equal`, `assert_ids_stable`, `assert_golden_equal` and nothing else public, with the S8.2.1 signatures verbatim.
- `expected.py`: `load_expected(path)` reading every column as VARCHAR with `\N` → None and an empty field → `""`; `label_map_from_membership(actual)` allocating `E1..En` in ascending order of each group's minimum `record_key`; `content_digest(rows, columns)`; `NULL_TOKEN`, `FLOAT_TOL`, `sort_key`.
- Volatile-column handling: both sides are stripped of `VOLATILE_COLUMNS` imported from `src/er/lake/columns.py` before any comparison.
- Re-sorting both sides by the byte-wise sort key before comparing, so a mis-sorted committed file is a lint failure (ER-028) and never a scenario-test failure.
- Failure messages carrying the symmetric difference (partition), the offending `(record_key, expected_id, actual_id)` rows (ids) and the differing cells (golden).
- `mypy --strict` clean over `tests/helpers`, including whatever `pyproject.toml` mypy path configuration that requires.

### Out of scope

- Scenario discovery, phase directories, the `base_scenario` manifest and `validate_fixtures.py` (ER-028).
- Any committed fixture data under `fixtures/static/` — no scenario is authored here.
- A fourth comparison helper or a two-universe overload: T-INC-1 compares each universe against one ID-insensitive expectation, which is equality to each other (S8.3).
- `table_content_hash` over live lake relations (ER-044); `content_digest` here operates on in-memory rows.

## Design decisions applied

M7 + B4 + M3. Constraints: (1) S8.2.1 says `compare.py` exposes exactly three functions **and no others** — `content_digest` and the label-space loader therefore live in `expected.py`, and a unit test enforces the module's public surface. (2) No helper may hard-code its own exclusion list: all three import `VOLATILE_COLUMNS` from `src/er/lake/columns.py` (S5.0). (3) `entity_label` is symbolic (`E1..En`) and MUST NEVER be a ULID; the loader rejects a ULID-shaped label so a fixture author cannot paste real ids. (4) Float tolerance is 1e-9 **absolute**, applied to `match_probability` and numeric golden columns only; every other column compares exactly after both sides are read as VARCHAR. (5) `assert_golden_equal` takes a `pd.DataFrame` per the pinned signature; pandas is already present transitively via Splink, and no new dependency row may be added to S2.1 by a code ticket.

## Acceptance criteria

- [ ] AC1: The public callables of `tests/helpers/compare.py` are exactly `{assert_partition_equal, assert_ids_stable, assert_golden_equal}`; adding a fourth public function fails the test.
- [ ] AC2: `assert_partition_equal` passes when every entity id in the actual partition is replaced by a fresh ULID with the grouping unchanged, and fails — printing the symmetric difference — when one record moves between groups.
- [ ] AC3: `assert_ids_stable` fails on that same id-replaced input, proving the two helpers are not interchangeable, and passes when the ids match the captured `label_map`.
- [ ] AC4: All three helpers drop `VOLATILE_COLUMNS` sourced from `src/er/lake/columns.py`: a test adds a member to the imported set and asserts the column stops being compared, and no exclusion-list literal appears in `tests/helpers`.
- [ ] AC5: A field written as `\N` loads as None while an empty field loads as `""`, and `assert_golden_equal` reports them as unequal.
- [ ] AC6: A golden numeric column differing by 9e-10 passes and one differing by 1.1e-9 fails; a non-numeric column differing by any amount fails.
- [ ] AC7: A shuffled expected CSV compares equal to the same content sorted, because both sides are re-sorted by `sort_key` before comparison.
- [ ] AC8: `label_map_from_membership` allocates `E1..En` in ascending order of each group's minimum `record_key`, and `load_expected` raises when an `entity_label` value is a 26-character ULID.

## Tests

- tests/unit/test_compare_helpers.py::test_compare_module_exposes_exactly_three_functions
- tests/unit/test_compare_helpers.py::test_partition_equal_is_id_insensitive
- tests/unit/test_compare_helpers.py::test_ids_stable_is_id_identical
- tests/unit/test_compare_helpers.py::test_volatile_columns_are_imported_not_relisted
- tests/unit/test_compare_helpers.py::test_null_token_distinct_from_empty_string
- tests/unit/test_compare_helpers.py::test_float_tolerance_boundary
- tests/unit/test_compare_helpers.py::test_both_sides_are_resorted_before_comparison
- tests/unit/test_compare_helpers.py::test_label_allocation_order_and_ulid_rejection
- tests/unit/test_compare_helpers.py::test_content_digest_is_row_order_stable

## Verification

```bash
uv run pytest tests/unit/test_compare_helpers.py -q && uv run mypy --strict tests/helpers
uv run ruff check tests/helpers
```

## Definition of Done

- All acceptance criteria demonstrated by the listed node ids
- `compare.py` public surface is exactly the three S8.2.1 functions with the pinned signatures
- `VOLATILE_COLUMNS` imported, never re-listed
- Null token, float tolerance, sort key and label space implemented as S8.2.1 states them
- `mypy --strict tests/helpers` green
- verify command passes
