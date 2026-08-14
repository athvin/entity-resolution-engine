---
id: ER-078
title: "assertions_scenario family: A1–A4, assertions_contradiction, assertions_non_batch, assertions_cut_tie, assertions_path_tie, assertions_two_iterations"
milestone: M3
status: todo
kind: fixture
size: L
gates: fast
depends_on: ["ER-028", "ER-063", "ER-076"]
spec_refs: ["s8-2", "s8-2-1", "s4-3", "s4-4", "s4-4-1", "s4-4-2", "s5-0", "s6", "s12-1"]
gap_refs: ["M6", "B5", "M20", "D5"]
provides: ["fixtures/static/assertions_scenario/", "fixtures/static/assertions_contradiction/", "fixtures/static/assertions_non_batch/", "fixtures/static/assertions_cut_tie/", "fixtures/static/assertions_path_tie/", "fixtures/static/assertions_two_iterations/", "tests/unit/fixtures/test_assertions_scenario.py::agreement_pattern"]
consumes: ["ER-028::load_scenario", "ER-028::validate_fixtures", "src/er/config/schema.py::SourcesConfig", "src/er/entities/ids.py::record_key", "src/er/entities/ids.py::canonicalize_pair", "src/er/review/queue.py::upsert_review", "src/er/review/never_cut.py::never_cut_fixpoint"]
owns: ["fixtures/static/assertions_scenario/", "fixtures/static/assertions_contradiction/", "fixtures/static/assertions_non_batch/", "fixtures/static/assertions_cut_tie/", "fixtures/static/assertions_path_tie/", "fixtures/static/assertions_two_iterations/", "tests/unit/fixtures/test_assertions_scenario.py"]
protected_paths: []
extra_paths: []
attempts: 0
verify: "uv run pytest tests/unit/fixtures/test_assertions_scenario.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

Author the fixture family that makes D5 and the assertion lifecycle testable end to end (S8.2, S4.4.2). `assertions_scenario` carries four cases: A1 an `always` pair scoring below `review_low`; A2 a `never` pair above `auto_merge` with no alternative path; A3 a `never` pair whose endpoints stay connected through a third record, which is the case that forces a partition-level cut; A4 a retracted assertion that must have no effect. Five sibling scenarios cover the branches nothing else reaches: `assertions_contradiction` (always/always/never), `assertions_non_batch` (a `never` on records in no batch), `assertions_cut_tie`, `assertions_path_tie` and `assertions_two_iterations`. Score-dependent design properties are made checkable without scoring by constructing tied edges whose comparison inputs agree field for field, so equality of `match_probability` follows from INV-SCORE rather than from hope.

## Scope

### In scope

