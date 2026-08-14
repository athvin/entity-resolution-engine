---
id: ER-082
title: "**NEW** supersession_scenario + invalidate_incident_edges on content_hash change + affected-set widening"
milestone: M3
status: todo
kind: code
size: M
gates: full
depends_on: ["ER-028", "ER-043", "ER-069", "ER-070", "ER-074"]
spec_refs: ["s4-1", "s4-2", "s4-3-3", "s4-3-4", "s4-5-1", "s4-5-5", "s5", "s8-2", "s8-2-1", "s8-3"]
gap_refs: ["M15", "M18", "NEW-Deletion"]
provides: ["fixtures/static/supersession_scenario/", "fixtures/static/supersession_scenario/base/", "fixtures/static/supersession_scenario/batch/", "fixtures/static/supersession_scenario/expected/base/", "fixtures/static/supersession_scenario/expected/batch/", "src/er/entities/retraction.py::invalidate_incident_edges", "src/er/entities/retraction.py::stale_edge_rows", "tests/integration/scenarios/test_supersession.py::test_superseded_record_invalidates_edges_and_leaves_its_entity"]
consumes: ["relation:raw_records", "relation:int_std_records", "relation:match_scores", "relation:entity_membership", "relation:entity_events", "src/er/ingest/hashing.py::content_hash", "src/er/entities/affected.py::affected_nodes", "src/er/entities/affected.py::affected_edges", "tests/helpers/compare.py::assert_partition_equal", "tests/helpers/expected.py::load_expected", "fixtures/static/model_test_v1.json"]
owns: ["fixtures/static/supersession_scenario/", "src/er/entities/retraction.py", "tests/integration/scenarios/test_supersession.py", "tests/unit/entities/test_retraction.py"]
protected_paths: []
extra_paths: ["src/er/cli.py", "src/er/matching/full.py", "src/er/matching/incremental.py", "src/er/entities/affected.py", "tests/unit/test_package_layout.py", "scripts/validate_fixtures.py"]
attempts: 0
verify: "bash scripts/ci/itest.sh tests/integration/scenarios/test_supersession.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

S4.5.5's supersession arm — a re-delivered record whose `content_hash` changes — is exercised by nothing else in the suite, yet it is the only construction that makes INV-SCORE's endpoint-hash clause and the affected-set widening rule observable. This ticket commits the `supersession_scenario` fixture (S8.2, S8.2.1 phases `base` then `batch`) and implements `invalidate_incident_edges`, the in-place `UPDATE` that marks every `match_scores` row scored under a now-stale endpoint `content_hash` as `is_active=false`. It closes gap M15's supersession half and supplies the deterministic construction ER-094's correction pass later depends on (M18).

## Scope

### In scope

- `fixtures/static/supersession_scenario/{base,batch}/` per-source CSVs plus `expected/{base,batch}/{membership,golden,events,std_hashes}.csv` in the S8.2.1 encoding
- `src/er/entities/retraction.py::invalidate_incident_edges(conn, run_id, now)` — a pure-SQL in-place UPDATE setting `is_active=false`, `invalidated_at`, `invalidated_run_id` on every active `match_scores` row whose stored `rec_a_content_hash`/`rec_b_content_hash` differs from that endpoint's current `int_std_records.content_hash`, or whose endpoint is absent from `int_std_records`
- Wiring the call as a pre-scoring step of the `match` stage in both modes, so a re-scored pair overwrites its single row and returns to `is_active=true` (S4.3.4)
- T-SUPER-1: the end-to-end assertion over the two phases

### Out of scope

- Tombstone derivation and the `--full-refresh-keys` ingest path (ER-032) and the deletion end-to-end arm (ER-083)
- Changing the affected-NODE seed formula — ER-069 already includes the `content_hash` delta; this ticket asserts the widening, it does not re-derive it
- `golden_*` expectations for this scenario beyond the committed `expected/*/golden.csv` values (M4 asserts them)
- Adding a second `match_scores` row per logical key — invalidation is an UPDATE, never an INSERT (S4.3.4, S5.0)

## Design decisions applied

