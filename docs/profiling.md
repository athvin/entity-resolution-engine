# Profiling the complete pipeline

For a controlled baseline/candidate performance comparison, see
[performance.md](performance.md).

## Primary workload: 1M reload followed by 100K new record keys

```sh
make profile-workloads
```

This runs one unprofiled control and one diagnostic trial using the same immutable
image, input files, reference config and seed 42. Each has an independent Compose
stack. Within each stack the full reload runs ingestion, cleaning, training, matching,
reconciliation and golden/lineage assembly; then the same lake receives 100,000 net-new
record keys and runs incremental standardization through assembly with the existing
model and frozen TF snapshot. Half the batch represents 50,000 existing people; half
represents 20,000 new people with two or three records each (`mixed-v1`). Existing
generator scenarios remain unchanged.

The runner prints the artifact path under `artifacts/bench/`. Open `report.md`, then
`full-report.md` and `incremental-report.md`; **both workloads are primary**. Inspect
control timing for throughput and diagnostic timing for collection overhead. A single
pair is evidence for choosing experiments, not a performance regression baseline.
`BENCHMARK_REPEAT=3 make profile-workloads` creates three trials in each arm.

Correctness and storage scans run after both measured workloads, reading their saved
DuckLake snapshots. An untimed full re-resolution of the final corpus checks
incremental/full equivalence. Timing includes CLI startup and the stage chains;
generation, stack setup, snapshots' validation and the full reference are excluded.
Independent trainings can differ at floating-point roundoff: reports retain byte
hashes and compare learned probabilities within `1e-12`, with other model fields
exact. Model/TF byte hashes must be unchanged from reload to incremental processing
within each trial. Scores compare within `1e-10` across trials; threshold classes,
clusters, golden values and lineage compare exactly.
The local resource envelope is recorded and verified; comparisons must use the same
CPU, container memory, DuckDB settings and versions. Other running workloads and OS
caches can still affect timing.

The diagnostic trial adds native SQL profiles, Python cProfile files and
capability-gated DuckLake/HTTP logs. Each SQL execution gets a unique file; immutable
identities link it to its phase, invocation, stage, dbt model and estimator/pass span.
Native SQL and Python profiling stop before the untimed correctness scans and full
reference. Those checks still run. Offline query/operator indexes include only the
two measured workloads; any older setup/validation profiles remain in the raw files.
Coverage checks catch missing consumed SQL, writes, dbt models, training estimators
and either incremental matching pass. Unsupported optional telemetry is marked
unavailable. File inventories and Parquet footer statistics are captured at each
checkpoint; current options are not mislabeled as historical settings.

cProfile observes each process's main thread. In particular, dbt worker-thread
Python work is not completely covered; native SQL profiles still capture the dbt
queries. Treat native calls, subprocess waits and lock time as boundaries to inspect,
not as measured Python CPU. Use model timings and process/cgroup samples alongside
the call summaries before proposing pushdown.
The Python summary also checks timing consistency. If self time exceeds cumulative
time, the profile is marked `inconsistent`: use its call counts, not duration rankings.
Native SQL profiles and unprofiled command/stage times remain the timing evidence.

Per-trial artifacts include `query-coverage.json`, `query-index.jsonl`,
`operator-index.jsonl`, `queries.parquet`, `python-summary.json`,
`base-lake-layout.json`, `batch-lake-layout.json`, Parquet metadata files, native
`sql/*.json`, `python/*.pstats` and `storage/*.jsonl`. Raw profiles can contain source
values; apply source-data access controls. Regenerate reports without Docker/network:

```sh
uv run python benchmarks/workload_report.py artifacts/bench/<campaign>
```

For a small harness check, use `benchmarks/full_pipeline.py --scale smoke --local
--with-incremental --incremental-records 100 --incremental-scenario mixed-v1 --profile
--with-profile-control` with `uv run python`. See the
[implementation plan](performance-profiling-plan.md) and the repository
[analysis skill](../.agents/skills/ducklake-performance/SKILL.md). The first completed
[1M/100K campaign and tuning backlog](performance-profiling-1m-100k.md) retain both
workloads' measured results. Ordered writes and
partitioning are future measured experiments; this campaign changes no production
layout, clustering or scoring algorithm.

Run from the repository root with Docker available:

```sh
bash scripts/ci/profile.sh
```

The campaign builds the current code and creates its own Compose project. It runs:

1. The committed 10-person fixture: 23 source records, then six incremental records.
   Standardized hashes, membership and golden records must match the expected fixtures
   after both phases. This case installs the committed model because training on 23
   records is degenerate.
2. The smoke workload: 1,000 source records representing 400 people, full training and
   resolution, then 50 incremental records.
3. Three independent 10,000-record passes representing 4,000 people, each with full
   training and a 100-record incremental batch.
4. One fresh 10,000-record control pass with detailed tracing and SQL profiling disabled.
   Resource sampling and command logging remain enabled in the control.

Every pass starts with a new catalog schema and object-store prefix. The generated
corpus is reused with seed 42. Training remains an explicit `er train` command;
`er run-all` executes ingestion, standardization, matching, reconciliation and assembly.
The incremental pass uses the real `run-all --skip-ingest` chain after ingesting all
three sources under one run ID. The script removes only its own Compose project and
volumes when it exits. The developer's normal stack is independent.

Smaller or targeted runs:

```sh
bash scripts/ci/profile.sh --case tiny
bash scripts/ci/profile.sh --case smoke
bash scripts/ci/profile.sh --case 10k --repeat 3
```

