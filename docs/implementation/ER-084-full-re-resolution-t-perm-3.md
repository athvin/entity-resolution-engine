---
id: ER-084
title: "Full re-resolution + T-PERM-3 restated as INV-PERM + --reason correction_pass stamping"
milestone: M3
status: todo
kind: code
size: M
gates: full
depends_on: ["ER-058", "ER-074", "ER-077", "ER-083"]
spec_refs: ["s4-0", "s4-3-3", "s4-5-3", "s4-5-4", "s4-5-6", "s5", "s5-2", "s8-2-1", "s8-3"]
gap_refs: ["M5", "M18"]
provides: ["tests/integration/test_perm_full_reresolution.py::test_full_reresolution_satisfies_inv_perm", "src/er/entities/reconcile.py::RebuildReason", "src/er/entities/events.py::stamp_reason", "cli:er reconcile --reason", "cli:er run-all --reason"]
consumes: ["relation:runs", "relation:run_stages", "relation:entity_events", "relation:entity_membership", "relation:match_scores", "tests/helpers/compare.py::assert_ids_stable", "src/er/entities/reconcile.py::reconcile_plan", "src/er/entities/events.py::append_events", "fixtures/static/incremental_batch/", "fixtures/static/model_test_v1.json"]
owns: ["tests/integration/test_perm_full_reresolution.py", "tests/unit/entities/test_event_reason_stamp.py"]
protected_paths: []
extra_paths: ["src/er/cli.py", "src/er/entities/reconcile.py", "src/er/entities/events.py"]
attempts: 0
verify: "bash scripts/ci/itest.sh tests/integration/test_perm_full_reresolution.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

T-PERM-3 is the G2 claim that entity ids survive a full re-resolution, and S4.5.3 states it as INV-PERM — but INV-PERM has never been asserted literally anywhere: the merge and split tests assert their own branches. This ticket runs a full `er match --mode full && er reconcile` over an incrementally built corpus at the same `model_version` and `tf_snapshot_id` and asserts INV-PERM clause by clause. It also lands the `--reason` stamping mechanism S4.0 requires of `er correct` (`runs.rebuild_reason` plus `details.reason` on every emitted event), so ER-094 wires a verb onto an already-tested mechanism. Closes gaps M5 and M18's full-pass half.

## Scope

### In scope

- `--reason {std_version_bump,survivorship_version_bump,correction_pass,operator}` on `er reconcile` and `er run-all`, written to `runs.rebuild_reason` and stamped into `entity_events.details.reason` for every event the run emits
- The literal INV-PERM assertion: set-equal groups keep their `entity_id`, mint no ULID and emit no event; changed partitions carry a `merged` or `split` event in this `run_id`
- Explicit precondition assertions (same `model_version`, same `tf_snapshot_id`, same `config_hash`, same `std_version`, same active assertion set) recorded before the comparison so a failure names which one moved
- Idempotence of the full re-resolution: a second identical pass emits zero events and exits 10

### Out of scope

- `er correct` itself, the new-`tf_snapshot_id` mint and `correction_pass.cadence` (ER-094)
- INV-EQ's two-universe comparison (ER-093 owns T-INC-1)
- The mixed-`model_version` guard (ER-085)
- Retraining or allocating a new model — this pass runs at the ACTIVE model with unchanged m/u values

## Design decisions applied

Implements gap M5's INV-PERM restatement and the S4.0 stamping contract. Constraints easy to miss: (1) INV-PERM's first clause is the load-bearing one — for every group of `P_new` set-equal to a group of `P_old` the assertion is `no ULID minted AND no event emitted`, not merely `the id is the same`; assert the event count for those entity_ids in this `run_id` is 0; (2) the two arms MUST pin the same `tf_snapshot_id`, because a TF change is one of the two INV-EQ loss vectors (S4.5.6) and would make a legitimate divergence look like an INV-PERM violation; (3) events are idempotent on `(run_id, entity_id, event_type, details_hash)` (S4.5.4) — a re-run producing identical output writes zero events, and `details.reason` is part of the canonicalised details document, so stamping a reason changes `details_hash`; state that consequence in the code comment; (4) `runs.rebuild_reason` is NULL by default and a run with a non-NULL value is explicitly outside T-INC-2's accounting (S5.1) — do not stamp it on ordinary runs; (5) the test function is named `test_full_reresolution_satisfies_inv_perm` so ER-103 can resolve the S8.3 T-PERM-3 row onto it.

## Acceptance criteria

- [ ] AC1: After `er run-all --mode incremental` history over `base/` then `batch/`, a subsequent `er match --mode full && er reconcile` at the same `model_version` and `tf_snapshot_id` leaves every record whose group is set-equal between `P_old` and `P_new` with an unchanged `entity_id` under `assert_ids_stable`
- [ ] AC2: For those unchanged entities, `select count(*) from entity_events where run_id = :full_run_id and entity_id in (...)` returns 0, and `entities.updated_run_id` is not the full run's `run_id`
- [ ] AC3: Every entity whose membership group changed between `P_old` and `P_new` has at least one `merged` or `split` event with this `run_id`, and every such event's entity is in the changed set (no event for an unchanged entity)
- [ ] AC4: The test asserts equality of `model_version`, `tf_snapshot_id`, `config_hash` and `std_version` between the incremental and full arms before comparing partitions, and fails naming the field when one differs
- [ ] AC5: `er reconcile --reason correction_pass` writes `runs.rebuild_reason = 'correction_pass'` and every `entity_events` row it emits has `details ->> 'reason' = 'correction_pass'`
- [ ] AC6: `er reconcile` without `--reason` writes `runs.rebuild_reason` NULL and emits events whose `details` document contains no `reason` key
- [ ] AC7: An invalid `--reason` value exits 2 before any lake connection is opened
- [ ] AC8: A second identical `er match --mode full && er reconcile` emits zero new `entity_events` rows, rewrites zero `entity_membership` rows and `er reconcile` exits 10

## Tests

- tests/integration/test_perm_full_reresolution.py::test_full_reresolution_satisfies_inv_perm
- tests/integration/test_perm_full_reresolution.py::test_second_full_pass_is_a_no_op
- tests/integration/test_perm_full_reresolution.py::test_reason_stamped_on_runs_and_events
- tests/unit/entities/test_event_reason_stamp.py::test_reason_is_part_of_details_hash
- tests/unit/entities/test_event_reason_stamp.py::test_unknown_reason_rejected

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_perm_full_reresolution.py -q
uv run pytest tests/unit/entities/test_event_reason_stamp.py -q
uv run mypy --strict src/er
```

## Definition of Done

- INV-PERM is asserted literally in three clauses (id retained, no ULID minted, no event emitted) plus the changed-partition converse
- The INV-EQ-adjacent preconditions are asserted explicitly before the comparison, with failure messages naming the field
- `--reason` is accepted by `er reconcile` and `er run-all`, validated against the four `runs.rebuild_reason` values, and threaded into every event's `details`
- `details.reason` participates in `details_hash`, documented in a code comment
- T-INV-1 green after the full pass
- `bash scripts/ci/itest.sh tests/integration/test_perm_full_reresolution.py -q` passes and failed before the change
