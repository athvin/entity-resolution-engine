---
id: ER-052
title: "M2 exit gate: standardization + blocking green on base_10 under the namespaced harness"
milestone: M2
status: todo
kind: code
size: S
gates: full
depends_on: ["ER-045", "ER-047", "ER-049", "ER-051"]
spec_refs: ["s12", "s8-1", "s4-2", "s8-2", "s5-0", "s4-3-1", "s10-1"]
gap_refs: ["MINOR-milestones"]
provides: ["tests/integration/test_m2_exit.py::M2_EXIT_TAGS"]
consumes: ["fixtures/static/base_10/expected/base/std_hashes.csv", "src/er/matching/model.py::blocking_rules_from_config", "src/er/matching/model.py::build_settings", "src/er/matching/splink_env.py::assert_no_splink_relations_in_lake", "fixtures/generator/emit.py::emit_corpus", "tests/conftest.py::lake_ns", "src/er/dbt_runner.py::run_dbt", "src/er/lake/hashing.py::table_content_hash"]
owns: ["tests/integration/test_m2_exit.py"]
protected_paths: ["fixtures/static/base_10/", "dbt/models/", "dbt/macros/", "src/er/", "fixtures/generator/"]
extra_paths: []
attempts: 0
verify: "bash scripts/ci/itest.sh tests/integration/test_m2_exit.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

The M2 exit gate of S12: one integration test that runs standardization and blocking end to end on `base_10` under the namespaced ephemeral lake of S8.1 and asserts the milestone's exit criteria as a set, so the milestone cannot be declared complete by inspection. It asserts nothing new — it re-asserts the M2 criteria in one collectible place and tags each assertion so the tag set is comparable against the milestone definition. A failure here is a defect in the owning ticket (ER-045/047/049/051), never something this ticket fixes.

## Scope

### In scope

- `dbt build --select staging intermediate --target lake` green on `base_10` through `run_dbt`
- `int_std_records` holds exactly 23 current rows and its per-row `std_hash` set equals `fixtures/static/base_10/expected/base/std_hashes.csv`
- `int_blocking_keys` contains zero NULL or empty `key_value` rows and its `key_type` set equals the `blocking:` key_type set of the loaded config
- The Splink settings built by `build_settings(cfg)` emit `NullLevel` first and `ElseLevel` last for every one of the six `comparisons` entries
- Zero relations matching `__splink__%` exist in `lake` after the run
- The generator emits `base_10`-identical headers and is byte-reproducible across two processes (invoked from the test as a subprocess pair)
- One `M2-EXIT-<k>` tag per criterion, plus a meta-assertion that the collected tag set equals the declared seven

### Out of scope

- Any assertion on matching, `match_scores`, entities, membership, events or golden relations — none of those relations exists at M2 (S12 gating rule)
- Fixing a failing criterion by editing `dbt/`, `src/er/` or `fixtures/` — all protected here
- Snapshot-count assertions (forbidden by the S4 preamble); the stage's snapshot range may be read but never counted
- Re-implementing helpers already provided by ER-044/045/047/049/051

## Design decisions applied

Implements MINOR-milestones (the gap report's "M1's exit criterion is satisfiable by six stubs" complaint, applied to M2): the exit criterion must be an executed test, not prose. Honour the S12 gating rule — a milestone may only gate on relations existing by the end of that milestone, so this test may not read `match_scores`, `entity_membership` or `golden_*`. Obtain the lake handle from the session-scoped namespace fixture (S8.1); do not attach a lake of your own. Import every symbol named in `consumes` from INTERFACES.md; where a path there differs, INTERFACES.md wins.

## Acceptance criteria

- [ ] AC1: `bash scripts/ci/itest.sh tests/integration/test_m2_exit.py -q` exits 0 on a clean namespace and the suite collects at least seven test functions
- [ ] AC2: After the run, `select count(*) from lake.main.int_std_records` returns 23 and the computed `std_hash` set is set-equal to `expected/base/std_hashes.csv` (symmetric difference empty, printed on failure)
- [ ] AC3: `select count(*) from lake.main.int_blocking_keys where key_value is null or key_value = ''` returns 0, and `select distinct key_type` equals `{email_exact, phone_exact, name_postal, dob_name}` derived from the loaded config rather than hard-coded
- [ ] AC4: For each of the six configured comparisons, the built settings' level list starts with a null level and ends with an else level; a settings builder that omits either fails this test
- [ ] AC5: `assert_no_splink_relations_in_lake(conn)` passes after the full M2 chain, and deliberately creating a `__splink__probe` relation in `lake` makes it fail (negative arm asserted in the same test)
- [ ] AC6: Two subprocess invocations of the generator CLI with the same seed produce identical bytes, and the emitted header rows equal the committed `base_10` headers
- [ ] AC7: The set of `M2-EXIT-<k>` tags collected from the module equals the declared literal set `{1..7}`; adding a criterion without a tag, or a tag without a criterion, fails

## Tests

- tests/integration/test_m2_exit.py::test_dbt_build_staging_and_intermediate_green
- tests/integration/test_m2_exit.py::test_int_std_records_matches_expected_std_hashes
- tests/integration/test_m2_exit.py::test_blocking_keys_have_no_null_or_empty_values
- tests/integration/test_m2_exit.py::test_settings_builder_brackets_every_comparison
- tests/integration/test_m2_exit.py::test_no_splink_relations_reach_the_lake
- tests/integration/test_m2_exit.py::test_generator_is_reproducible_and_header_identical
- tests/integration/test_m2_exit.py::test_exit_tag_set_is_complete

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_m2_exit.py -q
```

## Definition of Done

- Acceptance criteria met and the verify command passes
- Every protected path unmodified by this ticket's diff (`git diff --name-only` contains only `tests/integration/test_m2_exit.py`)
- No snapshot count is asserted anywhere in the file
- Each test function carries exactly one `M2-EXIT-<k>` tag and the completeness test enforces it
- Any criterion that fails is recorded as a blocker against the owning ticket rather than worked around here
- Committed on a branch off main
