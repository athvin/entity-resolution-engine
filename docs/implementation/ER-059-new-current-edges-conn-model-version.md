---
id: ER-059
title: "**NEW** current_edges(conn, model_version, tf_snapshot_id): one current row per canonical pair over cumulative match_scores, total-ordered"
milestone: M3
status: todo
kind: code
size: S
gates: full
depends_on: ["ER-058"]
spec_refs: ["s4-5-1", "s4-3-4", "s4-3-3", "s5", "s5-0", "s4-0b"]
gap_refs: ["B5", "M3", "M9"]
provides: ["src/er/matching/edges.py::current_edges_sql", "src/er/matching/edges.py::materialize_current_edges", "src/er/matching/edges.py::current_edges", "src/er/matching/edges.py::DuplicateEdgeKeyError", "src/er/matching/edges.py::CURRENT_EDGE_ORDER"]
consumes: ["src/er/matching/full.py::score_full", "src/er/matching/thresholds.py::is_auto_merge", "src/er/entities/ids.py::canonicalize_pair", "src/er/lake/ducklake.py::connect", "tests/conftest.py::lake_ns", "tests/helpers/model.py::load_fixture_model"]
owns: ["src/er/matching/edges.py", "tests/unit/matching/test_current_edges.py", "tests/integration/test_current_edges.py"]
protected_paths: []
extra_paths: []
attempts: 0
verify: "bash scripts/ci/itest.sh tests/integration/test_current_edges.py -q && uv run pytest tests/unit/matching/test_current_edges.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

Defines which row is *the* edge for a canonical pair once `match_scores` is cumulative, rescored across runs and invalidated in place (S4.3.4). S4.5.1's affected-edge query loads ALL currently-active edges among the affected nodes at the run's `model_version` and `tf_snapshot_id`; this ticket ships that selection as one reusable, total-ordered helper so the clustering, reconciliation and invariant paths cannot each invent their own reading — the same ambiguity M3 identified for membership and M15 for records, applied to edges. It is a read-only helper: it materializes into the in-memory database only (S4.0b) and commits nothing to the lake.

## Scope

### In scope

- `current_edges_sql(model_version, tf_snapshot_id, *, min_probability=None, include_inactive=False) -> str` — composable SQL selecting exactly one row per `(rec_a_key, rec_b_key)` for the given key, `is_active` by default, filtered `match_probability >= min_probability` when supplied
- `materialize_current_edges(conn, ...) -> str` — creates a temp view/table in the in-memory database and returns its name
- `current_edges(conn, ...) -> list[tuple[str, str, float]]` — the eager form for Python-side consumers
- `CURRENT_EDGE_ORDER` — the documented total order `(scored_at DESC, run_id DESC)` used to pick a row when the logical key is violated
- `DuplicateEdgeKeyError` — raised in strict mode (default) when more than one row shares a logical key, naming the pair

### Out of scope

- `cut_edges` exclusion, `never`/`always` assertion adjustment and the affected-node restriction — all ER-070's affected-edge set, which composes this helper
- Label propagation and connected components (ER-071/ER-072)
- Writing, invalidating or deleting `match_scores` rows (ER-058, ER-082)
- Choosing the run's `model_version`/`tf_snapshot_id` — callers pass them

## Design decisions applied

Closes B5's edge-set half, M3 and M9. Constraints: (1) `match_scores`'s logical key already includes `model_version` and `tf_snapshot_id`, so within one key pair there should be exactly one row — DuckLake enforces nothing (S5.0), so strict mode raises on a violation rather than silently picking one, and non-strict resolves by `CURRENT_EDGE_ORDER`; (2) invalidated rows (`is_active=false`) are excluded but never deleted — a superseded pair vanishes from the edge set with its row intact (S4.3.4/S4.5.5); (3) `min_probability` is inclusive, matching `p >= auto_merge` (S4.5.1); (4) nothing here may write to the lake or commit a snapshot; the S4.0b rule that iterations run in the in-memory database applies. Import consumed symbols from INTERFACES.md; where a path differs, INTERFACES.md wins.

## Acceptance criteria

- [ ] AC1: With `match_scores` holding rows for one canonical pair under two different `(model_version, tf_snapshot_id)` keys, `current_edges(conn, mv1, ts1)` returns exactly one row and it is the `(mv1, ts1)` row
- [ ] AC2: Setting `is_active=false` on a row removes it from `current_edges` while `select count(*) from lake.main.match_scores` is unchanged; `include_inactive=True` returns it again
- [ ] AC3: `min_probability=auto_merge` includes a row whose `match_probability` is exactly `auto_merge` and excludes one immediately below it
- [ ] AC4: Every returned row satisfies `rec_a_key < rec_b_key`; the result contains no self-pair and no duplicate pair
- [ ] AC5: With a deliberately inserted duplicate logical-key row, strict mode raises `DuplicateEdgeKeyError` naming the pair, and non-strict mode returns exactly one row — the one selected by `(scored_at DESC, run_id DESC)`
- [ ] AC6: `materialize_current_edges` creates no relation in `lake`: the set of relations in `lake.main` is identical before and after the call, and the returned name is queryable as a subquery in a composed statement
- [ ] AC7: Two consecutive calls with the same arguments return the identical row multiset (order-insensitive equality asserted on a fixed table)

## Tests

- tests/unit/matching/test_current_edges.py::test_sql_selects_one_row_per_canonical_pair
- tests/unit/matching/test_current_edges.py::test_duplicate_key_raises_in_strict_mode
- tests/unit/matching/test_current_edges.py::test_non_strict_resolves_by_total_order
- tests/unit/matching/test_current_edges.py::test_min_probability_is_inclusive
- tests/integration/test_current_edges.py::test_filters_by_model_version_and_tf_snapshot
- tests/integration/test_current_edges.py::test_excludes_invalidated_edges_without_deleting_rows
- tests/integration/test_current_edges.py::test_materializes_outside_the_lake
- tests/integration/test_current_edges.py::test_result_is_stable_across_calls

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_current_edges.py -q && uv run pytest tests/unit/matching/test_current_edges.py -q
uv run mypy --strict src/er/matching/edges.py
```

## Definition of Done

- Acceptance criteria met and the verify command passes
- `current_edges_sql` is the single definition of "the current edge for a pair"; no other module re-derives it
- The total order is a named constant with a docstring explaining that a tie means a violated logical key
- No write path, no snapshot commit, no relation created in `lake`
- `mypy --strict src/er/matching/edges.py` clean
- `provides` entries recorded in INTERFACES.md
- Committed on a branch off main
