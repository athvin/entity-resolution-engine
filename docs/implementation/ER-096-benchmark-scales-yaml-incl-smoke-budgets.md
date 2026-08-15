---
id: ER-096
title: "Benchmark A: scales.yaml (incl. smoke, budgets), bench_result.schema.json, validate-before-write, bench compose service"
milestone: M5
status: in_progress
kind: code
size: M
gates: fast
depends_on: ["ER-009", "ER-051"]
spec_refs: ["s3", "s7-1", "s9-2", "s10-1", "s10-2", "s10-3", "s10-4"]
gap_refs: ["M24"]
provides: ["benchmarks/scales.yaml", "benchmarks/scales.py::Scale", "benchmarks/scales.py::load_scales", "benchmarks/scales.py::get_scale", "benchmarks/scales.py::main", "benchmarks/bench_result.schema.json", "benchmarks/schema.py::validate_bench_result", "benchmarks/schema.py::write_result", "benchmarks/schema.py::BenchResultError"]
consumes: ["docker/compose.yaml", "src/er/config/schema.py", "fixtures/generator/emit.py", "scripts/ci/bench.sh"]
owns: ["benchmarks/scales.yaml", "benchmarks/scales.py", "benchmarks/bench_result.schema.json", "benchmarks/schema.py", "tests/unit/bench/__init__.py", "tests/unit/bench/test_scales_and_schema.py"]
protected_paths: []
extra_paths: ["docker/compose.yaml"]
attempts: 1
verify: "uv run pytest tests/unit/bench/test_scales_and_schema.py -q"
branch: "ticket/ER-096-benchmark-scales-yaml-incl-smoke-budgets"
commit: ""
spec_sha: "2abcfe433c322f74"
updated_at: "2026-08-15T07:20:10Z"
session: 3e89c849-0d74-48a0-8ba5-c82111eb4769
---
## Description

The benchmark gate has never executed, partly because its inputs are undefined: S10.2's scale table (personas, records, incremental batch, `min_free_gb`) and its per-scale resource envelope (`runner`, `cpu_limit`, `mem_limit`, `duckdb_memory_limit`) exist only as prose, and the S9.2 preflight shells out to `benchmarks/scales.py --scale S --field F` for values nothing produces. This ticket lands `scales.yaml` as the single source of truth for all four scales including `smoke`, the `scales.py` field accessor the workflow calls, a JSON Schema for the run document, and a validate-before-write helper so an invalid `latest.json` is never produced. It also aligns the Compose `benchmark` service (profile `bench`, artifacts bind mount, `benchdata` volume, envelope defaults) with that table.

## Scope

### In scope

- `benchmarks/scales.yaml` carrying `smoke`, `10k`, `100k`, `1m` with the ten S10.2 fields each
- `benchmarks/scales.py`: typed loader plus a `--scale S --field F` CLI printing one bare value on stdout, exiting non-zero on an unknown scale or field
- `benchmarks/bench_result.schema.json`: required keys for the S10.4 fingerprint, the S10.3 per-phase metrics, the three memory series, `incremental_ratio`, `blocking_recall` and `verdict`
- `benchmarks/schema.py`: `validate_bench_result` plus `write_result`, which validates before opening the destination and writes nothing on failure
- Internal consistency validators over `scales.yaml`: `ER_DUCKDB_THREADS == cpu_limit`, `duckdb_memory_limit < mem_limit`, dispatchable ⇒ baseline_committed
- Compose `benchmark` service wiring under the `bench` profile

### Out of scope

- `run_benchmark.py` and any measurement (ER-097)
- The memory sampler and cgroup readers (ER-098)
- `report.py`, verdicts and comparison (ER-099)
- `--validate-baselines` and the committed baseline JSONs (ER-102)
- `benchmark.yaml` itself (ER-101)

## Design decisions applied

