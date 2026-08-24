---
id: ER-077
title: "split_scenario (bridge, machine-checked) + split_scenario_tie_2_2 + T-PERM-2"
milestone: M3
status: blocked
kind: fixture
size: M
gates: full
depends_on: ["ER-028", "ER-074", "ER-076"]
spec_refs: ["s8-2", "s8-2-1", "s8-3", "s4-4", "s4-4-2", "s4-5-1", "s4-5-3", "s5-0"]
gap_refs: ["B5", "M6", "M5"]
provides: ["fixtures/static/split_scenario/", "fixtures/static/split_scenario_tie_2_2/", "tests/integration/scenarios/test_split_scenario.py::test_split_retains_id_for_rank_1_fragment", "tests/unit/fixtures/test_split_scenario.py::assert_pair_is_bridge"]
consumes: ["ER-028::load_scenario", "ER-028::validate_fixtures", "tests/helpers/compare.py::assert_ids_stable", "src/er/review/never_cut.py::never_cut_fixpoint", "src/er/entities/reconcile_stage.py::run_reconcile_stage", "src/er/entities/reconcile.py::fragment_rank", "fixtures/static/model_test_v1.json"]
owns: ["fixtures/static/split_scenario/", "fixtures/static/split_scenario_tie_2_2/", "tests/integration/scenarios/test_split_scenario.py", "tests/unit/fixtures/test_split_scenario.py"]
protected_paths: []
extra_paths: []
attempts: 1
verify: "bash scripts/ci/itest.sh tests/integration/scenarios/test_split_scenario.py -q && uv run pytest tests/unit/fixtures/test_split_scenario.py -q"
branch: "ticket/ER-077-split-scenario-bridge-machine-checked-split"
commit: ""
spec_sha: "7467bdacba1bd84c"
updated_at: "2026-08-24T03:02:10Z"
session: c379438e-8c95-4be8-8f5f-3832d8ceafaa
---
## Description

Commit `split_scenario` — a `never` assertion on a **bridge** edge that severs a previously merged entity (S8.2) — with the bridge property machine-checked rather than asserted in prose, plus `split_scenario_tie_2_2` for the even fragment tie. The scenario's `assertions.csv` applies before the `batch` phase and the asserted records appear in no batch delivery, so the split is driven purely by the assertion delta of the affected node set (S4.5.1), which is what makes `never` 'applied identically in incremental and full modes' true. The integration body is T-PERM-2: fragments ranked `(member_count DESC, min member record_key ASC)`, rank 1 keeps the `entity_id` under `assert_ids_stable`, the other fragment mints a new ULID, exactly one `split` event.

## Scope

### In scope

- `fixtures/static/split_scenario/{base,batch,assertions.csv,expected/{base,batch}}` in the S8.2.1 shape
- `fixtures/static/split_scenario_tie_2_2/` producing two equal-sized fragments
- a fixture unit test that rebuilds the component from the committed base expectation and proves the asserted pair is a bridge
- the T-PERM-2 integration body over `base -> batch`

### Out of scope

- the never-cut algorithm itself (ER-076)
- the assertion fixture family and the cut/path tie fixtures (ER-078)
- T-ASSERT-1 (ER-079)
- golden expectations (deferred to M4)
- deletion-driven splits (ER-083)

## Design decisions applied

Implements B5's fragment total order and 'the asserted edge must be a bridge', M6 (assertion applied identically in both modes), M5 (rank 1 retains the id). Easy to miss: because the asserted pair **is** the only connection, the S4.4 pre-clustering edge adjustment removes it directly and **no** partition-level cut is required — this scenario must therefore write zero `cut_edges` rows and emit zero `edge_cut` events, which is exactly what distinguishes it from `assertions_scenario`'s A3 case. Also: the `batch/` delivery must contain none of the asserted records, or the scenario proves nothing about the assertion-delta arm. Node-id divergence: S8.3 lists T-PERM-2 at `tests/integration/test_permanence.py::test_split_retains_id_for_rank_1_fragment`; the board realises it at `tests/integration/scenarios/test_split_scenario.py` — keep the function name and do not duplicate it under `test_permanence.py`.

## Acceptance criteria

- [ ] AC1: `tests/unit/fixtures/test_split_scenario.py` rebuilds the base-phase component from the committed expectation and asserts the asserted pair is a bridge: removing it yields exactly two non-empty parts; adding any other edge to the fixture makes the assertion fail.
- [ ] AC2: `split_scenario/assertions.csv` has the literal S8.2.1 header, one row with `phase=batch`, `kind=never`, and `rec_a_key < rec_b_key`.
- [ ] AC3: The `batch/` CSVs contain none of the asserted records (asserted from the committed files), so the affected set is reached only through the assertion delta.
- [ ] AC4: After the batch phase the rank-1 fragment by `(member_count DESC, min record_key ASC)` retains the base-phase `entity_id` under `assert_ids_stable`, and the other fragment's `entity_id` does not appear anywhere in the base-phase membership.
- [ ] AC5: Exactly one `entity_events` row with `event_type='split'` exists for the batch `run_id`, and `expected/batch/events.csv` encodes it.
- [ ] AC6: Zero `cut_edges` rows and zero `edge_cut` events are produced by either scenario — the asserted edge is removed by the S4.4 adjustment, not by a partition-level cut.
- [ ] AC7: In `split_scenario_tie_2_2` the 4-member entity yields two 2-member fragments and the one with the smaller minimum `record_key` keeps the `entity_id`; reversing the committed expectation fails the test.
- [ ] AC8: `assert_membership_equals_components` passes after both phases of both scenarios.

