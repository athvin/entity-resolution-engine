---
id: ER-074
title: "Reconcile apply: MERGE INTO entity_membership, entities/redirects/events, CONTRADICTION-1 wired as pre-clustering hard failure"
milestone: M3
status: todo
kind: code
size: L
gates: full
depends_on: ["ER-023", "ER-027", "ER-062", "ER-072", "ER-073"]
spec_refs: ["s4-5-3", "s4-5-1", "s4-4", "s4-4-1", "s4-5-4", "s4-7", "s4-0", "s5", "s5-0", "s5-2"]
gap_refs: ["M3", "M5", "M6", "D3"]
provides: ["src/er/entities/reconcile_stage.py::run_reconcile_stage", "src/er/entities/reconcile_stage.py::apply_reconcile_plan", "relation:entity_membership", "relation:entities", "relation:entity_events", "cli:er reconcile"]
consumes: ["src/er/entities/reconcile.py::reconcile_plan", "src/er/entities/cluster.py::label_propagate", "src/er/entities/cluster.py::cluster_full", "src/er/entities/affected.py::affected_nodes", "src/er/entities/affected.py::affected_edges", "src/er/review/assertions.py::check_contradiction_1", "src/er/review/assertions.py::active_assertions", "src/er/entities/events.py::append_events", "src/er/entities/ids.py::UlidFactory", "tests/helpers/compare.py::assert_ids_stable", "ER-023::RunContext"]
owns: ["src/er/entities/reconcile_stage.py", "tests/integration/test_reconcile_apply.py", "tests/integration/test_contradiction_1.py"]
protected_paths: []
extra_paths: ["src/er/cli.py"]
attempts: 0
verify: "bash scripts/ci/itest.sh tests/integration/test_reconcile_apply.py -q && bash scripts/ci/itest.sh tests/integration/test_contradiction_1.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

Turn the pure plan into committed state: `MERGE INTO entity_membership` on `(source_system, source_record_id)` (current state, D3), upsert `entities` with `status`/`merged_into`/`updated_run_id`, and append `entity_events` under the `(run_id, entity_id, event_type, details_hash)` idempotency key with dense 1-based `seq`. The whole thing is all-or-nothing at the logical level (S4.7): membership and events commit in one snapshot only after clustering succeeds. CONTRADICTION-1 (S4.4.1) is wired as the pre-clustering hard failure — exit `1`, zero events, zero membership writes, offending `assertion_id`s and the always-closure component in `run_stages.error_detail`.

## Scope

### In scope

- the `er reconcile` stage chain: affected nodes -> affected edges -> assertion adjustment -> CONTRADICTION-1 -> clustering -> `reconcile_plan` -> apply
- `MERGE INTO entity_membership`; `entities` insert/update including `merged_into` redirects; `entity_events` append with `seq` and `details_hash`
- exit codes: `10` on an empty affected set, `1` on CONTRADICTION-1 and on non-convergence, `0` otherwise
- `run_stages` promoted columns (`entities_created/merged/split/retired`) and the S4.5.6 counters JSON
- the T-ASSERT-2 body (`test_contradiction_1_fails_the_run`)

### Out of scope

- never-cut, `cut_edges`, `edge_cut` and the `never_unsatisfiable` escalation (ER-076)
- the mixed-`model_version` exit `3` activation guard (ER-085)
- golden assembly, `er_touched_entities` and the reap step (ER-092)
- the replay fold (ER-080)
- merge/split scenario fixtures and T-PERM-1/T-PERM-2 (ER-075/ER-077)

## Design decisions applied

Implements M3/D3 (current-state membership by `MERGE INTO`, all history in `entity_events`), M5 (apply the ER-073 plan — do not re-derive the overlap mapping in SQL), M6 (CONTRADICTION-1 is a hard deterministic pre-clustering failure, never a warning). Easy to miss: (a) assertion edges are materialised in memory only and are NEVER persisted to `match_scores` (S4.4), so `model_version`/`tf_snapshot_id` stay NOT NULL there; (b) merge losers' membership rows are rewritten to the survivor **in the same snapshot**, and `merged_into` is never used to resolve current membership; (c) a failed run must leave `entity_membership` byte-identical — assert it, do not assume rollback; (d) node-id divergence: S8.3 lists T-ASSERT-2 at `tests/integration/test_assertions.py::test_contradiction_1_fails_the_run`, but the board realises it in `tests/integration/test_contradiction_1.py`; keep the **function name** identical so the S8.3 row still resolves, and do not create a second copy in `test_assertions.py`.

## Acceptance criteria

- [ ] AC1: After `er reconcile` on base_10 the `entity_membership` row count equals the `int_std_records` row count and there is exactly one row per `(source_system, source_record_id)`.
- [ ] AC2: Re-running `er reconcile` with no new records and no assertion delta exits `10`, writes zero `entity_events` rows, mints zero ULIDs, and leaves every `entity_membership.entity_id` and `assigned_at` unchanged.
- [ ] AC3: With `always(a,b)`, `always(b,c)` and `never(a,c)` active, `er reconcile` exits `1` with `run_stages.error_class='contradiction'`, `error_detail` naming all three `assertion_id`s and the always-closure component; `entity_events` gains zero rows and `entity_membership` is byte-identical to its pre-run content.
- [ ] AC4: A batch introducing a bridging edge rewrites the loser's membership rows to the survivor, sets `entities.status='merged'` with `merged_into` on the loser, and leaves zero `entity_membership` rows referencing the loser `entity_id`.
- [ ] AC5: Every `entity_events` row carries a dense 1-based `seq` per `run_id`, and applying the same plan twice under one `run_id` writes each `(run_id, entity_id, event_type, details_hash)` exactly once.
- [ ] AC6: `run_stages` for the reconcile stage has non-null `entities_created`, `entities_merged`, `entities_split`, `entities_retired` and a `counters` JSON containing `affected_entities`, `affected_edges`, `label_prop_iterations`, `clusters_out`, `members_added`, `members_removed`, `events_emitted`, plus a recorded `(snapshot_start, snapshot_end)` range (no snapshot count is asserted).
- [ ] AC7: Zero relations matching `__splink__%` exist in `lake` after the stage, and `assert_membership_equals_components` passes at the end of both integration tests.

## Tests

- tests/integration/test_reconcile_apply.py::test_membership_is_one_row_per_record
- tests/integration/test_reconcile_apply.py::test_unchanged_rerun_emits_no_events_and_exits_10
- tests/integration/test_reconcile_apply.py::test_merge_rewrites_loser_membership_in_one_snapshot
- tests/integration/test_reconcile_apply.py::test_event_seq_is_dense_and_idempotency_key_unique
- tests/integration/test_reconcile_apply.py::test_run_stages_counters_populated
- tests/integration/test_contradiction_1.py::test_contradiction_1_fails_the_run
- tests/integration/test_contradiction_1.py::test_contradiction_leaves_membership_unchanged

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_reconcile_apply.py -q && bash scripts/ci/itest.sh tests/integration/test_contradiction_1.py -q
uv run mypy --strict src/er
uv run pytest tests/unit/entities -q
```

## Definition of Done

- `er reconcile` is no longer a stub: it writes its `run_stages` row, its counters and one JSON line on stderr
- Membership is written only by `MERGE INTO`; no delete+insert, no append
- CONTRADICTION-1 runs before clustering and before any write
- The T-ASSERT-2 function name matches S8.3 so the docs lint can resolve the row
- Both itest commands green; `assert_membership_equals_components` green on every scenario the suite touches
