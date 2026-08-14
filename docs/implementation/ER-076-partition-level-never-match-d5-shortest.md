---
id: ER-076
title: "Partition-level never_match (D5): shortest-path BFS total order, min-probability cut, cut_protect_probability, cut_edges persistence + affected.py/invariants.py updates, edge_cut event, escalation to review_queue, bounded fixpoint, collateral bound"
milestone: M3
status: todo
kind: code
size: L
gates: full
depends_on: ["ER-063", "ER-068", "ER-070", "ER-071", "ER-072", "ER-074"]
spec_refs: ["s4-4", "s4-4-1", "s4-4-2", "s4-5-1", "s4-5-2", "s4-7", "s5", "s5-0", "s6", "s8-4", "s12-1"]
gap_refs: ["M6", "D5"]
provides: ["src/er/review/never_cut.py::shortest_path", "src/er/review/never_cut.py::choose_cut_edge", "src/er/review/never_cut.py::never_cut_fixpoint", "src/er/review/never_cut.py::persist_cuts", "src/er/review/never_cut.py::release_cuts", "src/er/review/never_cut.py::CutResult", "relation:cut_edges", "event:edge_cut", "review_reason:never_unsatisfiable"]
consumes: ["src/er/entities/cluster.py::label_propagate", "src/er/entities/affected.py::affected_edges", "src/er/entities/reconcile_stage.py::run_reconcile_stage", "src/er/review/assertions.py::active_assertions", "src/er/review/queue.py::upsert_review", "src/er/entities/events.py::append_events", "src/er/entities/ids.py::canonicalize_pair", "src/er/config/schema.py::ClusteringConfig", "tests/helpers/invariants.py::assert_membership_equals_components"]
owns: ["src/er/review/never_cut.py", "tests/unit/review/test_never_cut.py", "tests/integration/test_never_cut_persistence.py"]
protected_paths: []
extra_paths: ["src/er/entities/affected.py", "src/er/entities/reconcile_stage.py", "tests/helpers/invariants.py"]
attempts: 0
verify: "uv run pytest tests/unit/review/test_never_cut.py -q && bash scripts/ci/itest.sh tests/integration/test_never_cut_persistence.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

Implement D5 as specified in S4.4.2: after clustering, every active `never` pair whose endpoints are co-clustered gets the shortest path between them (total order `hop_count ASC`, then lexically smallest vertex sequence), the minimum-probability unprotected edge on it is cut (total order `match_probability ASC, rec_a_key ASC, rec_b_key ASC`), the cut is persisted to `cut_edges`, an `edge_cut` event is emitted, components are recomputed, and the loop repeats to a fixpoint bounded by `clustering.max_iterations`. Edges at or above `clustering.cut_protect_probability` (default `1.0`) are protected; when every path is fully protected the pair escalates to a `review_queue` row with `reason='never_unsatisfiable'` instead of being cut. This ticket also carries the two edits without which the feature is a one-run no-op: `affected.py` must exclude active `cut_edges` from every subsequent clustering run, and the T-INV-1 helper must recompute components over the post-cut edge set.

## Scope

### In scope

- pure algorithm: `shortest_path`, `choose_cut_edge`, `never_cut_fixpoint` with both total orders and the protection rule
- escalation write: `review_queue` row `subject_type='pair'`, `reason='never_unsatisfiable'`, `status='open'`
- persistence: `cut_edges` rows (`cut_id` ULID, canonical pair, probability at cut time, `model_version`, `tf_snapshot_id`, `assertion_id`, `active`, `cut_run_id`, `cut_at`) and release (`released_run_id`, `released_at`, `active=false`) on assertion retraction or endpoint `content_hash` change
- `edge_cut` event on the affected entity; `edges_cut`, `cut_iterations`, `never_unsatisfiable_escalations` counters
- bounded fixpoint with a hard `non_convergence` failure: no `cut_edges` row, no event, no membership write
- edits to `src/er/entities/affected.py` (cut exclusion) and `tests/helpers/invariants.py` (post-cut edge set)
- collateral bound: the only entities whose membership changes are those in the component containing the never pair, asserted per run

### Out of scope

- CONTRADICTION-1 (ER-074) — the always-closure check has already failed the run before this code is reached
- assertion CRUD and precedence (ER-062); `review_queue` upsert mechanics (ER-063)
- the assertion fixture family (ER-078) and the T-ASSERT-1 invariant test (ER-079)
- the split fixtures (ER-077)
- any change to the assertion edge adjustment itself: `never` pairs are already removed from the edge set pre-clustering by S4.4; this ticket cuts *paths*, not the asserted edge

