---
name: ducklake-performance
description: Analyze this repository's full-reload and incremental DuckDB/DuckLake profiling campaigns, identify measured bottlenecks, and propose correctness-preserving SQL, layout, catalog or Python pushdown experiments. Use for workload reports, native query profiles, slow training or matching, partition/order/index questions, and benchmark comparisons.
---

# DuckLake performance analysis

Treat **full reloads and incremental runs as separate primary workloads**. Report
both, including a regression in either one. Follow `DesignDoc.md` contracts and
`docs/performance-profiling-plan.md`. Start with evidence; changing layout or moving
code to DuckDB requires a measured experiment, not a guess from an operator name.

## Establish the evidence

1. Locate the user-specified artifact directory. Read `campaign.json`, `analysis.json`,
   `full-report.md` and `incremental-report.md`. If only raw artifacts exist, regenerate
   offline with `uv run python benchmarks/workload_report.py <directory>`.
2. Check status, query coverage, completed command chains, model/TF reuse and
   incremental/full equivalence. Missing or failed evidence makes a conclusion
   incomplete. Never turn missing measurements into zeroes.
3. Check source/image digest, input hashes, config, generator scenario/seed, versions,
   CPU quota, container memory, DuckDB threads/memory and matching runtime. Different
   values mean a comparison is not controlled. Fresh catalogs do not mean cold caches.
4. Use unprofiled **control command time** as the end-to-end result. Report diagnostic
   overhead separately. Setup, generation, validation and full-rescore references are
   outside workload timing. Do not compare a diagnostic time with a prior control.
5. Read the relevant trial's `query-coverage.json`, `events-*.jsonl`,
   `query-index.jsonl`, `operator-index.jsonl`, `queries.parquet`, `python-summary.json`,
   `resources.jsonl` and the two `*-lake-layout.json` inventories. Follow links to raw
   SQL JSON before recommending a change. Artifacts may contain tenant data; keep
   analysis local and do not send their SQL or values to external services.

## Diagnose each workload

- Separate command startup from stage wall time; use the run's stage ledger and
  monotonic timestamps. Parent/child spans overlap. SQL/operator CPU time can exceed
  wall time with parallel execution. Never sum nested durations as total time.
- For reloads, rank cleaning, training (each estimation method and blocking rule),
  scoring, reconciliation and golden/lineage assembly. Distinguish training pairs
  from prediction candidates. A training sample cap can affect quality and needs
  its own correctness/quality experiment.
- For incremental runs, inspect preparation, new-versus-corpus and new-versus-new
  matching separately, affected-record expansion, cluster updates and touched-entity
  golden assembly. A 100K input batch can trigger much larger reads or rewrites.
- In expensive SQL, inspect actual and estimated cardinalities, rows scanned versus
  returned, filters/projections, join fan-out, repeated scans, windows/sorts, file
  reads, temporary tables and spill. Link each claim to a specific query/operator.
- Connection buffer/spill peaks are high-water marks, not per-query allocations or
  additive totals. cgroup memory includes page cache; process RSS may double-count
  shared pages. Correlate peaks and CPU throttling with the workload interval.
- Read Python self-time and call counts along with native SQL and child-process spans.
  Check `timing_status` first: if self time exceeds cumulative time, use that
  profile's call counts only and obtain timings from native SQL/control spans.
  Time in `execute`, `fetch*`, `poll`, locks or subprocess waits is not proof that
  Python computation is the bottleneck. Identify materialization, serialization or
  per-record callbacks before proposing pushdown. Preserve documented exact bytes,
  stable IDs, null semantics, tie breaks and existing processing exceptions.
  cProfile covers process main threads; dbt worker-thread Python work may be absent.
  Do not interpret an unprofiled thread as doing no work.
- Compare snapshot file counts, sizes, delete files and Parquet row-group statistics.
  Capability-gated storage logs are evidence only if present; unavailable catalog
  timings cannot be inferred from overall SQL latency. Local MinIO cannot establish
  remote S3 request latency or bandwidth.

## Choose a valid experiment

Read [storage and tuning rules](references/tuning.md) before any partition, index,
sort or extension proposal. In particular, do not recommend an ART index on a
DuckLake table. Keep DuckLake/Parquet, local DuckDB scratch, and PostgreSQL metadata
catalog recommendations distinct.

For each recommendation record:

| Item | Required evidence |
|---|---|
| Priority and confidence | Measured contribution and uncertainty |
| Workload and target | Reload/incremental, stage, query/table/operator |
| Observation | Actual rows, time, bytes or calls; artifact path |
| Hypothesis | What avoidable work causes the measured cost |
| Experiment | Exact SQL/config/code/layout change and control |
| Both-workload impact | Expected benefit, regression risk, write/maintenance cost |
| Correctness | Blocking parity, frozen-score purity, deterministic partitions, golden/lineage equality |
| Decision | Measurable success criterion, rejection criterion, next command |

Prefer one variable per experiment. Order by likely end-to-end benefit; a locally
faster query can make incremental writes slower. Future layout experiments must
test: baseline, ordered writes, partitioning, and partitioning plus within-partition
ordering. Include migration, sorting, small-file growth and compaction costs.

## Deliver

Provide separate reload and incremental timing tables, quality/output validation,
coverage limitations, and a ranked experiment ledger with query links. Distinguish
observations from hypotheses. A single control/diagnostic pair locates bottlenecks;
use repeated unprofiled baseline/candidate trials to claim gains. Save durable findings
in `docs/` and retain the artifact directory. Do not automatically change production
schema/config or run expensive campaigns when the request is analysis only.