- six scenario directories in the S8.2.1 shape, each with `assertions.csv` and the expected files the phase actually claims
- a pure unit test that derives each designed property from the committed CSVs: agreement patterns, hop counts of the designed paths, expected `edge_cut` counts, canonical pair ordering
- the `expected/<phase>/assertions.csv` state (including A4's `active=false`)
- cross-scenario lint: sorted files, `\N` null token, literal headers, symbolic labels

### Out of scope

- running the pipeline over these fixtures (ER-079)
- the cut algorithm and its persistence (ER-076)
- the CONTRADICTION-1 assertion body (ER-074 owns `tests/integration/test_contradiction_1.py`; this ticket only supplies its input fixture)
- gray-band review fixtures (base_10 owns the single gray-band pair)
- any new scenario-root file type — S8.2.1 enumerates exactly `assertions.csv`, `parity_pairs.csv`, `tf_flip_pairs.csv`

## Design decisions applied

Implements M6 (assertion lifecycle, precedence, contradiction), D5 (the three tie/iteration cases D5's mandate specifically requires), B5 (the non-batch assertion that proves the affected-set delta), M20 (`never_unsatisfiable` escalation needs a fixture that produces it). Easy to miss: (a) a designed tie in `match_probability` cannot be asserted by a unit test that does not score — construct the two candidate edges so their endpoint pairs have identical field-level agreement patterns under `sources.<name>.columns`, and check *that*, which makes equal probability a consequence of INV-SCORE; (b) `assertions_contradiction` must ship **no** `expected/batch/` directory, because the run fails and therefore makes no post-state claim (S8.2.1: an absent file/phase is an absent claim); (c) `never` dominates `always` for the same pair and a conflicting insert is rejected at write time — no scenario may commit both kinds for one pair.

## Acceptance criteria

- [ ] AC1: All six scenario directories exist in the S8.2.1 shape and pass `validate_fixtures.py`: every expected file byte-sorted on its full column tuple, `\N` the only null token, headers literal, `entity_label` symbolic.
- [ ] AC2: `assertions_scenario/assertions.csv` holds exactly the four rows A1–A4 with the literal header, canonical pair order and phases drawn from `{base, batch}`; `expected/batch/assertions.csv` records A4 with `active=false`.
- [ ] AC3: A1's two records are in two distinct `entity_label`s in `expected/base/membership.csv` and one in `expected/batch/membership.csv`, and they share no email and no phone in the committed inputs (so nothing but the `always` edge can join them).
- [ ] AC4: A2's two records are in two distinct `entity_label`s in `expected/batch/membership.csv` and `expected/batch/events.csv` carries no `edge_cut` row.
- [ ] AC5: A3's `expected/batch/events.csv` carries exactly one `edge_cut` row, and the third record remains co-clustered with exactly one endpoint of the never pair.
- [ ] AC6: In `assertions_cut_tie` the two candidate path edges' endpoint pairs have identical field-level agreement patterns under `sources.<name>.columns` (computed from the committed CSVs), and the expectation encodes the winner selected by `(rec_a_key ASC, rec_b_key ASC)`.
- [ ] AC7: In `assertions_path_tie` exactly two paths of equal minimum hop count connect the never pair over the designed edge set, and in `assertions_two_iterations` the summed `edge_cut` count in `expected/batch/events.csv` is exactly 2.
- [ ] AC8: `assertions_contradiction/assertions.csv` encodes `always(a,b)`, `always(b,c)`, `never(a,c)` and the scenario ships no `expected/batch/` directory; `assertions_non_batch/batch/` contains none of its asserted records.

## Tests

- tests/unit/fixtures/test_assertions_scenario.py::test_all_scenarios_pass_the_fixture_validator
- tests/unit/fixtures/test_assertions_scenario.py::test_a1_a4_rows_and_phases
- tests/unit/fixtures/test_assertions_scenario.py::test_a3_expects_exactly_one_edge_cut
- tests/unit/fixtures/test_assertions_scenario.py::test_cut_tie_edges_have_identical_agreement_patterns
- tests/unit/fixtures/test_assertions_scenario.py::test_path_tie_has_two_equal_hop_paths
- tests/unit/fixtures/test_assertions_scenario.py::test_two_iterations_expects_two_edge_cuts
- tests/unit/fixtures/test_assertions_scenario.py::test_contradiction_scenario_makes_no_post_state_claim
- tests/unit/fixtures/test_assertions_scenario.py::test_non_batch_scenario_batch_excludes_asserted_records

## Verification

```bash
uv run pytest tests/unit/fixtures/test_assertions_scenario.py -q
uv run python fixtures/validate_fixtures.py fixtures/static/assertions_scenario fixtures/static/assertions_contradiction fixtures/static/assertions_non_batch fixtures/static/assertions_cut_tie fixtures/static/assertions_path_tie fixtures/static/assertions_two_iterations
```

## Definition of Done

- Six scenario directories committed, each with only the S8.2.1-sanctioned files
- Every designed property is derived from the committed CSVs by the unit test, not documented in prose
- No scenario commits both an `always` and a `never` row for the same pair
- `assertions_contradiction` claims no post-batch state
- Verify command green and the fixture validator clean over all six
