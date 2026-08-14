---
id: ER-092
title: "Touched-only assembly: er_touched_entities, single run_id var, explicit reap step, assembled_at rule + T-INC-2 (rewritten ∪ reaped == touched)"
milestone: M4
status: todo
kind: code
size: L
gates: full
depends_on: ["ER-023", "ER-075", "ER-083", "ER-088", "ER-089"]
spec_refs: ["s4", "s4-0", "s4-6", "s5", "s5-0", "s5-1", "s5-2", "s8-3"]
gap_refs: ["M10"]
provides: ["src/er/golden/assemble.py::compute_touched_set", "src/er/golden/assemble.py::write_touched_entities", "src/er/golden/assemble.py::reap_retired_entities", "src/er/golden/assemble.py::assemble", "src/er/golden/assemble.py::TOUCHED_EVENT_TYPES", "dbt/macros/assembly/touched_entities.sql", "relation:er_touched_entities", "tests/integration/test_touched_assembly.py::test_rewritten_plus_reaped_equals_touched", "tests/integration/test_touched_assembly.py::SPEC_TEST_IDS"]
consumes: ["src/er/obs/run_context.py::RunContext", "src/er/dbt_runner.py::render_dbt_vars", "src/er/dbt_runner.py::run_dbt", "src/er/lake/ducklake.py::connect", "src/er/lake/columns.py::VOLATILE_COLUMNS", "relation:entity_events", "relation:entities", "relation:run_stages", "dbt/models/marts/golden_records.sql", "dbt/models/marts/golden_lineage.sql", "dbt/models/marts/golden_display.sql", "fixtures/static/merge_scenario/", "fixtures/static/deletion_scenario/", "tests/helpers/compare.py::assert_golden_equal", "tests/conftest.py::lake_ns", "scripts/ci/itest.sh"]
owns: ["src/er/golden/assemble.py", "dbt/macros/assembly/touched_entities.sql", "tests/integration/test_touched_assembly.py"]
protected_paths: ["fixtures/static/base_10/expected/", "fixtures/static/merge_scenario/expected/", "fixtures/static/deletion_scenario/expected/"]
extra_paths: ["src/er/cli.py", "dbt/models/marts/golden_records.sql", "dbt/models/marts/golden_lineage.sql", "dbt/models/marts/golden_display.sql", "dbt/models/schema.yml"]
attempts: 0
verify: "bash scripts/ci/itest.sh tests/integration/test_touched_assembly.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

`er assemble` currently has no touched-set mechanism: S4.6 requires the touched set to be computed as a formula over this run's `entity_events`, written to `er_touched_entities(run_id, entity_id, disposition)`, and joined by the marts, because passing tens of thousands of ULIDs as a dbt var hard-fails with E2BIG. It also requires an explicit reap step after the marts, because dbt's `delete+insert` cannot delete a key absent from the incoming batch, which is exactly the shape of a merge loser or an emptied split fragment. This ticket implements `assemble.py` end to end — touched formula, `er_touched_entities` write, `--vars` payload without any id list, mart join, reap of `disposition='retire'` from all three golden relations, `assembled_at = runs.started_at` for touched entities only, the counters of S4.6 and the exit-`10` path — and lands T-INC-2 as `rewritten ∪ reaped == touched`.

## Scope

### In scope

- `compute_touched_set(run_id)`: `{entity_id : an event of type created, member_added, member_removed, merged, split, retired or edge_cut exists with this run_id}` with `disposition='retire'` for merge losers, emptied fragments and retired entities and `rebuild` for everything else
- `write_touched_entities`: one row per `(run_id, entity_id)`, written before the marts run
- dbt invocation carrying no entity-id payload; a `touched_entities(disposition)` macro the three marts join on `var('run_id')`
- `reap_retired_entities`: explicit DELETE from `golden_records`, `golden_lineage` and `golden_display` for every `disposition='retire'` entity, executed only after the marts return zero
- `assembled_at` = the run's `started_at`, written only for touched entities
- `er assemble --touched-only` exit `10` on an empty touched set; the no-flag path rebuilding every active entity
- `run_stages` counters `{entities_touched, entities_rebuilt, entities_reaped, lineage_rows, tiebreak_deterministic_count, duration_ms}` plus the promoted columns
- T-INC-2 as an integration test over an incremental run

### Out of scope

