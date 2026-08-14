---
id: ER-083
title: "deletion_scenario + retraction path (member_removed/split/retired) + T-DEL-1"
milestone: M3
status: todo
kind: code
size: L
gates: full
depends_on: ["ER-032", "ER-077", "ER-082"]
spec_refs: ["s4-1-1", "s4-2", "s4-5-1", "s4-5-3", "s4-5-5", "s5", "s8-2", "s8-2-1", "s8-3", "s12-1"]
gap_refs: ["M15", "D8", "NEW-Deletion"]
provides: ["fixtures/static/deletion_scenario/resurrect/", "fixtures/static/deletion_scenario/expected/base/", "fixtures/static/deletion_scenario/expected/refresh/", "fixtures/static/deletion_scenario/expected/resurrect/", "src/er/entities/retraction.py::retract_tombstoned_records", "tests/integration/scenarios/test_deletion.py::test_deletion_retracts_edges_and_resurrection_restores_membership", "tests/unit/fixtures/test_deletion_scenario.py::test_refresh_omits_two_keys_one_of_which_is_a_bridge"]
consumes: ["src/er/entities/retraction.py::invalidate_incident_edges", "relation:raw_records", "relation:ingest_batches", "relation:int_std_records", "relation:match_scores", "relation:entity_membership", "relation:entities", "relation:entity_events", "tests/helpers/compare.py::assert_ids_stable", "tests/helpers/compare.py::assert_partition_equal", "fixtures/static/model_test_v1.json"]
owns: ["fixtures/static/deletion_scenario/resurrect/", "fixtures/static/deletion_scenario/expected/", "tests/integration/scenarios/test_deletion.py", "tests/unit/fixtures/test_deletion_scenario.py"]
protected_paths: []
extra_paths: ["fixtures/static/deletion_scenario/base/", "fixtures/static/deletion_scenario/refresh/", "src/er/entities/retraction.py", "src/er/entities/reconcile.py", "src/er/entities/affected.py", "src/er/entities/events.py", "scripts/validate_fixtures.py"]
attempts: 0
verify: "bash scripts/ci/itest.sh tests/integration/scenarios/test_deletion.py -q && uv run pytest tests/unit/fixtures/test_deletion_scenario.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

D8 puts deletion in v1 scope, which makes `member_removed`, `split`-by-removal and `entities.status='retired'` reachable states — but S4.5.5's retraction path has no end-to-end test and the `deletion_scenario` fixture has no `resurrect/` phase or expectations. This ticket completes the three-phase fixture (`base` → `refresh` → `resurrect`, S8.2.1 phase vocabulary), implements the reconcile-side retraction path over tombstoned records, and lands T-DEL-1. It closes gap M15's deletion half and the NEW-Deletion section, and is what makes the S5 status/event enums demonstrably non-vacuous.

## Scope

### In scope

- `fixtures/static/deletion_scenario/resurrect/` (an ordinary delivery re-appearing one tombstoned key) and `expected/{base,refresh,resurrect}/` in the S8.2.1 encoding
- Extending `retraction.py` with the tombstone arm: a record absent from `int_std_records` has its incident edges permanently invalidated and is removed from `entity_membership`
- Reconcile emission for the removal case: `member_removed` per departed record, `split` for the fragment the bridge removal disconnects, `retired` + `entities.status='retired'` for the entity that empties
- T-DEL-1 across the three phases, including resurrection re-entering as an ordinary new record with no special case
- A fixture unit test proving the `refresh/` delivery omits exactly two live keys and that one of them is a bridge in its persona's above-`auto_merge` edge set

### Out of scope

- Tombstone derivation, the sentinel `content_hash`, the empty-delivery guard and `resurrected_count` — all ER-032 (T-DEL-1a); this ticket consumes them
- The supersession arm and `invalidate_incident_edges` itself (ER-082)
- Fragment ranking and the 2–2 tie-break algorithm (ER-073/ER-077); this ticket asserts rank-1 keeps its `entity_id`, it does not re-implement the ordering
- Golden-record reaping for the retired entity (ER-092 owns the reap step and T-INC-2)

## Design decisions applied

