---
id: ER-013
title: "ids.py: record_key, canonicalize_pair, IdFactory/UlidFactory/MonotonicUlidFactory/CountingIdFactory, resolve() + cycle guard"
milestone: M1
status: todo
kind: code
size: M
gates: fast
depends_on: ["ER-003"]
spec_refs: ["s5-0", "s5", "s4-5-3", "s4-5-4", "s8-4", "s12-1"]
gap_refs: ["M1", "M7", "MINOR-event_id", "D6"]
provides: ["src/er/entities/ids.py::record_key", "src/er/entities/ids.py::split_record_key", "src/er/entities/ids.py::RECORD_KEY_SEPARATOR", "src/er/entities/ids.py::canonicalize_pair", "src/er/entities/ids.py::IdFactory", "src/er/entities/ids.py::UlidFactory", "src/er/entities/ids.py::MonotonicUlidFactory", "src/er/entities/ids.py::CountingIdFactory", "src/er/entities/ids.py::resolve", "src/er/entities/ids.py::IdCycleError", "src/er/entities/ids.py::InvalidRecordKeyError"]
consumes: ["src/er/__init__.py", "pyproject.toml::python-ulid==4.0.1"]
owns: ["src/er/entities/ids.py", "tests/unit/test_ids.py"]
protected_paths: []
extra_paths: ["src/er/entities/__init__.py"]
attempts: 0
verify: "uv run pytest tests/unit/test_ids.py -q && uv run mypy --strict src/er/entities/ids.py"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

Implement the identity primitives every later stage depends on: the scalar `record_key` of S5.0 (D6), the one canonical pair-ordering helper all four pair relations write through (D9), the injectable `IdFactory` that makes reconciliation a pure, reproducible function (D10, S4.5.4), and `resolve()` with the cycle guard that S4.5.3 requires for external id resolution through `entities.merged_into`. These are the definitions M1 and M7 of the gap report found missing, and everything from `match_scores` writes to the reconciler's mint order fails silently without them. The module must stay dependency-free so the lake, matching and review packages can all import it.

## Scope

### In scope

- `record_key(source_system, source_record_id) -> str` = `source_system || ':' || source_record_id`, rejecting `':'` in `source_record_id` and empty components
- `split_record_key(key) -> tuple[str, str]`, the inverse, splitting on the first separator only
- `canonicalize_pair(a, b) -> tuple[str, str]` returning the lexically ordered pair, orientation-independent, rejecting self-pairs
- `IdFactory` Protocol with `new() -> str`; `UlidFactory` (ULID), `MonotonicUlidFactory` (strictly increasing, for `entity_events.event_id`), `CountingIdFactory` (deterministic, for unit tests)
- `resolve(entity_id, redirects, *, max_hops)` following `merged_into` chains, returning the input when it has no redirect, raising `IdCycleError` on a cycle

### Out of scope

- Writing or reading any relation; this module performs no I/O and holds no connection
- The reconciler's mint ORDER (ascending minimum member `record_key`) — that is ER-073's `ORDER BY`; this ticket only supplies the factory it calls
- `details_hash` / `content_hash` computation (ER-029, ER-068) and `seq` allocation (ER-068)
- Populating `entities.merged_into`; `resolve()` takes the redirect mapping as an argument

## Design decisions applied

Implements D6 (record identity), D9 (canonical pair ordering), D10 (all ids are ULIDs minted in Python; `reconcile()` takes an `IdFactory`) and the MINOR-`event_id` fix (monotonic ULID, ordered replay). Easy to miss: (1) canonicalisation happens in exactly ONE helper at write time (S5.0) — readers never perform a two-sided join, so no other module may re-derive ordering; (2) `entities.merged_into` resolves external ids ONLY and is never used to resolve current membership (D3/S4.5.3), so `resolve()` must not be given a membership-shaped API; (3) `MonotonicUlidFactory` must be strictly increasing even within one millisecond, because `entity_events` replay orders by `(occurred_at, seq)` and `event_id` is the relation's logical key; (4) `CountingIdFactory` ids must be lexically ordered and 26 characters so tests that sort by id behave like production; (5) the module imports nothing from `src/er` — the lake registry (ER-017), config and review packages all depend on it, and an import back into them would create a cycle.

## Acceptance criteria

- [ ] AC1: `record_key('crm','123') == 'crm:123'`; `record_key('crm','1:2')` raises `InvalidRecordKeyError` naming the `':'` ban; empty `source_system` or `source_record_id` raises; `split_record_key(record_key(s, r)) == (s, r)` for every fixture pair including ids containing `|` and non-ASCII.
- [ ] AC2: `canonicalize_pair(a, b) == canonicalize_pair(b, a)` and returns `(min, max)` lexically for 1000 randomly generated key pairs; `canonicalize_pair(a, a)` raises.
- [ ] AC3: `[MonotonicUlidFactory().new() for _ in range(10000)]` is strictly ascending lexically, every element is 26 characters, and all elements are distinct even when the clock is frozen to one millisecond.
- [ ] AC4: `CountingIdFactory()` produces the identical sequence in two separate processes, each id is 26 characters, and the sequence is lexically ascending.
- [ ] AC5: `resolve('a', {'a':'b','b':'c'})` returns `'c'`; `resolve('z', {})` returns `'z'`; `resolve('a', {'a':'b','b':'a'})` raises `IdCycleError` naming both ids and returns within `max_hops` rather than looping.
- [ ] AC6: `uv run mypy --strict src/er/entities/ids.py` exits 0 and the module's import graph contains no module under `src/er` other than itself.

## Tests

- tests/unit/test_ids.py::test_record_key_rejects_colon
- tests/unit/test_ids.py::test_record_key_round_trips_through_split
- tests/unit/test_ids.py::test_canonicalize_pair_is_orientation_invariant
- tests/unit/test_ids.py::test_canonicalize_pair_rejects_self_pair
- tests/unit/test_ids.py::test_monotonic_ulid_is_strictly_increasing_within_one_millisecond
- tests/unit/test_ids.py::test_counting_factory_is_reproducible_across_processes
- tests/unit/test_ids.py::test_resolve_follows_three_deep_chain
- tests/unit/test_ids.py::test_resolve_raises_on_cycle
- tests/unit/test_ids.py::test_ids_module_has_no_intra_package_imports

## Verification

```bash
uv run pytest tests/unit/test_ids.py -q && uv run mypy --strict src/er/entities/ids.py
uv run ruff check src/er/entities tests/unit/test_ids.py
```

## Definition of Done

- `record_key`, `split_record_key`, `canonicalize_pair`, the four factories and `resolve` implemented and exported
- Cycle guard bounded and tested; three-deep redirect chain tested
- Module is I/O-free and imports nothing else from `src/er`
- Verify command passes; `mypy --strict src/er/entities/ids.py` clean; ruff clean