Closes the input half of M24. Constraints that are easy to miss: `smoke` exists so the weekly cron and any bootstrap fit a 40-minute job on a 2-vCPU runner, so it MUST be present and dispatchable; `1m` is defined but has `baseline_committed: no` and `dispatchable: no` and adding it to a dispatch list is a separate, later change; `cpu_limit` must never exceed the runner's vCPU count and `mem_limit` must leave RAM for `catalog` and `objectstore`; `duckdb_memory_limit` is roughly 70% of `mem_limit` because DuckDB's limit bounds the buffer manager only, not the Python heap or the dbt subprocess. `ER_CPU_LIMIT`/`ER_MEM_LIMIT` are read by Compose and are deliberately NOT part of S6's environment contract, while `ER_DUCKDB_THREADS`/`ER_DUCKDB_MEMORY_LIMIT` are derived from them inside `x-er-env`. Validate-before-write exists because a half-written `latest.json` is what makes `--compare` read a file that cannot exist.

## Acceptance criteria

- [ ] AC1: `uv run pytest tests/unit/bench/test_scales_and_schema.py -q` exits 0.
- [ ] AC2: `uv run python benchmarks/scales.py --scale smoke --field min_free_gb` prints `4` and exits 0; `--scale 100k --field cpu_limit` prints `6`; an unknown scale or unknown field exits non-zero with a message naming the offending value and prints nothing to stdout.
- [ ] AC3: Every one of the four scales in `scales.yaml` matches S10.2 literally in all ten fields, asserted value-by-value in the test (personas, records, incremental batch, `min_free_gb`, baseline_committed, dispatchable, runner, cpu_limit, mem_limit, duckdb_memory_limit).
- [ ] AC4: Loading a `scales.yaml` in which `duckdb_memory_limit >= mem_limit`, or in which a `dispatchable` scale has `baseline_committed: no`, raises `BenchResultError`/a validation error naming the scale.
- [ ] AC5: `write_result` refuses a document missing any required key — `blocking_recall`, `verdict`, `incremental_ratio`, or a fingerprint field — raising before the destination is opened, and the destination file does not exist afterwards.
- [ ] AC6: A document containing every required key validates and is written byte-for-byte as canonical JSON, and re-validating the written file passes.
- [ ] AC7: A unit test parses `docker/compose.yaml` without Docker and asserts the `benchmark` service declares profile `bench`, `ER_CONFIG`, the `../artifacts:/app/artifacts` bind mount, the `benchdata:/app/.bench` volume, `pull_policy: never`, and `deploy.resources.limits` defaulting to `cpus: 2` / `memory: 6g`.

## Tests

- tests/unit/bench/test_scales_and_schema.py::test_scales_yaml_matches_s10_2
- tests/unit/bench/test_scales_and_schema.py::test_field_accessor_cli
- tests/unit/bench/test_scales_and_schema.py::test_envelope_consistency_rules
- tests/unit/bench/test_scales_and_schema.py::test_write_result_refuses_invalid_document
- tests/unit/bench/test_scales_and_schema.py::test_compose_bench_service_contract

## Verification

```bash
uv run pytest tests/unit/bench/test_scales_and_schema.py -q
uv run python benchmarks/scales.py --scale smoke --field min_free_gb
uv run ruff check benchmarks && uv run ruff format --check benchmarks
```

## Definition of Done

- All four S10.2 scales present with their full envelope; `smoke` dispatchable, `1m` not.
- `scales.py --scale S --field F` prints a bare value suitable for `$GITHUB_ENV`, with no logging on stdout.
- `bench_result.schema.json` requires the S10.4 fingerprint keys, the S10.3 phase metrics, the three memory series, `incremental_ratio` and `blocking_recall`.
- `write_result` validates before writing and leaves no partial file on rejection.
- Compose `benchmark` service contract asserted by a service-less unit test.
- ruff clean on `benchmarks/`; gate receipt recorded and `board.py complete` run.
