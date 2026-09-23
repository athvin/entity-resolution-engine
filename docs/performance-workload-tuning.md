# Profile-guided workload tuning

This follows the [1M reload / 100K incremental profiling findings](performance-profiling-1m-100k.md).
Both workloads are primary. Training, blocking rules, model parameters, thresholds,
survivorship rules and clustering semantics retain their existing contracts.

## Complete workload measurements

Three unprofiled baseline trials and three candidate trials produced these local
results on 2026-09-23:

| Workload | Baseline median | Tuned median | Reduction | Baseline range | Tuned range |
|---|---:|---:|---:|---:|---:|
| Full reload, 1M records | 94.07s | 91.70s | 2.53% | 93.30–95.86s | 91.04–91.95s |
| Incremental, 100K net-new record keys | 39.96s | 39.48s | 1.21% | 39.64–40.87s | 39.43–39.60s |

These are modest improvements. All three candidate samples were faster than the
baseline range in both workloads, although the incremental ranges nearly touch.
Three samples on one corpus establish a local result, not a production-wide speedup
or a CI regression threshold. Full-load training command time was effectively
unchanged: 10.61s versus 10.63s. Incremental runs reuse training.

Each trial ran the entire reload (ingest, clean, train, match, reconcile, golden
records and lineage), then appended the same 100K `mixed-v1` batch on that lake.
It used seed 42, the baseline generator profile and the default semantic configuration.
The processing envelope was two CPUs, 10 GiB container memory, two DuckDB threads
and a 4 GB DuckDB memory limit. Runtime pins were DuckDB 1.5.5, DuckLake `d8a1881e`,
Splink `5.0.0.dev5`, dbt-core 1.12.2 and dbt-duckdb 1.11.0, on the same ARM64
Docker host with local PostgreSQL/MinIO storage.

Trials ran sequentially in fresh stacks. One baseline sample reuses the earlier
profiling campaign's unprofiled control; two new baseline runs and three candidate
runs supplement it. Application images, input hashes and resource settings are
recorded per trial. The original control and current runner have different harness
fingerprints; subsequent harness changes include disabling diagnostic collection
during untimed validation and adding analysis tools. The timed application image
is identical across all baseline samples and identical across all candidate samples.
Fresh lakes do not imply cold OS caches. Generation, initialization, validation and
the final full-reference rescore are outside both workload timings.

Stage ledger medians exclude CLI startup and do not sum to the command totals:

| Stage | Reload baseline | Reload tuned | Incremental baseline | Incremental tuned |
|---|---:|---:|---:|---:|
| Ingest | 4.156s | 4.096s | 1.247s | 1.247s |
| Standardize | 18.862s | 18.640s | 10.410s | 10.458s |
| Train | 8.379s | 8.376s | reused | reused |
| Match | 10.598s | 10.591s | 5.301s | 5.222s |
| Reconcile | 23.226s | 20.985s | 5.834s | 5.624s |
| Golden records and lineage | 11.055s | 11.163s | 8.269s | 8.242s |

Reconciliation improved by 9.65% on reload and 3.60% on incremental processing.
Unchanged stages have small timing movements too; those are not attributed to these
changes. Median pipeline CPU time fell from 127.38s to 124.88s on reload and from
52.61s to 51.36s on incremental processing. The largest sampled pipeline memory peak
across repetitions fell from 4.03 to 3.94 GiB and from 2.50 to 2.45 GiB, respectively.
Those peaks include children and page cache, exclude catalog/object-store memory,
and can miss short spikes. They are not minimum memory requirements.

## Correctness and diagnostic evidence

All six timing trials passed their incremental/full-reference checks. Within each
trial, the model and frozen TF bytes stayed unchanged; rescoring the final corpus
found zero mismatches and zero probability difference across 1,011,329 stored pairs.
Across versions, standardized rows, blocking keys/candidates, score classifications,
canonical cluster partitions, golden values and lineage match exactly under the
existing normalization of generated IDs and timestamps. Quality is unchanged.

| Output | After reload | After incremental |
|---|---:|---:|
| Active records and memberships | 1,000,000 | 1,100,000 |
| Golden records | 395,867 | 414,457 |
| Lineage decisions | 2,375,202 | 2,486,742 |
| Stored scored pairs | 840,771 | 1,011,329 |
| Cluster precision | 90.3394% | 89.9792% |
| Cluster recall | 98.7000% | 98.8003% |

Independent training produces different model bytes: the largest learned-parameter
difference among the six controls was `9.904e-14`, and the largest pair-probability
difference was `3.995e-13`. These are below the existing `1e-12` and `1e-10` tolerances;
all other model fields, TF content and threshold decisions compare exactly. This
does not relax the requirement to reuse the identical model/TF within a trial.

