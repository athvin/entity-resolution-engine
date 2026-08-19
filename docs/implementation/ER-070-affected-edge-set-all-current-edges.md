---
id: ER-070
title: "Affected EDGE set: ALL current edges via current_edges, always-assertion injection (evidence.source='assertion'), cut_edges exclusion"
milestone: M3
status: done
kind: code
size: M
gates: full
depends_on: ["ER-059", "ER-069"]
spec_refs: ["s3", "s4-5-1", "s4-4", "s4-4-2", "s4-3-4", "s4-2", "s5", "s5-0", "s12-1"]
gap_refs: ["B5", "M6", "D2", "D5"]
provides: ["src/er/entities/cluster.py::Edge", "src/er/entities/cluster.py::affected_edges", "src/er/entities/cluster.py::adjust_edges_with_assertions", "src/er/entities/cluster.py::ASSERTION_EVIDENCE_SOURCE", "tests/integration/test_affected_edges.py::test_prior_run_edges_are_loaded"]
consumes: ["src/er/entities/cluster.py::affected_nodes", "src/er/entities/cluster.py::AffectedSet", "src/er/matching/edges.py::current_edges", "src/er/review/assertions.py::active_assertions", "src/er/entities/ids.py::canonicalize_pair", "src/er/config/schema.py::Config", "src/er/lake/ducklake.py::connect", "tests/conftest.py::lake_conn", "tests/helpers/scenarios.py::load_scenario", "fixture:base_10", "fixture:incremental_batch", "relation:match_scores", "relation:cut_edges", "relation:int_std_records", "relation:assertions"]
owns: ["tests/unit/entities/test_affected_edges.py", "tests/integration/test_affected_edges.py"]
protected_paths: ["tests/unit/entities/test_affected_nodes.py", "tests/integration/test_affected_nodes.py"]
extra_paths: ["src/er/entities/cluster.py"]
attempts: 3
verify: "uv run pytest tests/unit/entities/test_affected_edges.py -q && bash scripts/ci/itest.sh tests/integration/test_affected_edges.py -q"
branch: "ticket/ER-070-affected-edge-set-all-current-edges"
commit: "e0efc697b21745c1d85b7ac6af4b8b429f6a969d"
spec_sha: "7467bdacba1bd84c"
updated_at: "2026-08-19T07:30:58Z"
session: 79367156-03be-4087-b24b-ad0d8741fa21
---
## Description

S4.5.1 is explicit that for every entity in the affected set the clustering edge set is ALL currently-active edges among its members — not merely this run's scored pairs — because `match_scores` is cumulative and never truncated; the wrong reading spuriously fragments every touched entity. The same query excludes edges invalidated by S4.5.5, edges whose endpoints left `int_std_records` (tombstones), and pairs in active `cut_edges`, and the loaded result is then assertion-adjusted in memory per S4.4: minus active `never` pairs, plus active `always` pairs at `p = 1.0`. This ticket implements `affected_edges` and `adjust_edges_with_assertions` on top of ER-059's `current_edges` and ER-069's node set.

## Scope

### In scope

- `affected_edges`: the S4.5.1 predicate list — pinned `model_version` and `tf_snapshot_id`, `is_active`, `match_probability >= auto_merge`, both endpoints in the affected node set, not in active `cut_edges`, both endpoints present in `int_std_records`
- `adjust_edges_with_assertions`: in-memory adjustment applying `always` first and `never` second, injecting `always` edges at exactly `1.0` with `evidence = {"source": "assertion", "assertion_id": ...}`
- The guarantee that assertion edges are never persisted: `match_scores` is untouched by the adjustment
- Unit coverage of the adjustment as a pure function over hand-built edge lists, including the always/never ordering case constructed directly

### Out of scope

- Producing `cut_edges` rows and the never-cut fixpoint (ER-076) — this ticket only EXCLUDES rows that are already there
- Label propagation and connected components over the returned edge set (ER-071, ER-072)
- Edge invalidation (`is_active=false`) on supersession or deletion (ER-082, ER-083) — this ticket only honours the flag
- Any change to `current_edges` (ER-059) or to the affected node computation (ER-069)

