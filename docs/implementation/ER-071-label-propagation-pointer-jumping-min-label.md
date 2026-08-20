---
id: ER-071
title: "Label propagation: pointer-jumping min-label, ceil(log2 n)+1 bound, clustering.max_iterations, hard non-convergence failure"
milestone: M3
status: blocked
kind: code
size: M
gates: full
depends_on: ["ER-011", "ER-049", "ER-070"]
spec_refs: ["s4-5-2", "s4-5-1", "s4-5-6", "s4-0b", "s4-7", "s6", "s6-1"]
gap_refs: ["B5", "M17", "M26"]
provides: ["src/er/entities/cluster.py::label_propagate", "src/er/entities/cluster.py::LabelPropagationResult", "src/er/entities/cluster.py::MAX_ITERATION_BOUND", "src/er/errors.py::NonConvergenceError", "counter:label_prop_iterations"]
consumes: ["src/er/entities/affected.py::affected_nodes", "src/er/entities/affected.py::affected_edges", "src/er/config/schema.py::ClusteringConfig", "src/er/entities/ids.py::record_key", "ER-049::splink_api", "ER-049::assert_no_splink_relations_in_lake", "ER-016::lake connection factory"]
owns: ["tests/unit/entities/test_label_propagation.py", "tests/integration/test_clustering.py"]
protected_paths: []
extra_paths: ["src/er/errors.py", "src/er/entities/cluster.py"]
attempts: 0
verify: "uv run pytest tests/unit/entities/test_label_propagation.py -q && bash scripts/ci/itest.sh tests/integration/test_clustering.py -q"
branch: ""
commit: ""
spec_sha: "7467bdacba1bd84c"
updated_at: "2026-08-20T20:35:21Z"
session: 1f99f231-79a8-4655-a027-6abc6d48e477
---
## Description

Build the incremental clustering primitive of S4.5.2: iterative min-label propagation over the affected subgraph, where `label(v) = min(record_key)` over the closed neighbourhood, propagated to fixpoint. The loop runs entirely in the in-memory DuckDB database (S4.0b) so it cannot commit one snapshot per iteration and cannot leave a `__splink__` or scratch relation in `lake`. It is bounded by `clustering.max_iterations` (S6, default 50) and fails hard on non-convergence — exit `1`, error_class `non_convergence` (S4.7), no snapshot, no events — logging the unconverged component's size and minimum `record_key`. Pointer jumping halves path length per round, so a component of n nodes settles in at most `ceil(log2 n)+1` rounds; the configured cap is the safety net, not the expected count.

## Scope

### In scope

- `label_propagate(nodes, edges, max_iterations)` returning `{record_key: label}` plus the iteration count actually used
- pointer-jumping implementation over in-memory temp tables on the run connection; only the returned labelling ever leaves the function
- isolated nodes (present in `affected_nodes`, incident to no edge) labelled with themselves
- hard non-convergence raise carrying component size and minimum `record_key`, mapped to error_class `non_convergence`
- `label_prop_iterations` exposed for the reconcile stage counters payload (S4.5.6)

### Out of scope

- Splink connected-components parity and the explicit `threshold_match_probability` call (ER-072)
- cluster -> entity_id mapping, INV-PERM, events (ER-073/ER-074)
- wiring `er reconcile`'s exit codes and `run_stages` row (ER-074)
- never-cut / `cut_edges` (ER-076)
- constructing the affected node or edge set (ER-069/ER-070) — consume them, do not re-derive

## Design decisions applied

Implements B5's label-propagation pin (min-`record_key` label over the closed neighbourhood to fixpoint, bounded, hard failure), M17 (the loop is in-memory; nothing per-iteration reaches the lake), and M26 (`clustering.max_iterations` comes from the validated config, never a literal). Easy to miss: (a) nodes with no incident edge MUST appear in the output labelled with themselves, or the 'record leaving all clusters becomes a singleton' path in reconcile has no input; (b) the returned mapping must be independent of input iteration order and of the orientation of each pair, because it feeds a determinism claim (D1/D2, S4.5.4); (c) `ceil(log2 n)+1` is an assertion about the algorithm, `clustering.max_iterations` is the configured cap — both must be enforced and they are not the same number.

## Acceptance criteria

- [ ] AC1: `label_propagate` over a 1024-node path graph labels every node with the component minimum and reports `iterations <= ceil(log2(1024)) + 1 == 11`.
- [ ] AC2: A node passed in `nodes` with no incident edge appears in the returned mapping labelled with itself.
- [ ] AC3: Shuffling the edge iterable and swapping both endpoints of every pair yields an identical output mapping (compared for exact equality).
- [ ] AC4: Called with `max_iterations=1` on an 8-node path graph, `label_propagate` raises `NonConvergenceError`, returns no labels, and the exception message contains the unconverged component's size and its minimum `record_key`.
- [ ] AC5: `NonConvergenceError` carries `error_class == 'non_convergence'` and the S4.7 exit code `1` (asserted on the error object, not on a CLI invocation).
- [ ] AC6: After `tests/integration/test_clustering.py` propagates over base_10's affected subgraph, the partition equals the connected components computed by an independent reference implementation in the test, and `assert_no_splink_relations_in_lake` passes.
- [ ] AC7: The integration test asserts the set of relations in `lake.main` is unchanged across the propagation call — the loop creates no lake table and commits no membership.

## Tests

- tests/unit/entities/test_label_propagation.py::test_min_label_over_closed_neighbourhood
- tests/unit/entities/test_label_propagation.py::test_pointer_jumping_converges_within_log2_bound
- tests/unit/entities/test_label_propagation.py::test_isolated_node_labels_itself
- tests/unit/entities/test_label_propagation.py::test_output_is_order_and_orientation_independent
- tests/unit/entities/test_label_propagation.py::test_exceeding_max_iterations_raises_non_convergence
- tests/integration/test_clustering.py::test_components_match_reference_on_base_10
- tests/integration/test_clustering.py::test_loop_writes_no_lake_relation_and_no_splink_scratch

## Verification

```bash
uv run pytest tests/unit/entities/test_label_propagation.py -q && bash scripts/ci/itest.sh tests/integration/test_clustering.py -q
uv run mypy --strict src/er
uv run ruff check . && uv run ruff format --check .
```

## Definition of Done

- `src/er/entities/cluster.py` exports `label_propagate` and `LabelPropagationResult`, both typed under `mypy --strict`
- `clustering.max_iterations` is read from the validated config; no literal iteration cap appears in the module
- Non-convergence raises before any write and is classified `non_convergence` in the S4.7 taxonomy
- `label_prop_iterations` is returned by the call so ER-074 can put it in `run_stages.counters`
- Integration test proves zero lake relations and zero `__splink__%` relations are created by the loop
- Both verify commands pass on a clean namespace; `scripts/gates.sh` green

## Blocker log

### Attempt 0 — gate_failed (2026-08-20T20:35:21Z)

- **Failing command:** `bash scripts/gates.sh --scope full --no-cache (driver re-verification on main)`
- **Assertion / contradiction:** The ticket was marked done, but an independent full-ladder run on merged main failed. See /Users/athvin/github.com/athvin/entity-resolution-engine/.loop/runs/3be15946-408e-44df-8029-220d3aca8849/reverify-ER-071.log
- **Smallest change that would unblock:** Inspect branch loop-quarantine/ER-071, fix the failing gate, then unblock the ticket.
- **Log:** `/Users/athvin/github.com/athvin/entity-resolution-engine/.loop/runs/3be15946-408e-44df-8029-220d3aca8849/reverify-ER-071.log`
