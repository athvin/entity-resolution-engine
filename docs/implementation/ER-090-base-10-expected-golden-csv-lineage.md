---
id: ER-090
title: "base_10/expected/golden.csv + lineage.csv + machine-checked rule-coverage table"
milestone: M4
status: todo
kind: fixture
size: M
gates: fast
depends_on: ["ER-027", "ER-041", "ER-088"]
spec_refs: ["s4-6", "s5", "s6", "s8-2", "s8-2-1", "s8-3"]
gap_refs: ["B4", "M11", "M7"]
provides: ["fixtures/static/base_10/expected/base/golden.csv", "fixtures/static/base_10/expected/base/lineage.csv", "tests/helpers/expected.py::load_lineage", "tests/unit/fixtures/test_base_10_golden.py::test_rule_coverage_is_exactly_six_tokens"]
consumes: ["fixtures/static/base_10/base/", "fixtures/static/base_10/truth.csv", "fixtures/static/base_10/expected/base/membership.csv", "tests/helpers/expected.py::load_expected", "tests/helpers/compare.py::assert_golden_equal", "src/er/lake/columns.py::GOLDEN_SURVIVABLE_COLUMNS", "src/er/lake/columns.py::GOLDEN_LINEAGE_ATTRIBUTES", "src/er/config/loader.py::load_config"]
owns: ["fixtures/static/base_10/expected/base/golden.csv", "fixtures/static/base_10/expected/base/lineage.csv", "tests/unit/fixtures/test_base_10_golden.py"]
protected_paths: []
extra_paths: ["fixtures/static/base_10/base/", "fixtures/static/FORMAT.md", "tests/helpers/expected.py", "scripts/validate_fixtures.py", "tests/unit/fixtures/test_fixture_format.py"]
attempts: 0
verify: "uv run pytest tests/unit/fixtures/test_base_10_golden.py -q"
branch: ""
commit: ""
spec_sha: "7467bdacba1bd84c"
updated_at: "2026-09-10T06:13:23Z"
session: c379438e-8c95-4be8-8f5f-3832d8ceafaa
---
## Description

T-GOLD-1 (ER-091) asserts golden values AND that every `(entity_label, attribute)` carries the expected `golden_lineage.rule`, covering all five rules plus `tiebreak_deterministic` — but no expectation file for either exists. This ticket authors and commits `base_10`'s `expected/base/golden.csv` and a new `expected/base/lineage.csv`, and machine-checks them against the committed inputs with a pure-Python survivorship oracle so that a fixture edit which silently drops a rule from coverage fails at the unit layer rather than as a confusing integration diff. Closes gaps B4, M11 and M7 at the fixture level.

## Scope

### In scope

- `fixtures/static/base_10/expected/base/golden.csv` in the S8.2.1 header and encoding: symbolic `entity_label`, `\N` nulls, S8.2.1 sort key, `assembled_at` excluded
- `fixtures/static/base_10/expected/base/lineage.csv` — a sixth expected file with header `entity_label,attribute,record_key,source_system,source_record_id,rule,survivorship_version`, following the same encoding rules
- A pure-Python survivorship oracle over the committed CSVs and `configs/test.yaml` chains that reproduces both the winner and the deciding rule for all 60 lineage rows
- The machine-checked rule-coverage assertion: the set of `rule` values is exactly the six-token vocabulary
- Value-level edits to `base_10`'s input CSVs, if and only if a rule token is otherwise unreachable, preserving the 23/10/18 truth counts and every designed trap

### Out of scope

- Running the pipeline or comparing against the real marts — T-GOLD-1 (ER-091) is the arbiter between the oracle and the dbt macros
- Adding `batch/` expectations to `base_10` — it has no batch phase (S8.2.1)
- Changing the persona structure, record count or true-pair count of `base_10` (ER-041 owns them and they are machine-checked at 23/10/18)
- The survivorship macros themselves (ER-087)