## Design decisions applied

Implements D5/M6 partition-level `never`. The escalation condition as written in D5 is degenerate (the clustering edge set is by construction `p >= auto_merge`), which S4.4.2 resolves with `clustering.cut_protect_probability` defaulting to `1.0` so ordinary edges are cuttable and only assertion-sourced edges are protected; setting it to `auto_merge` yields the strict reading in which nothing is cuttable — both configurations MUST work and both are tested here. Easy to miss and load-bearing: (1) without the `affected.py` exclusion every cut is silently re-merged from cumulative `match_scores` on the next run and `never` has a one-run half-life; (2) without the `invariants.py` update T-INV-1 fails on every scenario containing a cut, because membership will legitimately differ from the pre-cut components; (3) the run id for a cut goes in `cut_run_id`, not `run_id` (S5); (4) cutting is monotone, so exceeding `max_iterations` means a protected-edge cycle or a bug — fail, never warn.

## Acceptance criteria

- [ ] AC1: On the graph a–b–c with `never(a,c)`, edges `(a,b)=0.97` and `(b,c)=0.96`, `never_cut_fixpoint` cuts exactly `(b,c)` and returns a partition placing a and c in different components.
- [ ] AC2: Two path edges with equal `match_probability` are broken by `(rec_a_key ASC, rec_b_key ASC)`; the test names the edge that must be cut and fails if the other is chosen.
- [ ] AC3: Two shortest paths of equal `hop_count` resolve to the lexically smallest vertex sequence; the chosen path is asserted explicitly.
- [ ] AC4: With `cut_protect_probability` set to `auto_merge` and every path edge at or above it, no cut is made and exactly one `review_queue` row appears with `subject_type='pair'`, `reason='never_unsatisfiable'`, `status='open'`; with the `1.0` default the same graph is cut instead.
- [ ] AC5: A graph requiring more rounds than `clustering.max_iterations` raises the `non_convergence` error and leaves zero `cut_edges` rows, zero `edge_cut` events and an unchanged `entity_membership`.
- [ ] AC6: After a run that cuts, `cut_edges` holds exactly one `active` row for the pair with `rec_a_key < rec_b_key`, the probability at cut time, the satisfying `assertion_id` and `cut_run_id` set; a second full re-run over the unchanged corpus writes no second cut row, emits no second `edge_cut` event, and does not re-merge the pair.
- [ ] AC7: Retracting the `never` assertion sets `active=false`, `released_run_id` and `released_at` on the cut row, and the next run re-merges the component and emits no `edge_cut`.
- [ ] AC8: `assert_membership_equals_components` passes on a scenario containing a cut, i.e. the helper computes components over the current edge set minus active `cut_edges`.

## Tests

- tests/unit/review/test_never_cut.py::test_cuts_minimum_probability_edge_on_shortest_path
- tests/unit/review/test_never_cut.py::test_cut_choice_tie_resolved_by_pair_keys
- tests/unit/review/test_never_cut.py::test_path_tie_resolved_by_lexically_smallest_vertex_sequence
- tests/unit/review/test_never_cut.py::test_fully_protected_path_escalates_instead_of_cutting
- tests/unit/review/test_never_cut.py::test_exceeding_max_iterations_raises_non_convergence
- tests/unit/review/test_never_cut.py::test_two_two_split_induced_by_cut
- tests/integration/test_never_cut_persistence.py::test_cut_row_persists_and_is_excluded_next_run
- tests/integration/test_never_cut_persistence.py::test_retraction_releases_cut_and_remerges
- tests/integration/test_never_cut_persistence.py::test_invariant_holds_against_post_cut_edge_set

## Verification

```bash
uv run pytest tests/unit/review/test_never_cut.py -q && bash scripts/ci/itest.sh tests/integration/test_never_cut_persistence.py -q
bash scripts/ci/itest.sh tests/integration/test_reconcile_apply.py tests/integration/test_clustering_parity.py -q
uv run mypy --strict src/er
```

## Definition of Done

- `src/er/entities/affected.py` excludes active `cut_edges` from the clustering edge set and a test proves a cut survives the next run
- `tests/helpers/invariants.py` computes components over the post-cut edge set; T-INV-1 green on a cut scenario
- Both `cut_protect_probability` configurations (1.0 default and `auto_merge`) are exercised
- `cut_edges` writes use `cut_run_id`; canonical pair ordering enforced by the shared helper
- Counters `edges_cut`, `cut_iterations`, `never_unsatisfiable_escalations` land in `run_stages.counters`
- Both verify commands green and no previously green integration test regresses