## Tests

- tests/unit/fixtures/test_split_scenario.py::test_asserted_pair_is_a_bridge
- tests/unit/fixtures/test_split_scenario.py::test_batch_contains_no_asserted_record
- tests/unit/fixtures/test_split_scenario.py::test_expected_files_sorted_and_headers_literal
- tests/integration/scenarios/test_split_scenario.py::test_split_retains_id_for_rank_1_fragment
- tests/integration/scenarios/test_split_scenario.py::test_minority_fragment_gets_new_ulid_and_one_split_event
- tests/integration/scenarios/test_split_scenario.py::test_two_two_split_resolved_by_min_record_key
- tests/integration/scenarios/test_split_scenario.py::test_no_cut_edges_written

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/scenarios/test_split_scenario.py -q && uv run pytest tests/unit/fixtures/test_split_scenario.py -q
uv run python fixtures/validate_fixtures.py fixtures/static/split_scenario fixtures/static/split_scenario_tie_2_2
```

## Definition of Done

- Bridge property is machine-checked in a unit test, not documented in a comment
- `assertions.csv` phase column drawn from the S8.2.1 vocabulary
- No golden expectation committed; `expected/<phase>/golden.csv` absent for both scenarios
- T-PERM-2's function name matches S8.3
- Both scenarios pass the fixture validator and both verify commands are green

## Blocker log

### Attempt 1 — verify_failed (2026-08-24T03:02:10Z)

- **Failing command:** `bash scripts/ci/itest.sh tests/integration/scenarios/test_split_scenario.py -q`
- **Assertion / contradiction:** A BRIDGE IS NOT CONSTRUCTIBLE UNDER THE COMMITTED MODEL, so AC1 (and with it AC4-AC8) cannot be satisfied by any authored corpus. Measured, not inferred: a five-record probe corpus (fixtures/static/split_scenario/base, committed on the ticket branch at 1f24c5a) was ingested, standardized and scored at fixtures/static/model_test_v1.json, and the ENTIRE match_scores relation came back with one row -- 'billing:B301 | crm:C301 p=0.999995'. Three deliberately planted link types all scored below review_low=0.60 despite being blocked: (1) webforms:W301 vs crm:C301 and billing:B301 share given_name 'Nadia', family_name 'Okafor', street address '10 Vine Street Apt 1', postcode 94101 and birth_date 1980-01-15, differing only in phone -- blocked by name_postal ('Okaf|94101') and dob_name ('1980-01-15|Nad'), so they were scored and rejected, not missed; (2) crm:C302 vs billing:B302 share given_name 'Owen', family_name 'Petrov', address, postcode and birth_date, differing only in phone -- blocked identically, likewise rejected; (3) webforms:W301 vs crm:C302 share phone_e164 +14155550702 exactly and nothing else -- blocked by phone_exact, likewise rejected. The model therefore has NO intermediate-strength link: a pair either agrees on (given_name, family_name, phone_e164) and scores ~1.0, or it falls under review_low. That makes every component a CLIQUE, and a clique has no bridge: if a1~a2 are co-fragment they agree on those three fields, and if a2~b1 is the bridge they agree on those three fields, so a1~b1 by transitivity and the 'single connection' the ticket requires cannot exist. This is the THIRD manifestation of the same root cause -- ER-060 found base_10's gray band empty and its rob/robert nickname link below review_low, ER-074 found incremental_batch's declared bridge does not bridge despite an exact email match -- and in all three cases Splink warns 'm values not fully trained' for email, birth_date and addr_postal, which is why only given_name, family_name and phone carry weight.
- **Smallest change that would unblock:** Fix the model, not this fixture. (a) The single highest-value action is to refit or repair fixtures/static/model_test_v1.json (ER-056) so that email, birth_date and addr_postal carry trained m values -- Splink emits 'm values not fully trained' for exactly those three on every predict call. With birth_date and addr_postal contributing, a pair agreeing on given_name + family_name + address + birth_date but NOT phone would land between review_low and 1.0, which is precisely the intermediate band a bridge needs and which the probe shows is currently empty. That one change also unblocks ER-060 and removes the workaround ER-074 needed. (b) If refitting is out of scope now, the alternative is to relax this ticket: a bridge could be supplied as an  assertion over a pair the model does not link (S4.4 injects it at p=1.0 and it becomes a genuine single edge), which makes the split testable -- but AC1 says the bridge must be machine-checked from the committed base expectation, so that reading needs the ticket amended rather than the fixture bent. (c) Do NOT hand-author expected/*/membership.csv to describe a component the pipeline does not produce; that is how incremental_batch came to declare a bridge it does not have. The probe corpus and the probe test are committed at 1f24c5a on ticket/ER-077-partition... so the next attempt can re-measure in one Docker run rather than re-deriving this.
- **Log:** `.loop/logs/ER-077.attempt-1.log`