## Design decisions applied

Implements gaps B4, M11 and M7 at the fixture layer. Constraints easy to miss: (1) `expected/<phase>/lineage.csv` is NOT in S8.2.1's enumerated file list — it is additive and legal under S8.2.1's rule that a missing expected file simply makes no claim; if `validate_fixtures.py` or `test_fixture_format.py` carries a closed filename whitelist, extend it and document the header in `FORMAT.md`; (2) `entity_label` is symbolic (`E1..E10`) allocated in ascending order of the minimum `record_key` in the expected group and MUST NEVER be a ULID; the labels here must agree with the ones already committed in `expected/base/membership.csv`; (3) the lineage grid is complete — six rows per entity even where the winning value is NULL — matching ER-088's emission rule; (4) the deciding-rule convention is ER-087's: the first rule in the chain whose sort key differs between rank 1 and rank 2, else `tiebreak_deterministic` (which also covers single-candidate entities); (5) the oracle is a fixture-authoring aid, not a second implementation of survivorship — the dbt macros remain authoritative and ER-091 is where the two are compared; say so in the module docstring; (6) if `completeness` (or any other rule) cannot decide any contest on the current inputs, edit VALUES within existing rows rather than adding or removing rows, and re-run ER-041's truth-count test to prove 23/10/18 is unchanged.

## Acceptance criteria

- [ ] AC1: `expected/base/golden.csv` carries the literal S8.2.1 header, is stored sorted by the S8.2.1 sort key, uses `\N` for every null, and holds exactly 10 rows with `entity_label` values `E1..E10` matching the labels in `expected/base/membership.csv`
- [ ] AC2: `expected/base/lineage.csv` holds exactly 60 rows (10 entities x 6 attributes) and its `attribute` values are exactly the six-token vocabulary with each appearing 10 times
- [ ] AC3: Every lineage row's `(source_system, source_record_id)` is a member of that `entity_label` per `expected/base/membership.csv`, and its `record_key` equals `source_system || ':' || source_record_id`
- [ ] AC4: For `attribute='address'`, all six `addr_*` values in `golden.csv` equal the standardized values of the single record named by that lineage row; for every other attribute the `golden.csv` value equals that record's standardized value
- [ ] AC5: `set(lineage.rule)` is exactly `{source_priority, recency, frequency, completeness, validated, tiebreak_deterministic}` — all six present, none extra
- [ ] AC6: Every `rule` value appears only for an attribute whose configured chain in `configs/test.yaml` contains it (`validated` only on `email` and `phone_e164`); `tiebreak_deterministic` may appear anywhere
- [ ] AC7: Re-running the Python survivorship oracle over the committed input CSVs reproduces both the winning record and the rule for all 60 lineage rows, and reproduces every non-null value in `golden.csv`
- [ ] AC8: `uv run pytest tests/unit/fixtures/test_base_10.py -q` still reports 23 records, 10 personas and 18 true pairs after any value-level input edit this ticket makes

## Tests

- tests/unit/fixtures/test_base_10_golden.py::test_golden_csv_format_and_labels
- tests/unit/fixtures/test_base_10_golden.py::test_lineage_grid_is_complete
- tests/unit/fixtures/test_base_10_golden.py::test_lineage_records_are_entity_members
- tests/unit/fixtures/test_base_10_golden.py::test_address_composite_comes_from_one_record
- tests/unit/fixtures/test_base_10_golden.py::test_rule_coverage_is_exactly_six_tokens
- tests/unit/fixtures/test_base_10_golden.py::test_oracle_reproduces_winner_and_rule
- tests/unit/fixtures/test_base_10_golden.py::test_truth_counts_unchanged

## Verification

```bash
uv run pytest tests/unit/fixtures/test_base_10_golden.py -q
uv run pytest tests/unit/fixtures/test_base_10.py tests/unit/fixtures/test_fixture_format.py -q
uv run python scripts/validate_fixtures.py fixtures/static/base_10
uv run ruff check .
```

