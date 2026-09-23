# 1M full reload and 100K incremental profiling

## Objective

Measure and analyze both workloads with equal priority. Start from merged PR #30
(`00a65e4`); retain current production algorithms, model settings and blocking.
Deliver complete profiles, independent workload reports, a repository-owned analysis
skill and an evidence-linked tuning backlog. Optimization follows measurement.

## Workload and campaign

| Workload | Input | Processing |
|---|---|---|
| Full reload | Fresh lake, 1M records / 400K people | ingest, standardize, train, match, reconcile, golden records and lineage |
| Incremental | Same lake, 100K new record keys | ingest, incremental standardize, both matching passes, reconcile, golden records and lineage |

- Baseline generator, seed 42, `configs/default.yaml`; unchanged base CSV bytes.
- `mixed-v1`: 50K additional records for deterministically selected existing people,
  and 50K records for 20K new people. Source/persona IDs and ground-truth contacts
  remain disjoint; new people have two or three records to exercise batch deduping.
- Exactly 1.1M active records after incremental processing. Reuse the same model and
  frozen term-frequency snapshot. Golden counts are measured, not assumed.
- Extend `benchmarks/full_pipeline.py` with `--incremental-records`,
  `--incremental-scenario` and `--with-profile-control`. Expose `make profile-workloads`.
- One unprofiled control and one profiled trial, each with fresh isolated services
  and a full/incremental sequence. Repeat support remains available. Freeze image,
  source, harness, inputs, config, versions, settings and hardware identities.
- Initial local envelope: 2 CPUs, 10 GiB container, 2 DuckDB threads, 4 GB DuckDB.
  Fail capacity checks rather than reducing data. Do not run competing benchmarks.
- Time complete processing commands, with training separately visible and included
  in full-load totals. Exclude generation, setup, validation and teardown.
- Record snapshots after each workload. Defer expensive validation and file audits
  until both workloads finish to avoid benchmark-induced incremental cache warming.
  Fresh state is not a claim of cold OS caches; incremental state is naturally warm.
- One control/profile pair measures instrumentation overhead only indicatively;
  later performance claims require repeated unprofiled comparisons of both workloads.

## Collection and analysis

Retain the existing FIFO native JSON collector. Capture detailed DuckDB plans during
real execution, including writes; never replay mutations with EXPLAIN ANALYZE.
Use unique session, invocation, connection, query and execution identities. Retain
compiled dbt SQL and model IDs, EM-rule spans, and both incremental matching passes.
Audit eligible executed statements and explicit exclusions, failures and missing
profiles. Failed or incomplete measurements remain visible and fail coverage.

Keep per-query JSON, event logs, commands, resource samples, dbt artifacts, per-process
cProfile files, snapshot/file inventories and manifests under unique campaign paths.
Produce compact offline query/operator summaries and separate full/incremental
reports. Index only measured workload queries. Disable native SQL/Python profiling
before untimed validation/reference processing, retaining all correctness checks.

Capture query latency/planning, operator/cardinality data, filters/projections,
scans and available I/O counters; CPU/throttling, process/container memory and spill;
candidate/block skew, affected scope and clustering iterations; DuckLake files,
delete files, sizes, Parquet row groups/statistics, layout and snapshots; available
metadata logging and service telemetry. Unsupported fields are unavailable, not zero.
Engine time, nested/cumulative operator timings and process waiting are not Python
row-processing time. Connection memory peaks must not be summed as query allocations.
Flag internally inconsistent cProfile timing data; use its call counts only. Native
query latency and control wall times provide independent timing evidence.

## Skill and tuning decisions

Version `.agents/skills/ducklake-performance/SKILL.md` with focused references. The
skill validates provenance/coverage, invokes shared offline reporting, examines
source SQL and produces a ranked backlog independently for both workloads.

- DuckLake: pruning, projection, ordering, partitioning, file sizing, compaction,
  small writes and catalog costs. Ordinary table indexes are unsupported.
- Native scratch: materialization/reuse, SQL shape, statistics, resource settings
  and measured selective ART indexes, including their build/memory costs.
- Postgres: investigate catalog queries separately using metadata evidence.
- Ordered-write experiments compare current layout, ordering alone, partitioning
  alone and both. Derive keys from observed filters. Verify actual file/row-group
  statistics and pinned sorted-table support. Include all sorting/writing/maintenance
  costs; layout alterations affect new writes and require an existing-file strategy.
- SQL pushdown starts with the existing processing-exception inventory and measured
  serialization/conversion/loop costs. Preserve hashes, exact event encoding,
  assertions, scoring, deterministic clustering and incremental equivalence.
- Each finding links evidence and code, identifies the cost, explains the hypothesis,
  states effects/costs for both workloads and supplies a reproducible next experiment.
  Large scans are not inherently wrong, particularly for a full reload.

## Validation and completion

Run focused generator/profiler/report tests and one smoke campaign before the 1M
campaign. Check base-byte stability, cohort composition, unique profiles, deferred
fetches, repeated statements, failures, coverage, and profiler semantic transparency.
Validate exact membership cardinality and complete golden lineage at both snapshots.
Use the canonical evaluation implementation for blocking/edge/cluster metrics.
After measurement, perform an untimed full re-resolution of 1.1M with the same
frozen model/TF. Compare canonical partitions, golden values and lineage, normalizing
generated IDs/timestamps through the existing comparison conventions.
Retain the learned model JSON. Independent fits may differ in roundoff: compare
learned probabilities within `1e-12`, other fields exactly, and report byte inequality.
Model and TF byte hashes remain exact within each trial; cross-trial score tolerance
is `1e-10`, while classifications, clusters, golden values and lineage are exact.

Completion requires reproducible raw artifacts, two self-contained workload reports,
an evidence-linked backlog, and a validated offline analysis skill. Subsequent tuning
must preserve correctness and demonstrate benefits without hiding regressions in the
other workload.

## References

- [DuckDB profiling](https://duckdb.org/docs/current/dev/profiling)
- [DuckDB metrics](https://duckdb.org/docs/current/dev/metrics)
- [DuckDB indexing](https://duckdb.org/docs/current/guides/performance/indexing)
- [DuckLake limitations](https://ducklake.select/docs/stable/duckdb/unsupported_features)
- [DuckLake partitioning](https://ducklake.select/docs/stable/duckdb/advanced_features/partitioning)
- [DuckLake sorted tables](https://ducklake.select/docs/stable/duckdb/advanced_features/sorted_tables)
- [DuckLake logging](https://ducklake.select/docs/stable/duckdb/advanced_features/logging)

The linked current documentation is guidance. Feature probes and the installed
literal pins determine what this campaign can collect or recommend.
