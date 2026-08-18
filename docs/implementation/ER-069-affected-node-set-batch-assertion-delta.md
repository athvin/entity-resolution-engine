---
id: ER-069
title: "Affected NODE set: batch ∪ assertion delta ∪ content_hash delta ∪ deletions ∪ blocking-discovered partners ∪ co-members"
milestone: M3
status: in_progress
kind: code
size: M
gates: full
depends_on: ["ER-043", "ER-047", "ER-062", "ER-065"]
spec_refs: ["s3", "s4-5-1", "s4-5-5", "s4-3-4", "s4-4", "s4-2", "s4-1-1", "s5", "s5-2", "s4-0", "s12-1"]
gap_refs: ["B5", "M6", "M15", "D2"]
provides: ["src/er/entities/cluster.py::AffectedSet", "src/er/entities/cluster.py::affected_nodes", "src/er/entities/cluster.py::seed_records", "src/er/entities/cluster.py::partners_of", "src/er/entities/cluster.py::last_reconciled_watermark", "tests/integration/test_affected_nodes.py::test_assertion_delta_widens_without_a_batch"]
consumes: ["src/er/matching/edges.py::current_edges", "src/er/review/assertions.py::active_assertions", "src/er/review/assertions.py::Assertion", "src/er/entities/ids.py::record_key", "src/er/entities/ids.py::canonicalize_pair", "src/er/matching/incremental.py::score_incremental", "src/er/obs/run_context.py::RunContext", "src/er/config/schema.py::Config", "src/er/lake/ducklake.py::connect", "tests/conftest.py::lake_conn", "tests/helpers/scenarios.py::load_scenario", "fixture:base_10", "fixture:incremental_batch", "relation:int_std_records", "relation:int_blocking_keys", "relation:entity_membership", "relation:review_queue", "relation:runs", "relation:ingest_batches"]
owns: ["src/er/entities/cluster.py", "tests/unit/entities/test_affected_nodes.py", "tests/integration/test_affected_nodes.py"]
protected_paths: []
extra_paths: []
attempts: 1
verify: "uv run pytest tests/unit/entities/test_affected_nodes.py -q && bash scripts/ci/itest.sh tests/integration/test_affected_nodes.py -q"
branch: "ticket/ER-069-affected-node-set-batch-assertion-delta"
commit: ""
spec_sha: "7467bdacba1bd84c"
updated_at: "2026-08-18T17:32:21Z"
session: 0ddb196a-cb0c-40c1-aff3-f0043b2489a2
---
## Description

S4.5.1 states the affected NODE set as a formula, not prose: seed = this run's batch records ∪ `content_hash` deltas ∪ tombstones/resurrections ∪ records referenced by the assertion delta ∪ records referenced by review resolutions; partners are the far ends of assertion-adjusted edges at or above `auto_merge`; affected nodes are all current members of the affected entities plus seed and partners. Seeding only from batch records is what made `split_scenario` unexecutable in incremental mode and contradicted S4.4's 'applied identically in incremental and full modes'. This ticket implements the formula in `src/er/entities/cluster.py` (the module S3 assigns the affected node+edge set to) with a pure core and an integration arm proving the non-batch trigger paths.

## Scope

### In scope

- `last_reconciled_watermark(conn)`: the `started_at` of the most recent run whose `reconcile` stage succeeded, against which every 'since the last successful run' delta is computed
- `seed_records`: the five seed arms of S4.5.1, each independently testable
- `partners_of`: far ends of assertion-adjusted edges with `p >= auto_merge` incident to a seed record
- `affected_nodes`: `all current members of the entities of (seed ∪ partners)` ∪ seed ∪ partners, returned with the entity ids it widened through
- A pure core taking already-loaded rows (unit-testable without a lake) plus thin loaders that read the relations

### Out of scope

