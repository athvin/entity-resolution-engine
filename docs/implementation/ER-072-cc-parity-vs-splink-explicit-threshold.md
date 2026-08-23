---
id: ER-072
title: "CC parity vs Splink (explicit threshold_match_probability) + assert_membership_equals_components (T-INV-1) autouse finalizer"
milestone: M3
status: done
kind: code
size: M
gates: full
depends_on: ["ER-056", "ER-071"]
spec_refs: ["s4-5-2", "s4-3", "s4-5-1", "s8-3", "s8-1", "s5-0", "s12-1"]
gap_refs: ["B5", "M3", "MINOR-thresholds"]
provides: ["src/er/entities/cluster.py::cluster_full", "tests/helpers/invariants.py::assert_membership_equals_components", "tests/helpers/invariants.py::current_partition", "tests/integration/test_invariants.py::test_membership_equals_connected_components", "fixture:integration autouse T-INV-1 finalizer"]
consumes: ["src/er/entities/cluster.py::label_propagate", "src/er/entities/affected.py::affected_edges", "src/er/matching/current_edges.py::current_edges", "src/er/config/schema.py::ThresholdsConfig", "fixtures/static/model_test_v1.json", "ER-049::splink_api", "ER-049::assert_no_splink_relations_in_lake", "tests/helpers/compare.py::assert_partition_equal"]
owns: ["tests/helpers/invariants.py", "tests/integration/test_invariants.py", "tests/integration/test_clustering_parity.py", "tests/unit/entities/test_cluster_threshold.py"]
protected_paths: []
extra_paths: ["src/er/entities/cluster.py", "tests/integration/conftest.py"]
attempts: 1
verify: "bash scripts/ci/itest.sh tests/integration/test_clustering_parity.py -q && uv run pytest tests/unit/entities/test_cluster_threshold.py -q"
branch: "ticket/ER-072-cc-parity-vs-splink-explicit-threshold"
commit: "ce8a1dd0064303b3f3de890be906b76ea7376400"
spec_sha: "7467bdacba1bd84c"
updated_at: "2026-08-23T22:40:07Z"
session: c379438e-8c95-4be8-8f5f-3832d8ceafaa
---
## Description

Close the drift between the two clustering implementations S4.5.2 specifies and arm the suite's only standing invariant. `cluster_full` wraps `linker.clustering.cluster_pairwise_predictions_at_threshold(threshold_match_probability=auto_merge)` — passed **explicitly**, because omitting it treats every supplied edge as a match (S4.3, D13) — over the same assertion-adjusted, cut-excluded edge set the incremental path consumes. `assert_membership_equals_components` implements T-INV-1 (S8.3) and is registered in `tests/integration/conftest.py` as an autouse session-and-function finalizer, reported under the node id `tests/integration/test_invariants.py::test_membership_equals_connected_components`.

## Scope

### In scope

- `cluster_full(conn, edges, auto_merge)` in `src/er/entities/cluster.py`, threshold always passed explicitly and sourced from `thresholds.auto_merge`
- parity assertion: label propagation and `cluster_full` produce identical set-partitions over one edge set, symmetric difference printed on failure
- `tests/helpers/invariants.py::assert_membership_equals_components` asserting all five T-INV-1 clauses
- autouse finalizer registration in `tests/integration/conftest.py` and the collectible reporting node in `tests/integration/test_invariants.py`
- empty-membership tolerance: the finalizer is meaningful before ER-074 exists and must pass (not skip) on an empty `entity_membership`

### Out of scope

- writing `entity_membership` (ER-074)
- cut_edges exclusion inside the invariant — ER-076 extends this helper to the post-cut edge set
- the label-propagation algorithm itself (ER-071)
- T-MATCH-1b cluster-level quality (ER-081)

## Design decisions applied

Implements B5's 'both paths consume the identical edge set', M3's 'state what `entity_membership` means', MINOR-thresholds (D13: the clustering cut IS `auto_merge`, passed explicitly). Easy to miss: (a) the finalizer must run after **every** integration test and report failures against T-INV-1's node id rather than the scenario's, per S8.3's closing paragraph; (b) at this point on the board `entity_membership` is still empty — the helper must therefore assert its non-membership clauses (one row per record where rows exist, active entity references, canonical pair ordering on `match_scores`/`assertions`/`review_queue`, zero `__splink__%` relations) and pass vacuously on the membership clause rather than being disabled; (c) `cluster_full` is added to `src/er/entities/cluster.py` (owned by ER-071) — do not create a second clustering module, and do not put it in `src/er/matching/full.py`, which owns scoring only.

## Acceptance criteria

- [ ] AC1: Over base_10's affected edge set at the committed fixture model, `label_propagate` and `cluster_full` return the same set of frozensets; the test prints the symmetric difference on failure.
- [ ] AC2: A spy on the Splink clustering call asserts `threshold_match_probability` is present as a keyword and equals `thresholds.auto_merge` from the validated config; removing the kwarg fails the unit test.
- [ ] AC3: An edge set containing an edge with `match_probability < auto_merge` yields a partition in which that edge's endpoints are in different components — the observable consequence of passing the threshold.
- [ ] AC4: `assert_membership_equals_components` raises an AssertionError naming the offending `record_key` when one `entity_membership` row is repointed to a different `entity_id`, and passes again once restored.
- [ ] AC5: `assert_membership_equals_components` passes against an empty `entity_membership` while still asserting canonical `rec_a_key < rec_b_key` on `match_scores`/`assertions`/`review_queue` and zero `__splink__%` relations in `lake`.
- [ ] AC6: Running `pytest tests/integration/test_clustering_parity.py -q` under the harness reports a deliberately induced invariant failure under the node id `tests/integration/test_invariants.py::test_membership_equals_connected_components`, verified from the junit XML.
- [ ] AC7: `tests/integration/test_invariants.py::test_membership_equals_connected_components` is collectible by pytest (S8.3 node-id resolution).

## Tests

- tests/unit/entities/test_cluster_threshold.py::test_cluster_call_passes_explicit_threshold
- tests/unit/entities/test_cluster_threshold.py::test_threshold_value_comes_from_config_auto_merge
- tests/integration/test_clustering_parity.py::test_label_propagation_equals_splink_components
- tests/integration/test_clustering_parity.py::test_sub_threshold_edge_is_not_clustered
- tests/integration/test_clustering_parity.py::test_invariant_helper_detects_membership_drift
- tests/integration/test_invariants.py::test_membership_equals_connected_components

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_clustering_parity.py -q && uv run pytest tests/unit/entities/test_cluster_threshold.py -q
bash scripts/ci/itest.sh tests/integration/test_invariants.py -q
uv run mypy --strict src/er tests/helpers
```

## Definition of Done

- `cluster_full` never calls the Splink clustering API without `threshold_match_probability`
- Parity assertion is bidirectional set equality of frozensets, not a count comparison
- `assert_membership_equals_components` lives in exactly one place and is imported by the conftest finalizer
- The finalizer is autouse at both session and function scope and does not swallow failures
- `tests/integration/test_invariants.py` resolves as a real collectible node id for S8.3
- Verify command green; `mypy --strict` clean over `tests/helpers`
