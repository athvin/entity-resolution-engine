---
id: ER-080
title: "Event replay: replay_membership reproduces entity_membership; zero events on unchanged re-run"
milestone: M3
status: done
kind: code
size: S
gates: full
depends_on: ["ER-065", "ER-068", "ER-074"]
spec_refs: ["s4-5-3", "s4-5-4", "s4", "s5", "s5-0", "s8-4", "s8-1"]
gap_refs: ["M3", "MINOR-event_id", "D3"]
provides: ["src/er/entities/events.py::replay_membership", "src/er/entities/events.py::EVENT_FOLD_VOCABULARY", "tests/helpers/invariants.py::assert_replay_reproduces_membership"]
consumes: ["src/er/entities/events.py::append_events", "src/er/entities/reconcile_stage.py::run_reconcile_stage", "src/er/matching/incremental.py::run_incremental_match", "relation:entity_events", "relation:entity_membership", "ER-023::RunContext", "ER-023::run_stages snapshot range accessor"]
owns: ["tests/integration/test_event_replay.py", "tests/unit/entities/test_replay.py"]
protected_paths: []
extra_paths: ["src/er/entities/events.py", "tests/helpers/invariants.py"]
attempts: 1
verify: "bash scripts/ci/itest.sh tests/integration/test_event_replay.py -q"
branch: "ticket/ER-080-event-replay-replay-membership-reproduces-entity"
commit: "b2ebc38ab1e3c1dd9795a8199f62550b5edc6d13"
spec_sha: "7467bdacba1bd84c"
updated_at: "2026-08-27T13:24:32Z"
session: c379438e-8c95-4be8-8f5f-3832d8ceafaa
---
## Description

Make S3's promised 'append + replay' checkable: `replay_membership` folds `entity_events` in `(occurred_at, seq)` order and must reproduce `entity_membership` exactly, which is the only executable statement of what D3's current-state table means relative to its log (M3, MINOR-event_id). The same ticket asserts the other half of the event contract — a re-run over an unchanged edge set and unchanged `P_old` writes zero events and mints nothing (S4 preamble, S4.5.4) — and proves the fold reconstructs a past state by replaying up to a run boundary and comparing against `entity_membership` read through DuckLake time travel at that run's recorded snapshot.

## Scope

### In scope

- `replay_membership(events)` folding `created`, `member_added`, `member_removed`, `merged`, `split`, `retired`; `edge_cut` changes no membership and is skipped explicitly
- an unknown `event_type` raises rather than being ignored
- `assert_replay_reproduces_membership` helper usable from any scenario test
- `seq` density/uniqueness assertions and the `(run_id, entity_id, event_type, details_hash)` idempotency assertion
- the zero-events-on-unchanged-re-run arm
- point-in-time replay compared against a time-travelled read at a `run_stages`-sourced snapshot version

### Out of scope

- writing events (ER-068/ER-074)
- the touched-set formula over events (ER-092)
- `ids.resolve()` redirect chains (ER-013)
- any new event type or details payload

## Design decisions applied

Implements M3's 'make append+replay checkable by stating the fold order `(occurred_at, seq)` and adding a test that replay reproduces `entity_membership`', MINOR-event_id (idempotency key, `seq`), D3 (`merged_into` is never consulted by the fold — a merged entity's members appear under the survivor because `merged` carries them, not because a redirect is followed). Easy to miss: (a) the fold must sort explicitly and never rely on physical row order; (b) the point-in-time arm must capture the snapshot version at runtime from `run_stages.snapshot_end` for a named `(run_id, stage)` — no absolute version may appear in the test (S8.1); (c) `edge_cut` is in the event vocabulary and in the touched-set formula but is membership-neutral, so skipping it must be deliberate and tested, not incidental.

## Acceptance criteria

- [ ] AC1: For every scenario the test exercises (base_10, merge_scenario, split_scenario, assertions_scenario), folding all `entity_events` in `(occurred_at, seq)` order yields a `(source_system, source_record_id) -> entity_id` mapping equal to `entity_membership` in both directions.
- [ ] AC2: `replay_membership` raises on an `event_type` outside `EVENT_FOLD_VOCABULARY`, and folds an `edge_cut` event without changing the mapping.
- [ ] AC3: Every `run_id` in the namespace has dense 1-based `seq` values in `entity_events`, and `(run_id, entity_id, event_type, details_hash)` is unique across the relation.
- [ ] AC4: Re-running `er reconcile` over an unchanged edge set and unchanged `P_old` adds zero `entity_events` rows, mints zero ULIDs and exits `10`, while `entity_membership` stays byte-identical.
- [ ] AC5: Replaying only the events with `occurred_at`/`seq` up to and including run N reproduces the `entity_membership` content read at `run_stages.snapshot_end` for run N's reconcile stage via `AT (VERSION => :snap)`; the snapshot version is captured at runtime and no absolute version appears in the test source.
- [ ] AC6: Feeding the fold a deliberately shuffled event list produces the same mapping, proving the sort is inside the function.

## Tests

- tests/unit/entities/test_replay.py::test_fold_is_sorted_internally
- tests/unit/entities/test_replay.py::test_unknown_event_type_raises
- tests/unit/entities/test_replay.py::test_edge_cut_is_membership_neutral
- tests/integration/test_event_replay.py::test_replay_reproduces_membership_on_every_scenario
- tests/integration/test_event_replay.py::test_seq_dense_and_idempotency_key_unique
- tests/integration/test_event_replay.py::test_unchanged_rerun_emits_zero_events
- tests/integration/test_event_replay.py::test_point_in_time_replay_matches_time_travelled_membership

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_event_replay.py -q
uv run pytest tests/unit/entities/test_replay.py -q
uv run mypy --strict src/er/entities/events.py
```

## Definition of Done

- `replay_membership` lives in `src/er/entities/events.py` (the S3-declared home of the replay fold), not in a new module
- The fold sorts by `(occurred_at, seq)` internally and consults no `merged_into` redirect
- The point-in-time test captures its snapshot version from `run_stages` at runtime
- Zero-events-on-unchanged-re-run is asserted with an exit-code check as well as a row count
- Verify command green and the collection guard against snapshot literals still passes
