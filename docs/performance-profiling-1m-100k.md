# 1M reload and 100K incremental: measured profiling baseline

On 2026-09-23 the unprofiled full reload completed in **93.30 seconds**, including
cleaning, training, deduplication, reconciliation and golden/lineage assembly.
Adding **100,000 new record keys to that same lake** completed in **39.64 seconds**.
Both workloads passed output validation. Each incremental result matched an untimed
full re-resolution using its unchanged model and frozen term frequencies.

This implements the [profiling plan](performance-profiling-plan.md). It measures the
current pipeline and identifies experiments; it does not establish new optimization
gains. Full reloads and ongoing incremental runs remain separate primary workloads.

The subsequent [implementation and repeated measurements](performance-workload-tuning.md)
apply event-encoding reuse, larger bounded batches and a captured current-score
table. That report records the measured gains and the SQL/layout alternatives that
did not improve the workload; this document retains the original diagnostic baseline.

## Workload, environment and evidence

The baseline generator uses seed 42 and `configs/default.yaml`. The initial million
records represent 400,000 people. The `mixed-v1` delivery contains 50,000 additional
records for existing people and 50,000 records for 20,000 new people. The final lake
contains 1,100,000 active records. The reference configuration retains uncapped EM.

One unprofiled control and one diagnostic trial ran sequentially in separate, fresh
Compose stacks, each performing the reload followed by the incremental delivery.
Both used the same immutable image, input hashes and semantic configuration. Tenant
namespace differences change the complete config hashes. Existing idle audit services
were left intact; no other pipeline benchmark ran alongside the campaign.