The separate diagnostic was also validated against the first candidate control
using the same immutable candidate image. Its maximum model and score differences
were `1.840e-13` and `5.446e-13`, respectively, with exact semantic outputs and
unchanged quality. Diagnostic timings are excluded from the performance medians.

The retained workload profiles confirm that the intended repeated work disappeared:

| Diagnostic observation | Reload before | Reload after | Incremental before | Incremental after |
|---|---:|---:|---:|---:|
| Canonical event encodes | 1,583,468 | 791,734 | 284,344 | 142,172 |
| `dataclasses.replace` calls in reconcile CLI / incremental `run-all` | 395,867 | 0 | 72,589 | 1,503 |
| Native workload query profiles | 2,881 | 1,159 | 843 | 577 |
| Query profiles within bulk-load spans | 1,973 | 251 | 307 | 41 |
| Reconciliation queries containing the current-edge window | 4 | 2 | 4 | 2 |

Bulk-load spans include interleaved page reads as well as inserts; their counts are
not a count of lake commits. Remaining incremental `replace` calls come from other
code, while the 71,086 event copies disappear. One current-edge window builds the
temporary captured relation, and the other belongs to the unchanged generation
guard. See the [full-load capture](../artifacts/performance/workload-tuning-20260923/candidate-profile/run-001/sql/5f3b18c443d74861bc0772eccf565dea-3d2a8f7c795044e4b328866bb176cb29.json)
and [incremental capture](../artifacts/performance/workload-tuning-20260923/candidate-profile/run-001/sql/b444e0b922134940b150605fb3d36b60-51141b083edb4790ada8e1eb6bf88d64.json).

The new capture has 1,736 workload profiles, plus 71 setup/checkpoint profiles,
with no missing required or unattributed profiles. The older collector also saved
untimed validation queries; the comparison above uses workload indexes only.
The Python profiles have internally inconsistent timings/caller statistics, so only
top-level function call counts are used. No speedup is inferred from cProfile times.

Validation also includes 923 passing unit tests, a subsequent eight-test benchmark
check including the new cross-image comparison gate, and 37 targeted integration
tests covering current scores, incremental equivalence, assertions, cuts, deletion,
splits and golden survivorship. Spec validation, Ruff and strict mypy checks pass.
The integration image and final timing image differ in application source only by
event docstrings; all million-record trials use the same final candidate image.

The repository retains [machine-readable measurements](measurements/workload-tuning-20260923.json).
Raw artifacts are ignored by Git under `artifacts/performance/workload-tuning-20260923/`;
retain that directory and the original profiling campaign when sharing full evidence.
Entry points are the [timing comparison](../artifacts/performance/workload-tuning-20260923/comparison/report.md),
[diagnostic reload report](../artifacts/performance/workload-tuning-20260923/profile-analysis/full-report.md),
[diagnostic incremental report](../artifacts/performance/workload-tuning-20260923/profile-analysis/incremental-report.md)
and [native query index](../artifacts/performance/workload-tuning-20260923/candidate-profile/run-001/query-index.jsonl).

## Implementation

The implementation removes repeated work in reconciliation and bounded staging:

- [Event encoding](../src/er/entities/events.py) produces canonical JSON and its
  digest together, then reuses the encoded document when persisting the event.
  [Lifecycle planning](../src/er/entities/relational.py) does the same for its
  unreasoned ordering document. These remain separate passes because reason stamping
  participates in the persisted hash. Page offsets assign event sequence numbers
  at construction, eliminating a dataclass copy per emitted event.
- [Temporary staging](../src/er/lake/bulk.py) uses 8,192-row pages instead of 1,024.
  SQL still loads column arrays with `UNNEST`; Python does not insert individual
  lake rows. This amortizes binding and statement overhead while keeping a row bound.
  It increases the maximum page size eightfold; a large entity's event payload can
  still be large, so this is not a byte limit.
- [Current scored edges](../src/er/matching/edges.py) are captured once in a
  temporary DuckDB table for each reconciliation. Multiple consumers reuse the
  filtered/current-generation result. The writer lock prevents concurrent score
  changes; the table is dropped when its owner finishes. Assertions and cuts are
  applied separately, and the default live view remains available to callers that
  require it. No persistent lake table or extra DuckLake snapshot is introduced.

Canonical encoding stays in Python under the existing
[documented exception](python-processing-exceptions.md). DuckDB's JSON formatting
differs from Python's for some floats and escaped characters; pushing that operation
into SQL without a compatible encoder would change event hashes and retry keys.
Membership, joins, sorting, aggregation and clustering relations stay in DuckDB.

The captured score table uses DuckDB-managed memory and can spill; it trades an
extra temporary relation for fewer repeated scans/windows. The larger Python pages
and that temporary table require measured memory checks as well as timing checks.
This campaign covers the 1M workload and its first 100K append on local storage.
It does not establish performance at 10M, under a smaller memory envelope, for
repeated update/delete deliveries or for a remote object store.

