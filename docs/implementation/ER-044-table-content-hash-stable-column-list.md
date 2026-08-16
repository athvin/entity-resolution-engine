---
id: ER-044
title: "table_content_hash (stable column list, VOLATILE_COLUMNS excluded) + T-STD-1"
milestone: M2
status: in_progress
kind: code
size: S
gates: full
depends_on: ["ER-006", "ER-043"]
spec_refs: ["s8-3", "s5-0", "s5", "s8-2-1", "s4-2"]
gap_refs: ["M7"]
provides: ["src/er/lake/hashing.py::table_content_hash", "src/er/lake/hashing.py::STD_HASH_COLUMNS", "tests/integration/test_std_determinism.py", "tests/unit/lake/test_hashing.py"]
consumes: ["src/er/lake/columns.py::VOLATILE_COLUMNS", "src/er/lake/columns.py::STD_RECORD_COLUMNS", "relation:int_std_records", "src/er/dbt_runner.py::run_dbt", "tests/conftest.py::lake_ns"]
owns: ["src/er/lake/hashing.py", "tests/unit/lake/test_hashing.py", "tests/integration/test_std_determinism.py"]
protected_paths: ["dbt/models/intermediate/int_std_records.sql", "src/er/lake/columns.py"]
extra_paths: []
attempts: 1
verify: "bash scripts/ci/itest.sh tests/integration/test_std_determinism.py -q && uv run pytest tests/unit/lake/test_hashing.py -q"
branch: "ticket/ER-044-table-content-hash-stable-column-list"
commit: ""
spec_sha: "2e62460d9f41a842"
updated_at: "2026-08-16T05:25:17Z"
session: 4c0f1bc3-c077-42f0-a523-08a309197b5b
---
## Description

Implement `table_content_hash`, the one function that reduces an `int_std_records` row to the `std_hash` T-STD-1 defines: SHA-256 over the eighteen named columns joined by `\x1f` with NULL encoded as the empty string and `name_variants` rendered as `array_to_string(name_variants,'\x1f')`. The column list is derived from `src/er/lake/columns.py` and asserted disjoint from `VOLATILE_COLUMNS`, so no determinism comparison can accidentally hash a run-dependent value (M7, S5.0). It ships T-STD-1 itself: `er standardize` run twice produces byte-identical hashes, and Parquet byte identity is explicitly not asserted.

## Scope

### In scope

- `src/er/lake/hashing.py`: `STD_HASH_COLUMNS` (the T-STD-1 order) and `table_content_hash(row) -> str`
- Canonical scalar rendering rules — booleans, DATE, TIMESTAMP, NULL, and the `name_variants` list join — pinned by unit tests with a committed golden vector
- A `VOLATILE_COLUMNS` disjointness assertion that reads the frozen set from `columns.py` rather than restating it
- T-STD-1 as `tests/integration/test_std_determinism.py::test_std_content_hash_stable`: standardize twice over `base_10`, compare the 23 `(record_key, std_hash)` pairs

### Out of scope

- The committed `expected/base/std_hashes.csv` file and its comparison (ER-045)
- `content_hash` over *source* columns — that is `src/er/ingest/hashing.py` (ER-029) and is a different function with a different column list
- Any change to `int_std_records`'s column list or to `columns.py`
- Golden or membership determinism helpers in `tests/helpers/compare.py`

## Design decisions applied

Closes M7: "byte-identical std_records" is unimplementable (Parquet is not reproducible and the relation carries `VOLATILE_COLUMNS`), so determinism is redefined as a content hash over an explicit stable column list. Easy to miss: the column order is the literal T-STD-1 order in S8.3 (`record_key, std_version, given_name, family_name, array_to_string(name_variants,'\x1f'), email, email_valid, phone_e164, phone_valid, addr_number, addr_street, addr_unit, addr_city, addr_region, addr_postal, birth_date, updated_at_source, content_hash`) and changing it invalidates every committed `std_hashes.csv`; NULL is the empty string, which is deliberately indistinguishable from an empty-string value at hash level and must not be given a sentinel; the hash is per row, so no ORDER BY and no aggregate is involved.

## Acceptance criteria

- [ ] AC1: `table_content_hash` reproduces a committed golden vector: one hand-authored row with a NULL in every nullable position and one fully populated row hash to the exact hex digests recorded in the unit test.
- [ ] AC2: `STD_HASH_COLUMNS` equals the eighteen-element T-STD-1 sequence in order, and `set(STD_HASH_COLUMNS) & VOLATILE_COLUMNS == set()` where `VOLATILE_COLUMNS` is imported from `src/er/lake/columns.py` (the test fails if the module re-lists the set locally).
- [ ] AC3: Scalar rendering is pinned: `True`→`true`, `False`→`false`, `date(2001,2,3)`→`2001-02-03`, `None`→`''`, and `name_variants=['bob','robert']`→`bob\x1frobert`; changing any single one of these changes the digest.
- [ ] AC4: Running `er standardize` twice over `base_10` yields 23 `(record_key, std_hash)` pairs that are byte-identical across the two runs, while `ingest_batch_id` and `ingested_at` are allowed to differ; the test asserts no Parquet file comparison is performed.
- [ ] AC5: Re-ingesting one record with a corrected `given_name` changes exactly that record's `std_hash` and leaves the other 22 unchanged.
- [ ] AC6: Two rows differing only in a `VOLATILE_COLUMNS` value hash identically.

## Tests

- tests/unit/lake/test_hashing.py::test_golden_vector_matches
- tests/unit/lake/test_hashing.py::test_column_list_is_t_std_1_order_and_non_volatile
- tests/unit/lake/test_hashing.py::test_scalar_rendering_is_pinned
- tests/unit/lake/test_hashing.py::test_volatile_only_difference_hashes_equal
- tests/integration/test_std_determinism.py::test_std_content_hash_stable
- tests/integration/test_std_determinism.py::test_single_record_change_moves_one_hash

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_std_determinism.py -q
uv run pytest tests/unit/lake/test_hashing.py -q
uv run mypy --strict src/er/lake/hashing.py
bash scripts/gates.sh
```

## Definition of Done

- All acceptance criteria met and both verify commands pass
- `VOLATILE_COLUMNS` is imported, never restated, and the disjointness assertion is executable
- The golden vector is committed inside the unit test with the inputs that produced it
- T-STD-1's node id is exactly `tests/integration/test_std_determinism.py::test_std_content_hash_stable` and is collectible
- `int_std_records.sql` and `columns.py` unmodified
- Committed on main with the board updated
