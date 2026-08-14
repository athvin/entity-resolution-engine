---
id: ER-097
title: "Benchmark B: run_benchmark.py six phases, run_stages-sourced timings, incremental_ratio, no __splink__ in lake"
milestone: M5
status: todo
kind: code
size: M
gates: full
depends_on: ["ER-023", "ER-049", "ER-092", "ER-096"]
spec_refs: ["s4", "s4-0", "s4-0b", "s5-2", "s10-1", "s10-2", "s10-3", "s10-4"]
gap_refs: ["M24", "M2", "M17"]
provides: ["benchmarks/run_benchmark.py::PHASES", "benchmarks/run_benchmark.py::PhaseRecord", "benchmarks/run_benchmark.py::run_pass", "benchmarks/run_benchmark.py::incremental_ratio", "benchmarks/fingerprint.py::environment_fingerprint", "benchmarks/fingerprint.py::read_cgroup_cpu_max", "tests/integration/test_benchmark_smoke.py::test_smoke_pass_produces_schema_valid_result"]
consumes: ["benchmarks/scales.py::get_scale", "benchmarks/schema.py::write_result", "benchmarks/schema.py::validate_bench_result", "src/er/obs/run_context.py::RunContext", "relation:run_stages", "relation:runs", "relation:match_scores", "relation:int_blocking_keys", "src/er/lake/ducklake.py::connect", "fixtures/generator/emit.py", "src/er/versions.py", "scripts/ci/bench.sh"]
owns: ["benchmarks/run_benchmark.py", "benchmarks/fingerprint.py", "tests/unit/bench/test_run_benchmark.py", "tests/integration/test_benchmark_smoke.py"]
protected_paths: ["src/er/matching/", "src/er/entities/", "src/er/golden/"]
extra_paths: []
attempts: 0
verify: "bash scripts/ci/bench.sh pytest tests/unit/bench/test_run_benchmark.py tests/integration/test_benchmark_smoke.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

`run_benchmark.py` executes one measured pass of the six S10.3 phases and returns raw phase records; without it there is nothing for `report.py` to aggregate and the G4 claim is unmeasured. This ticket implements the pass in order — ingest, standardize, train, full match + reconcile, assemble, then the incremental cycle — sourcing each phase's timing and row counts from the `run_stages` rows the stages themselves wrote rather than from a second clock, computing `records_per_sec`, `candidate_pair_count`, `pairs_above_auto_merge` and `snapshot_count`, and deriving the run-level `incremental_ratio`. It also lands the S10.4 environment fingerprint and the assertion that no `__splink__` relation ever reaches the lake during a measured run.

## Scope

### In scope

- The six phases of S10.3 in order, as one `run_pass(scale)` returning `PhaseRecord`s
- Per-phase metrics: `wall_ms`, `records_per_sec = rows_out / (wall_ms / 1000)`, `candidate_pair_count` from the DISTINCT canonicalised `int_blocking_keys` pair set, `pairs_above_auto_merge`, `snapshot_count = Σ(snapshot_end - snapshot_start)` over the phase's `run_stages` rows
- `incremental_ratio` = phase 6 wall time ÷ (phases 1+2+4+5) wall time
- Corpus generation and `er init` executed before measurement and excluded from every timing; the generated corpus reused across repeat passes
- `benchmarks/fingerprint.py`: image digest, git SHA, runner label, cgroup `cpu.max` (quota ÷ period), cgroup `memory.max`, in-container `nproc`, `ER_DUCKDB_THREADS`, `ER_DUCKDB_MEMORY_LIMIT`, pinned tool versions, `config_hash`, `generator.seed`, `model_version`, `tf_snapshot_id`, scale
- A `smoke`-scale integration run producing a schema-valid result document

### Out of scope

- The memory sampler and `memory.peak` (ER-098) — `memory_peak_bytes` is left absent/None here
- Aggregation across repeats, medians, CV, verdicts and baselines (ER-099)
- The quality block (ER-100)
- `benchmark.yaml` and the preflight (ER-101)
- Changing any pipeline stage to make a phase faster or a timing easier to read

## Design decisions applied

