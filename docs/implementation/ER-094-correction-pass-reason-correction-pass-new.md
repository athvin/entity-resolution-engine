---
id: ER-094
title: "Correction pass (--reason correction_pass, new tf_snapshot_id, never retrains, cadence) + T-INC-1b + T-CORR-1 (supersession-driven)"
milestone: M4
status: todo
kind: code
size: L
gates: full
depends_on: ["ER-082", "ER-085", "ER-093"]
spec_refs: ["s4-0", "s4-3-3", "s4-5-3", "s4-5-5", "s4-5-6", "s5", "s5-2", "s6", "s6-1", "s8-2-1", "s8-3"]
gap_refs: ["M5", "M18", "M9", "M2"]
provides: ["src/er/matching/full.py::run_correction_pass", "src/er/matching/full.py::CORRECTION_REASON", "src/er/matching/tf.py::mint_tf_snapshot", "src/er/cli.py::correct", "fixtures/static/correction_scenario/", "fixtures/static/incremental_batch/tf_flip_pairs.csv", "tests/integration/test_correction_pass.py::test_correction_pass_links_two_pre_existing_records", "tests/integration/test_correction_pass.py::test_inc_eq_violation_diverges_then_repairs"]
consumes: ["src/er/matching/full.py", "src/er/matching/tf.py::register_tf", "src/er/matching/tf.py::materialize_tf_lookup", "src/er/obs/run_context.py::RunContext", "src/er/golden/assemble.py::assemble", "tests/helpers/compare.py::assert_partition_equal", "tests/helpers/expected.py::load_scenario", "tests/conftest.py::sub_namespace", "fixtures/static/supersession_scenario/", "fixtures/static/incremental_batch/expected/batch/membership.csv", "fixtures/static/model_test_v1.json", "relation:entity_events", "relation:runs", "relation:model_registry", "scripts/ci/itest.sh"]
owns: ["fixtures/static/correction_scenario/", "fixtures/static/incremental_batch/tf_flip_pairs.csv", "tests/integration/test_correction_pass.py"]
protected_paths: ["fixtures/static/incremental_batch/expected/", "fixtures/static/supersession_scenario/", "src/er/matching/train.py"]
extra_paths: ["src/er/cli.py", "src/er/matching/full.py", "src/er/matching/tf.py"]
attempts: 0
verify: "bash scripts/ci/itest.sh tests/integration/test_correction_pass.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

`er correct` is the CLI verb for the periodic correction pass that repairs the two INV-EQ loss vectors named in S4.5.6 — incremental candidate generation, which can never pair two pre-existing records, and corpus-dependent term frequency. This ticket implements the chain `er match --mode full --new-tf-snapshot → er reconcile → er assemble` at the ACTIVE `model_version`, stamps `runs.mode='correction_pass'`, `runs.rebuild_reason='correction_pass'` and `details.reason='correction_pass'` on every event it emits, and proves it never retrains. It lands both acceptance tests: T-CORR-1 (a link only the full pass can find, built on a supersession-bearing corpus) and T-INC-1b (violate exactly one INV-EQ precondition, assert the bounded divergence, then assert `er correct` restores partition equality).

## Scope

### In scope

- `er correct`: the three-stage chain, exit codes `0` / `3` (no active model) / `10` (no edge and no membership row changed) / `1`
- `--new-tf-snapshot`, accepted by `er match` only in `--mode full` and only from `er correct`; it mints a new `tf_snapshot_id`, re-materializes `tf_lookup` from the current corpus and re-scores the whole corpus at the active `model_version`
- `details.reason='correction_pass'` on every emitted event, plus `runs.mode` and `runs.rebuild_reason`
- A never-trains guard: no `model_registry` row is written and `model_version` is unchanged across the pass
- Surfacing `correction_pass.cadence` (validated by V15) in the stage output and log line
- `fixtures/static/correction_scenario/` with `base/` and a `batch/` supersession delivery, plus `expected/{base,batch}/`
- `fixtures/static/incremental_batch/tf_flip_pairs.csv` as the T-INC-1b divergence bound

### Out of scope

- Scheduling: v1 ships no scheduler; `cadence` is config metadata an operator's cron consumes
- Retraining, model activation and the mixed-`model_version` guard (ER-055/ER-085)
- Changing the incremental two-pass scoring path to close the gap — the gap is by design and is what this pass repairs
- A fifth expected-phase directory for the post-`er correct` state