| Setting | Value |
|---|---|
| Processing container | 2 CPUs, 10 GiB |
| DuckDB | 1.5.5; 2 threads; 4 GB memory limit |
| DuckLake extension | `d8a1881e` |
| Splink | `5.0.0.dev5` |
| dbt-core / dbt-duckdb | `1.12.2` / `1.11.0` |
| Docker host | ARM64, 8 CPUs, 12,529,836,032 bytes RAM |
| Storage | Local MinIO and PostgreSQL catalog |
| Starting Git revision | `00a65e4` (merged PR #30) |

The committed [measurement JSON](measurements/workload-profiling-20260923.json)
contains input, model, TF, source and image hashes; settings; workload timings;
quality; storage counts; and validation outcomes. Application source SHA-256 is
`b9da830995ebfe7ce1c45c9d61851a0feac78f1d7f21ee885ed576433ddea9c6`.
This is a source-content fingerprint, not a Git commit. The separate harness
fingerprint is retained in the measurement and campaign manifest.

Raw artifacts are local, ignored by Git, under
`artifacts/bench/workload-1m-100k-20260923/`. Retain or transfer that directory when
sharing the full evidence. Entry points:

- [Full workload report](../artifacts/bench/workload-1m-100k-20260923/full-report.md).
- [Incremental workload report](../artifacts/bench/workload-1m-100k-20260923/incremental-report.md).
- [Machine-readable report](../artifacts/bench/workload-1m-100k-20260923/analysis.json).
- [Native query index](../artifacts/bench/workload-1m-100k-20260923/profiled/run-001/query-index.jsonl).
- [Python diagnostics](../artifacts/bench/workload-1m-100k-20260923/profiled/run-001/python-summary.json).

## Both workload timings

| Workload | Unprofiled control | Diagnostic | Indicative collection overhead |
|---|---:|---:|---:|
| Full reload, 1M records | 93.30s | 132.39s | 41.9% |
| Incremental, 100K new records | 39.64s | 59.02s | 48.9% |

These are complete pipeline command times, including CLI startup. Generation,
stack setup, validation and the full reference are excluded. Validation/file audits
ran after both deliveries using saved snapshots, so they did not warm the incremental
workload. Fresh catalogs do not imply cold OS caches. One pair is insufficient for
a regression threshold or a statistically supported performance improvement.

Full-load command times:

| Stage | Control | Diagnostic |
|---|---:|---:|
| Ingest, three sources | 10.29s | 11.66s |
| Clean / standardize | 20.88s | 28.48s |
| Train | 10.55s | 12.37s |
| Match | 13.00s | 13.78s |
| Reconcile | 25.39s | 46.35s |
| Golden records and lineage | 13.19s | 19.75s |

The incremental chain runs inside one `run-all` command after three ingest commands.
Its stage ledger excludes some command startup and is therefore not directly
comparable to the full-load command table:

| Incremental stage | Control stage time | Diagnostic stage time |
|---|---:|---:|
| Ingest, three sources | 1.25s | 1.42s |
| Clean / standardize | 10.35s | 17.28s |
| Match, both passes | 5.30s | 5.71s |
| Reconcile | 5.80s | 9.54s |
| Golden records and lineage | 8.24s | 14.98s |

The incremental run reused training. Its 100K inputs produced 170,558 scored pairs;
reconciliation processed 234,337 affected records into 69,947 clusters. Golden
assembly received 71,086 touched entity IDs and rebuilt 69,947 active golden records.
The size of this expanded work is more informative than batch size alone.

Control CPU consumption was 126.31s for reload and 51.85s for incremental processing.
Sampled cgroup memory peaks were 3.91 GiB and 2.50 GiB, respectively. CPU throttling
was 14.86s and 7.43s. No sampled temporary-directory spill was observed; sampling
does not prove that no temporary I/O occurred between samples. Memory includes page
cache and is not a minimum capacity requirement.

## Correctness, quality and collection coverage

Every standardized record has membership, golden lineage has six decisions per
active entity, and lineage winners belong to their entities. The control and
diagnostic agree on standardized data, blocking keys/candidates, score classes,
canonical cluster partitions, golden values and lineage at each checkpoint.

| Output or metric | After reload | After incremental |
|---|---:|---:|
| Active records and memberships | 1,000,000 | 1,100,000 |
| Golden records | 395,867 | 414,457 |
| Lineage decisions | 2,375,202 | 2,486,742 |
| Candidate pairs in the resulting corpus | 6,430,399 | 7,785,713 |
| Stored scored pairs | 840,771 | 1,011,329 |
| Blocking recall | 99.5583% | 99.5589% |
| Edge precision | 97.3505% | 97.3793% |
| Cluster precision | 90.3394% | 89.9792% |
| Cluster recall | 98.7000% | 98.8003% |
| False-positive cluster pairs | 84,437 | 106,170 |

Candidate totals describe each resulting corpus, not the work of the incremental
matching passes. Quality is identical between control and diagnostic for each
checkpoint; it changes when the corpus changes. The baseline generator remains
optimistic, and these precision results still leave substantial false merges.
All quality calculations use the repository's canonical evaluation implementation.

Within each trial, model/TF bytes remain unchanged. Comparing incremental against
the final full re-resolution found **zero mismatches across 1,011,329 stored pairs**
and a maximum probability difference of **zero**, as well as identical semantic
outputs. Independent training in the two trials produced different model bytes:
the largest learned-parameter difference was `2.404e-14`, below `1e-12`; other model
fields and TF data were exact. Cross-trial probability differences reached
`1.760e-13`, below `1e-10`, with exact threshold classifications and partitions.
Generated entity IDs and timestamps are normalized by the existing comparison rules.

The native collector captured **2,881 reload profiles and 843 incremental profiles**,
covering all eight dbt models per phase, the training estimators and both incremental
matching passes. There were no missing required or unattributed workload profiles.
The [coverage report](../artifacts/bench/workload-1m-100k-20260923/profiled/run-001/query-coverage.json)
retains explicit metadata/unconsumed-query exclusions.

This first execution also captured untimed validation SQL, for 51,378 raw profiles
in total. The current offline indexes include only the 3,724 workload profiles.
The harness now disables SQL/cProfile collection before untimed validation/reference
processing while retaining every correctness check. That post-measurement change
does not alter any timed pipeline command; the recorded campaign retains its
original harness fingerprint and raw validation files.

All 16 workload cProfile files have internally inconsistent function timings, such
as self time exceeding cumulative time. Their duration rankings are unusable.
The analyzer flags this and preserves call counts as structural evidence. Timings
below come from control commands or native DuckDB profiles. Main-thread cProfile
also does not fully cover dbt's worker threads. A timing gap between a command and
its SQL is not proof of Python CPU overhead.

## Evidence-linked tuning backlog

The following are experiments, not measured savings. Run one change at a time.
For each candidate, repeat the paired campaign three times for both the baseline
and candidate source/image; compare their **unprofiled** medians and spread. Require
the same inputs, envelope, output-equivalence gates and quality results. Reject a
change whose gain is within measurement noise or whose cost is displaced into the
other workload. Record both total times, even for an experiment aimed at one stage.

### 1. Remove repeated event serialization and reduce bounded bulk calls

**Observation:** reconciliation is the largest reload command at 25.39s. Its
[Python call-count evidence](../artifacts/bench/workload-1m-100k-20260923/profiled/run-001/python/cli-1150-92b941072b5949b2bccf204e81518c6f.pstats)
contains 1,583,468 calls to `canonical_details`, exactly four per initial entity,
and 395,867 `dataclasses.replace` calls. Native profiles contain 1,936
`sql.bulk_load` executions in this stage, totaling 4.07s of diagnostic SQL latency.
The stage is expensive and repeated work exists; the avoidable Python seconds
are not yet measured. Confidence in an end-to-end gain is therefore medium.

**Experiment:** follow [relational event construction](../src/er/entities/relational.py),
[event encoding](../src/er/entities/events.py) and [bulk loading](../src/er/lake/bulk.py).
First compute canonical event bytes once and reuse them for hashing/persistence.
Separately compare larger bounded bulk pages with the current page size. A SQL
encoder is a later variant only if it reproduces canonical JSON bytes exactly,
including Unicode, member ordering, reason stamps and idempotency hashes.

**Both workloads:** fewer encodings/uploads may help reload and incremental entity
changes. Larger pages can increase memory and weaken latency for small batches.
Keep the distinct entity/event IDs and existing event contracts. Accept only after
event replay, retries, assertions and full/incremental output checks pass, with a
repeatable total-time gain and no material regression in the other workload.

### 2. Reduce the work needed to choose the first two survivorship records

**Observation:** the [full lineage write](../artifacts/bench/workload-1m-100k-20260923/profiled/run-001/sql/3486646b94544fa591ed291f648c9f01-c9a1f244cc9549aa9c9f228866ae4eb3.json)
took 3.98s. Its three most expensive ranking operators each handle 1M rows.
Lineage-model SQL totals 4.01s on reload and 1.29s on the incremental delivery.
Golden values already reuse lineage winners; implementing that reuse again would
not remove work. Confidence in the ranking cost is high; the alternative is unproven.

**Experiment:** prototype bounded top-two aggregation inside
[survivorship_decision](../dbt/macros/survivorship/survivorship.sql), or reuse rankings
only where complete ordering expressions and input rows are identical. Preserve
both the winner and runner-up: lineage attributes the first rule separating them.
Email and phone validity columns differ even when compiled aliases look identical.
Keep null behavior, frequency/completeness rules, the composite address and terminal
`record_key` ordering. Compare [lineage](../dbt/models/marts/golden_lineage.sql) and
[golden values](../dbt/models/marts/golden_records.sql) exactly for all configured chains.

**Both workloads:** measure the full relation and touched subset separately. Reject
an aggregate that saves reload sorting but uses excessive per-group memory or slows
incremental assembly. Include preparation/materialization in the comparison.

### 3. Reuse the current-score relation within reconciliation

**Observation:** the [incremental edge-selection profile](../artifacts/bench/workload-1m-100k-20260923/profiled/run-001/sql/3617ab5df7b1404caf1bfd46c2344295-5ca2c1a2ffa941f98c35567c60c1dadf.json)
ranks 1,011,329 score rows by pair, `scored_at` and `run_id` before affected-edge work.
Four lifecycle queries execute this window: 0.99s combined SQL latency in incremental
processing and 0.83s on reload. The scan reads both score files. This is a bounded
opportunity, not an explanation for the whole incremental duration.

**Experiment:** compare the lazy view in [current edges](../src/er/matching/edges.py)
with a temporary materialization reused inside one reconciliation transaction.
Include materialization and cleanup costs. Check whether assertion/cut changes in
the fixpoint require rebuilding a filtered relation; never cache stale effective
edges across those changes or across runs. Confidence is medium.

**Both workloads:** reload may pay a materialization cost it cannot recover;
incremental may reuse the result more often. Require unchanged latest-score,
generation, assertion and cut semantics, and compare total reconcile times.

### 4. Test label-propagation work proportional to changed labels

**Observation:** label propagation used seven iterations (14 native statements)
and 5.63s of diagnostic SQL latency on reload, versus 1.23s on incremental input.
An [iteration profile](../artifacts/bench/workload-1m-100k-20260923/profiled/run-001/sql/8178200f3c1f4fe7bfedaf2f32ade539-c68d28b00d9c4040bb17c46677001a32.json)
shows adjacency joins, grouped minima and pointer jumping over temporary tables.

**Experiment:** investigate reusing adjacency work or restricting later rounds to
changed labels in [label propagation](../src/er/entities/cluster.py). This has higher
correctness risk than serialization reuse: the final minimum label, pointer-jump
behavior, never-assertion cuts and non-convergence failure must remain deterministic.
Test long/adversarial chains and measure extra frontier bookkeeping on small touched
subgraphs. Confidence in the observed cost is high; a frontier algorithm's net gain
is low-confidence until demonstrated. Broad hash joins alone do not justify ART indexes.

### 5. Measure ordered writes before choosing partitions

**Observation:** the reload has one file for each main derived table; raw input has
three. After this batch, standardization, blocking and scores have two files each.
Membership has three data files and one delete file; golden records and lineage
each have two data files and one delete file. The current catalog has no partition
or sorted-table declarations. See the [base](../artifacts/bench/workload-1m-100k-20260923/control/run-001/base-lake-layout.json)
and [batch](../artifacts/bench/workload-1m-100k-20260923/control/run-001/batch-lake-layout.json)
inventories and Parquet metadata. This first delivery does not demonstrate a
small-file explosion or a need for compaction.

**Experiment:** extract actual selective predicates from native plans first. For a
target that uses equality on blocking keys, `(key_type, key_value, record_key)` is
an ordering hypothesis; it helps only consumers that read that table and prune those
ranges. Splink's full scoring joins standardized columns, so a blocking-table layout
change does not automatically accelerate that join. Randomly distributed affected
record keys can still touch most row groups.

Compare four fresh-table layouts: current writes; ordered writes; useful low-cardinality
partitioning; and partitioning with ordering inside each partition. Inspect resulting
min/max statistics, row groups, files opened and bytes read. Include sort, write,
delete-file, rewrite and compaction costs for reload and incremental processing.
Then repeat several deliveries to assess layout decay. Reject a layout that improves
isolated scans while making either complete workload slower.

`ORDER BY` can improve locality and min/max pruning without matching an index.
DuckLake does not support ordinary DuckDB ART indexes. Sorted-table and partition
features must be verified against the installed extension; a change to new writes
also needs an explicit existing-file strategy. Confidence in a layout benefit here
is low until selectivity and pruning are measured.
See [DuckDB ordered data](https://duckdb.org/docs/current/guides/performance/indexing),
[DuckLake limitations](https://ducklake.select/docs/stable/duckdb/unsupported_features),
[partitioning](https://ducklake.select/docs/stable/duckdb/advanced_features/partitioning)
and [sorted tables](https://ducklake.select/docs/stable/duckdb/advanced_features/sorted_tables).

### 6. Keep training fan-out visible as a reload-specific experiment

**Observation:** training is 10.55s, about 11.3% of this reload. The surname/postcode
[comparison-vector query](../artifacts/bench/workload-1m-100k-20260923/profiled/run-001/sql/c7dc3ba26b85492bb1786b0506335c84-80c26744ea1c47768d3fbbc1502743f4.json)
produces 5,491,664 comparisons and takes 2.64s. The rule's 19 SQL executions total
4.74s. Prediction's `name_postal` block contributes 5,939,078 raw pair occurrences;
its largest bucket has 58 rows. These are different candidate populations.

**Experiment:** use the existing [training experiments](performance-experiments.md)
to compare pair targets/rules under matched inputs. Capping training or replacing a
rule changes learned parameters and requires quality evaluation, including hard and
held-out profiles. It is not a transparent storage optimization. The incremental
path reuses the model, so this gives it no direct training-time saving. Continue
measuring its resulting scores/clusters and quality. Do not extrapolate this 1M
training share to the known 10M fan-out problem.

Cleaning and assembly also have noticeable command time beyond their native SQL.
A subsequent dbt startup/compilation experiment should use command/model spans and
subprocess evidence. Any cache must preserve tenant configuration, run variables and
compiled SQL; the present timings cannot label that entire gap as Python computation.

## Reproduce and extend

Run the current implementation with `make profile-workloads`; use
`BENCHMARK_REPEAT=3 make profile-workloads` for repeated measurements. The recorded
campaign used this command with a previously built immutable image:

```sh
uv run python benchmarks/full_pipeline.py --scale 1m --local \
  --with-incremental --incremental-records 100000 --incremental-scenario mixed-v1 \
  --profile --with-profile-control --keep-failed \
  --image sha256:d3d6927490cc8639cc38a9b9dd8e39bacd1743b5969b883431550bd2e3211a99 \
  --image-source b9da830995ebfe7ce1c45c9d61851a0feac78f1d7f21ee885ed576433ddea9c6 \
  --out artifacts/bench/workload-1m-100k-20260923
```

Use a new output directory for another execution. Omit the image arguments to build
current source. Regenerate existing reports offline with:

```sh
uv run python benchmarks/workload_report.py artifacts/bench/workload-1m-100k-20260923
```

The repository [analysis skill](../.agents/skills/ducklake-performance/SKILL.md)
encodes these evidence and tuning rules. This first campaign covers one append to a
fresh lake; repeated deliveries, updates/deletions, remote object-store latency and
hard/tenant datasets remain separate experiments. It supplies a reproducible starting
point for both workloads without treating local diagnostic timings as production gains.
