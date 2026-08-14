---
id: ER-081
title: "Cluster-level T-MATCH-1b: transitive-closure precision/recall, robert/bob merged, household not merged, placeholder email forms no component"
milestone: M3
status: todo
kind: code
size: S
gates: full
depends_on: ["ER-067", "ER-074"]
spec_refs: ["s8-2", "s8-3", "s8-5", "s4-5-2", "s4-5-3"]
gap_refs: ["M21"]
provides: ["tests/integration/test_match_quality_cluster.py::test_cluster_quality_base_10", "src/er/eval/metrics.py::cluster_closure_pairs", "src/er/eval/metrics.py::membership_partition"]
consumes: ["src/er/eval/metrics.py::pairwise_metrics", "src/er/eval/metrics.py::PairwiseMetrics", "fixtures/static/base_10/", "fixtures/static/base_10/truth.csv", "fixtures/static/model_test_v1.json", "relation:entity_membership", "relation:match_scores", "relation:review_queue", "tests/conftest.py::lake_ns"]
owns: ["tests/integration/test_match_quality_cluster.py"]
protected_paths: []
extra_paths: ["src/er/eval/metrics.py", "tests/unit/eval/test_pairwise_metrics.py"]
attempts: 0
verify: "bash scripts/ci/itest.sh tests/integration/test_match_quality_cluster.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

T-MATCH-1a (ER-067) gates edge-level quality over the blocked universe, which cannot see a blocking regression and cannot see one bad edge chaining two clusters. This ticket adds the cluster-level arm S8.5 calls the headline metric: precision/recall over the transitive closure of `entity_membership` against the full C(23,2) universe, plus the four named trap sub-assertions S8.3's T-MATCH-1b row requires. It closes gap M21's second half — that quality had no defined pair universe — by making the closure computation a function in `src/er/eval/metrics.py` so the benchmark (ER-100) and this test cannot drift into two implementations.

## Scope

### In scope

- `cluster_closure_pairs(membership_rows)` in `src/er/eval/metrics.py`: the transitive closure of `entity_membership` as a canonical pair set (S5.0 ordering)
- The integration test running the full chain on `base_10` and asserting cluster-level precision == 1.0, recall >= 0.94, entity count == 10
- The four named sub-assertions: robert/bob co-clustered; the two household personas in two distinct entities; the two `test@test.com` records form no component; the singleton persona is its own entity
- Asserting the single gray-band pair is NOT co-clustered (the precondition that makes entity count == 10 true)

### Out of scope

- Any second precision/recall arithmetic — `pairwise_metrics` from ER-067 is the only implementation (S8.5, `scripts/lint_metrics.py`)
- Edge-level or blocking-recall assertions (ER-067 owns T-MATCH-1a)
- The `review_queue` row's own shape and lifecycle (ER-086 owns T-REVIEW-1)
- Editing `fixtures/static/base_10/` — trap authoring is ER-041 and trap verification under the committed model is ER-060

## Design decisions applied

Closes gap M21's cluster-level arm. Constraints easy to miss: (1) the universe for cluster-level metrics is the FULL `C(n,2)` set over all 23 current records (253 pairs), NOT the blocked set — passing the blocked set makes a blocking regression invisible, which is the exact defect M21 names; (2) precision is asserted `== 1.0` exactly, recall `>= 0.94` (>= 17 of 18 true pairs), per S8.3 — no float tolerance games; (3) entity count == 10 is only satisfiable because of the two S8.2 authoring constraints (the gray-band pair is cross-persona, and any tolerated missed edge lies inside a persona of 3+ records) — if this assertion fails, read those two paragraphs before touching the metric; (4) truth pairs come from the committed `persona_id` column via ER-041's truth artifact, never recomputed by a second labeller; (5) the test function MUST be named `test_cluster_quality_base_10` so ER-103's S8.3 node-id resolution maps the T-MATCH-1b row onto it even though the board's file path differs from S8.3's.

## Acceptance criteria

- [ ] AC1: Given `base_10` loaded and `er run-all --mode full` run at the committed `fixtures/static/model_test_v1.json`, `pairwise_metrics(cluster_closure_pairs(entity_membership), truth_pairs, full_universe).precision == 1.0` and `.recall >= 0.94`, where `full_universe` has exactly 253 pairs and `truth_pairs` has exactly 18
- [ ] AC2: `select count(distinct entity_id) from lake.main.entity_membership` returns exactly 10
- [ ] AC3: The crm 'Robert Chen' record and the webforms 'Bob Chen' record have the same `entity_id`; the two shared-household records have different `entity_id`s and zero `match_scores` rows between them at `match_probability >= auto_merge`
- [ ] AC4: The two records carrying `test@test.com` have different `entity_id`s and the closure pair set contains neither their pair nor any pair transitively joining them
- [ ] AC5: The persona with a single record occupies an entity whose `entity_membership` row count is exactly 1
- [ ] AC6: The two endpoints of `base_10`'s single gray-band pair have different `entity_id`s
- [ ] AC7: `uv run python scripts/lint_metrics.py` exits 0 with the new module contents — the test file itself contains no precision/recall arithmetic and imports `pairwise_metrics`
- [ ] AC8: Deleting one true pair's edge from `match_scores` before the assertion (an in-test perturbation) makes recall drop below 0.94 and the test fail — the assertion is not vacuous

## Tests

- tests/integration/test_match_quality_cluster.py::test_cluster_quality_base_10
- tests/integration/test_match_quality_cluster.py::test_designed_traps_at_cluster_level
- tests/unit/eval/test_pairwise_metrics.py::test_cluster_closure_pairs_is_canonical_and_transitive

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_match_quality_cluster.py -q
uv run pytest tests/unit/eval/test_pairwise_metrics.py -q
uv run python scripts/lint_metrics.py
uv run mypy --strict src/er
```

## Definition of Done

- `cluster_closure_pairs` lives in `src/er/eval/metrics.py` and is typed under `mypy --strict`
- The test uses `pairwise_metrics` for all three numbers; no second precision/recall definition exists (`lint_metrics.py` green)
- All four S8.3 named sub-assertions and the gray-band non-clustering assertion are present as separate asserts with messages naming the trap
- Truth pairs are read from the committed fixture truth artifact, not recomputed
- The T-INV-1 autouse finalizer passes for this scenario
- `bash scripts/ci/itest.sh tests/integration/test_match_quality_cluster.py -q` passes; the same command failed before the change
