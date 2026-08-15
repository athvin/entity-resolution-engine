---
id: ER-028
title: "S8.2.1 fixture machinery: FORMAT.md, phase dirs (base/,batch/,refresh/), base_scenario manifest, load_scenario, validate_fixtures.py + self-test fixtures"
milestone: M1
status: todo
kind: code
size: M
gates: fast
depends_on: ["ER-006", "ER-027"]
spec_refs: ["s3", "s8-2", "s8-2-1"]
gap_refs: ["M7", "M22", "NEW-S8.2.1"]
provides: ["fixtures/static/FORMAT.md", "scripts/validate_fixtures.py", "tests/helpers/scenario.py::load_scenario", "tests/helpers/scenario.py::Scenario", "tests/helpers/scenario.py::PHASES", "tests/helpers/scenario.py::EXPECTED_HEADERS", "tests/helpers/scenario.py::AUX_FILES", "tests/unit/test_fixture_lint.py"]
consumes: ["src/er/lake/columns.py::VOLATILE_COLUMNS", "tests/helpers/expected.py::load_expected", "tests/helpers/expected.py::NULL_TOKEN", "tests/helpers/expected.py::sort_key"]
owns: ["fixtures/static/FORMAT.md", "scripts/validate_fixtures.py", "tests/helpers/scenario.py", "tests/unit/fixtures/__init__.py", "tests/unit/fixtures/test_fixture_format.py", "tests/unit/test_fixture_lint.py", "tests/fixtures/scenarios"]
protected_paths: []
extra_paths: []
attempts: 1
verify: "uv run pytest tests/unit/fixtures/test_fixture_format.py -q"
branch: ""
commit: ""
spec_sha: "28d8d8e366a7b49b"
updated_at: "2026-08-15T03:54:42Z"
session: d453cfe7-4537-494b-86cc-b38cb65936e8
---
## Description

Every scenario in S8.2 is one directory with the same shape, and no fixture can be authored until that shape, its phase vocabulary and its encoding rules are mechanical. This ticket ships the S8.2.1 machinery: `FORMAT.md`, the phase vocabulary `{base, batch, refresh, resurrect}`, the per-scenario manifest (including `base_scenario` composition), `load_scenario` for tests, and `validate_fixtures.py` — a linter with committed negative self-test fixtures so a mis-sorted or mis-headered expected file is a lint failure rather than a scenario-test failure. Closes NEW-S8.2.1 and the fixture-format arm of M7 and M22.

## Scope

### In scope

- `fixtures/static/FORMAT.md`: the directory shape, the four phases and their order, the eight literal header rows, the `\N` null token, the 1e-9 absolute float tolerance, the byte-wise sort key, and the `VOLATILE_COLUMNS` exclusion.
- A per-scenario manifest (`fixtures/static/<scenario>/scenario.yaml`) declaring `scenario`, `phases`, optional `base_scenario`, and the optional scenario-root aux files.
- `tests/helpers/scenario.py::load_scenario(name)`: returns phases in `base → batch → refresh → resurrect` order restricted to those declared, per-source input CSV paths per phase, expected file paths per phase (absent file = no claim), `assertions.csv` rows grouped by their `phase` column, and the resolved `base_scenario` chain.
- `scripts/validate_fixtures.py`: validates manifest/phase agreement, header literals, sort order, null-token usage, absence of `VOLATILE_COLUMNS` columns, `entity_label` symbolic-ness, phase-vocabulary membership of `assertions.csv` rows, and the allowed aux-file set.
- Committed self-test fixtures under `tests/fixtures/scenarios/`: one valid minimal scenario and one negative fixture per rule.
- `tests/unit/test_fixture_lint.py`: discovers and lints **every** scenario under `fixtures/static/`, so fixtures added by later tickets are linted without editing the linter.

### Out of scope

- Authoring any real scenario: `base_10` (ER-041), `incremental_batch` (ER-064), `merge`/`split`/`assertions`/`supersession`/`deletion` scenarios all belong to their own tickets.
- The comparison helpers themselves (ER-027) — this ticket consumes them.
- `parity_pairs.csv` derivation (T-INC-3, ER-066) and `tf_flip_pairs.csv` bounds (T-TF-1) — only their file names, headers and permitted locations are validated here.
- A top-level `fixtures/expected/` directory: it does not exist; `expected/` lives inside each scenario (S3, S8.2.1).