## Design decisions applied

Closes M5 (the correction-pass policy is inline and executable: cadence, what it recomputes, the invariant it restores, `details.reason`, and that it never retrains), M18 (T-INC-1b exhibits a real precondition violation and its repair), M9 (only this pass mints a `tf_snapshot_id` outside `er train`) and M2 (the pass is a first-class run with its own `runs`/`run_stages` rows). Two constraints are easy to miss. (1) The phase vocabulary `{base, batch, refresh, resurrect}` is CLOSED (S8.2.1) and `er correct` is not a delivery, so the post-correction state is asserted inline — `expected/batch/*` describes the pre-correction state only. (2) Because the pass never trains, INV-PERM still governs: entities whose membership is unchanged keep their `entity_id` and emit NO event, so the test must assert zero events for unchanged partitions as well as the merge event for the repaired one. T-CORR-1's construction: a true persona pair (A,B) blocks together but scores in the gray band under `tf_snapshot_id_1`; later incremental deliveries — including the supersession delivery that changes an already-ingested record's `content_hash` and shifts corpus term frequency — never re-score (A,B) because neither endpoint is ever in a batch; `er correct` rebuilds TF under a new `tf_snapshot_id` and the pair crosses `auto_merge`.

## Acceptance criteria

- [ ] AC1: `bash scripts/ci/itest.sh tests/integration/test_correction_pass.py -q` exits 0.
- [ ] AC2: `er correct` on `correction_scenario` after the incremental phases writes one `runs` row with `mode='correction_pass'` and `rebuild_reason='correction_pass'`, and three `run_stages` rows (`match`, `reconcile`, `assemble`) each with a snapshot range.
- [ ] AC3: Before `er correct`, records A and B of `correction_scenario` hold distinct `entity_id`s and no active `match_scores` row for `(A,B)` at or above `auto_merge` exists under the current `tf_snapshot_id`; after `er correct` they share one `entity_id`, exactly one `merged` event exists for the run, and every event of that run carries `details.reason='correction_pass'`.
- [ ] AC4: `er correct` writes no `model_registry` row and leaves `model_registry` `status='active'` and its `model_version` byte-identical; `match_scores` rows written by the pass carry the new `tf_snapshot_id` and the unchanged `model_version`.
- [ ] AC5: Entities whose membership is unchanged across the pass keep their `entity_id` and produce zero `entity_events` rows for that `run_id` (INV-PERM asserted literally).
- [ ] AC6: T-INC-1b: re-scoring universe B under a second `tf_snapshot_id` makes `assert_partition_equal` against `expected/batch/membership.csv` fail, and every edge that crossed `auto_merge` in either direction is listed in `fixtures/static/incremental_batch/tf_flip_pairs.csv` with the matching `direction`; running `er correct` then makes `assert_partition_equal` pass again.
- [ ] AC7: `er match --mode incremental --new-tf-snapshot` is rejected with exit `2`, and a second `er correct` over an unchanged corpus exits `10` having changed no edge and no membership row.

## Tests

- tests/integration/test_correction_pass.py::test_correction_pass_links_two_pre_existing_records
- tests/integration/test_correction_pass.py::test_inc_eq_violation_diverges_then_repairs
- tests/integration/test_correction_pass.py::test_correction_pass_never_trains
- tests/integration/test_correction_pass.py::test_new_tf_snapshot_rejected_outside_full_mode

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_correction_pass.py -q
uv run mypy --strict src/er/matching src/er/cli.py
uv run pytest tests/unit/fixtures -q
```

## Definition of Done

- `er correct` implements exactly the S4.0 chain and exit-code set; `--new-tf-snapshot` is reachable from no other path.
- T-CORR-1 and T-INC-1b both green; the divergence bound file is committed in S8.2.1 header order and the fixture-format lint passes on it.
- `correction_scenario` committed with `base/`, `batch/` and `expected/{base,batch}/`; the post-correction state is asserted inline, not as a new phase directory.
- The never-trains guard is a test, not a comment: `model_registry` is asserted unchanged.
- No file under `fixtures/static/incremental_batch/expected/` or `fixtures/static/supersession_scenario/` modified.
- ruff + `mypy --strict src/er` clean; gate receipt recorded and `board.py complete` run.