## Screening experiments

The reproducible [kernel screen](../benchmarks/tuning_kernels.py) uses DuckDB 1.5.5,
two threads and a 4 GB memory limit. Its survivorship relation has one million
members in 400,000 entities, with two or three members each; the touched subset
selects every sixth entity. Every alternative is compared using `EXCEPT ALL` in
both directions. Three executions alternate variant order. These are native local
DuckDB screens, not complete DuckLake pipeline measurements.

| Survivorship workload | Existing window | Top-two aggregate | Shared window with `lead` |
|---|---:|---:|---:|
| Full; validation/source/recency | 0.2075s | 0.3289s | 0.2003s |
| Full; frequency/completeness/recency | 0.2510s | 0.3955s | 0.2534s |
| Touched; validation/source/recency | 0.0572s | 0.0788s | 0.0602s |
| Touched; frequency/completeness/recency | 0.0665s | 0.0901s | 0.0654s |

The bounded `arg_min(..., 2)` aggregate was slower. The `lead` variant had mixed
small changes, with no consistent benefit across both workloads. Neither is promoted;
the production golden-record macro is unchanged. Experimental macros and instructions
remain in [benchmarks/experiments](../benchmarks/experiments/README.md).

The same screen measured 1.5655s for 1,024-row staging pages, 1.4580s for 8,192,
and 1.4484s for 16,384. The smaller of the two larger pages was selected for the
pipeline trial. Four consumers of current scored edges took 0.4661s using a view
versus 0.2374s with a table, including construction. These isolated measurements
justify trying the implementation; only complete unprofiled workloads can establish
its end-to-end benefit.

Raw kernel evidence is in
[`kernels-lead.json`](../artifacts/performance/workload-tuning-20260923/kernels-lead.json).

## Storage results: retain the current raw-record layout

[The layout screen](../benchmarks/layout_screen.py) uses the installed DuckLake
extension with a real PostgreSQL catalog and MinIO storage. It compares four
fresh-table layouts: current writes, sorted writes, source partitions, and source
partitions with sorting. Sorting uses
`(source_system, ingest_batch_id, source_record_id)`; partitioning uses only
`source_system`. It loads each source separately, as production ingestion does,
then adds the same 100K batch. It includes insertion/sorting costs and the
source/batch payload scans in each phase's total. Three repetitions reverse arm
order on alternating passes. Unique native profiles replay reads after both timed
deliveries, at the saved snapshots.

| Layout | Full write + read median | Incremental write + read median | Data files after full / batch |
|---|---:|---:|---:|
| Current | 1.8894s | 0.4354s | 3 / 6 |
| Ordered | 2.0417s | 0.4505s | 3 / 6 |
| Source partitioned | 2.0355s | 0.5068s | 3 / 6 |
| Partitioned and ordered | 2.1302s | 0.4638s | 3 / 6 |

None improved the total of writes and reads in either phase. Full-data read medians
were 0.3371s for current writes and 0.3167s for partitions plus ordering, but write
medians rose from 1.5555s to 1.8239s. Production raw-record DDL remains unchanged.
The screen used two CPUs, a 6 GiB container limit and DuckDB's 4 GB memory limit;
the complete pipeline comparisons use a 10 GiB container limit.

The diagnostic CRM scan reported one data file after the full load and two after
the batch for every arm. Its scan filter included `source_system='crm'`; the
`ingest_batch_id NOT IN (...)` condition was applied above the scan. Thus source
partitioning did not reduce the reported data-file reads relative to the natural
source grouping, and sorting did not make this batch predicate prune the old file.
A future predicate-pushdown experiment should check whether expressing eligible
batches differently can prune old deliveries while preserving checkpoint semantics.
This is more specific than adding partitions and hoping the optimizer uses them.
The profiles report zero bytes read and inflated rows-scanned counters on this
cached extension path; those counters are not physical I/O or distinct-row evidence.

See [all trials and file sizes](../artifacts/performance/workload-tuning-20260923/layout/results.json)
and example [current](../artifacts/performance/workload-tuning-20260923/layout/2-current-batch.json)
and [partitioned/ordered](../artifacts/performance/workload-tuning-20260923/layout/2-both-batch.json)
incremental read profiles. The pinned `d8a1881e` extension accepted both layout DDL
features; this result does not rely on features available only in newer documentation.

This is a screen of raw-record storage and scans. It does not measure a complete
standardization stage, scoring, golden assembly, compaction or repeated-delivery
layout decay. Reads immediately follow writes, and the local object store is not
a remote S3 latency experiment. A successful screen would still require both full
pipeline workloads, an existing-file migration strategy and repeated deliveries
before changing production DDL.