- The affected EDGE set and assertion adjustment of the edge list (ER-070)
- Label propagation and the convergence bound (ER-071)
- Reconciliation, membership writes and the exit-`10` wiring on an empty affected set (ER-074)
- Edge invalidation on supersession or tombstone (ER-082, ER-083) — this ticket consumes the resulting deltas, it does not produce them
- The never-cut widening (ER-076)

## Design decisions applied

Implements gap-report B5's affected-set amendment plus M6, M15 and D2. Constraints: (1) partners are defined by EDGES at or above `auto_merge`, not by blocking co-keys — S4.3.4 names `int_blocking_keys` the driver of the touched-subgraph computation because Splink's candidates come from the same rules, but the normative formula in S4.5.1 is edge-based, so `cluster.py` must not join `int_blocking_keys` into the node set; (2) gray-band edges (`review_low <= p < auto_merge`) create no partners; (3) the assertion arm reads the delta itself — created OR retracted since the watermark — which is the only thing that makes a `never` asserted on two non-batch records executable in incremental mode; (4) tombstoned records are absent from `int_std_records` (S4.2) but MUST still enter the seed, so the tombstone arm reads `raw_records`/`ingest_batches`, not the standardized view; (5) the widening is transitive through entities only, never through edges — one hop of entity membership, exactly as the formula is written.

## Acceptance criteria

- [ ] AC1: Unit: given a synthetic membership partition and edge list, `affected_nodes` for a seed record assigned to entity E returns every current member of E plus the seed; a seed record with no `entity_membership` row returns just itself (plus any partners)
- [ ] AC2: Unit: a partner is produced for an edge at `p >= auto_merge` and NOT for an edge in `[review_low, auto_merge)`; an `always` assertion between a seed record and a non-seed record produces a partner even with no scored edge
- [ ] AC3: Integration: an `always` assertion added between two records that appear in NO batch (and after the last successful run) yields a non-empty affected set containing both endpoints and every co-member of both endpoints' entities
- [ ] AC4: Integration: a re-delivered record whose `content_hash` changed enters the seed and widens the affected set to its entity's full membership even though no other member appears in the batch
- [ ] AC5: Integration: a tombstoned key and a resurrected key each enter the seed; the tombstoned key is in the seed despite being absent from `int_std_records`
- [ ] AC6: Integration: a `review_queue` row resolved after the watermark puts both of its endpoints in the seed
- [ ] AC7: Integration: re-running with no ingest, no assertion delta, no hash change and no deletion yields an empty affected set (`affected_nodes` returns an empty node set and an empty entity set)
- [ ] AC8: `src/er/entities/cluster.py` contains no reference to `int_blocking_keys` (grep-clean), and the partner rule reads only the assertion-adjusted edge set

## Tests

- tests/unit/entities/test_affected_nodes.py::test_entity_widening_from_one_seed
- tests/unit/entities/test_affected_nodes.py::test_partners_require_auto_merge
- tests/unit/entities/test_affected_nodes.py::test_unassigned_seed_returns_itself
- tests/unit/entities/test_affected_nodes.py::test_always_assertion_creates_a_partner
- tests/integration/test_affected_nodes.py::test_assertion_delta_widens_without_a_batch
- tests/integration/test_affected_nodes.py::test_content_hash_delta_widens_to_full_membership
- tests/integration/test_affected_nodes.py::test_tombstone_and_resurrection_seed
- tests/integration/test_affected_nodes.py::test_review_resolution_seed
- tests/integration/test_affected_nodes.py::test_unchanged_rerun_is_empty

## Verification

```bash
uv run pytest tests/unit/entities/test_affected_nodes.py -q
bash scripts/ci/itest.sh tests/integration/test_affected_nodes.py -q
uv run mypy --strict src/er/entities/cluster.py
```

## Definition of Done

- Each of the five seed arms of S4.5.1 has its own named test
- The watermark helper is used by every delta arm — no arm computes its own cutoff
- The pure core is importable and testable with no lake connection
- `bash scripts/gates.sh` green; INTERFACES entry lists `affected_nodes`, `seed_records`, `partners_of`, `last_reconciled_watermark`
