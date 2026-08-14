---
id: ER-098
title: "Memory sampler (250 ms; duckdb_memory + RSS + cgroup peak) + ER_DUCKDB_THREADS/ER_DUCKDB_MEMORY_LIMIT on every connection + compose limits 2/6g"
milestone: M5
status: todo
kind: code
size: M
gates: full
depends_on: ["ER-016", "ER-097"]
spec_refs: ["s4-0b", "s7-1", "s10-2", "s10-3", "s10-4"]
gap_refs: ["M24", "M25"]
provides: ["benchmarks/memory.py::MemorySampler", "benchmarks/memory.py::SAMPLE_INTERVAL_MS", "benchmarks/memory.py::MemoryPeaks", "benchmarks/memory.py::read_cgroup_memory_peak", "benchmarks/memory.py::current_duckdb_memory_bytes", "tests/integration/test_benchmark_memory.py::test_every_connection_pins_threads_and_memory"]
consumes: ["benchmarks/run_benchmark.py::run_pass", "benchmarks/run_benchmark.py::PhaseRecord", "benchmarks/fingerprint.py::environment_fingerprint", "benchmarks/scales.py::get_scale", "src/er/lake/ducklake.py::connect", "docker/compose.yaml", "dbt/profiles/profiles.yml", "scripts/ci/bench.sh"]
owns: ["benchmarks/memory.py", "tests/unit/bench/test_memory_sampler.py", "tests/integration/test_benchmark_memory.py"]
protected_paths: []
extra_paths: ["docker/compose.yaml", "src/er/lake/ducklake.py", "dbt/profiles/profiles.yml"]
attempts: 0
verify: "bash scripts/ci/bench.sh pytest tests/unit/bench/test_memory_sampler.py tests/integration/test_benchmark_memory.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

`duckdb_memory()` reports current buffer-manager usage with no peak counter and excludes vectors, query results, the Python heap and the dbt subprocess, so a single reading at phase end is ~0 and S10.4's pod-sizing deliverable is unmeasurable. This ticket adds the 250 ms background sampler that records the maximum of three sources — `sum(memory_usage_bytes)` from `duckdb_memory()` on a cursor duplicate, process RSS, and `/sys/fs/cgroup/memory.peak` — reports all three separately with the cgroup peak as the authoritative sizing number, and closes the other half of the same defect: DuckDB reads neither cgroup CPU nor cgroup memory, so `ER_DUCKDB_THREADS` and `ER_DUCKDB_MEMORY_LIMIT` must be applied with `SET` on every connection the CLI, the harness and dbt open, with the Compose envelope defaulting to `cpus: 2` / `memory: 6g` / `4GB`.

## Scope

### In scope

- `MemorySampler`: background thread sampling every 250 ms, `start()`/`stop()` with a bounded join, returning `MemoryPeaks(duckdb_memory_bytes, rss_bytes, cgroup_peak_bytes, memory_peak_bytes)`
- Sampling DuckDB memory on a `.cursor()` duplicate of the run connection, never on the run connection itself
- `read_cgroup_memory_peak` with a cgroup-v1/absent-file fallback returning `None` rather than raising, recorded as such in the result
- Per-phase wiring of the sampler into `run_pass` so each `PhaseRecord` carries all three series' peaks
- Verifying `SET threads` / `SET memory_limit` are applied on every connection (CLI, benchmark harness, dbt `lake` target) and that they equal the scale's `cpu_limit` / `duckdb_memory_limit`
- Compose envelope defaults `ER_CPU_LIMIT=2`, `ER_MEM_LIMIT=6g`, `ER_DUCKDB_MEMORY_LIMIT=4GB` with `ER_DUCKDB_THREADS` derived from `ER_CPU_LIMIT`

### Out of scope

- The NON_COMPARABLE verdict logic that consumes these numbers (ER-099)
- Per-scale envelope values themselves (ER-096 owns `scales.yaml`)
- Larger-runner provisioning for `100k` (an M5 environmental precondition, not code)
- Tuning DuckDB for speed

