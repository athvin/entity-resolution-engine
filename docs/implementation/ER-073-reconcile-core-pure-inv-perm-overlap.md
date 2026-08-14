---
id: ER-073
title: "Reconcile core (pure): INV-PERM, overlap matrix, member_added, merge+split-at-once, fragment order by min(record_key), singleton orphan, mint order, IdFactory"
milestone: M3
status: todo
kind: code
size: L
gates: fast
depends_on: ["ER-013", "ER-068"]
spec_refs: ["s4-5-3", "s4-5-4", "s5", "s5-0", "s8-4", "s12-1"]
gap_refs: ["M5", "M7", "B5", "D3"]
provides: ["src/er/entities/reconcile.py::reconcile_plan", "src/er/entities/reconcile.py::ReconcilePlan", "src/er/entities/reconcile.py::MembershipAssignment", "src/er/entities/reconcile.py::EntityTransition", "src/er/entities/reconcile.py::PlannedEvent", "src/er/entities/reconcile.py::overlap_matrix", "src/er/entities/reconcile.py::fragment_rank"]
consumes: ["src/er/entities/ids.py::IdFactory", "src/er/entities/ids.py::CountingIdFactory", "src/er/entities/ids.py::record_key", "src/er/entities/events.py::details_hash", "ER-068::entity_events event_type vocabulary and per-type details schema"]
owns: ["src/er/entities/reconcile.py", "tests/unit/entities/test_reconcile_plan.py"]
protected_paths: []
extra_paths: []
attempts: 0
verify: "uv run pytest tests/unit/entities/test_reconcile_plan.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

The reconciler as a pure function: `reconcile_plan(p_old, p_new, id_factory)` builds the overlap matrix between the current membership partition and the clustering output and emits a plan of membership assignments, entity transitions, redirects and events per INV-PERM (S4.5.3). One mapping subsumes merge, split, extend, mint and retire and — unlike a branch list keyed on the number of prior entities per cluster — correctly covers a cluster that is simultaneously a merge of two entities and a split of a third (M5). No lake access, no clock, no ULID except through the injected `IdFactory`, so determinism (S4.5.4 D1/D2) is testable rather than asserted.

## Scope

### In scope

- set-equality fast path: a `P_new` group set-equal to a `P_old` group produces no assignment, no mint and no event
- overlap matrix, claimant rule (largest overlap, tiebroken by min member `record_key` ASC within the overlap)
- merge / split / retire / mint branches emitting `merged`, `split`, `retired`, `created`, `member_added`, `member_removed` planned events
- fragment ranking `(member_count DESC, min member record_key ASC)`; rank 1 keeps the `entity_id`
- singleton-orphan rule (a record in no new group becomes its own entity) and size-0 fragment retirement
- mint order: new groups minted in ascending order of their minimum member `record_key`, applied by explicit sort inside the function

### Out of scope

- any SQL, any `MERGE INTO`, any lake or `run_id` awareness (ER-074)
- clustering (ER-071/ER-072) and cut handling (ER-076)
- `entity_events` persistence, `seq` allocation and `occurred_at` stamping (ER-068/ER-074)
- `ids.resolve()` and the redirect cycle guard (ER-013 owns them; the plan only records `merged_into` edges)
- golden assembly / `er_touched_entities` (ER-092)

## Design decisions applied

Implements M5 (general overlap mapping replacing the four-branch list), B5 (fragment total order, singleton orphan), M7 (D1/D2, injectable `IdFactory`, explicit mint order), D3 (`entity_membership` is current state — the plan emits at most one assignment per record and `merged_into` is a redirect for external id resolution only, never a way to resolve current membership). Easy to miss: (a) the set-equality fast path must emit **absolutely nothing**, including no re-assertion of an unchanged assignment, or ER-080's 'zero events on unchanged re-run' and T-IDEM-1 both fail; (b) the entity-level tiebreaks (`created_at`, lexical ULID) are useless for fragments of one entity — the fragment order is the only total order available; (c) `member_added` is a real event type in S5's vocabulary and the S4.6 touched-set formula reads it, so a record joining an existing entity must produce one.

## Acceptance criteria

- [ ] AC1: With `P_old` and `P_new` equal as set-partitions, the returned plan has zero assignments, zero transitions and zero events, and the injected `CountingIdFactory` reports zero mints.
- [ ] AC2: When a new group overlaps two old entities by an equal number of members, the claimant is the one with the smaller minimum member `record_key` inside the overlap, and the loser gets exactly one `merged` transition with `merged_into` set to the claimant.
- [ ] AC3: Merge-and-split-at-once (a new group equal to E1 ∪ E2 ∪ a proper subset of E3) yields exactly one `merged` event for E1, one for E2, one `split` event for E3's departing fragment, and zero mints for the claimant.
- [ ] AC4: A 2–2 split of a 4-member entity gives the `entity_id` to the fragment with the smaller minimum member `record_key`, mints exactly one new id, and emits exactly one `split` event.
- [ ] AC5: An old entity holding zero members yields `status='retired'` with exactly one `retired` event; a record present in no new group yields a singleton entity with one mint and one `created` event.
- [ ] AC6: A record joining an existing entity produces exactly one `member_added` event on that entity and zero mints; a record leaving one produces exactly one `member_removed` event.
- [ ] AC7: Two calls with identical inputs and two fresh `CountingIdFactory` instances produce equal plans, and the minted ids are handed out in ascending order of each new group's minimum member `record_key` regardless of the iteration order of the input partitions.
- [ ] AC8: Every `MembershipAssignment` in the plan references an entity whose planned status is `active`; no assignment ever references a `merged` or `retired` entity.

## Tests

- tests/unit/entities/test_reconcile_plan.py::test_set_equal_partition_emits_nothing
- tests/unit/entities/test_reconcile_plan.py::test_claimant_tiebreak_by_min_record_key_in_overlap
- tests/unit/entities/test_reconcile_plan.py::test_merge_and_split_at_once
- tests/unit/entities/test_reconcile_plan.py::test_two_two_split_resolved_by_min_record_key
- tests/unit/entities/test_reconcile_plan.py::test_empty_fragment_retires_entity
- tests/unit/entities/test_reconcile_plan.py::test_record_leaving_all_clusters_becomes_singleton
- tests/unit/entities/test_reconcile_plan.py::test_member_added_and_member_removed_events
- tests/unit/entities/test_reconcile_plan.py::test_mint_order_is_ascending_min_record_key

## Verification

```bash
uv run pytest tests/unit/entities/test_reconcile_plan.py -q
uv run mypy --strict src/er/entities/reconcile.py
uv run ruff check . && uv run ruff format --check .
```

## Definition of Done

- `reconcile_plan` imports nothing from `src/er/lake` and opens no connection
- Every id in the plan comes from the injected `IdFactory`; no direct ULID call in the module
- All eight S8.4 reconciler cases have a named test
- Mint order is an explicit sort in the function, not incidental dict/scan order
- `mypy --strict` clean; verify command green
