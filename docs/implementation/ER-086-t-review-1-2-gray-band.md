---
id: ER-086
title: "T-REVIEW-1/2: gray-band capture, resolution-driven merge (proves assertion-delta affected-set expansion), no reopen"
milestone: M3
status: todo
kind: code
size: S
gates: full
depends_on: ["ER-060", "ER-063", "ER-069", "ER-074"]
spec_refs: ["s4-0", "s4-3-5", "s4-4", "s4-5-1", "s5", "s5-0", "s8-2", "s8-3"]
gap_refs: ["M20", "M6"]
provides: ["tests/integration/test_review_loop.py::test_gray_band_pair_lands_open", "tests/integration/test_review_loop.py::test_resolution_triggers_merge_without_new_records"]
consumes: ["relation:review_queue", "relation:assertions", "relation:entity_membership", "relation:entity_events", "src/er/review/queue.py::upsert_gray_band", "src/er/review/queue.py::resolve", "src/er/entities/ids.py::resolve", "src/er/entities/affected.py::affected_nodes", "fixtures/static/base_10/", "fixtures/static/model_test_v1.json"]
owns: ["tests/integration/test_review_loop.py"]
protected_paths: []
extra_paths: ["src/er/review/queue.py", "src/er/entities/affected.py"]
attempts: 0
verify: "bash scripts/ci/itest.sh tests/integration/test_review_loop.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

M20's whole complaint was that `review_queue` is written but never read, resolved, or connected to anything: there is no tested path from a gray-band score to a steward decision to a merge. This ticket lands T-REVIEW-1 and T-REVIEW-2 end to end on `base_10`'s single designed gray-band pair — capture with a populated waterfall, idempotent refresh, resolution writing the `always` assertion in one transaction, and a reconcile with zero new records producing the merge. That last step is the only proof that the affected-node seed really reads the assertion and review-resolution deltas (S4.5.1), which is the mechanism `split_scenario` also depends on.

## Scope

### In scope

- T-REVIEW-1: exactly one open `gray_band` pair row with a populated `waterfall`, `first_seen_run_id == last_seen_run_id`, and the pair not co-clustered
- Idempotent refresh: a second run updates `last_seen_run_id` only and inserts no duplicate
- T-REVIEW-2: `er review resolve --as match --by tester` writes the `always` assertion row, and a subsequent `er reconcile` with zero new ingest merges the pair
- No-reopen: a third run re-scores the pair in the gray band and inserts no new `review_queue` row, leaving `status='resolved_match'`

### Out of scope

- `review_queue` schema, the upsert implementation and the `er review` CLI surface (ER-063) — this ticket exercises them
- `never_unsatisfiable` escalation rows (ER-076) and `coherence` rows (ER-104)
- The gray-band pair's authoring and its verification under the committed model (ER-041, ER-060)
- Cluster-level quality after the merge — merging a cross-persona pair legitimately lowers precision and is not asserted here

## Design decisions applied

Implements gaps M20 and M6's trigger arm. Constraints easy to miss: (1) `base_10`'s gray-band pair is CROSS-PERSONA by construction (S8.2) — after T-REVIEW-2's resolution the corpus holds 9 entities and cluster-level precision is below 1.0; that is expected and must not be 'fixed', and this test must be function-isolated from ER-081's; (2) the gray band is half-open, `review_low <= p < auto_merge` (S4.3), so the pair must never be clustered before resolution; (3) resolution writes the `assertions` row in the SAME transaction as the status change (S4.3.5) — assert both or neither by re-reading after the command; (4) the merge with zero new records only happens if `er reconcile` seeds the affected set from the assertion delta AND the review-resolution delta since the last successful run (S4.5.1) — if the merge does not occur, the defect is in the seed, not in the reconciler; (5) the skip rule for already-resolved pairs is what stops a dismissed pair resurfacing every run, and is asserted here rather than assumed; (6) the review-queue open-row logical key includes `reason` (S5.0), so a later `never_unsatisfiable` row on the same pair is a distinct row and must not be counted by this test's `count(*)`.

## Acceptance criteria

- [ ] AC1: After `er run-all --mode full` on `base_10`, `select count(*) from review_queue where reason='gray_band' and status='open'` is exactly 1, with `subject_type='pair'`, `rec_a_key < rec_b_key`, `review_low <= match_probability < auto_merge`, and `first_seen_run_id = last_seen_run_id`
- [ ] AC2: That row's `waterfall` JSON contains one `gamma_*` key per configured comparison and one Bayes factor per comparison — no comparison projected away
- [ ] AC3: The pair's two records have different `entity_id`s before resolution
- [ ] AC4: A second `er run-all --mode full` leaves the row count at 1, updates `last_seen_run_id` to the second `run_id` and leaves `first_seen_run_id` unchanged
- [ ] AC5: `er review resolve --review-id <id> --as match --by tester` exits 0, sets `status='resolved_match'` with `resolved_by='tester'` and non-null `resolved_at`, and creates exactly one `assertions` row with `kind='always'`, `active=true` for the canonicalised pair
- [ ] AC6: A following `er reconcile` with zero new ingest merges the two records into one `entity_id`, emits exactly one `merged` event for that `run_id`, and `ids.resolve(loser)` returns the survivor
- [ ] AC7: A third `er run-all --mode full` inserts no `review_queue` row for that pair and leaves its `status='resolved_match'`
- [ ] AC8: `er review resolve --review-id <unknown-id>` exits 2 and writes nothing

## Tests

- tests/integration/test_review_loop.py::test_gray_band_pair_lands_open
- tests/integration/test_review_loop.py::test_second_run_refreshes_last_seen_only
- tests/integration/test_review_loop.py::test_resolution_triggers_merge_without_new_records
- tests/integration/test_review_loop.py::test_resolved_pair_does_not_reopen

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_review_loop.py -q
uv run mypy --strict src/er/review
```

## Definition of Done

- All four behaviours (capture, refresh, resolution→merge, no-reopen) are separate tests with separate node ids
- The waterfall completeness assertion enumerates the configured comparisons rather than checking the column is non-NULL
- The zero-new-record merge is asserted through the CLI (`er reconcile`), not by calling the reconciler directly — the affected-set seed is the thing under test
- T-INV-1 green after the resolution-driven merge
- `bash scripts/ci/itest.sh tests/integration/test_review_loop.py -q` passes and failed before the change
