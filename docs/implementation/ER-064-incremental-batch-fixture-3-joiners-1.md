---
id: ER-064
title: "incremental_batch fixture (3 joiners, 1 bridge, 2 new-vs-new) + base_scenario: base_10 + pass attribution"
milestone: M3
status: in_progress
kind: fixture
size: M
gates: fast
depends_on: ["ER-028", "ER-045", "ER-056"]
spec_refs: ["s8-2", "s8-2-1", "s4-3-4", "s4-5-3", "s5", "s5-0", "s12", "s12-1"]
gap_refs: ["B3", "M18", "M7"]
provides: ["fixture:incremental_batch", "fixtures/static/incremental_batch/scenario.yaml", "fixtures/static/incremental_batch/base/crm.csv", "fixtures/static/incremental_batch/base/billing.csv", "fixtures/static/incremental_batch/base/webforms.csv", "fixtures/static/incremental_batch/batch/crm.csv", "fixtures/static/incremental_batch/batch/billing.csv", "fixtures/static/incremental_batch/batch/webforms.csv", "fixtures/static/incremental_batch/expected/batch/membership.csv", "fixtures/static/incremental_batch/expected/batch/events.csv", "fixtures/static/incremental_batch/expected/batch/std_hashes.csv"]
consumes: ["tests/helpers/scenarios.py::load_scenario", "tests/helpers/expected.py::load_expected", "tests/helpers/compare.py::assert_partition_equal", "scripts/validate_fixtures.py", "fixture:base_10", "fixtures/static/base_10/base", "fixtures/static/base_10/expected/base/membership.csv", "fixtures/static/base_10/expected/base/std_hashes.csv", "src/er/ingest/hashing.py::content_hash", "src/er/lake/columns.py::VOLATILE_COLUMNS"]
owns: ["fixtures/static/incremental_batch/scenario.yaml", "fixtures/static/incremental_batch/base", "fixtures/static/incremental_batch/batch", "fixtures/static/incremental_batch/expected", "tests/unit/fixtures/test_incremental_batch.py"]
protected_paths: ["fixtures/static/base_10/base", "fixtures/static/base_10/expected"]
extra_paths: []
attempts: 1
verify: "uv run pytest tests/unit/fixtures/test_incremental_batch.py -q"
branch: "ticket/ER-064-incremental-batch-fixture-3-joiners-1"
commit: ""
spec_sha: "2e62460d9f41a842"
updated_at: "2026-08-17T05:50:38Z"
session: 9d4ee22c-17c4-4d50-92df-2cfbc99939c2
---
## Description

S8.2 requires an `incremental_batch` scenario of six new records — three joining existing entities, two forming a new entity, one bridging two existing entities and forcing a merge — because the 'two new records form a new entity' case is reachable only through the new-vs-new Splink pass (S4.3.4, D2). Without it the second pass can be omitted and every downstream test still passes. This ticket commits the scenario in the S8.2.1 shape with a `scenario.yaml` manifest declaring `base_scenario: base_10` and a per-record pass attribution, plus a unit test that machine-checks the fixture's structural claims against the committed CSVs.

## Scope

### In scope

- `base/` holding the three source CSVs byte-identical to `fixtures/static/base_10/base/`, with `scenario.yaml` declaring `base_scenario: base_10` and `validate_fixtures.py` asserting the byte-identity
- `batch/` holding exactly six new records across the three sources with the literal `base_10` headers of S8.2 and a `persona_id` truth column on every row
- A `scenario.yaml` `attribution:` block assigning each of the six batch `record_key`s exactly one role (`joiner` x3, `bridge` x1, `new_pair` x2) and its expected discovering pass (`pass1` or `pass2`)
- `expected/batch/membership.csv`, `expected/batch/events.csv` and `expected/batch/std_hashes.csv` in the S8.2.1 encoding (symbolic `entity_label`, `\N` null token, byte-sorted)
- A unit test asserting every structural claim above from the committed files alone

### Out of scope

- `parity_pairs.csv` — derived from actual scoring, owned by ER-066
- `tf_flip_pairs.csv` — the T-INC-1b bound, owned by the correction-pass ticket
- `expected/batch/golden.csv` — the golden relations do not exist until M4, and S12's gating rule forbids a milestone asserting on a later milestone's relation
- Running any pipeline stage: this ticket's verify is unit-only and reads committed files
- Asserting that the bridge pair actually scores at or above `auto_merge` — that is a property of the committed model and is verified by ER-065

## Design decisions applied

Implements gap-report B3's fixture half, M18 and M7 under D2. Constraints: (1) S3 shows `incremental_batch/base/`, and S8.2.1 says a missing phase directory means the scenario has no such phase, so `base/` is committed rather than inherited — `base_scenario: base_10` is a byte-identity assertion, not an indirection, which is what keeps the two scenarios from drifting; (2) `entity_label` is allocated per expected file in ascending order of the minimum `record_key` in the group (S8.2.1), so the batch labels are recomputed and MUST NOT be assumed equal to base_10's; (3) the bridge is cross-persona by construction — base_10's ten personas are exactly ten entities (S8.2), so no truthful bridge exists, and `incremental_batch` is a mechanics fixture on which no quality metric is gated (S8.5 gating lives on base_10 via T-MATCH-1a/1b); the manifest records the bridged pair explicitly so the intent cannot be mistaken for a defect; (4) batch `source_record_id`s must not collide with any base key, or the delivery becomes a supersession rather than an insertion (S4.2).

## Acceptance criteria

- [ ] AC1: `fixtures/static/incremental_batch/base/{crm,billing,webforms}.csv` are byte-identical to the same-named files under `fixtures/static/base_10/base/`, and `scenario.yaml` declares `base_scenario: base_10`
- [ ] AC2: `batch/` holds exactly 6 data rows in total across the three CSVs, each file carrying the literal S8.2 header for its source plus `persona_id`, and no batch `(source_system, source_record_id)` equals any base key
- [ ] AC3: `scenario.yaml`'s `attribution:` block covers exactly the six batch `record_key`s with role counts `joiner=3`, `bridge=1`, `new_pair=2`; the two `new_pair` records are the only ones assigned `pass2`, and the other four are assigned `pass1`
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
