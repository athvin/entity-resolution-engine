---
id: ER-079
title: "T-ASSERT-1: no active never pair shares an entity in either mode; stale-violation recheck against current membership; no silent outcome"
milestone: M3
status: todo
kind: code
size: M
gates: full
depends_on: ["ER-058", "ER-074", "ER-076", "ER-077", "ER-078"]
spec_refs: ["s8-3", "s4-4", "s4-4-2", "s4-5-1", "s4-5-2", "s5", "s12-1"]
gap_refs: ["M6", "D5"]
provides: ["tests/integration/scenarios/test_assertions_scenario.py::test_never_pairs_never_co_cluster", "src/er/review/never_cut.py::recheck_violations", "tests/helpers/invariants.py::assert_never_pairs_resolved"]
consumes: ["fixtures/static/assertions_scenario/", "fixtures/static/assertions_non_batch/", "fixtures/static/assertions_cut_tie/", "fixtures/static/assertions_path_tie/", "fixtures/static/assertions_two_iterations/", "src/er/review/never_cut.py::never_cut_fixpoint", "src/er/entities/reconcile_stage.py::run_reconcile_stage", "tests/helpers/compare.py::assert_partition_equal", "tests/helpers/invariants.py::assert_membership_equals_components", "src/er/matching/full.py::run_full_match"]
owns: ["tests/integration/scenarios/test_assertions_scenario.py"]
protected_paths: []
extra_paths: ["src/er/review/never_cut.py", "tests/helpers/invariants.py"]
attempts: 0
verify: "bash scripts/ci/itest.sh tests/integration/scenarios/test_assertions_scenario.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

T-ASSERT-1 (S8.3): over `assertions_scenario` in both incremental and full mode, no active `never` pair shares an `entity_id`. It adds the two behaviours the algorithm ticket cannot assert on its own — the stale-violation recheck, in which a recorded violation is re-evaluated against *current* membership before the run acts on it, and the no-silent-outcome rule, in which every active `never` pair ends a run in exactly one recorded state: not co-clustered, cut (a `cut_edges` row plus an `edge_cut` event), or escalated (a `review_queue` row with `reason='never_unsatisfiable'`). Nothing is silently ignored, which is precisely the failure mode M6 describes.

## Scope

### In scope

- the T-ASSERT-1 body run twice — `--mode incremental` and `--mode full` — in separate namespaces, with the two partitions compared
- `recheck_violations`: re-evaluate each active `never` pair against current membership before cutting or releasing
- `assert_never_pairs_resolved`: classify every active `never` pair into exactly one of {not co-clustered, cut, escalated} and assert the classes are disjoint and total
- recomputation of the shortest path and cut edge from `match_scores` inside the test, compared with the persisted `cut_edges` row
- the exclusion-rule end-to-end arm: a second full re-run does not undo an existing cut
- the `assertions_non_batch` arm proving the assertion-delta affected-set expansion in incremental mode

### Out of scope

- the cut algorithm, its total orders and its persistence schema (ER-076)
- authoring the fixtures (ER-078)
- CONTRADICTION-1 (ER-074)
- gray-band review flow / T-REVIEW-1/2 (ER-086)
- golden assertions of any kind

## Design decisions applied

Implements M6's restatement of T-ASSERT-1 as an invariant ('no active `never` pair shares an `entity_id`, in either mode') and D5's requirement that the outcome be recorded rather than silent. Easy to miss: (a) the recheck must run against *current* membership, so a violation recorded in an earlier run whose endpoints are no longer co-clustered must not trigger a fresh cut, and a pair that has newly become co-clustered must; (b) at `cut_protect_probability = 1.0` the escalation branch is a narrow residual — the escalation arm must be exercised with the config set to `auto_merge`, not by contriving a scenario at the default; (c) node-id divergence: S8.3 lists T-ASSERT-1 at `tests/integration/test_assertions.py::test_never_pairs_never_co_cluster` while the board realises it at `tests/integration/scenarios/test_assertions_scenario.py` — keep the function name so the docs lint resolves the row, and do not create a second copy.

## Acceptance criteria

- [ ] AC1: Running `assertions_scenario` through `er run-all --mode incremental` and, in a fresh sub-namespace, `--mode full`, zero active `never` pairs share an `entity_id` in either arm, and the two arms' membership partitions are equal under `assert_partition_equal`.
- [ ] AC2: `assert_never_pairs_resolved` classifies every active `never` pair into exactly one of {not co-clustered, cut, escalated}; a pair in none of the three fails the test with the pair named.
- [ ] AC3: For the A3 pair, the test independently recomputes the shortest path and the minimum-probability edge from `match_scores` under the run's `(model_version, tf_snapshot_id)` and asserts the persisted `cut_edges` row is that edge.
- [ ] AC4: A second `er match --mode full && er reconcile` after the cut leaves `cut_edges` with exactly one active row for the pair, emits no new `edge_cut` event, and keeps the endpoints in two entities.
- [ ] AC5: After retracting the `never` behind an existing cut, the next run releases the cut (`active=false`, `released_run_id` set) and re-merges the component; a violation whose endpoints ceased to be co-clustered for an unrelated reason is not re-cut and produces no new row.
- [ ] AC6: `assertions_non_batch` produces the same membership partition in incremental mode as in full mode, with neither asserted record appearing in the batch delivery.
- [ ] AC7: With `clustering.cut_protect_probability` set to `thresholds.auto_merge`, the A3 pair is escalated instead of cut: exactly one `review_queue` row with `subject_type='pair'`, `reason='never_unsatisfiable'`, `status='open'`, and zero `cut_edges` rows.
- [ ] AC8: `run_stages.counters` for the reconcile stage reports `never_applied`, `edges_cut`, `cut_iterations` and `never_unsatisfiable_escalations` equal to the row counts the test observes, and `assert_membership_equals_components` passes at the end of both mode arms.

## Tests

- tests/integration/scenarios/test_assertions_scenario.py::test_never_pairs_never_co_cluster
- tests/integration/scenarios/test_assertions_scenario.py::test_every_never_pair_has_a_recorded_outcome
- tests/integration/scenarios/test_assertions_scenario.py::test_cut_edge_matches_independently_recomputed_choice
- tests/integration/scenarios/test_assertions_scenario.py::test_second_full_run_does_not_undo_the_cut
- tests/integration/scenarios/test_assertions_scenario.py::test_stale_violation_recheck_against_current_membership
- tests/integration/scenarios/test_assertions_scenario.py::test_non_batch_assertion_matches_full_mode
- tests/integration/scenarios/test_assertions_scenario.py::test_escalation_when_every_path_is_protected

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/scenarios/test_assertions_scenario.py -q
bash scripts/ci/itest.sh tests/integration/test_never_cut_persistence.py tests/integration/test_contradiction_1.py -q
uv run mypy --strict src/er
```

## Definition of Done

- T-ASSERT-1 runs in both modes and both arms are compared, not just asserted separately
- The cut choice is verified against an independent recomputation, not against the implementation's own output
- Escalation is exercised by raising `cut_protect_probability`, and the default configuration is exercised by the cut arm
- `recheck_violations` is called before any cut or release and has a test that fails without it
- The S8.3 function name is preserved; no duplicate under `tests/integration/test_assertions.py`
- Verify command green with T-INV-1 green on every touched scenario
