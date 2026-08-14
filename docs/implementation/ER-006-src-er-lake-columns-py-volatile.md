---
id: ER-006
title: "src/er/lake/columns.py: VOLATILE_COLUMNS, GOLDEN_SURVIVABLE_COLUMNS, STD_RECORD_COLUMNS (dependency-free single definitions)"
milestone: M1
status: todo
kind: code
size: S
gates: fast
depends_on: ["ER-001", "ER-003"]
spec_refs: ["s5", "s5-0", "s8-2-1", "s8-3"]
gap_refs: ["M7", "B4", "M4"]
provides: ["src/er/lake/columns.py::VOLATILE_COLUMNS", "src/er/lake/columns.py::GOLDEN_SURVIVABLE_COLUMNS", "src/er/lake/columns.py::STD_RECORD_COLUMNS", "tests/unit/test_columns.py"]
consumes: ["src/er/lake/__init__.py", "DesignDoc.md::s5-0", "pyproject.toml"]
owns: ["src/er/lake/columns.py", "tests/unit/test_columns.py"]
protected_paths: ["DesignDoc.md"]
extra_paths: []
attempts: 0
verify: "uv run pytest tests/unit/test_columns.py -q && uv run mypy --strict src/er/lake/columns.py"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

Three column sets are consumed by mutually distant parts of the system — determinism comparisons, config validation, golden assembly, the fixture helpers — and S5.0 requires each to have exactly one definition. This ticket ships `src/er/lake/columns.py` with `VOLATILE_COLUMNS` (the nine columns excluded from every determinism comparison), `GOLDEN_SURVIVABLE_COLUMNS` (the ordered eleven produced by survivorship) and `STD_RECORD_COLUMNS` (the `int_std_records` column list of S5, which S6.1 V6 validates config references against). The module is deliberately dependency-free so `tests/helpers/compare.py`, the config validators and the dbt-facing code can all import it without a cycle.

## Scope

### In scope

- `VOLATILE_COLUMNS: Final[frozenset[str]]` — exactly the nine names in S5.0
- `GOLDEN_SURVIVABLE_COLUMNS: Final[tuple[str, ...]]` — the eleven `golden_records` survivable columns in S5 DDL order
- `STD_RECORD_COLUMNS: Final[tuple[str, ...]]` — the `int_std_records` columns in S5 DDL order
- Tests that parse the literal lists out of DesignDoc.md so a spec edit without a code edit fails
- A dependency-free guard: the module's import set is stdlib-only

### Out of scope

- The T-STD-1 `std_hash` column projection and `table_content_hash` (ER-044) — it is a separate ordered list derived from these; this ticket only asserts membership
- The `TableSpec` registry and all DDL (ER-017/ER-019)
- `tests/helpers/compare.py` (ER-027), which imports `VOLATILE_COLUMNS` rather than re-listing it
- Config validation of the survivorship key set against `GOLDEN_SURVIVABLE_COLUMNS` (ER-012)

## Design decisions applied

Implements M7 (`VOLATILE_COLUMNS` defined once, excluded from every determinism comparison), B4 (the survivable column set is the key set of `survivorship:`, so it must exist as a checkable object) and M4 (single ownership). Three constraints. (1) `GOLDEN_SURVIVABLE_COLUMNS` is a *tuple*, not a set: its order is the DDL order and downstream comparisons rely on it — do not sort it. (2) `email_valid` and `phone_valid` are deliberately absent; they are `int_std_records` inputs to the `validated` rule, not golden columns. (3) The module MUST NOT import anything outside the stdlib — every later ticket consumes it, and a dependency here becomes an import cycle in `config/`, `lake/` and `tests/helpers/`.

## Acceptance criteria

- [ ] AC1: `VOLATILE_COLUMNS` is a `frozenset` equal to the nine names parsed from the S5.0 `VOLATILE_COLUMNS` code block of DesignDoc.md (`ingest_batch_id, ingested_at, assembled_at, scored_at, assigned_at, occurred_at, run_id, event_id, seq`); the test compares against the parsed spec, so removing a name from either side fails.
- [ ] AC2: `GOLDEN_SURVIVABLE_COLUMNS` is a `tuple` equal, element for element and in order, to the S5.0 block; `GOLDEN_SURVIVABLE_COLUMNS != tuple(sorted(GOLDEN_SURVIVABLE_COLUMNS))` (order is load-bearing), and it contains none of `entity_id`, `survivorship_version`, `assembled_at`, `email_valid`, `phone_valid`.
- [ ] AC3: `STD_RECORD_COLUMNS` equals the `int_std_records` column list parsed from the S5 dbt-owned DDL block, in that order; `set(STD_RECORD_COLUMNS) & VOLATILE_COLUMNS == {"ingest_batch_id", "ingested_at"}`.
- [ ] AC4: Every column named in the T-STD-1 `std_hash` list of S8.3 is a member of `STD_RECORD_COLUMNS` (the test parses that row), so ER-044's hash projection cannot name a column that does not exist.
- [ ] AC5: `GOLDEN_SURVIVABLE_COLUMNS` expands the S6 `survivorship:` key set with `address` → the six `addr_*` columns: the test asserts `set(GOLDEN_SURVIVABLE_COLUMNS) == {given_name, family_name, email, phone_e164, birth_date} | {addr_number, addr_street, addr_unit, addr_city, addr_region, addr_postal}`.
- [ ] AC6: `ast.parse` of `src/er/lake/columns.py` yields no `Import`/`ImportFrom` outside `typing` and `collections.abc` — the dependency-free property is asserted, not assumed.
- [ ] AC7: `uv run mypy --strict src/er/lake/columns.py` exits 0 with the three names annotated `Final`, and `grep -rn 'GOLDEN_SURVIVABLE_COLUMNS *=' src tests` returns exactly one line.

## Tests

- tests/unit/test_columns.py::test_volatile_columns_match_spec
- tests/unit/test_columns.py::test_golden_survivable_columns_are_ordered_and_match_spec
- tests/unit/test_columns.py::test_std_record_columns_match_spec_ddl
- tests/unit/test_columns.py::test_t_std_1_hash_columns_are_subset_of_std_record_columns
- tests/unit/test_columns.py::test_survivorship_keyset_expansion_equals_golden_survivable
- tests/unit/test_columns.py::test_module_is_dependency_free

## Verification

```bash
uv run pytest tests/unit/test_columns.py -q && uv run mypy --strict src/er/lake/columns.py
uv run ruff check src/er/lake/columns.py
```

## Definition of Done

- Acceptance criteria met
- Verify command passes
- All three constants parsed-and-compared against DesignDoc.md, not hand-copied
- Order preserved for the two tuples
- Module imports stdlib only
- Exactly one definition site in the repo
- DesignDoc.md unmodified
- Committed on main