Implements D8 and gap M15's deletion arm. Constraints easy to miss: (1) a tombstone is a version row with the sentinel `content_hash = '0'*64` and `payload = NULL` — the row is never deleted, so `raw_records` counts only ever grow; (2) tombstoned records are excluded from `int_std_records` entirely (S4.2), which is what makes their edge invalidation permanent rather than provisional; (3) the seed of the affected node set includes records tombstoned OR resurrected since the last successful run (S4.5.1) — without that clause the refresh phase touches nothing and the whole test passes vacuously; (4) phases run strictly `base → refresh → resurrect` and `refresh` is the only `--full-refresh-keys` delivery — reusing `refresh` for the resurrection would tombstone every key it omits, the opposite of what T-DEL-1 asserts (S8.2.1); (5) the removed bridge key is what forces a `split`; the fixture unit test machine-checks bridge-ness so a later value edit cannot silently turn the split assertion into a no-op; (6) the resurrected record must be re-scored and re-clustered by the ordinary code path — any `if resurrected:` branch in reconcile is a defect (S4.1.1).

## Acceptance criteria

- [ ] AC1: After `er ingest --full-refresh-keys` over `refresh/`, `ingest_batches.tombstone_count == 2`, exactly two `raw_records` rows carry `is_deleted=true` with `content_hash = '0'*64`, non-null `deleted_at` and NULL `payload`, and both keys are absent from `int_std_records` after `er standardize`
- [ ] AC2: Every `match_scores` row incident to either tombstoned record has `is_active=false` with `invalidated_run_id` equal to the refresh run's `run_id`, and no row was inserted or deleted (per-key row count still 1)
- [ ] AC3: The refresh run emits exactly one `member_removed` event per tombstoned record on its prior entity, exactly one `split` event for the fragment the bridge removal disconnects, and exactly one `retired` event for the entity that empties; that entity has `entities.status='retired'` and zero `entity_membership` rows
- [ ] AC4: `expected/refresh/membership.csv` and `expected/refresh/events.csv` compare equal; the rank-1 fragment of the split entity retains its `entity_id` under `assert_ids_stable`
- [ ] AC5: After the `resurrect/` delivery, `ingest_batches.resurrected_count == 1`, the key is present in `int_std_records` again, has at least one `match_scores` row with `is_active=true` carrying its current endpoint hashes, and holds exactly one `entity_membership` row; `expected/resurrect/membership.csv` compares equal
- [ ] AC6: `tests/unit/fixtures/test_deletion_scenario.py` fails if the `refresh/` delivery omits a number of live keys other than two, or if the omitted key designated as the bridge is not a cut vertex of its persona's above-`auto_merge` edge set
- [ ] AC7: The T-INV-1 autouse finalizer passes after all three phases, with tombstoned records absent from both sides of the comparison
- [ ] AC8: Re-running the `resurrect/` phase unchanged appends zero `raw_records` rows, emits zero events, and `er run-all` exits 0 with every stage reporting 10

## Tests

- tests/integration/scenarios/test_deletion.py::test_deletion_retracts_edges_and_resurrection_restores_membership
- tests/integration/scenarios/test_deletion.py::test_emptied_entity_is_retired_and_holds_no_members
- tests/unit/fixtures/test_deletion_scenario.py::test_refresh_omits_two_keys_one_of_which_is_a_bridge
- tests/unit/fixtures/test_deletion_scenario.py::test_phase_dirs_and_expected_headers_match_s8_2_1

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/scenarios/test_deletion.py -q
uv run pytest tests/unit/fixtures/test_deletion_scenario.py -q
uv run python scripts/validate_fixtures.py fixtures/static/deletion_scenario
uv run mypy --strict src/er/entities/retraction.py
```

## Definition of Done

- `fixtures/static/deletion_scenario/` carries `base/`, `refresh/`, `resurrect/` and `expected/{base,refresh,resurrect}/`, all in the S8.2.1 encoding, with `validate_fixtures.py` green
- The retraction path emits `member_removed`, `split` and `retired` from the ordinary reconcile code path — no deletion-specific branch in the clustering or mapping code
- `entities.status='retired'` and `entity_events.event_type='member_removed'` are demonstrably reached (the M3 exit criterion)
- The bridge property of the omitted key is machine-checked in a unit test, not asserted in prose
- T-INV-1 green after every phase
- Both arms of the verify command pass and both failed before the change
