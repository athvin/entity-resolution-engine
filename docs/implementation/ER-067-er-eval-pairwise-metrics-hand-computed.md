---
id: ER-067
title: "er.eval.pairwise_metrics + hand-computed unit + blocking-recall & edge-level T-MATCH-1a on base_10"
milestone: M3
status: todo
kind: code
size: M
gates: full
depends_on: ["ER-027", "ER-058", "ER-060"]
spec_refs: ["s8-5", "s8-3", "s8-2", "s4-3", "s5", "s5-0", "s9-1"]
gap_refs: ["M21"]
provides: ["src/er/eval/__init__.py", "src/er/eval/metrics.py::pairwise_metrics", "src/er/eval/metrics.py::PairwiseMetrics", "tests/helpers/quality.py::truth_pairs", "tests/helpers/quality.py::all_pairs_universe", "tests/helpers/quality.py::blocked_universe", "tests/helpers/quality.py::predicted_edges_at", "tests/integration/test_match_quality.py::test_edge_quality_base_10"]
consumes: ["src/er/entities/ids.py::record_key", "src/er/entities/ids.py::canonicalize_pair", "tests/helpers/pairs.py::blocked_pairs", "tests/helpers/expected.py::load_expected", "tests/helpers/scenarios.py::load_scenario", "tests/conftest.py::lake_conn", "fixture:base_10", "fixtures/static/base_10/truth.csv", "fixtures/static/model_test_v1.json", "relation:match_scores", "relation:int_blocking_keys"]
owns: ["src/er/eval/metrics.py", "tests/helpers/quality.py", "tests/unit/eval/__init__.py", "tests/unit/eval/test_pairwise_metrics.py", "tests/integration/test_match_quality.py"]
protected_paths: ["fixtures/static/base_10", "fixtures/static/model_test_v1.json"]
extra_paths: ["scripts/lint_metrics.py", "src/er/eval/__init__.py"]
attempts: 0
verify: "bash scripts/ci/itest.sh tests/integration/test_match_quality.py -q && uv run pytest tests/unit/eval/test_pairwise_metrics.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

S8.5 makes one function — `er.eval.pairwise_metrics(predicted, truth, universe)` — the only precision/recall implementation in the repository, and defines the three `predicted`/`truth`/`universe` triples: blocking recall over the full `C(n,2)` set, edge-level over the blocked set, and cluster-level over the full set. This ticket ships that function with its degenerate-input contract, and the edge-level arm of T-MATCH-1a on `base_10`: blocking recall `== 1.0` against the 18 committed true pairs, zero false positives at `auto_merge`, and at most one missed true pair which must satisfy the S8.2 authoring constraint. Blocking recall over the full universe is the only number that can catch a blocking regression, which is why it is gated here rather than reported.

## Scope

### In scope

- `src/er/eval/metrics.py`: `pairwise_metrics` returning precision, recall, f1 and raw tp/fp/fn, with a documented convention for empty `predicted`/`truth` and a hard raise for out-of-universe or non-canonical pairs
- `tests/helpers/quality.py`: the three triples of S8.5 built from `truth.csv`, `int_blocking_keys` and `match_scores`
- T-MATCH-1a: blocking recall, edge-level precision/recall at `auto_merge` as absolute counts, and the four named sub-assertions (robert/bob present, typo surname present, shared household absent, the two `test@test.com` records produce no edge)
- The missed-pair guard: if one true pair is missed at `auto_merge`, assert it lies inside a persona of >= 3 records and that removing it leaves that persona's above-`auto_merge` edges connected
- Keeping `scripts/lint_metrics.py` honest: a test that injects a second precision implementation and asserts the lint fails

### Out of scope

- Cluster-level T-MATCH-1b over the transitive closure of `entity_membership` (ER-081) — membership is not populated by this ticket's chain
- The benchmark quality block (ER-100), which calls the same function
- Any change to blocking rules, the settings builder or the committed model to move a number: if the gate fails, the fixture or the model is wrong and this ticket blocks
- Reporting metrics into `artifacts/` — S8.5 gates here and reports in the benchmark