## Definition of Done

- Both expected files are committed under `fixtures/static/base_10/expected/base/`, sorted, `\N`-encoded, with no `assembled_at` column and no ULID anywhere
- `lineage.csv`'s header is documented in `fixtures/static/FORMAT.md` and accepted by `validate_fixtures.py`
- All six rule tokens are covered and the coverage assertion is exact set equality, not a superset check
- The Python oracle is documented as a fixture-authoring aid whose arbiter is T-GOLD-1 (ER-091)
- `base_10`'s 23/10/18 truth counts are re-asserted green after any input edit
- `uv run pytest tests/unit/fixtures/test_base_10_golden.py -q` passes and failed before the change

## Blocker log

### Attempt 1 — spec_contradiction (2026-08-27T22:19:34Z)

- **Failing command:** `uv run python .loop/er090_author.py`
- **Assertion / contradiction:** AC5 requires set(lineage.rule) to equal exactly the six-token vocabulary, and 'frequency' is UNREACHABLE for base_10, so the set equality can never hold. Measured, not inferred: a Python survivorship oracle verified against a real run (23 int_std_records rows x 13 fields, 0 mismatches; 72 golden_lineage rows reproducing winner AND rule, 0 mismatches) produces source_priority 38, tiebreak_deterministic 11, recency 8, validated 2, completeness 1, frequency 0 over the committed 10-entity membership. Two independent reasons. (1) Where frequency is FIRST in the chain (birth_date), it can never be the deciding rule for ANY input: ER-087 attributes the decision to the first chain rule whose sort key differs between rank 1 and rank 2, and rank1.freq always equals rank2.freq -- if rank-1's value is shared by f1 records then those f1 records hold the top key and fill the first f1 ranks, so rank 2 is one of them when f1 >= 2, and when f1 = 1 every group has size 1 so all frequencies are 1. Brute-forced over every value multiset for entity sizes 2 to 6 across 3 symbols: 0 of 1092 cases. (2) Where frequency follows source_priority (given_name, family_name), it decides only when rank 1 and rank 2 tie on source_priority, i.e. both sit at the best-priority source present in the entity, AND a third record makes their value frequencies differ. No base_10 entity qualifies: E1, E2 and E3 have 3 or 4 records but exactly one crm record each, E4 to E9 have 2 or 1 records, and E10 is the only entity whose top two share a source but it has 2 members, where frequencies are 1,1 or 2,2 and therefore always tied. email, phone_e164 and address carry no frequency term. A value-level edit cannot fix this because a record's source_system is the file it lives in, so no edit can give an entity a second record at its best priority, and moving or adding records would change the membership ER-041 owns and the 23/10/18 counts that are machine-checked.
- **Smallest change that would unblock:** Amend AC5. It is the only unsatisfiable criterion; AC1 to AC4 and AC6 to AC8 all hold and the oracle that proves them is written and verified. Proposed wording: 'AC5: set(lineage.rule) is a subset of the six-token vocabulary and contains at least source_priority, recency, completeness, validated and tiebreak_deterministic. frequency is not reachable on base_10 -- where it leads a chain it can never separate rank 1 from rank 2, and where it follows source_priority no base_10 entity holds two records at its best-priority source with three or more members -- so full six-token coverage belongs to a fixture built for it rather than to base_10.' If instead full coverage on base_10 is wanted, the smallest structural change is to give one entity a second crm record plus a third member sharing one of their given_names, which means ER-041 re-authoring base_10's membership and re-checking 23/10/18 and every designed trap; that is a bigger change than the criterion it satisfies. A third option is to amend ER-087 so a rule is credited when it eliminates ANY candidate rather than only rank 2, but that changes merged, tested behaviour and the meaning of S5's closed golden_lineage.rule vocabulary, so it should not be done for a coverage target.
- **Log:** `.loop/logs/ER-090.attempt-1.log`