## Design decisions applied

Closes the memory half of M24 and M25's envelope arm. Constraints easy to miss: `deploy.resources.limits.cpus` is a CPU QUOTA, not a cpuset, so in-container `nproc` keeps reporting host cores and must never be used as the thread count or as a gate; `ER_DUCKDB_THREADS` equals `cpu_limit` always, which is why Compose writes it as `${ER_CPU_LIMIT:-2}` — the two cannot be set independently. `memory.peak` is the only source that includes DuckDB's out-of-buffer allocations, the Python heap and the dbt subprocess, so it is the number fed to sizing even though it is the least precise about which component grew. The sampler must not perturb the measurement: it issues its query on a cursor duplicate and holds no lock on the run connection.

## Acceptance criteria

- [ ] AC1: `bash scripts/ci/bench.sh pytest tests/unit/bench/test_memory_sampler.py tests/integration/test_benchmark_memory.py -q` exits 0.
- [ ] AC2: `SAMPLE_INTERVAL_MS == 250`; over a synthetic 2-second workload the sampler records at least 6 samples, and `stop()` joins the thread within 1 second and is idempotent.
- [ ] AC3: `MemoryPeaks.memory_peak_bytes` equals the maximum of the three series, and all three are reported separately in the result document; a synthetic series test pins the max selection.
- [ ] AC4: With `/sys/fs/cgroup/memory.peak` absent (path injected), `read_cgroup_memory_peak` returns `None`, the sampler still produces the other two series, and the result records the cgroup peak as null rather than 0.
- [ ] AC5: The DuckDB series is sampled on a `.cursor()` duplicate: a test asserts the run connection executes no sampler statement (spy on the run connection) while samples are still collected.
- [ ] AC6: On every connection the CLI and the harness open, `current_setting('threads')` equals `ER_DUCKDB_THREADS` and `current_setting('memory_limit')` equals `ER_DUCKDB_MEMORY_LIMIT`; the same holds for the connection dbt opens against the `lake` target, asserted after a `dbt run`.
- [ ] AC7: A service-less unit test over `docker/compose.yaml` asserts the defaults `ER_CPU_LIMIT=2`, `ER_MEM_LIMIT=6g`, `ER_DUCKDB_MEMORY_LIMIT=4GB` and that `ER_DUCKDB_THREADS` is written as `${ER_CPU_LIMIT:-2}` so it cannot diverge from the quota.

## Tests

- tests/unit/bench/test_memory_sampler.py::test_sample_interval_and_sample_count
- tests/unit/bench/test_memory_sampler.py::test_peak_is_max_of_three_series
- tests/unit/bench/test_memory_sampler.py::test_cgroup_peak_absent_yields_none
- tests/unit/bench/test_memory_sampler.py::test_compose_envelope_defaults
- tests/integration/test_benchmark_memory.py::test_every_connection_pins_threads_and_memory
- tests/integration/test_benchmark_memory.py::test_phase_records_carry_three_memory_series

## Verification

```bash
bash scripts/ci/bench.sh pytest tests/unit/bench/test_memory_sampler.py tests/integration/test_benchmark_memory.py -q
uv run mypy --strict src/er/lake/ducklake.py
uv run ruff check benchmarks && uv run ruff format --check benchmarks
```

## Definition of Done

- Sampler runs at 250 ms, records three series, and reports the cgroup peak as authoritative for sizing.
- Sampling never touches the run connection directly.
- `SET threads` / `SET memory_limit` verified on CLI, harness and dbt connections against the scale's envelope.
- Compose defaults are 2 / 6g / 4GB with `ER_DUCKDB_THREADS` derived from `ER_CPU_LIMIT`.
- `nproc` is used nowhere as a thread count or gate.
- ruff + `mypy --strict src/er` clean; gate receipt recorded and `board.py complete` run.