`ORDER BY` can improve Parquet locality and min/max pruning without an index.
It helps only when the resulting ranges let the actual access path skip data;
sorting also has a write cost. Source-by-source writes already provide some source
clustering. A layout for `int_blocking_keys` would not automatically accelerate
Splink's scoring joins, which read standardized columns. Random affected-record
keys may touch most row groups even in an ordered table. DuckLake does not support
ordinary DuckDB ART indexes. See the official
[DuckDB indexing guide](https://duckdb.org/docs/current/guides/performance/indexing),
[DuckLake limitations](https://ducklake.select/docs/stable/duckdb/unsupported_features),
[partitioning](https://ducklake.select/docs/stable/duckdb/advanced_features/partitioning)
and [sorted tables](https://ducklake.select/docs/stable/duckdb/advanced_features/sorted_tables).

## What the evidence points to next

1. Measure dbt process startup, parsing and connection/catalog work separately from
   model SQL. Cleaning and assembly have substantial command time beyond their native
   queries. Reuse must preserve tenant configuration, run variables and compiled SQL;
   the unexplained wall-time gap is not proof of row-by-row Python work. The original
   diagnostic's [CLI import spans](../artifacts/performance/workload-tuning-20260923/import-spans.json)
   alone total 6.85s across eight reload commands and 3.41s across four incremental
   commands. These are instrumented wall spans, not estimates of an achievable gain,
   and are already included in their enclosing command times.
2. Test an incremental batch predicate that permits file pruning. Preserve the
   existing batch-ID checkpoint and replay behavior, including tied timestamps and
   partially processed deliveries. Measure several appends and source updates/deletes,
   not only this first append.
3. Investigate label-propagation work proportional to changed labels. The original
   diagnostic found seven iterations on the full load. Any frontier or adjacency
   reuse needs deterministic minimum labels, adversarial-chain tests, never-cut
   parity and unchanged non-convergence failure behavior. No new index is justified
   merely by seeing a large join.
4. Keep the 10M training fan-out investigation separate. Training is around eleven
   percent of this 1M reload, and incremental processing reuses the model. Changing
   EM rules or sampling changes learned parameters and needs the existing hard and
   held-out quality gates. These implementation changes do not address quadratic
   candidate growth at larger scales.

## Reproducing comparisons

Freeze each application image before measuring. Keep the base corpus and the
100K `mixed-v1` batch identical across arms. Run trials sequentially, each in a fresh
PostgreSQL/MinIO stack, and use the same CPU quota, memory limits and DuckDB threads.
Run the reload and its incremental delivery before validation or diagnostic replay.

The [full pipeline runner](../benchmarks/full_pipeline.py) accepts an immutable
`--image` plus `--image-source`, and `--corpus-root` reuses the generated inputs.
Use `--scale 1m --local --with-incremental --incremental-records 100000
--incremental-scenario mixed-v1`. A separate run with `--profile` captures native
SQL and Python diagnostics; do not include its times in the control medians.

Compare successful unprofiled trial directories with:

```sh
uv run python benchmarks/tuning_report.py \
  --baseline artifacts/performance/baseline-1 artifacts/performance/baseline-2 artifacts/performance/baseline-3 \
  --candidate artifacts/performance/candidate-1 artifacts/performance/candidate-2 artifacts/performance/candidate-3 \
  --out artifacts/performance/comparison
```

The report requires incremental/full equivalence in every trial. It compares input
bytes, configuration, runtime settings, canonical partitions, golden values, lineage,
candidate sets, threshold classifications, quality and frozen TF. Independent fits
allow only learned-probability roundoff of `1e-12`; cross-trial pair probabilities
allow `1e-10`. Model/TF bytes must remain unchanged within each trial. Application
image differences are explicitly allowed for this comparison; normal paired profiling
still requires the same image. Missing evidence fails the comparison.

The recorded [layout Compose override](../artifacts/performance/workload-tuning-20260923/layout-compose.json)
pins the candidate image and mounts the corpus, benchmark code and output directory.
Adjust its absolute mounts on another checkout. Choose a fresh project name and output
directory; the screen deliberately refuses to overwrite an existing result directory.
For example, with that override on the original checkout:

```sh
docker compose -p er-layout-repro-001 -f docker/compose.yaml \
  -f artifacts/performance/workload-tuning-20260923/layout-compose.json \
  --profile test run --rm --no-build --entrypoint python pipeline \
  benchmarks/layout_screen.py --corpus /corpus --out /screen/layout-repro-001 --repeat 3
docker compose -p er-layout-repro-001 -f docker/compose.yaml \
  -f artifacts/performance/workload-tuning-20260923/layout-compose.json \
  --profile test down -v --remove-orphans
```

The second command removes only this disposable project's services and volumes.
Do not run layout screens, tests or another pipeline concurrently with timing controls.