## Design decisions applied

NEW-S8.2.1 + M7 + M22. Constraints: (1) the phase vocabulary is exactly `{base, batch, refresh, resurrect}` — the board title lists only three; S8.2.1 is the authority and `resurrect` must be supported, because reusing `refresh` for it would tombstone every key the resurrection delivery omits. (2) `base` is always present and always first; phases run in the fixed order over the phases the scenario has. (3) A missing `expected/<phase>/<file>.csv` means that phase makes **no claim** about that relation — it is not an empty expectation and must not raise. (4) The three scenario-root files (`assertions.csv`, `parity_pairs.csv`, `tf_flip_pairs.csv`) are inputs and bounds, not expectations, and belong beside `base/`, never under `expected/`. (5) The linter must have committed negative fixtures: a linter with no failing arm proves nothing, and each rule needs its own negative case.

## Acceptance criteria

- [ ] AC1: `python3 scripts/validate_fixtures.py` exits 0 over `tests/fixtures/scenarios/ok_minimal` and over every scenario committed under `fixtures/static/`, and exits non-zero naming the file, line and rule for each committed negative fixture.
- [ ] AC2: There is one negative self-test fixture per rule — mis-sorted expected file, wrong header literal, undeclared/unknown phase directory, `expected/<phase>/` without the matching input phase, `entity_label` that is a ULID, a `VOLATILE_COLUMNS` member as a column, an `assertions.csv` `phase` value outside the vocabulary, and a disallowed scenario-root file — and each is asserted to fail for its own reason, not merely to fail.
- [ ] AC3: `EXPECTED_HEADERS` equals the eight literal header rows of S8.2.1 character for character, and `FORMAT.md`'s quoted headers are asserted equal to that constant so the document cannot drift.
- [ ] AC4: `PHASES == ('base','batch','refresh','resurrect')`; a manifest declaring `batch` without `base`, or a phase outside the vocabulary, fails validation.
- [ ] AC5: `load_scenario('ok_minimal')` returns phases in vocabulary order restricted to those declared, per-source input paths keyed by source name, and `assertions.csv` rows grouped by phase; a missing `expected/base/golden.csv` yields None rather than raising.
- [ ] AC6: `base_scenario` composes: a scenario declaring `base_scenario: ok_minimal` and its own `batch/` resolves its `base/` inputs from `ok_minimal`, and a manifest cycle raises with both scenario names named.
- [ ] AC7: `tests/unit/test_fixture_lint.py` discovers scenarios by walking `fixtures/static/`, so adding a new scenario directory brings it under lint with no edit to the test or the linter.

## Tests

- tests/unit/fixtures/test_fixture_format.py::test_expected_headers_match_s8_2_1_literals
- tests/unit/fixtures/test_fixture_format.py::test_format_md_headers_match_constants
- tests/unit/fixtures/test_fixture_format.py::test_phase_vocabulary_and_ordering
- tests/unit/fixtures/test_fixture_format.py::test_load_scenario_returns_phases_inputs_and_expectations
- tests/unit/fixtures/test_fixture_format.py::test_absent_expected_file_is_no_claim
- tests/unit/fixtures/test_fixture_format.py::test_base_scenario_composition_and_cycle_detection
- tests/unit/fixtures/test_fixture_format.py::test_each_negative_fixture_fails_for_its_own_rule
- tests/unit/test_fixture_lint.py::test_every_committed_scenario_validates

## Verification

```bash
uv run pytest tests/unit/fixtures/test_fixture_format.py -q
uv run pytest tests/unit/test_fixture_lint.py -q
python3 scripts/validate_fixtures.py
uv run mypy --strict tests/helpers
```

## Definition of Done

- All acceptance criteria demonstrated by the listed node ids
- Four-phase vocabulary supported, `base` first, order fixed
- Eight header literals held in one constant, mirrored by `FORMAT.md` and asserted equal
- One negative self-test fixture per validation rule, each failing for its own reason
- Repo-wide fixture lint discovers scenarios automatically
- No scenario under `fixtures/static/` authored by this ticket; no top-level `fixtures/expected/` created
- ruff clean; verify command passes
