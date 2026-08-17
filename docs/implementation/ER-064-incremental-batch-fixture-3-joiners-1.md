---
id: ER-064
title: "incremental_batch fixture (3 joiners, 1 bridge, 2 new-vs-new) + base_scenario: base_10 + pass attribution"
milestone: M3
status: done
kind: fixture
size: M
gates: full
depends_on: ["ER-028", "ER-045", "ER-056"]
spec_refs: ["s8-2", "s8-2-1", "s4-3-4", "s4-5-3", "s5", "s5-0", "s12", "s12-1"]
gap_refs: ["B3", "M18", "M7"]
provides: ["fixture:incremental_batch", "fixtures/static/incremental_batch/scenario.yaml", "fixtures/static/incremental_batch/base/crm.csv", "fixtures/static/incremental_batch/base/billing.csv", "fixtures/static/incremental_batch/base/webforms.csv", "fixtures/static/incremental_batch/batch/crm.csv", "fixtures/static/incremental_batch/batch/billing.csv", "fixtures/static/incremental_batch/batch/webforms.csv", "fixtures/static/incremental_batch/expected/batch/membership.csv", "fixtures/static/incremental_batch/expected/batch/events.csv", "fixtures/static/incremental_batch/expected/batch/std_hashes.csv", "fixtures/static/incremental_batch/attribution.csv"]
consumes: ["tests/helpers/scenarios.py::load_scenario", "tests/helpers/expected.py::load_expected", "tests/helpers/compare.py::assert_partition_equal", "scripts/validate_fixtures.py", "fixture:base_10", "fixtures/static/base_10/base", "fixtures/static/base_10/expected/base/membership.csv", "fixtures/static/base_10/expected/base/std_hashes.csv", "src/er/ingest/hashing.py::content_hash", "src/er/lake/columns.py::VOLATILE_COLUMNS"]
owns: ["fixtures/static/incremental_batch/scenario.yaml", "fixtures/static/incremental_batch/base", "fixtures/static/incremental_batch/batch", "fixtures/static/incremental_batch/expected", "tests/unit/fixtures/test_incremental_batch.py", "fixtures/static/incremental_batch/attribution.csv"]
protected_paths: ["fixtures/static/base_10/base", "fixtures/static/base_10/expected"]
extra_paths: []
attempts: 1
verify: "uv run pytest tests/unit/fixtures/test_incremental_batch.py -q"
branch: "ticket/ER-064-incremental-batch-fixture-3-joiners-1"
commit: "383377c"
spec_sha: "7467bdacba1bd84c"
updated_at: "2026-08-17T09:36:28Z"
session: 1d05948d-ce5a-4274-bbe7-05c8aff17052
---
## Description

S8.2 requires an `incremental_batch` scenario of six new records — three joining existing entities, two forming a new entity, one bridging two existing entities and forcing a merge — because the 'two new records form a new entity' case is reachable only through the new-vs-new Splink pass (S4.3.4, D2). Without it the second pass can be omitted and every downstream test still passes. This ticket commits the scenario in the S8.2.1 shape with a `scenario.yaml` manifest declaring `base_scenario: base_10` and a per-record pass attribution, plus a unit test that machine-checks the fixture's structural claims against the committed CSVs.

## Scope

### In scope

- `base/` holding the three source CSVs byte-identical to `fixtures/static/base_10/base/`, with `scenario.yaml` declaring `base_scenario: base_10` and `validate_fixtures.py` asserting the byte-identity
- `batch/` holding exactly six new records across the three sources with the literal `base_10` headers of S8.2 and a `persona_id` truth column on every row
- An `attribution.csv` at the scenario root assigning each of the six batch `record_key`s exactly one role (`joiner` x3, `bridge` x1, `new_pair` x2) and its expected discovering pass (`pass1` or `pass2`)
- `expected/batch/membership.csv`, `expected/batch/events.csv` and `expected/batch/std_hashes.csv` in the S8.2.1 encoding (symbolic `entity_label`, `\N` null token, byte-sorted)
- A unit test asserting every structural claim above from the committed files alone

### Out of scope

- `parity_pairs.csv` — derived from actual scoring, owned by ER-066
- `tf_flip_pairs.csv` — the T-INC-1b bound, owned by the correction-pass ticket
- `expected/batch/golden.csv` — the golden relations do not exist until M4, and S12's gating rule forbids a milestone asserting on a later milestone's relation
- Authoring `expected/` files for phases other than `batch`
- Asserting that the bridge pair actually scores at or above `auto_merge` — that is a property of the committed model and is verified by ER-065

## Design decisions applied

