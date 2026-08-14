---
id: ER-091
title: "T-GOLD-1: values and lineage rules on base_10, sensitivity to a priority flip"
milestone: M4
status: todo
kind: code
size: S
gates: full
depends_on: ["ER-090"]
spec_refs: ["s4-6", "s5", "s5-0", "s6", "s6-1", "s8-2", "s8-2-1", "s8-3"]
gap_refs: ["B4", "M11", "M7"]
provides: ["tests/integration/test_golden_survivorship.py::test_survivorship_values_and_rules", "tests/integration/test_golden_survivorship.py::test_priority_flip_changes_source_priority_winners", "tests/helpers/config_mutation.py::mutated_config"]
consumes: ["fixtures/static/base_10/expected/base/golden.csv", "fixtures/static/base_10/expected/base/lineage.csv", "tests/helpers/compare.py::assert_golden_equal", "tests/helpers/expected.py::load_scenario", "tests/conftest.py::lake_ns", "dbt/models/marts/golden_records.sql", "dbt/models/marts/golden_lineage.sql", "src/er/lake/columns.py::GOLDEN_SURVIVABLE_COLUMNS", "src/er/lake/columns.py::VOLATILE_COLUMNS", "scripts/ci/itest.sh", "fixtures/static/model_test_v1.json"]
owns: ["tests/integration/test_golden_survivorship.py", "tests/helpers/config_mutation.py"]
protected_paths: ["fixtures/static/base_10/expected/base/golden.csv", "fixtures/static/base_10/expected/base/lineage.csv", "dbt/macros/survivorship/"]
extra_paths: []
attempts: 0
verify: "bash scripts/ci/itest.sh tests/integration/test_golden_survivorship.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

T-GOLD-1 is the acceptance test for the survivorship dispatch of S4.6 against `base_10`'s committed golden and lineage expectations. It asserts attribute values with `assert_golden_equal` and, for every `(entity_label, attribute)`, that `golden_lineage.rule` is the rule the fixture says decided — covering `source_priority`, `recency`, `frequency`, `completeness`, `validated` and `tiebreak_deterministic`, plus S4.6's composite rule that all six `addr_*` columns come from one contributing record. It adds a sensitivity arm: swapping two sources' `priority_rank` must change at least one `source_priority`-decided value, so a dispatch that vacuously takes the physically-first row cannot pass.

## Scope

### In scope

- An integration test running the base phase of `base_10` through `er run-all --mode full` against the committed fixture model and asserting `golden_records` with `assert_golden_equal`
- Per-`(entity_label, attribute)` assertion of `golden_lineage.rule` against the committed lineage expectation, including rule-token coverage
- Assertion that the winning `address` row supplies all six `addr_*` values from a single `record_key`
- A priority-flip sensitivity arm driven by a temporary config whose `sources.crm.priority_rank` and `sources.billing.priority_rank` are swapped
- `tests/helpers/config_mutation.py`: write a temp YAML copy of a config with dotted-path overrides, for this and later tickets

### Out of scope

- Changing any survivorship macro, mart model or committed expectation to make the test pass (those ship in ER-087/ER-088/ER-090)
- Touched-only assembly, the reap step and `assembled_at` accounting (ER-092)
- `golden_display` presentation rules (ER-089)
- Any assertion on `assembled_at`, `run_id` or other `VOLATILE_COLUMNS`

## Design decisions applied

Closes B4 (golden_records is a real typed relation with an asserted value set), M11 (every chain is a total order ending in `record_key ASC`, so the winner cannot depend on physical row order) and M7 (symbolic `entity_label`, volatile columns excluded). Easy to miss: `expected/base/golden.csv` carries `entity_label` instead of `entity_id` and omits `assembled_at`; the null token is the two-character `\N` and an empty field is a distinct value; float comparison tolerance is 1e-9 absolute and every other column compares exactly as VARCHAR (S8.2.1). V11 requires `priority_rank` values to be unique across sources, so the sensitivity arm must SWAP two ranks, never duplicate one, and it must run against a temp config file — `configs/test.yaml` is never mutated in place. The flipped arm runs in its own function-isolated namespace so it cannot leak into the primary assertion.

## Acceptance criteria

- [ ] AC1: `bash scripts/ci/itest.sh tests/integration/test_golden_survivorship.py -q` exits 0 and collects both test functions.
- [ ] AC2: After the `base` phase of `base_10`, `golden_records` holds exactly 10 rows and `assert_golden_equal(actual, fixtures/static/base_10/expected/base/golden.csv, label_map)` passes without any edit to the committed expectation.
- [ ] AC3: For every `(entity_label, attribute)` row in `fixtures/static/base_10/expected/base/lineage.csv`, `golden_lineage.rule` equals the expected token, and the set of observed tokens contains all six of `source_priority`, `recency`, `frequency`, `completeness`, `validated`, `tiebreak_deterministic`.
- [ ] AC4: The designed survivorship-tie persona's contested attribute resolves with `golden_lineage.rule = 'tiebreak_deterministic'` and its winning `record_key` equals the lexicographic minimum of the tied records' `record_key`s.
- [ ] AC5: For every entity, the six `addr_*` values in `golden_records` are all equal to the `int_std_records` values of the single `record_key` named by `golden_lineage` where `attribute = 'address'`.
- [ ] AC6: Re-running assembly under a config whose `crm` and `billing` `priority_rank` values are swapped changes at least one `golden_records` value whose lineage rule is `source_priority`, and `assert_golden_equal` against the committed expectation then FAILS (the test asserts the failure, proving the primary assertion is sensitive).
- [ ] AC7: `golden_lineage` holds exactly one row per `(entity_id, attribute)` and every `attribute` is in `{email, phone_e164, given_name, family_name, address, birth_date}`.

## Tests

- tests/integration/test_golden_survivorship.py::test_survivorship_values_and_rules
- tests/integration/test_golden_survivorship.py::test_priority_flip_changes_source_priority_winners

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_golden_survivorship.py -q
uv run pytest tests/unit/fixtures/test_base_10_golden.py -q
uv run ruff check tests && uv run ruff format --check tests
```

## Definition of Done

- Both node ids green under `scripts/ci/itest.sh`; T-INV-1's autouse finalizer green for both.
- No file under `fixtures/static/base_10/expected/` and no survivorship macro modified by this ticket (`git diff --stat` shows only the two owned files).
- All six lineage rule tokens are exercised by at least one asserted row.
- `configs/test.yaml` unchanged; the flip runs from a temp config produced by `tests/helpers/config_mutation.py::mutated_config`.
- ruff + `mypy --strict` clean on the new helper; gate receipt recorded and `board.py complete` run.