Use the default campaign to enforce the small-workload gates before the large run.
`--case` intentionally selects just that case. Check free disk space before a campaign:
the image, manifests, complete subprocess logs and per-query JSON profiles need storage.
A collection error fails the coverage gate and retains the partial results.

## Reading the artifacts

The script prints `artifacts/profile/<session>/` when it exits. Open `report.md` there.

| Artifact | Contents |
|---|---|
| `report.md` | Stage/model timings, input/output units, throughput, CPU, memory, slow SQL and operators; 10k median, variation and estimated profiling overhead |
| `results.json` | All pass results and environment fingerprints |
| `<pass>/result.json` | Every command, all per-invocation stage records, ledger, table counts, partition hashes, quality and coverage checks |
| `<pass>/summary.json` | Every completed transformation with resource samples joined by monotonic timestamps, plus SQL/operator rankings |
| `<pass>/events-<pid>.jsonl` | Versioned span start/end, query, dbt-model and count-probe events |
| `<pass>/commands/<invocation>/` | Complete stdout/stderr files; successful output is retained too |
| `<pass>/dbt/<invocation>/target/` | Isolated `manifest.json` and `run_results.json` for each dbt invocation |
| `<pass>/sql/*.json` | Native DuckDB query plans, operator cardinalities/timings, scanned/returned rows, buffer/spill peaks |
| `<pass>/resources.jsonl` | 250 ms container CPU/memory and per-process RSS samples |
| `services-resources.jsonl`, `services.log` | Timestamped Docker stats and service logs for the campaign's containers |
| `campaign.log`, `source-status.txt` | Live command output and source worktree status; fingerprints also record image digest and Git revision |

Spans have session, invocation, run, process, parent and stage identities. Child CLI
and dbt processes inherit the parent's identity. dbt model spans correlate SQL with
the model being executed. Training estimator spans name their Splink method and EM
blocking rule; label propagation records each iteration and how many labels moved.
Source file reads, landing/classification/appends, TF materialization and registration,
candidate generation/scoring, review operations, clustering, entity/event writes and
golden assembly all emit nested spans.

Reconciliation separately times current membership expansion, standardized-record
lookups, current partition loading, absent-record detection and tombstone retraction.
These helper spans expose lookup costs that would otherwise sit between the graph
and persistence spans.

## Measurement semantics

- Wall durations use monotonic clocks. Total time includes setup and correctness
  checks; processing time sums the CLI commands that execute the pipeline. Stage time
  excludes some command startup; child spans are included in their parent's time.
  Do not sum overlapping spans or treat operator time as wall time.
- `rows_in` and `rows_out` carry explicit units. Matching consumes records and emits
  pairs; reconciliation emits entities; blocking emits keys; lineage emits attributes.
  Standardization reports newly processed input and the current output relation size.
  Staging model `rows_written` is the append delta. Other dbt models report the current
  rebuilt output scope, including only touched entities where appropriate.
- Detailed transformation result counts remain named (for example `edges_in`,
  `clusters_out`, `tf_rows`). Unmeasured values are unavailable. A short span with no
  sample does not receive another stage's memory peak.
- cgroup v2 CPU deltas include the benchmark's CLI and dbt descendants. Throttling
  measures quota-related delays. The small `scripts/ci/profile.sh` campaign uses
  two CPUs, 6 GiB container memory, two DuckDB threads and a 4 GB DuckDB limit.
  The primary local 1M/100K campaign uses a 10 GiB container with the same CPU and
  DuckDB limits; always read the recorded resource envelope.
- Phase memory is the maximum **memory.current** observed inside that phase. It
  includes page cache. The sampled sum of process RSS is also retained; shared pages
  can be counted repeatedly. The fingerprint's **memory.peak** is the container's
  lifetime high-water mark, and is never copied into every stage as a phase peak.
- DuckDB's buffer and spill metrics are connection high-water marks. They exclude
  Python and other processes. The SQL collector captures execution once through a
  FIFO; it never reruns queries with `EXPLAIN ANALYZE`.
- Counts require extra queries. Count-probe timings and profiling overhead are part
  of the diagnostic result. The single control comparison is indicative: scheduling,
  caches and run order can affect it. It is not a performance regression gate.
- Generated-data precision/recall/F1 are reported separately. Their implementation
  validates record membership without allocating all roughly 50 million possible
  pairs at 10k. Truth and predicted pairs still contribute the same TP/FP/FN counts.

Coverage gates require completed spans, stage counts/units, all eight dbt models and
their counts, SQL profiles for every main stage, usable CPU/memory samples and no
collector errors. The driver also reconciles ingestion totals and verifies that every
current standardized record has membership. Unknown/failed measurements cannot pass
as successful zeroes.

## Profiling an existing CLI invocation

```sh
ER_PROFILE_DIR="$PWD/artifacts/profile/manual" ER_PROFILE_SQL=1 \
  uv run er run-all --mode full --source crm --path /path/to/drop --json
```

Use the normal lake configuration for this command. `ER_PROFILE_DIR` enables spans
and detailed dbt capture; `ER_PROFILE_SQL=1` additionally enables native SQL profiles.
Without these variables, the original CLI stdout and one-line stage stderr contracts
remain intact. Use the campaign driver for continuous resource sampling. SQL profiles
and subprocess logs should be handled with the same access controls as source data.

The `benchmarks/report.py --run` entry point uses the same runner with fresh state
and per-phase memory while preserving its six-phase JSON shape. See
[benchmark baselines](performance.md#benchmark-baselines) for reviewing a measured
run, and [CONTRIBUTING.md](../CONTRIBUTING.md) for local checks and CI shards.