Closes the measurement half of M24, plus M2 (timings come from the recorded snapshot ranges and `run_stages` rows, so a phase's number and the lake's own record cannot disagree) and M17 (a measured run asserts zero `__splink__%` relations in the lake, the failure mode that would otherwise corrupt both timings and snapshot history at scale). Easy to miss: `records_per_sec` divides by `wall_ms / 1000` — dividing by `wall_ms` yields records per millisecond and is off by 1000, which is the exact defect the unit test must pin. Snapshot COUNT is a per-phase metric computed from ranges; it is never an assertion about how many snapshots a stage 'should' commit (S4 preamble). The cgroup CPU value is read from `cpu.max` as quota ÷ period, never from `nproc`, because `deploy.resources.limits.cpus` is a quota and `nproc` reports host cores (S10.4). `er train` is a measured phase here, but scenario tests still never train — that rule is about `tests/`, not about the benchmark.

## Acceptance criteria

- [ ] AC1: `bash scripts/ci/bench.sh pytest tests/unit/bench/test_run_benchmark.py tests/integration/test_benchmark_smoke.py -q` exits 0.
- [ ] AC2: `run_pass` executes exactly the six S10.3 phases in the documented order and returns six `PhaseRecord`s whose `name` values equal the six literals; a spy asserts the underlying `er` invocations are `ingest, standardize, train, match --mode full + reconcile, assemble, incremental cycle` in that sequence.
- [ ] AC3: Each phase record's `wall_ms`, `rows_in` and `rows_out` are reconciled against the `run_stages` rows for that phase's `run_id`: every phase names at least one `run_stages` row and `wall_ms >= Σ duration_ms` of its stages.
- [ ] AC4: `records_per_sec` computed on a synthetic record with `rows_out=1000, wall_ms=2000` equals `500.0` (not `0.5`), and `incremental_ratio` on synthetic phases equals phase-6 wall ÷ the summed wall of phases 1, 2, 4 and 5 exactly.
- [ ] AC5: `snapshot_count` for each phase equals `Σ(snapshot_end - snapshot_start)` over its `run_stages` rows, and no test in this ticket asserts an expected snapshot count value.
- [ ] AC6: After a `smoke` pass, zero relations matching `__splink__%` exist in the lake, asserted against the catalog.
- [ ] AC7: Corpus generation and `er init` produce no `PhaseRecord` and their elapsed time is excluded: the summed phase wall time is strictly less than the wall time of the whole invocation, and a second pass over the same scale reuses the generated corpus without regenerating it.
- [ ] AC8: The result document written by `write_result` validates against `bench_result.schema.json` and its fingerprint reports `cpu_max` parsed as quota ÷ period plus the DuckDB, Splink, dbt-core and dbt-duckdb versions equal to the S2.1 pins.

## Tests

- tests/unit/bench/test_run_benchmark.py::test_phase_order_and_names
- tests/unit/bench/test_run_benchmark.py::test_records_per_sec_uses_seconds
- tests/unit/bench/test_run_benchmark.py::test_incremental_ratio_formula
- tests/unit/bench/test_run_benchmark.py::test_cgroup_cpu_max_parsing
- tests/integration/test_benchmark_smoke.py::test_smoke_pass_produces_schema_valid_result
- tests/integration/test_benchmark_smoke.py::test_no_splink_relations_after_measured_run

## Verification

```bash
bash scripts/ci/bench.sh pytest tests/unit/bench/test_run_benchmark.py tests/integration/test_benchmark_smoke.py -q
uv run ruff check benchmarks && uv run ruff format --check benchmarks
uv run pytest tests/unit/bench -q
```

## Definition of Done

- Six phases executed in S10.3 order, with timings and row counts reconciled against `run_stages`.
- `records_per_sec` and `incremental_ratio` unit-pinned against hand-computed values.
- Generation and `er init` excluded from timings; corpus reused across passes.
- Zero `__splink__%` relations after a measured smoke run.
- Fingerprint reads `cpu.max` (quota ÷ period), not `nproc`, and carries every S10.4 field the schema requires.
- No pipeline module under `src/er/` modified; ruff clean; gate receipt recorded and `board.py complete` run.
