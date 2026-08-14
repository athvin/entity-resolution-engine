---
id: ER-085
title: "Model lifecycle guard (mixed model_version/tf_snapshot_id above review_low → exit 3) + T-MODEL-1"
milestone: M3
status: todo
kind: code
size: S
gates: full
depends_on: ["ER-055", "ER-059", "ER-084"]
spec_refs: ["s4-0", "s4-3-2", "s4-3-3", "s4-4", "s4-7", "s5", "s8-2-1", "s8-3"]
gap_refs: ["M8", "M9"]
provides: ["src/er/entities/guards.py::assert_single_scoring_generation", "src/er/entities/guards.py::MixedScoringGenerationError", "tests/integration/test_model_lifecycle.py::test_retrain_full_rescore_preserves_ids"]
consumes: ["src/er/matching/current_edges.py::current_edges", "src/er/lake/model.py::register_model", "relation:model_registry", "relation:match_scores", "relation:entity_membership", "src/er/errors.py::PreconditionError", "tests/helpers/compare.py::assert_ids_stable", "fixtures/static/model_test_v1.json"]
owns: ["src/er/entities/guards.py", "tests/integration/test_model_lifecycle.py", "tests/unit/entities/test_model_guard.py"]
protected_paths: []
extra_paths: ["src/er/cli.py", "src/er/entities/reconcile.py", "tests/unit/test_no_training_in_scenarios.py"]
attempts: 0
verify: "bash scripts/ci/itest.sh tests/integration/test_model_lifecycle.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

S4.3.2's activation guard is the only thing stopping a reconcile from clustering two probability scales against one `auto_merge` threshold: activating a new `model_version` requires a full rescore before the next reconcile. Nothing implements or tests it, and S4.3.3 adds the same hazard for a `tf_snapshot_id` boundary. This ticket implements the guard as a pure predicate over the run's current edge set and lands T-MODEL-1 (retrain → full rescore → reconcile preserves ids). Closes gaps M8 and M9's lifecycle arms.

## Scope

### In scope

- `assert_single_scoring_generation(edges)` — raises a precondition error when the rows selected by `current_edges` at `match_probability >= review_low` carry more than one distinct `(model_version, tf_snapshot_id)`
- Wiring it into `er reconcile` before clustering: exit 3, `run_stages.error_class='precondition'`, no snapshot, no events, no membership write
- T-MODEL-1: allocate `v0002` by re-registering the committed fixture model under a new `model_version`, prove the guard fires, then `er match --mode full` and assert `assert_ids_stable` for every unchanged partition
- The error message naming both offending `(model_version, tf_snapshot_id)` pairs

### Out of scope

- `er train` / EM (ER-054, ER-055) — this ticket allocates a second `model_version` by registering the committed artifact, so no scenario test trains (S8.3, S12 M3)
- Minting a new `tf_snapshot_id` (`er correct`, ER-094) — the tf arm is exercised by registering a second frozen `tf_lookup` snapshot
- `current_edges` row-selection semantics (ER-059)
- T-TRAIN-1 byte reproducibility (ER-056)

## Design decisions applied

Implements gaps M8 and M9. Constraints easy to miss: (1) the guard is evaluated over the rows `current_edges` SELECTS — one current row per canonical pair — not over raw `match_scores`; evaluating it over the raw cumulative table would fail every reconcile after a correction pass, because a re-scored pair keeps its old-`tf_snapshot_id` row forever (the MERGE key includes `tf_snapshot_id`); (2) assertion-sourced edges can never confound the guard because they are never persisted to `match_scores` (S4.4) — a test asserts exactly this, so nobody 'fixes' the guard by adding a NULL-`model_version` branch; (3) the exit code is 3 and the class is `precondition` (S4.0, S4.7) — not 1, and not a warning; (4) the threshold in the guard is `review_low`, not `auto_merge`, per S4.3.2; (5) v0002 is registered from the committed `fixtures/static/model_test_v1.json` rather than trained, so the partition is genuinely unchanged and INV-PERM's set-equal clause is the assertion rather than an artefact of degenerate EM over 23 records — register this test in ER-056's 'scenarios never train' allowlist if it inspects file paths.

## Acceptance criteria

- [ ] AC1: With `match_scores` holding active rows above `review_low` under two distinct `model_version` values for the same pairs, `er reconcile` exits 3, writes `run_stages.error_class='precondition'` and `error_detail` naming both `model_version`s, emits zero `entity_events` rows and leaves `entity_membership` byte-identical
- [ ] AC2: The same holds with one `model_version` and two distinct `tf_snapshot_id` values above `review_low`
- [ ] AC3: With an active `always` assertion whose pair has no `match_scores` row, `er reconcile` exits 0 — the guard counts only persisted scored rows, and `match_scores.model_version` is NOT NULL by construction
- [ ] AC4: After `er match --mode full` at `v0002`, `current_edges` yields exactly one distinct `(model_version, tf_snapshot_id)` above `review_low` and `er reconcile` exits 0
- [ ] AC5: T-MODEL-1: after registering `v0002` and running `er match --mode full && er reconcile`, `assert_ids_stable` holds for every partition unchanged between `v0001` and `v0002`, and no ULID was minted for those entities
- [ ] AC6: `assert_single_scoring_generation` is a pure function tested over hand-built edge lists: 0 rows passes, 1 generation passes, 2 generations raises, and rows below `review_low` are ignored even when they carry a second generation
- [ ] AC7: No scenario test in this ticket invokes `er train`; `uv run pytest tests/unit/test_no_training_in_scenarios.py -q` stays green

## Tests

- tests/integration/test_model_lifecycle.py::test_mixed_model_version_above_review_low_exits_3
- tests/integration/test_model_lifecycle.py::test_mixed_tf_snapshot_above_review_low_exits_3
- tests/integration/test_model_lifecycle.py::test_assertion_edges_do_not_trip_the_guard
- tests/integration/test_model_lifecycle.py::test_retrain_full_rescore_preserves_ids
- tests/unit/entities/test_model_guard.py::test_guard_is_pure_over_edge_rows
- tests/unit/entities/test_model_guard.py::test_rows_below_review_low_are_ignored

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_model_lifecycle.py -q
uv run pytest tests/unit/entities/test_model_guard.py tests/unit/test_no_training_in_scenarios.py -q
uv run mypy --strict src/er/entities/guards.py
```

## Definition of Done

- The guard is a pure function in `src/er/entities/guards.py`, unit-tested independently of the lake, and called by `er reconcile` before clustering
- Failure path proven to commit nothing: no snapshot, no events, unchanged membership, `error_class='precondition'`, exit 3
- The tf arm and the model arm are separate tests with separate messages
- T-MODEL-1 allocates its second `model_version` by registration, not by training, and the no-training guard stays green
- `bash scripts/ci/itest.sh tests/integration/test_model_lifecycle.py -q` passes and failed before the change