## Design decisions applied

Implements gap-report B5's edge-set amendment, M6 and D5 under D2. Constraints: (1) an implementer who loads only this run's scored pairs will fragment every touched entity — the test that catches it is the one where the entity's edges were written by a PRIOR run, so that case is a required acceptance criterion, not an optional one; (2) `cut_edges` exclusion applies on EVERY subsequent run (S4.4.2) — without it every cut is silently re-merged from cumulative `match_scores` and `never` becomes a one-run no-op; (3) `always` edges enter only the in-memory set: every `match_scores` row is scored and `model_version`/`tf_snapshot_id` are NOT NULL there (S4.4), so the test asserts the `match_scores` row count and column nullability are unchanged by the adjustment; (4) ordering is fixed as always-then-never so the two orderings cannot disagree — the unit test constructs a conflicting pair directly, bypassing ER-062's write-time rejection, and asserts `never` wins; (5) `src/er/entities/cluster.py` is owned by ER-069 and extended here, per S3's assignment of the affected node+edge set to that module.

## Acceptance criteria

- [ ] AC1: An entity whose only above-`auto_merge` edges were written by a previous run is loaded intact by `affected_edges` in a later run: the edge set is non-empty and equals the edges among its members (an implementation restricted to this run's `run_id` fails this test)
- [ ] AC2: Edges below `auto_merge`, edges with `is_active=false`, and edges at a different `model_version` or `tf_snapshot_id` are all absent from the returned set
- [ ] AC3: A pair present in `cut_edges` with `active=true` is excluded; after that row is deactivated (assertion retracted), the same pair reappears in the edge set on the next call
- [ ] AC4: An edge with an endpoint absent from `int_std_records` (a tombstoned record) is excluded even though its `match_scores` row is still `is_active=true`
- [ ] AC5: An active `always` assertion between two affected nodes with no scored edge injects exactly one edge with `match_probability == 1.0` and `evidence['source'] == 'assertion'` carrying the `assertion_id`; an active `never` removes the edge between its endpoints regardless of its probability
- [ ] AC6: Before and after `adjust_edges_with_assertions`, `match_scores` row count is unchanged and no row has a NULL `model_version` or `tf_snapshot_id` — assertion edges are never persisted
- [ ] AC7: Unit: given a hand-built input containing both an `always` and a `never` for the same pair, the adjusted set omits the pair (always applied first, never second)
- [ ] AC8: Every edge returned satisfies `rec_a_key < rec_b_key` and appears at most once

## Tests

- tests/unit/entities/test_affected_edges.py::test_always_injection_and_evidence
- tests/unit/entities/test_affected_edges.py::test_never_removes_edge_after_always
- tests/unit/entities/test_affected_edges.py::test_threshold_and_canonical_ordering
- tests/integration/test_affected_edges.py::test_prior_run_edges_are_loaded
- tests/integration/test_affected_edges.py::test_cut_edges_are_excluded_and_released
- tests/integration/test_affected_edges.py::test_tombstoned_endpoint_excluded
- tests/integration/test_affected_edges.py::test_assertion_edges_are_not_persisted

## Verification

```bash
uv run pytest tests/unit/entities/test_affected_edges.py -q
bash scripts/ci/itest.sh tests/integration/test_affected_edges.py -q
uv run mypy --strict src/er/entities/cluster.py
```

## Definition of Done

- The edge loader goes through `current_edges` — no second query over `match_scores` exists in `cluster.py`
- The prior-run-edges test is written so that restricting the loader to the current `run_id` makes it fail
- `cut_edges` exclusion and its release path both have tests
- ER-069's test files are unmodified
- `bash scripts/gates.sh` green; INTERFACES entry lists `affected_edges`, `adjust_edges_with_assertions`, `Edge`
