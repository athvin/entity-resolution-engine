---
id: ER-100
title: "Benchmark quality block: shared pairwise_metrics, blocking recall, edge + cluster P/R/F1, reported never gated"
milestone: M5
status: todo
kind: code
size: S
gates: full
depends_on: ["ER-067", "ER-081", "ER-099"]
spec_refs: ["s8-5", "s9-1", "s10-1", "s10-3", "s10-5"]
gap_refs: ["M21", "M24"]
provides: ["benchmarks/quality.py::quality_block", "benchmarks/quality.py::blocking_recall", "benchmarks/quality.py::edge_level_metrics", "benchmarks/quality.py::cluster_level_metrics", "tests/integration/test_benchmark_quality.py::test_quality_block_is_reported_not_gated"]
consumes: ["src/er/eval/metrics.py::pairwise_metrics", "benchmarks/run_benchmark.py::run_pass", "benchmarks/report.py::main", "benchmarks/schema.py::validate_bench_result", "fixtures/generator/emit.py", "relation:entity_membership", "relation:match_scores", "relation:int_blocking_keys", "scripts/lint_metrics.py", "scripts/ci/bench.sh"]
owns: ["benchmarks/quality.py", "tests/unit/bench/test_quality_metrics.py", "tests/integration/test_benchmark_quality.py"]
protected_paths: ["src/er/eval/metrics.py"]
extra_paths: ["benchmarks/run_benchmark.py", "benchmarks/report.py"]
attempts: 0
verify: "bash scripts/ci/bench.sh pytest tests/unit/bench/test_quality_metrics.py tests/integration/test_benchmark_quality.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

S10.5 requires the benchmark to report match quality beside speed using the SAME implementation the tests call, so a throughput win that costs recall is visible in the same report and a benchmark number and a test number are never produced by two code paths. This ticket computes the three S8.5 families over the generator's `persona_id` ground truth — blocking recall over the full C(n,2) universe, edge-level precision/recall over the blocked universe, cluster-level precision/recall over the transitive closure of `entity_membership` — by calling `er.eval.pairwise_metrics` and nothing else, and emits them into the run document with `blocking_recall` as a required key. Quality is reported and never gated: a quality drop must not change the verdict or the exit code.

## Scope

### In scope

- `quality_block(conn, truth)` returning the three families with precision, recall, F1 and raw tp/fp/fn counts each
- The three `predicted` / `truth` / `universe` triples exactly as S8.5 defines them, including the truth restriction to the blocked set for the edge-level family
- Ground truth built from the generator's `persona_id` sidecar, which never reaches the pipeline
- Wiring the block into the run document and the `report.md` table
- A test proving the verdict and exit code are independent of every quality number

### Out of scope

- Defining or re-implementing precision/recall anywhere outside `src/er/eval/metrics.py` — `scripts/lint_metrics.py` fails on a second definition
- Gating a job or a baseline on quality; quality regressions are gated by S8's tests on committed fixtures
- T-MATCH-1a/1b themselves (ER-067/ER-081)
- Changing baselines or verdict logic (ER-099/ER-102)

## Design decisions applied

Closes M21's benchmark arm and the quality half of M24. Constraints easy to miss: cluster-level is the HEADLINE because one bad edge chaining two 4-record clusters costs 1 false pair at edge level and 16 at cluster level; edge-level alone cannot see a blocking regression at all, since a rule that stops emitting a key removes the pair from both `predicted` and `universe` and leaves edge recall at 1.0 — which is why blocking recall is computed over the full C(n,2) universe and is a REQUIRED key of `artifacts/bench/latest.json`. `pairwise_metrics` raises when a predicted or truth pair falls outside `universe`, so the universes must be built before the sets are filtered, not after. All pairs are canonical `rec_a_key < rec_b_key`.

## Acceptance criteria

- [ ] AC1: `bash scripts/ci/bench.sh pytest tests/unit/bench/test_quality_metrics.py tests/integration/test_benchmark_quality.py -q` exits 0.
- [ ] AC2: The run document's quality block contains exactly the three families, each with precision, recall, f1 and tp/fp/fn, and `blocking_recall` is present as a required key that `validate_bench_result` rejects the document without.
- [ ] AC3: A spy asserts `benchmarks/quality.py` obtains every number from `src/er/eval/metrics.py::pairwise_metrics` — three calls, one per family — and defines no arithmetic of its own; `uv run python scripts/lint_metrics.py` exits 0.
- [ ] AC4: The three universes match S8.5: blocking recall over the full C(n,2) set of current records, edge-level over the DISTINCT canonical blocked set with truth restricted to it, cluster-level over the transitive closure of `entity_membership` against the full C(n,2) set.
- [ ] AC5: Injecting a degraded truth set that drops cluster-level recall to ~0.5 leaves the run's verdict and the process exit code unchanged (still `OK`/`NO_BASELINE`, exit 0) — quality is reported, never gated.
- [ ] AC6: A predicted or truth pair outside the declared universe raises rather than being silently dropped, and the raised message names the offending pair.
- [ ] AC7: On a `smoke` run the quality block is present in `artifacts/bench/latest.json` and in the `report.md` table alongside the timing rows.

## Tests

- tests/unit/bench/test_quality_metrics.py::test_three_families_and_universes
- tests/unit/bench/test_quality_metrics.py::test_delegates_to_pairwise_metrics
- tests/unit/bench/test_quality_metrics.py::test_pair_outside_universe_raises
- tests/integration/test_benchmark_quality.py::test_quality_block_is_reported_not_gated
- tests/integration/test_benchmark_quality.py::test_blocking_recall_present_in_latest_json

## Verification

```bash
bash scripts/ci/bench.sh pytest tests/unit/bench/test_quality_metrics.py tests/integration/test_benchmark_quality.py -q
uv run python scripts/lint_metrics.py
uv run ruff check benchmarks && uv run ruff format --check benchmarks
```

## Definition of Done

- Three quality families emitted per run, all computed by the single `pairwise_metrics` implementation.
- `blocking_recall` is a schema-required key of the run document.
- `scripts/lint_metrics.py` green — no second precision/recall definition anywhere.
- A degraded-quality run demonstrably leaves verdict and exit code unchanged.
- `src/er/eval/metrics.py` unmodified by this ticket.
- ruff clean on `benchmarks/`; gate receipt recorded and `board.py complete` run.