Implements gap M15 (supersession half) and D7's append-only consequence. Constraints easy to miss: (1) `match_scores`' logical key is `(rec_a_key, rec_b_key, model_version, tf_snapshot_id)` with AT MOST ONE row regardless of `is_active` — invalidation must never leave a second row, and the test asserts the count stays 1; (2) invalidation runs BEFORE scoring in the `match` stage, so the same run both invalidates the stale row and overwrites it with the new endpoint hashes at `is_active=true` — this is INV-SCORE working, not breaking (S4.3.3); (3) the affected set must widen from the superseded record to its entity's FULL membership even though no other member is in this batch (S4.5.1) — that widening is the single thing this scenario exists to prove; (4) `int_std_records` keeps exactly one current row per `(source_system, source_record_id)`, chosen by greatest `ingested_at` with `ingest_batch_id DESC` as the tie-break (S4.2) — `ASC` would let the older version win and silently pass every other assertion; (5) `src/er/entities/retraction.py` is a new module under an S3-declared package — if `tests/unit/test_package_layout.py` enforces an exact module list, extend its allowlist there rather than relocating the code.

## Acceptance criteria

- [ ] AC1: `er ingest` over `batch/` reports `new_count = 0` and `changed_count = 1`; `raw_records` afterwards holds exactly two rows for that `(source_system, source_record_id)` with two distinct `content_hash` values and no row was overwritten
- [ ] AC2: After `er standardize`, `int_std_records` holds exactly one row for that key and its `content_hash` equals the batch version's; `expected/batch/std_hashes.csv` matches
- [ ] AC3: After the batch `match` stage, every `match_scores` row incident to that record and carrying its prior `content_hash` has `is_active=false`, non-null `invalidated_at` and `invalidated_run_id` equal to this run's `run_id`; `select count(*) group by (rec_a_key, rec_b_key, model_version, tf_snapshot_id)` returns 1 for every group
- [ ] AC4: `invalidate_incident_edges` leaves untouched every row whose both stored endpoint hashes still equal the current ones — a unit test over a hand-built table asserts zero rows updated when nothing changed
- [ ] AC5: The reconcile stage's affected node set contains every member of the superseded record's prior entity, including members that appear in no `ingest_batches` row for this run
- [ ] AC6: After the batch phase `entity_events` holds exactly one `member_removed` event on the prior entity naming the departed record, and `expected/batch/membership.csv` and `expected/batch/events.csv` both match via the S8.2.1 helpers
- [ ] AC7: Re-running the batch phase unchanged updates zero `match_scores` rows, emits zero `entity_events` rows and `er reconcile` exits 10
- [ ] AC8: The T-INV-1 autouse finalizer passes after both phases — `entity_membership` equals the connected components of the post-invalidation active edge set

## Tests

- tests/integration/scenarios/test_supersession.py::test_superseded_record_invalidates_edges_and_leaves_its_entity
- tests/integration/scenarios/test_supersession.py::test_affected_set_widens_to_full_entity_membership
- tests/unit/entities/test_retraction.py::test_invalidate_only_touches_stale_endpoint_hashes
- tests/unit/entities/test_retraction.py::test_invalidate_never_inserts_a_second_row

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/scenarios/test_supersession.py -q
uv run pytest tests/unit/entities/test_retraction.py -q
uv run python scripts/validate_fixtures.py fixtures/static/supersession_scenario
uv run mypy --strict src/er/entities/retraction.py
```

## Definition of Done

- `fixtures/static/supersession_scenario/` carries `base/`, `batch/` and `expected/{base,batch}/` with S8.2.1 headers, `\N` null tokens and the S8.2.1 sort key; `validate_fixtures.py` green
- `invalidate_incident_edges` is a single UPDATE statement, typed under `mypy --strict`, and is invoked once per run at the start of the `match` stage in both modes
- The batch phase's `changed_count = 1` / `new_count = 0` manifest assertion is present, proving append-only semantics were not bypassed
- Membership and events for both phases compare equal to the committed expectations through `tests/helpers/`
- T-INV-1 green for both phases
- `bash scripts/ci/itest.sh tests/integration/scenarios/test_supersession.py -q` passes and failed before the change
