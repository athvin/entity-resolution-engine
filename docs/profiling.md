# Profiling the complete pipeline

For a controlled baseline/candidate performance comparison, see
[performance.md](performance.md).

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
  measures quota-related delays. The standard envelope is two CPUs, 6 GiB container
  memory, two DuckDB threads and a 4 GB DuckDB limit.
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
