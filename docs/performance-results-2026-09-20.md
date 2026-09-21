# Pipeline performance results: 2026-09-20

The optimized pipeline reduced median processing time from **90.46 to 65.46 seconds**
for 10,000 source records followed by 100 incremental records: **27.6% less runtime,
1.38× throughput, and 24.99 seconds saved per run**.

| Metric | Instrumented baseline | Optimized candidate | Change |
|---|---:|---:|---:|
| Median processing time | 90.46 s | 65.46 s | −27.6% |
| Median container CPU time | 114.07 CPU-s | 69.47 CPU-s | −39.1% |
| Median sampled memory peak | 2,326.6 MiB | 2,317.5 MiB | −0.4% |
| Runtime coefficient of variation | 0.79% | 1.19% | Both below 5% |

## Method

Five alternating baseline/candidate pairs used immutable images, identical generated
inputs and fresh Compose stacks on an Apple M2 host running Docker Desktop. Each
processing container had two CPUs, 6 GiB memory, two DuckDB threads and a 4 GB
DuckDB memory limit. The baseline already included the complete pipeline wiring and
profiling instrumentation; this comparison measures the subsequent optimizations.

Processing time includes complete CLI lifetimes, normal stage logs and resource
measurement for full ingestion, standardization, training, matching, reconciliation
and assembly, followed by incremental processing. Initialization, corpus generation,
correctness verification and reporting are excluded. Memory is sampled every 250 ms
and includes container page cache; its measured change is effectively flat.

Normal-run processing times, in campaign order:

| Pair | Baseline (seconds) | Candidate (seconds) |
|---|---:|---:|
| 1 | 91.867850 | 66.528420 |
| 2 | 89.925517 | 65.464218 |
| 3 | 89.885412 | 64.451343 |
| 4 | 90.455813 | 64.449962 |
| 5 | 90.586459 | 65.475926 |

## Improvements

Bounded, typed array inserts replace row-by-row ingestion and graph/key loading.
Native connections are reused within a locked CLI invocation and closed before dbt.
Schema preflight batches metadata reads, heavy CLI imports are deferred, and the
first nickname seed and standardization build share one dbt invocation.

| Largest component improvements | Baseline median | Candidate median |
|---|---:|---:|
| Time outside stage ledgers | 39.70 s | 25.40 s |
| Initial standardization | 15.43 s | 11.40 s |
| Initial ingestion | 2.78 s | 0.34 s |
| Full reconciliation | 6.97 s | 5.50 s |
| Incremental ingestion | 1.43 s | 0.17 s |

Component medians need not sum to the total median. The changes were measured
together, so these deltas do not isolate the effect of each individual optimization.
Deferred imports also move some work between startup and preflight/stage boundaries.

Separate detailed 10k runs captured 38,953 → 1,010 native SQL profiles, 98 → 16
CLI lake connection opens, and 5 → 4 dbt invocations. These counts are restricted to
processing command windows; connection counts exclude dbt's internal adapter
connections. Detailed profiling overhead above the normal-run median fell from
44.0% to 7.7%. The headline improvement uses the normal runs, with SQL profiling off.

Standardization and golden assembly still account for roughly 29 seconds of the
candidate's stage medians, with another 25.4 seconds outside stage ledgers. These
results apply to this workload and resource envelope; they do not establish 100k
performance.

## Correctness and validation

All 16 pipeline runs succeeded: tiny and smoke gates for both versions, ten normal
10k trials and two detailed 10k trials. All 13 cross-run comparisons matched
standardized values, blocking keys/candidates, score classifications, membership
partitions, golden records and lineage after normalizing generated IDs and
timestamps. The largest probability difference was 1.33×10⁻¹⁵, below the 1×10⁻¹⁰
tolerance. Every large trial finished with 10,100 current records, 4,023 entities
and 24,138 lineage rows.

Local validation passed 710 unit tests and 46 distinct affected integration checks,
plus Ruff lint/format, strict mypy and diff checks. The integration results combine
44 passing checks from the broad run with successful reruns after updating two
legacy fixture/assertion assumptions for the real pipeline and new counters.

## Provenance and reproduction

The local campaign is `artifacts/performance/20260920t213140z/`. Raw profiles,
command logs, comparison JSON, source archives, per-file source hashes, the common
harness and case-level validation results are retained there and ignored by Git.

| Snapshot | SHA-256 |
|---|---|
| Baseline image | `7b430de47a81f8d385379b6391ee5d2d5552d153bc3717b5794a8856b9b750b8` |
| Candidate image | `30d0fe2d14071753bcc31a68b96c8ec6b4e6f96ddf63c062d3622be66c499bc6` |
| Baseline source overlay | `f297ba0c2079f5495837e4b9ce93ad7cc36f7f50e5b3376277b81b95e7804a7e` |
| Candidate source overlay | `85099c648bee69131cf7a4c5e4175af015f695bd1908db587b9eb3d79395dc3a` |

A post-campaign reporting correction excludes post-command validation events from
diagnostic counts. That reporting correction left pipeline sources, measured
timings/resources and output checks unchanged; both reports remain in the campaign.

Subsequent PR validation corrected the reconciliation cutoff to use the successful
stage's start time, so unchanged follow-up runs do not process already reconciled
changes again. It also made the benchmark restore its caller's environment settings.
The paired measurements above belong to the frozen images listed here; they are
not a fresh five-pair measurement of those follow-up fixes.

See [performance.md](performance.md) to reproduce the comparison and
[profiling.md](profiling.md) for instrumentation and measurement semantics.