Implements gap-report B3's fixture half, M18 and M7 under D2. Constraints: (1) S3 shows `incremental_batch/base/`, and S8.2.1 says a missing phase directory means the scenario has no such phase, so `base/` is committed rather than inherited — `base_scenario: base_10` is a byte-identity assertion, not an indirection, which is what keeps the two scenarios from drifting; (2) `entity_label` is allocated per expected file in ascending order of the minimum `record_key` in the group (S8.2.1), so the batch labels are recomputed and MUST NOT be assumed equal to base_10's; (3) the bridge is cross-persona by construction — base_10's ten personas are exactly ten entities (S8.2), so no truthful bridge exists, and `incremental_batch` is a mechanics fixture on which no quality metric is gated (S8.5 gating lives on base_10 via T-MATCH-1a/1b); the manifest records the bridged pair explicitly so the intent cannot be mistaken for a defect; (4) batch `source_record_id`s must not collide with any base key, or the delivery becomes a supersession rather than an insertion (S4.2).

## Acceptance criteria

- [ ] AC1: `fixtures/static/incremental_batch/base/{crm,billing,webforms}.csv` are byte-identical to the same-named files under `fixtures/static/base_10/base/`, and `scenario.yaml` declares `base_scenario: base_10`
- [ ] AC2: `batch/` holds exactly 6 data rows in total across the three CSVs, each file carrying the literal S8.2 header for its source plus `persona_id`, and no batch `(source_system, source_record_id)` equals any base key
- [ ] AC3: `attribution.csv` at the scenario root (columns `record_key,role,pass`) covers exactly the six batch `record_key`s with role counts `joiner=3`, `bridge=1`, `new_pair=2`; the two `new_pair` records are the only ones assigned `pass2`, and the other four are assigned `pass1`. It is a CSV, not a `scenario.yaml` block: the manifest grammar is a flat `key: value` map whose scalar alphabet excludes the `:` in a `record_key` (S8.2.1)
- [ ] AC4: The `bridge` entry names two distinct base `entity_label`s from `base_10/expected/base/membership.csv`, and those two labels are distinct groups in that file
- [ ] AC5: `expected/batch/membership.csv` holds 29 rows (23 base + 6 batch), induces exactly 10 distinct `entity_label`s, contains one group equal to the union of the two bridged base groups plus the bridge record, and one group of exactly the two `new_pair` records
- [ ] AC6: `expected/batch/events.csv` contains exactly one `merged` row for the surviving label, one `created` row for the new-pair entity, and `member_added` counts totalling 4 across the three joiners and the bridge
- [ ] AC7: `expected/batch/std_hashes.csv` holds 29 rows and its 23 base rows are byte-identical to `base_10/expected/base/std_hashes.csv`
- [ ] AC8: Every committed expected file is byte-sorted on the full column tuple in header order, uses `\N` for NULL, and contains no `VOLATILE_COLUMNS` member and no ULID

## Tests

- tests/unit/fixtures/test_incremental_batch.py::test_base_is_byte_identical_to_base_10
- tests/unit/fixtures/test_incremental_batch.py::test_batch_shape_and_no_key_collisions
- tests/unit/fixtures/test_incremental_batch.py::test_attribution_roles_and_passes
- tests/unit/fixtures/test_incremental_batch.py::test_expected_batch_membership_partition
- tests/unit/fixtures/test_incremental_batch.py::test_expected_batch_events_counts
- tests/unit/fixtures/test_incremental_batch.py::test_expected_files_encoding_and_sort

## Verification

```bash
uv run pytest tests/unit/fixtures/test_incremental_batch.py -q
uv run pytest tests/unit/test_fixture_lint.py -q
uv run python scripts/validate_fixtures.py fixtures/static/incremental_batch
```

## Definition of Done

- `scenario.yaml` records the bridged label pair and the cross-persona intent in a machine-readable field, not a comment
- No `expected/batch/golden.csv` is committed and the ticket states why in the manifest
- No ULID appears anywhere under `fixtures/static/incremental_batch/`
- `base_10` inputs and expectations are unmodified
- `bash scripts/gates.sh` green

## Blocker log
### Resolution of attempt 1 (applied by the board owner, 2026-08-17)

Both defects were real and both are fixed by following precedent already on the board.

**(1) The `attribution:` block had no legal representation.** ER-028's manifest grammar is
normative and narrow on purpose: a flat `key: value` map, keys from a fixed set, values matching
`[A-Za-z0-9_.-]+` — which excludes the `:` inside every `record_key` (D6). Widening `_MANIFEST_KEYS`
would not have helped, since indented lines are rejected outright. The data is tabular ground truth
keyed by `record_key`, so **S8.2.1 now defines `attribution.csv`** alongside `truth.csv` and
`traps.csv`, with columns `record_key,role,pass`. AC3 is rewritten accordingly.