- Survivorship rule semantics and the golden/lineage column lists (ER-087/ER-088)
- `golden_display` presentation transforms (ER-089); this ticket only reaps it
- The `CoherenceScorer` hook that S11 places at the start of assemble (ER-104)
- Runs with a non-NULL `runs.rebuild_reason` — S5.1 places version-bump rebuilds outside T-INC-2's accounting
- Time travel to the pre-incremental golden state (ER-095)

## Design decisions applied

Closes M10 in both halves: the stale-row half (explicit reap, because `delete+insert` deletes only keys present in the batch) and the argv half (`er_touched_entities` instead of a var payload). Two constraints are easy to miss. (1) S4.6 says dbt is invoked with `--vars '{run_id: <ulid>}'` while S6 says every dbt invocation also carries `std_version` and `survivorship_version`; the normative content is that NO entity-id list may appear, so the payload is `render_dbt_vars(cfg)` plus `run_id` and nothing per-entity. (2) `assemble`'s reap runs only after the marts return zero (S4.7 all-or-nothing at the logical level), and history for reaped entities is recovered through the snapshot range in `run_stages`, never by keeping the row. Test-layout convention introduced here and reused by ER-093/094/095: the board's file name wins over the S8.3 `file path` column, the test FUNCTION name is exactly the S8.3 node-id function name, and each scenario module declares a module-level `SPEC_TEST_IDS` tuple so the S8.3 id stays machine-resolvable.

## Acceptance criteria

- [ ] AC1: `bash scripts/ci/itest.sh tests/integration/test_touched_assembly.py -q` exits 0.
- [ ] AC2: After an incremental run, `{entity_id : golden_records.assembled_at == runs.started_at} ∪ {entity_id deleted from golden_records by the reap}` equals exactly the `er_touched_entities` set for that `run_id`, compared as sets with the symmetric difference printed on failure.
- [ ] AC3: Every entity absent from `er_touched_entities` has a byte-unchanged `assembled_at` across the incremental run (captured before and compared after).
- [ ] AC4: Zero rows exist in `golden_records`, `golden_lineage` and `golden_display` for every `disposition='retire'` entity; on `merge_scenario` the loser entity has zero golden rows while `entities.status='merged'` and `ids.resolve(loser)` still returns the survivor.
- [ ] AC5: The dbt argv recorded for the assemble invocation contains no `entity_id` value and every single argv element is under 128 KB; asserted by inspecting the rendered command, not by string search on stdout.
- [ ] AC6: `er assemble --touched-only` against a run whose `er_touched_entities` set is empty exits `10`, writes no golden row and leaves every `assembled_at` unchanged.
- [ ] AC7: The stage's `run_stages` row carries non-NULL `entities_touched`, `entities_rebuilt`, `entities_reaped`, `lineage_rows` and `duration_ms` in `counters`, and `entities_rebuilt + entities_reaped == entities_touched`.
- [ ] AC8: On `deletion_scenario`, the entity that empties is written with `disposition='retire'` and its golden and lineage rows are gone after the run, while the reaped state is still readable at the stage's `snapshot_start`.

## Tests

- tests/integration/test_touched_assembly.py::test_rewritten_plus_reaped_equals_touched
- tests/integration/test_touched_assembly.py::test_retire_disposition_reaps_all_three_marts
- tests/integration/test_touched_assembly.py::test_touched_only_with_empty_set_exits_10
- tests/integration/test_touched_assembly.py::test_dbt_vars_carry_no_entity_id_list

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_touched_assembly.py -q
uv run mypy --strict src/er/golden
uv run dbt parse --project-dir dbt --profiles-dir dbt/profiles --target mem
```

## Definition of Done

- T-INC-2 green as `rewritten ∪ reaped == touched`, with the reap arm and the exit-`10` arm each covered by their own node id.
- `er_touched_entities` is written before the marts run and holds exactly one row per `(run_id, entity_id)`.
- The three marts join the touched table on `var('run_id')`; no mart receives an entity-id list.
- dbt tests still green: every `golden_records.entity_id` has `entities.status='active'`, and every active entity with ≥1 member has exactly one golden row.
- No committed `expected/` file modified; `SPEC_TEST_IDS` declared in the new module.
- ruff + `mypy --strict src/er` clean; gate receipt recorded and `board.py complete` run.
