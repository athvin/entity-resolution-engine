---
id: ER-075
title: "merge_scenario + merge_scenario_tie fixtures + T-PERM-1 (survivor rule, redirect, exactly one merge event, zero golden rows deferred to ER-092)"
milestone: M3
status: done
kind: fixture
size: M
gates: full
depends_on: ["ER-028", "ER-074"]
spec_refs: ["s8-2", "s8-2-1", "s8-3", "s4-5-3", "s4-5-4", "s5-0"]
gap_refs: ["M5", "M7", "M10", "MINOR-event_id"]
provides: ["fixtures/static/merge_scenario/", "fixtures/static/merge_scenario_tie/", "tests/integration/scenarios/test_merge_scenario.py::test_merge_preserves_survivor_id", "fixture:merge_scenario expected/{base,batch}"]
consumes: ["ER-028::load_scenario", "ER-028::validate_fixtures", "tests/helpers/compare.py::assert_ids_stable", "tests/helpers/compare.py::assert_partition_equal", "src/er/entities/ids.py::resolve", "src/er/entities/reconcile_stage.py::run_reconcile_stage", "fixtures/static/model_test_v1.json"]
owns: ["fixtures/static/merge_scenario/", "fixtures/static/merge_scenario_tie/", "tests/integration/scenarios/test_merge_scenario.py", "tests/unit/fixtures/test_merge_scenario.py"]
protected_paths: []
extra_paths: []
attempts: 1
verify: "bash scripts/ci/itest.sh tests/integration/scenarios/test_merge_scenario.py -q && uv run pytest tests/unit/fixtures/test_merge_scenario.py -q"
branch: "ticket/ER-075-merge-scenario-merge-scenario-tie-fixtures"
commit: "391587d73bdf22cd4fb3006cf61d8e83c26c1de4"
spec_sha: "7467bdacba1bd84c"
updated_at: "2026-08-27T08:03:54Z"
session: c379438e-8c95-4be8-8f5f-3832d8ceafaa
---
## Description

Commit the two merge fixtures S8.2 requires and the T-PERM-1 body. `merge_scenario` is a `base/` corpus holding two separate entities plus a `batch/` delivery whose record bridges them into one; `merge_scenario_tie` makes the two old entities overlap the new group equally so the claimant tiebreak (`min member record_key ASC`) is the only thing that can decide the survivor. The scenario test asserts the survivor rule, the `ids.resolve(loser)` redirect, exactly one `merged` event, and `assert_ids_stable` on the survivor's members (S4.5.3, S8.3). Golden-row reaping for the loser is T-INC-2's job (ER-092) and is deliberately not asserted here.

## Scope

### In scope

- `fixtures/static/merge_scenario/{base,batch,expected/{base,batch}}` in the S8.2.1 shape with literal headers
- `fixtures/static/merge_scenario_tie/` with equal overlap into one new group
- a fixture-lint unit test: sort order, `\N` null token, literal headers, canonical pair ordering, symbolic `entity_label` space
- the T-PERM-1 integration body over the `base -> batch` phase sequence

### Out of scope

- `golden.csv` expectations for either scenario (deferred to ER-092/ER-090)
- the split fixtures and T-PERM-2 (ER-077)
- full re-resolution / T-PERM-3 (ER-084)
- any change to reconcile behaviour — this ticket asserts ER-073/ER-074, it does not extend them

## Design decisions applied

Implements M5 (claimant/survivor rule), M7 (symbolic `entity_label` expectations, never a ULID), M10 (the loser's golden row is T-INC-2's assertion, so this ticket must not add a golden expectation that would be unassertable in M3), MINOR-event_id (exactly one `merged` event is only checkable because of the `(run_id, entity_id, event_type, details_hash)` idempotency key). Easy to miss: (a) `expected/<phase>/golden.csv` MUST be absent for both scenarios — an absent file means the phase makes no claim (S8.2.1), which is the honest encoding of the deferral; (b) the label map must be captured after the `base` phase and reused for the `batch` comparison, otherwise `assert_ids_stable` passes vacuously; (c) node-id divergence: S8.3 lists T-PERM-1 at `tests/integration/test_permanence.py::test_merge_preserves_survivor_id`; the board realises it at `tests/integration/scenarios/test_merge_scenario.py` — keep the function name so the S8.3 row still resolves and do not create a duplicate under `test_permanence.py`.

## Acceptance criteria

- [ ] AC1: `uv run pytest tests/unit/fixtures/test_merge_scenario.py -q` re-sorts every committed expected file byte-wise on the UTF-8 column tuple and asserts the committed order matches, and asserts `\N` is the only null token used.
- [ ] AC2: `expected/base/membership.csv` places the merging records in two distinct `entity_label`s; `expected/batch/membership.csv` places them in one; `expected/batch/events.csv` carries exactly one row with `event_type='merged'` and `count=1`.
- [ ] AC3: After the batch phase `ids.resolve(loser_entity_id)` returns the survivor `entity_id`, `entities.status='merged'` and `merged_into` is set on the loser.
- [ ] AC4: `assert_ids_stable` passes for every survivor member against the label map captured after the base phase — the survivor's `entity_id` is unchanged across the merge.
- [ ] AC5: Zero `entity_membership` rows reference the loser `entity_id` after the batch phase.
- [ ] AC6: In `merge_scenario_tie` the two old entities have exactly equal overlap with the new group and the survivor is the one with the smaller minimum member `record_key`; swapping the expectation makes the test fail.
- [ ] AC7: Neither scenario commits `expected/base/golden.csv` or `expected/batch/golden.csv`, and the scenario test reads no `golden_*` relation.
- [ ] AC8: `assert_membership_equals_components` passes at the end of both phases of both scenarios.

## Tests

- tests/unit/fixtures/test_merge_scenario.py::test_expected_files_are_sorted_and_headers_literal
- tests/unit/fixtures/test_merge_scenario.py::test_base_and_batch_labels_encode_a_merge
- tests/unit/fixtures/test_merge_scenario.py::test_tie_scenario_overlaps_are_equal
- tests/integration/scenarios/test_merge_scenario.py::test_merge_preserves_survivor_id
- tests/integration/scenarios/test_merge_scenario.py::test_loser_is_redirected_and_unreferenced
- tests/integration/scenarios/test_merge_scenario.py::test_claimant_tiebreak_selects_min_record_key_survivor

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/scenarios/test_merge_scenario.py -q && uv run pytest tests/unit/fixtures/test_merge_scenario.py -q
uv run python fixtures/validate_fixtures.py fixtures/static/merge_scenario fixtures/static/merge_scenario_tie
```

## Definition of Done

- Both fixture directories match the S8.2.1 shape and pass the fixture validator
- Every `entity_label` is symbolic (`E1`, `E2`, …), allocated by ascending minimum `record_key`
- The scenario never trains — it loads `fixtures/static/model_test_v1.json`
- T-PERM-1's function name matches S8.3 so the docs lint resolves the row
- No golden expectation is committed for either scenario; the deferral to ER-092 is stated in the fixture directory's expectation set by omission
- Both verify commands green
