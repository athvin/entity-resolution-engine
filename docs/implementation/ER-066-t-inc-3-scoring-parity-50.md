---
id: ER-066
title: "T-INC-3 scoring parity (50 committed pairs) + T-MATCH-SYM orientation invariance"
milestone: M3
status: done
kind: code
size: S
gates: full
depends_on: ["ER-058", "ER-065"]
spec_refs: ["s4-2", "s4-3", "s4-3-1", "s4-3-3", "s4-3-4", "s8-2-1", "s8-3", "s8-4"]
gap_refs: ["B3", "M13", "M9"]
provides: ["fixtures/static/incremental_batch/parity_pairs.csv", "tests/helpers/parity.py::derive_parity_pairs", "tests/helpers/parity.py::write_parity_pairs", "tests/integration/test_scoring_parity.py::test_incremental_and_full_scores_are_bit_equal", "tests/integration/test_scoring_parity.py::test_score_is_orientation_invariant"]
consumes: ["src/er/matching/incremental.py::score_incremental", "src/er/matching/full.py::score_full", "src/er/matching/model.py::build_settings", "src/er/matching/model.py::blocking_rules_from_config", "src/er/matching/tf.py::register_tf", "src/er/lake/ducklake.py::splink_api", "src/er/entities/ids.py::canonicalize_pair", "tests/helpers/pairs.py::blocked_pairs", "tests/helpers/scenarios.py::load_scenario", "tests/conftest.py::lake_conn", "tests/conftest.py::sub_namespace", "fixture:incremental_batch", "fixture:base_10", "fixtures/static/model_test_v1.json", "relation:match_scores"]
owns: ["fixtures/static/incremental_batch/parity_pairs.csv", "tests/helpers/parity.py", "tests/integration/test_scoring_parity.py"]
protected_paths: ["src/er/matching/incremental.py", "src/er/matching/full.py", "fixtures/static/model_test_v1.json", "fixtures/static/incremental_batch/base", "fixtures/static/incremental_batch/batch"]
extra_paths: []
attempts: 1
verify: "bash scripts/ci/itest.sh tests/integration/test_scoring_parity.py -q"
branch: "ticket/ER-066-t-inc-3-scoring-parity-50"
commit: "b3cd98d480291c586d3193c6c7f5b5a0591f04f1"
spec_sha: "7467bdacba1bd84c"
updated_at: "2026-08-23T13:08:53Z"
session: c379438e-8c95-4be8-8f5f-3832d8ceafaa
---
## Description

T-INC-3 is the oracle that localises a T-INC-1 failure to clustering rather than scoring: the same pairs scored through the incremental two-pass path and through the corpus-wide `full.py` pass, at one pinned `model_version` and `tf_snapshot_id`, must produce bit-equal `match_probability` (S8.3, S4.3.4). T-MATCH-SYM is the companion guard that scoring is orientation-invariant, which is only provable because S4.2 guarantees the normalized `given_name` is element 0 of its own `name_variants` array. This ticket commits the derived `parity_pairs.csv` for `incremental_batch` and implements both tests.

## Scope

### In scope

- `derive_parity_pairs`: recompute the S8.2.1 parity set (pairs scored by both paths, i.e. pairs with at least one endpoint in `batch/` clearing `review_low`) and compare it, as a set, to the committed file
- `fixtures/static/incremental_batch/parity_pairs.csv` with the literal header `rec_a_key,rec_b_key`, byte-sorted per S8.2.1, regenerable via `write_parity_pairs`
- Bit-equal probability comparison for every parity pair, with the pinned `(model_version, tf_snapshot_id, config_hash, std_version)` asserted before the comparison
- T-MATCH-SYM over every `base_10` blocked pair: `compare_two_records(a,b).match_probability == compare_two_records(b,a).match_probability` within `1e-12`, plus the `name_variants` element-0 sub-assertion on every `base_10` record