## Design decisions applied

Implements gap-report M21 and S8.5. Constraints: (1) exactly one implementation may exist — `scripts/lint_metrics.py` fails the static job on a second definition, so no helper in `tests/` may recompute precision or recall; the helpers here only build the three sets; (2) the degenerate conventions are pinned by this ticket and unit-tested: empty `predicted` yields precision `1.0` (vacuously, tp=fp=0), empty `truth` yields recall `1.0`, and `f1 = 0.0` when precision + recall == 0; (3) a `predicted` or `truth` pair outside `universe`, a universe smaller than `predicted`, and any non-canonical pair (`a >= b`) all raise rather than being silently coerced; (4) `base_10`'s ground truth is fixed at 23 records / 10 personas / 18 true pairs over `C(23,2) = 253` (S8.2) and is read from the committed truth file, never hard-coded twice; (5) the missed-pair tolerance is exactly one and is conditional on the S8.2 authoring constraint — a missed pair inside a 2-record persona fails the test even though the count tolerance would allow it.

## Acceptance criteria

- [ ] AC1: `pairwise_metrics({(a,b),(a,c)}, {(a,b),(b,c)}, C(3,2))` returns `tp=1, fp=1, fn=1, precision=0.5, recall=0.5, f1=0.5`, checked against hand-computed values
- [ ] AC2: Empty `predicted` with non-empty `truth` returns `precision=1.0, recall=0.0, f1=0.0`; empty `truth` with non-empty `predicted` returns `recall=1.0`; all-singletons and all-in-one partitions produce the documented counts
- [ ] AC3: A `predicted` or `truth` pair outside `universe`, a `universe` smaller than `predicted`, and a non-canonical pair `(b, a)` each raise `ValueError`, asserted individually
- [ ] AC4: On `base_10`, blocking recall computed over the full 253-pair universe is exactly `1.0` — all 18 true pairs appear in the DISTINCT canonicalised `int_blocking_keys` pair set
- [ ] AC5: On `base_10` at `auto_merge`, edge-level false-positive pairs == 0 and missed true pairs <= 1; when one is missed, the test asserts that pair's persona holds >= 3 records and that the persona stays connected without it
- [ ] AC6: The four named sub-assertions hold: the robert/bob pair and the typo-surname pair are present at `auto_merge`; the shared-household pair is absent; the two `test@test.com` records share no edge at any probability
- [ ] AC7: Adding a second `precision`/`recall` implementation under `tests/` makes `uv run python scripts/lint_metrics.py` exit non-zero, asserted by a test that writes the offending file into a temp tree

## Tests

- tests/unit/eval/test_pairwise_metrics.py::test_hand_computed_case
- tests/unit/eval/test_pairwise_metrics.py::test_degenerate_inputs
- tests/unit/eval/test_pairwise_metrics.py::test_out_of_universe_and_non_canonical_raise
- tests/integration/test_match_quality.py::test_edge_quality_base_10
- tests/integration/test_match_quality.py::test_blocking_recall_is_one
- tests/integration/test_match_quality.py::test_named_trap_subassertions
- tests/integration/test_match_quality.py::test_lint_metrics_rejects_a_second_implementation

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_match_quality.py -q
uv run pytest tests/unit/eval/test_pairwise_metrics.py -q
uv run python scripts/lint_metrics.py && uv run mypy --strict src/er/eval
```

## Definition of Done

- `pairwise_metrics` is the only precision/recall implementation in the repository and `scripts/lint_metrics.py` proves it
- The degenerate-input conventions are documented in the function docstring and each has a unit test
- `base_10`'s 23/10/18 counts are read from the committed truth file in both the unit and integration arms
- No fixture, blocking rule or model file is modified to satisfy the gate
- `bash scripts/gates.sh` green; INTERFACES entry lists `pairwise_metrics` and `PairwiseMetrics`