You will need to extend the fixture machinery to accept it: `tests/helpers/scenario.py` currently
has `TRUTH_FILES = ("truth.csv", "traps.csv")`, and `validate_fixtures.py` / the `unknown-file` rule
enumerate the root by name. Those files belong to ER-028, which is `done`, so `plan-check` permits
your plan to touch them. **Extend the list; do not weaken the linter** — the inputs/bounds vs
ground-truth distinction is normative, so `attribution.csv` goes in `TRUTH_FILES`, not `AUX_FILES`.

**(2) AC7 required real pipeline output under a no-pipeline scope.** `std_hash` is SHA-256 over
`int_std_records`, so the six batch values exist only after an ingest+standardize run. **This ticket
is now `gates: full`** and the "Running any pipeline stage" exclusion is removed, matching ER-045,
which owns `base_10`'s `expected/base/std_hashes.csv` and is `full` for exactly this reason.
Generate the values from a real run. Do not hand-author them: attempt 1 correctly refused to invent
six 64-hex strings on the grounds that it would poison ER-065 and ER-066, and that judgement stands.


### Attempt 1 — underspecified (2026-08-17T05:55:50Z)

- **Failing command:** `uv run python scripts/validate_fixtures.py .loop/probe/incremental_batch   # probe manifest carrying AC3's attribution: block`
- **Assertion / contradiction:** Two defects. (1) AC3 and the DoD require a scenario.yaml 'attribution:' block, but the manifest grammar refuses it: 'scenario.yaml:4: manifest: unknown key attribution; the manifest keys are aux_files, base_scenario, phases, scenario' (tests/helpers/scenario.py:170 _MANIFEST_KEYS, raised at :256). Widening that set is not sufficient either: :248 rejects any indented line ('the manifest is a flat key: value map') so a block has no representation, and :168 pins the value alphabet to [A-Za-z0-9_.-], which excludes the ':' that every record_key contains by S5.0/D6. FORMAT.md:56-73 pins this grammar as normative and is cross-checked by tests/unit/fixtures/test_fixture_format.py. All three files are owned by ER-028 (done); ER-064 lists them under consumes, not owns, and names no replacement spelling. Two of the ticket's own three Verification commands therefore fail for any scenario.yaml satisfying AC3. (2) AC7 requires expected/batch/std_hashes.csv to hold 29 rows = 23 base + 6 batch, but Scope/out-of-scope forbids 'Running any pipeline stage' and gates are fast. A std_hash is SHA-256 over int_std_records (T-STD-1), so the 6 batch values exist only after an ingest+standardize run; ER-045's DoD says the file is 'generated from a real pipeline run' and tests/integration/scenarios/test_base_10_std.py:9-21 documents that as the only production path. ER-064 consumes src/er/ingest/hashing.py::content_hash (the S4.1 raw-record hash), not table_content_hash, so it did not plan to compute one. AC7 is satisfiable only by inventing six 64-hex strings, which would poison ER-065 and ER-066.
- **Smallest change that would unblock:** Two edits, both to ER-064's ticket, no DesignDoc change (S8.2.1 never mentions scenario.yaml). (1) Move the attribution out of the manifest into a scenario-root ground-truth file: add 'attribution.csv' with literal header 'record_key,role,pass' beside truth.csv and traps.csv. It is read only by tests and never fed to the pipeline, so it belongs to S8.2.1's GROUND TRUTH kind, needs no manifest key, no grammar change, and no new lint rule -- only appending it to TRUTH_FILES in tests/helpers/scenario.py and one paragraph in FORMAT.md. Record the bridged pair as two more rows or as a second file 'bridged_labels.csv' (header 'entity_label_a,entity_label_b'). Then either add 'tests/helpers/scenario.py' and 'fixtures/static/FORMAT.md' to ER-064's owns, or -- cleaner -- cut a predecessor ticket owning both, and make ER-064 depend on it. Rewrite AC3/AC4 and the DoD bullet to name attribution.csv instead of a scenario.yaml block. If the attribution must stay in scenario.yaml, the ticket has to state the exact grammar extension, because indentation and ':' are both currently refused. (2) Drop expected/batch/std_hashes.csv from ER-064 entirely: by S8.2.1 an absent expected file means the phase makes no claim about that relation, so removing it costs nothing and is the same staging ER-045 used. Delete AC7, delete the file from provides/owns, and give it to the first ticket that actually runs the batch phase (ER-065 or ER-066), following ER-045's pattern -- an integration test that emits artifacts/incremental_batch/expected/batch/std_hashes.csv and compares it. Everything else in ER-064 (AC1, AC2, AC5, AC6, AC8) is hand-authorable and unaffected.
- **Log:** `.loop/logs/ER-064.attempt-1.log`