### Out of scope

- T-INC-1 (partition equivalence across two universes) — ER-093
- T-INC-1b and `tf_flip_pairs.csv` — the correction-pass ticket
- Any change to `incremental.py` or `full.py`: if the two paths disagree, this ticket blocks rather than edits the implementations it is testing
- Reconciling the S8.3 node ids `tests/integration/test_incremental.py::test_incremental_and_full_scores_are_bit_equal` and `tests/integration/test_matching.py::test_score_is_orientation_invariant` — the node-id resolution ticket re-exports these functions; this ticket only fixes their names

## Design decisions applied

Implements gap-report B3's acceptance gate, M13 and M9 under D2. Constraints: (1) the board title's '50 committed pairs' is indicative of scale only — S8.2.1 is normative that `parity_pairs.csv` is DERIVED and that no test asserts a fixed pair count, so the test asserts non-emptiness and set equality with the committed file and nothing about cardinality; (2) 'bit-equal' is `==` on the DOUBLE, not `pytest.approx` — the S8.2.1 `1e-9` tolerance applies to expected-file comparison, not to this parity assertion; (3) both arms must be pinned to the same `model_version` AND `tf_snapshot_id` and that pinning is asserted before the probabilities are compared, so a failure cannot be blamed on TF drift; (4) the function names in this file are exactly the S8.3 node-id names so the S8.3 rows resolve once re-exported; (5) regeneration is deliberate and visible: setting `ER_REGEN_PARITY=1` rewrites the file in S8.2.1 sort order and fails the test, so a silent shrink of the parity set can never pass unnoticed.

## Acceptance criteria

- [ ] AC1: The recomputed parity set is non-empty and set-equal to `fixtures/static/incremental_batch/parity_pairs.csv`; on mismatch the test prints the symmetric difference in both directions
- [ ] AC2: Every pair in the recomputed set has at least one endpoint in `batch/` and a `match_probability >= review_low` on both paths
- [ ] AC3: For every parity pair, the incremental probability and the full probability are exactly equal (`==`), and the test asserts beforehand that both arms ran at the same `model_version` and the same `tf_snapshot_id`
- [ ] AC4: Perturbing one committed row of `parity_pairs.csv` (added, removed or reordered endpoints) makes the test fail with the symmetric-difference message rather than passing
- [ ] AC5: With `ER_REGEN_PARITY=1` the test rewrites `parity_pairs.csv` byte-sorted with the literal header `rec_a_key,rec_b_key` and exits non-zero
- [ ] AC6: For every `base_10` blocked pair, `compare_two_records(a,b)` and `compare_two_records(b,a)` differ by at most `1e-12`, and the failure message names the offending pair and both probabilities
- [ ] AC7: For every `base_10` record, the normalized `given_name` is element 0 of its own `name_variants` array (the precondition `variant_match` orientation independence rests on)
- [ ] AC8: No file under `src/er/matching/` is modified by this ticket

## Tests

- tests/integration/test_scoring_parity.py::test_incremental_and_full_scores_are_bit_equal
- tests/integration/test_scoring_parity.py::test_parity_pairs_file_matches_derivation
- tests/integration/test_scoring_parity.py::test_score_is_orientation_invariant
- tests/integration/test_scoring_parity.py::test_name_variants_element_zero_symmetry

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_scoring_parity.py -q
uv run pytest tests/unit/test_fixture_lint.py -q
uv run mypy --strict tests/helpers/parity.py
```

## Definition of Done

- `parity_pairs.csv` is committed, byte-sorted, header-literal, and contains no count assertion anywhere in the test file
- The two S8.3-named test functions exist under exactly those names
- The bit-equality assertion uses `==`, not a tolerance, and the file contains no `approx` call for it
- `src/er/matching/` and the committed model are unmodified
- `bash scripts/gates.sh` green; INTERFACES entry lists `derive_parity_pairs`
