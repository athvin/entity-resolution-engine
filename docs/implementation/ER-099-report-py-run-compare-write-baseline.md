---
id: ER-099
title: "report.py: --run/--compare/--write-baseline/--repeat N, median+CV, verdicts OK/REGRESSION/NO_BASELINE/NON_COMPARABLE, --baselines-dir"
milestone: M5
status: todo
kind: code
size: M
gates: fast
depends_on: ["ER-097", "ER-098"]
spec_refs: ["s9-2", "s10-2", "s10-3", "s10-4"]
gap_refs: ["M24"]
provides: ["benchmarks/report.py::main", "benchmarks/report.py::aggregate_passes", "benchmarks/report.py::compare_to_baseline", "benchmarks/report.py::write_baseline", "benchmarks/report.py::Verdict", "benchmarks/report.py::comparability_violations", "tests/unit/bench/data/"]
consumes: ["benchmarks/run_benchmark.py::run_pass", "benchmarks/run_benchmark.py::PhaseRecord", "benchmarks/fingerprint.py::environment_fingerprint", "benchmarks/memory.py::MemoryPeaks", "benchmarks/scales.py::get_scale", "benchmarks/schema.py::write_result", "benchmarks/schema.py::validate_bench_result"]
owns: ["benchmarks/report.py", "tests/unit/bench/test_report.py", "tests/unit/bench/data/"]
protected_paths: []
extra_paths: []
attempts: 0
verify: "uv run pytest tests/unit/bench/test_report.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

`report.py` is the single benchmark entrypoint and today has no interface: the workflow's comparison step reads a baseline with no defined bootstrap, and a single-sample 25% gate on a shared runner fires on noise. This ticket implements `--run`, `--compare`, `--write-baseline`, `--repeat N`, `--out`, `--fail-threshold` and `--baselines-dir` per S10.3, reporting the median per phase per metric plus the coefficient of variation of `wall_ms`, and the four verdicts `OK` / `NO_BASELINE` / `REGRESSION` / `NON_COMPARABLE` with their exit codes. It also implements the S10.4 comparability rule so a run measured outside its scale's envelope carries no gate authority and cannot be frozen into a baseline.

## Scope

### In scope

- Flag surface: `--run --scale`, `--repeat N` (default 3), `--out`, `--compare RUN --baselines-dir DIR --scale s`, `--fail-threshold F` (default 1.25), `--write-baseline`
- Aggregation: per-phase, per-metric median across passes plus `CV = stdev/mean` of `wall_ms`
- Verdicts and exit codes: `OK` 0, `NO_BASELINE` 0, `REGRESSION` 1, `NON_COMPARABLE` 3, argument/IO errors 2
- `comparability_violations`: cgroup quota ≠ `cpu_limit`, cgroup `memory.max` ≠ `mem_limit`, `ER_DUCKDB_THREADS` ≠ `cpu_limit`, `ER_DUCKDB_MEMORY_LIMIT` ≠ `duckdb_memory_limit`, scale mismatch, any phase CV > 0.15
- Outputs: the run JSON at `--out` (via the ER-096 validate-before-write path) and a sibling `report.md` table; the verdict string stored in the JSON and printed as the last stdout line

### Out of scope

- `--validate-baselines` and the committed baseline JSONs (ER-102)
- `benchmark.yaml`, the in-image invocation and the preflight (ER-101)
- The quality block's contents (ER-100) — this ticket only carries it through unchanged
- Any gating on quality metrics: quality is reported, never gated

## Design decisions applied

Closes the reporting half of M24. Constraints easy to miss: `report.py` is NOT an `er` command — its exit codes are local to the tool, and its `3` means `NON_COMPARABLE`, not the S4.0 precondition failure, so no `er` code path may reuse them. `NO_BASELINE` exits 0 deliberately: the first run at a scale is a bootstrap, not a failure, and `--write-baseline` is the documented way out of it. `--write-baseline` MUST refuse a `NON_COMPARABLE` run, because a baseline measured on the wrong-shaped machine silently redefines every later comparison. The threshold comparison is on the phase MEDIAN, and the boundary is inclusive: exactly `F ×` baseline is `OK`, above it is `REGRESSION`.

## Acceptance criteria

- [ ] AC1: `uv run pytest tests/unit/bench/test_report.py -q` exits 0.
- [ ] AC2: Given three synthetic passes with `wall_ms` of 100, 200 and 300 for a phase, the aggregate reports median `200` and CV equal to the hand-computed `stdev/mean` to within 1e-9.
- [ ] AC3: `--compare` against a `--baselines-dir` with no `<scale>.json` prints verdict `NO_BASELINE`, exits 0, and writes `NO_BASELINE` into the run JSON.
- [ ] AC4: A phase median exactly `1.25 ×` the baseline median yields `OK`; a median just above it yields `REGRESSION` and exit 1, with the offending phase named in the output.
- [ ] AC5: A run whose fingerprint violates any single S10.4 condition — quota, `memory.max`, `ER_DUCKDB_THREADS`, `ER_DUCKDB_MEMORY_LIMIT`, scale mismatch, or any phase CV above 0.15 — yields `NON_COMPARABLE` and exit 3, and `--write-baseline` on that same run exits non-zero leaving the baselines directory byte-unchanged.
- [ ] AC6: A malformed CLI invocation and an unreadable/invalid run JSON both exit 2, distinct from 1 and 3.
- [ ] AC7: `--run` writes both `--out` and a sibling `report.md` containing one row per phase, and the verdict string is the last line of stdout in every mode.

## Tests

- tests/unit/bench/test_report.py::test_median_and_cv_across_repeats
- tests/unit/bench/test_report.py::test_no_baseline_verdict_exits_zero
- tests/unit/bench/test_report.py::test_regression_threshold_boundary
- tests/unit/bench/test_report.py::test_non_comparable_conditions
- tests/unit/bench/test_report.py::test_write_baseline_refuses_non_comparable
- tests/unit/bench/test_report.py::test_bad_arguments_exit_2
- tests/unit/bench/test_report.py::test_report_md_and_verdict_line

## Verification

```bash
uv run pytest tests/unit/bench/test_report.py -q
uv run python benchmarks/report.py --help
uv run ruff check benchmarks && uv run ruff format --check benchmarks
```

## Definition of Done

- All four verdicts implemented with their S10.3 exit codes; `2` reserved for bad arguments and unreadable input.
- Median-plus-CV aggregation pinned against hand-computed values.
- Every S10.4 comparability condition implemented and individually covered by a test.
- `--write-baseline` refuses `NON_COMPARABLE` and leaves the baselines directory untouched.
- `report.md` and the run JSON both produced; verdict is the last stdout line.
- ruff clean on `benchmarks/`; gate receipt recorded and `board.py complete` run.
