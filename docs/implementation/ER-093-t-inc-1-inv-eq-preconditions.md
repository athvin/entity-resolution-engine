---
id: ER-093
title: "T-INC-1 with INV-EQ preconditions, two isolated sub-namespaces, partition + golden equality"
milestone: M4
status: todo
kind: code
size: L
gates: full
depends_on: ["ER-026", "ER-061", "ER-065", "ER-084", "ER-092"]
spec_refs: ["s1", "s4-0", "s4-3-3", "s4-3-4", "s4-5-6", "s8-1", "s8-2-1", "s8-3"]
gap_refs: ["M18", "M9", "M7"]
provides: ["tests/integration/test_inc_equivalence.py::test_incremental_equals_full", "tests/integration/test_inc_equivalence.py::assert_inv_eq_preconditions", "fixtures/static/incremental_batch/expected/batch/golden.csv"]
consumes: ["tests/conftest.py::sub_namespace", "tests/helpers/compare.py::assert_partition_equal", "tests/helpers/compare.py::assert_golden_equal", "tests/helpers/expected.py::load_scenario", "fixtures/static/incremental_batch/expected/batch/membership.csv", "fixtures/static/model_test_v1.json", "src/er/matching/incremental.py", "src/er/matching/tf.py::register_tf", "src/er/golden/assemble.py::assemble", "src/er/config/hashing.py::config_hash", "relation:entity_membership", "relation:runs", "scripts/ci/itest.sh"]
owns: ["tests/integration/test_inc_equivalence.py", "fixtures/static/incremental_batch/expected/batch/golden.csv"]
protected_paths: ["fixtures/static/incremental_batch/expected/batch/membership.csv", "fixtures/static/incremental_batch/base/", "fixtures/static/incremental_batch/batch/", "src/er/matching/incremental.py", "src/er/entities/"]
extra_paths: []
attempts: 0
verify: "bash scripts/ci/itest.sh tests/integration/test_inc_equivalence.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

T-INC-1 is the acceptance test for G3: under the INV-EQ preconditions of S4.5.6 an incremental run yields the same set-partition of current members as a from-scratch full run. This ticket builds it as two isolated sub-namespaces from the `sub_namespace` fixture — universe A runs `er run-all --mode full` over `base/ ∪ batch/` at once, universe B runs `base/` then `batch/` incrementally — asserts the four INV-EQ preconditions explicitly before comparing, and then compares each universe against the committed ID-insensitive expectation for membership and golden values. Entity ids legitimately differ between the two universes and are never compared; equality of both universes to one expectation is equality to each other.

## Scope

### In scope

- `sub_namespace`-based construction of two independent lake namespaces (`er_test_<ns>_a` / `er_test_<ns>_b`) with teardown of both under `try/finally`
- Explicit precondition assertions before comparison: identical `model_version`, identical `tf_snapshot_id`, identical `config_hash` and `std_version`, identical active assertion set, append-only corpus (no `content_hash` change, no deletion)
- `assert_partition_equal(A, expected/batch/membership.csv)` and `assert_partition_equal(B, ...)`, then `assert_golden_equal` for both with their own label maps
- Authoring and committing `fixtures/static/incremental_batch/expected/batch/golden.csv` in the S8.2.1 format
- A negative arm proving the comparison is not vacuous

### Out of scope

- Violating an INV-EQ precondition and asserting the divergence, and the correction pass that repairs it (T-INC-1b / T-CORR-1, ER-094)
- Scoring parity between the two code paths (T-INC-3, ER-066) — a failure here with T-INC-3 green localises to clustering
- Any change to the incremental matching, clustering or reconciliation modules to make the comparison pass
- Comparing entity ids across universes, or asserting snapshot counts

## Design decisions applied

Closes M18 (INV-EQ is stated and its preconditions are asserted, not assumed), M9 (both arms are forced onto one `tf_snapshot_id`, so corpus-dependent TF cannot silently differentiate them) and M7 (ID-insensitive comparison against a symbolic-label expectation). Easy to miss: both arms MUST load `fixtures/static/model_test_v1.json` and never train (S8.3); the `tf_snapshot_id` is pinned by registering the committed `tf_lookup`, not recomputed; `assert_partition_equal` compares a set of frozensets of `record_key`, so a run that re-mints every entity still passes it — which is why the negative arm below is mandatory. Integration tests run single-process (S8.1); the two universes are sequential, not concurrent, because v1 is a single-writer batch model. Follows the `SPEC_TEST_IDS` convention introduced in ER-092.

## Acceptance criteria

- [ ] AC1: `bash scripts/ci/itest.sh tests/integration/test_inc_equivalence.py -q` exits 0.
- [ ] AC2: Before any comparison the test asserts A and B report the same `model_version`, the same `tf_snapshot_id`, the same `config_hash`, the same `std_version` and the same active assertion set, and the test FAILS with a named precondition message when any one is perturbed (covered by a parametrised arm that perturbs `tf_snapshot_id`).
- [ ] AC3: Universe A and universe B attach distinct `METADATA_SCHEMA` and `DATA_PATH` values read back from the live connection, and both namespaces are dropped on teardown even when the body fails.
- [ ] AC4: `assert_partition_equal` passes for both universes against `fixtures/static/incremental_batch/expected/batch/membership.csv`, and `assert_golden_equal` passes for both against the committed `expected/batch/golden.csv` with each universe's own label map.
- [ ] AC5: The negative arm merges two entities in universe B's `entity_membership` and asserts `assert_partition_equal` then raises `AssertionError` — the comparison is proven sensitive.
- [ ] AC6: No entity_id is compared across universes: the test contains no assertion relating an A entity_id to a B entity_id (enforced by construction and asserted in review, with the label maps kept per-universe).
- [ ] AC7: T-INV-1's autouse finalizer passes in both sub-namespaces after the run.

## Tests

- tests/integration/test_inc_equivalence.py::test_incremental_equals_full
- tests/integration/test_inc_equivalence.py::test_inv_eq_preconditions_are_asserted
- tests/integration/test_inc_equivalence.py::test_partition_comparison_is_sensitive

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_inc_equivalence.py -q
uv run pytest tests/unit/fixtures -q
uv run ruff check tests && uv run ruff format --check tests
```

## Definition of Done

- T-INC-1 green with both arms compared against the one committed ID-insensitive expectation.
- `expected/batch/golden.csv` committed in S8.2.1 order, with `\N` nulls, `entity_label` instead of `entity_id` and no `assembled_at` column; the fixture-format lint passes on it.
- The four INV-EQ preconditions are asserted in code before the comparison, each with its own message.
- The sensitivity arm proves `assert_partition_equal` fails on a perturbed partition.
- No production module under `src/er/matching/` or `src/er/entities/` modified; both sub-namespaces torn down.
- Gate receipt recorded and `board.py complete` run.
