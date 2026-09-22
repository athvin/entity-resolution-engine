# Splink 5 pipeline performance

This change applies the [Splink performance article](https://gist.github.com/RobinL/c6d56a27d8f83c40b6b09643c0fa5d14) to matching and training, then removes separate work in standardization and golden-record assembly. The tested dependency is exactly `splink==5.0.0.dev5`.

## Million-record results

Three fresh initial loads per version give a median of **90.84 seconds** for the
optimized pipeline versus **157.04 seconds** for the baseline: **42.2% less time**,
**1.73× throughput**, and about **11,009 input records per second**.

| Stage | Baseline median, seconds | Optimized median, seconds |
|---|---:|---:|
| Ingestion | 10.55 | 10.60 |
| Standardization | 82.67 | 21.33 |
| Training | 11.52 | 10.59 |
| Matching | 12.75 | 12.34 |
| Reconciliation | 23.13 | 22.95 |
| Golden assembly | 16.40 | 13.20 |
| **Complete initial load** | **157.04** | **90.84** |

Stage medians need not sum to the median total. Baseline passes ranged from
154.78–159.03 seconds (CV 1.36%); optimized passes ranged from 90.16–92.52 seconds
(CV 1.33%). Median pipeline CPU fell from 255.01 to 124.63 seconds. Sampled peak
container memory ranged from 4.00–4.16 GiB for the baseline and 3.29–3.54 GiB for
the candidate. Neither version showed sampled DuckDB spill at this scale.

Standardization accounts for most of the improvement at one million records.
All six initial loads have identical input hashes, standardized/blocking counts,
entity memberships, golden/lineage counts and measured quality. They produce
395,867 golden records and 2,375,202 lineage rows from 1,000,000 source records.

The candidate's separate 10,000-record delivery takes a median of **30.79 seconds**
(range 30.64–31.44 seconds), including ingestion, standardization, incremental
matching, reconciliation and assembly. Its output quality and memberships repeat
exactly across all three trials.

The separate baseline incremental run was still matching after its 300-second
`run-all` budget and was stopped at 300.95 seconds. Its timing is **over 300 seconds**,
not a completed duration; the optimized delivery is at least 9.7 times faster on
this workload. The baseline budget excludes the preceding batch ingests, while the
candidate's 30.79 seconds includes them.

A frozen-model Splink 4 full rescore after the same delivery confirms identical
entity memberships, pair counts, precision and recall. The old full-assembly path
retains 216 stale golden records for merge losers, so its raw golden/lineage counts
are not an equivalence check. Incremental assembly correctly reaps these entities.
The implementation also fixes full assembly to reap merged and retired entities,
including leftovers from older runs. That cleanup fix followed the timed image;
its separate validation is recorded with the measurements.

## Ten-million-record capacity

The optimized pipeline completes a fresh ten-million-record initial load in
**6,458.21 seconds (1h 47m 38s)** versus **7,159.82 seconds (1h 59m 20s)** for the
baseline: **9.8% less time**, or about **11m 42s saved**. Both use the same two-CPU,
10 GiB container and 4 GB DuckDB limits. Each version has one pass, so variation
is unmeasured.

| Stage | Baseline seconds | Optimized seconds |
|---|---:|---:|
| Ingestion | 50.73 | 50.04 |
| Standardization | 753.61 | 117.40 |
| Training | 5,055.44 | 5,155.33 |
| Matching | 746.13 | 737.21 |
| Reconciliation | 241.45 | 241.81 |
| Golden assembly | 312.46 | 156.41 |
| **Complete initial load** | **7,159.82** | **6,458.21** |

Standardization takes 84.4% less time and golden assembly takes 49.9% less time.
Full matching remains similar, and training takes 2.0% longer in this single pass.
The library migration alone does not show a substantial full-matching or training
speedup on this workload. Training retains term-frequency adjustments and uncapped
EM; it accounts for approximately 80% of the optimized processing time.

Both runs have identical input hashes, quality metrics, entity memberships and
output counts: 10,000,000 current memberships, 3,898,183 golden records and
23,389,098 lineage decisions. Pipeline CPU is 8,143.45 seconds for the baseline and
6,864.19 seconds for the candidate. Sampled peak container memory is 9.59 versus
9.51 GiB; sampled DuckDB spill is 140.77 versus 138.78 GiB. These are observed peaks,
not minimum resource requirements or total disk writes.

## Prediction execution settings

One fixed million-record model and corpus were rescored under six configurations.
All persisted pairs, probabilities and evidence matched exactly; no Parquet scratch
files remained afterward. Each setting has one probe, so variation is unmeasured. These
rescores update existing scores and are excluded from the initial-load medians above.

| Storage | Chunks, left × right | Seconds | CPU seconds | Sampled peak GiB | Sampled Parquet MiB |
|---|---:|---:|---:|---:|---:|
| table | 1 × 1 | 14.33 | 24.57 | 4.30 | 0.0 |
| table | 2 × 2 | 17.71 | 30.56 | 4.46 | 0.0 |
| table | 4 × 4 | 29.23 | 49.28 | 4.24 | 0.0 |
| parquet | 1 × 1 | 14.52 | 24.31 | 4.01 | 123.5 |
| parquet | 2 × 2 | 19.86 | 32.64 | 4.12 | 167.3 |
| parquet | 4 × 4 | 27.69 | 44.32 | 4.10 | 143.0 |

No DuckDB spill was sampled in these probes. Keep the default **table storage,
one chunk per side**: chunking added work at this scale, and the small difference
between single-chunk table and Parquet timings does not justify switching storage.
Parquet remains available as an execution option. Memory samples include page cache
from earlier probes; they are not measurements of fresh-container memory requirements.

## Changes

- Standardization uses scalar sentinel checks instead of repeated MARK joins. Incremental runs journal pending ingestion batches, resolve affected record keys against their full history, and replace their blocking keys. Interrupted staging or derived-table writes remain retryable, including deletions and records whose new key set is empty.
- Matching uses registered SQL inputs and Splink 5's full prediction and two incremental APIs. Frozen TF lookup tables remain explicit. Unscored-record discovery expands score endpoints before equality joins, avoiding the previous quadratic OR join. Prediction can use table or local Parquet storage and configurable full-prediction chunks; scratch is removed on success and handled failure.
- Training uses Splink 5's estimation engine while retaining the previous ordered, seeded Bernoulli sample for u. Deterministic-prior estimation remains exact, EM is uncapped, and u early stopping is disabled by default.
- Golden lineage selects each winner once. Golden records read the same winners, preserving null results and whole-address selection. Incremental display assembly updates touched entities.
- Full assembly schedules merged and retired entities for cleanup, repairing stale golden, lineage and display rows after full corrections or interrupted runs.
- Review processing accepts log2 `mw_*` evidence and historical `bf_*` evidence in SQL. Models record the exact Splink version; legacy models require retraining and a completed full resolution before incremental scoring.

## Quality gates

The fixed-model oracle captures 276 pairs under Splink 4.0.16, including nulls and TF-adjusted terms. Under all six table/Parquet and 1×1, 2×2, 4×4 prediction combinations, probabilities agree within `1e-10`, comparison levels and merge/review decisions agree, and scratch is removed. Full and incremental scoring retain their exact parity requirement.

Retraining was evaluated on three independent 100,000-record synthetic datasets.
The selected defaults give **identical quality metrics, entity partitions and
output counts** to Splink 4 on all three, and on the million- and ten-million-record
initial loads. The following metrics are identical between versions at each size
and seed.

The 100,000-record trials use [configs/test.yaml](../configs/test.yaml), including
its configured email placeholders. Trials at one and ten million records use
[configs/default.yaml](../configs/default.yaml). Inputs, cleaning policy and matching
thresholds are held constant within each baseline/candidate comparison.

Edge recall is measured among true pairs that survived blocking; blocking recall
is recorded separately in the measurement data. Cluster recall uses all true pairs.

| Records | Seed | Edge precision | Edge recall | Cluster precision | Cluster recall |
|---|---|---:|---:|---:|---:|
| 100,000 | 42 | 0.990557983 | 0.990458494 | 0.963882481 | 0.992437500 |
| 100,000 | 1729 | 0.991087478 | 0.990153970 | 0.964319568 | 0.991875000 |
| 100,000 | 2026 | 0.991555243 | 0.990497232 | 0.967007797 | 0.992150000 |
| 1,000,000 | 42 | 0.973504776 | 0.982521539 | 0.903394250 | 0.987000000 |
| 10,000,000 | 42 | 0.944081737 | 0.971038826 | 0.827405588 | 0.979096750 |

Native Splink 5 hash sampling and u early stopping at 100 observations per comparison level caused small measured precision or recall regressions. Neither became a default. Capping EM at 1,000,000 pairs also missed two additional true cluster pairs at the million-record scale, so no sampled-training preset is shipped. The regenerated fixture retains the old u values and prior exactly, with m differences below `1e-12`.

A separate execution-plan check evaluates the compiled CRM standardization SELECT over 33,334 records from the 100,000-record corpus. It removes **49 MARK joins**, and both directions of `EXCEPT ALL` confirm identical standardized rows. This is a query-plan check, separate from end-to-end timing. The current dbt profiler does not correlate the second statement in a multi-statement materialization, so this SELECT was captured directly rather than inferred from incomplete dbt operator profiles.

## Measurement method

The baseline is commit `fc5070e` with Splink 4.0.16, built before edits. Each trial starts a fresh PostgreSQL/MinIO/DuckLake stack and uses the same immutable CSV inputs. Docker on Apple Silicon provides an eight-CPU host; each measured pipeline is limited to two CPUs, 10 GiB container memory, two DuckDB threads and a 4 GB DuckDB memory limit.

Initial-load time includes command startup, ingestion, standardization, training, full matching, reconciliation and golden assembly. A separate delivery of 10,000 records measures incremental performance on the million-record corpus. Setup, generation, validation, profiling, matrix probes and teardown are outside processing times. The repeated trials run sequentially without integration tests or other benchmark workloads.

Three baseline/candidate million-record pairs alternate execution order. The three baseline passes measure initial loads; each candidate also receives a separately timed 10,000-record delivery. A separate baseline incremental trial has a five-minute limit on `run-all`, because batch discovery uses a quadratic nested-loop join. A timeout is retained as a failed trial and reported only as a lower bound. Initial and incremental quality are measured separately. Ten-million-record measurements are single fresh passes; their variation is unmeasured. The six prediction modes use one fixed million-record model and corpus and require exact equality of persisted pairs, probabilities and evidence.

Detailed SQL profiles are separate diagnostic runs. Memory and spill figures are 250 ms samples; short peaks can fall between samples. Container memory includes cache and child processes, but excludes PostgreSQL and MinIO. A sampled spill peak is a lower bound, not total bytes written.

## Validation and provenance

`make check` passes with **889 unit tests**, specification and fixture validation,
linting, formatting, strict type checks, workflow validation and dbt parsing.
All **347 collected integration cases** are covered by passing isolated sessions
and targeted reruns, including fixture regeneration, scoring parity, model migration,
standardization recovery and golden assembly. The final cleanup change reruns all
seven touched-assembly cases, including both full and incremental merge/deletion
cleanup.

The timed candidate application is commit `bb749ca`; the baseline is `fc5070e`.
The frozen-model full-rescore benchmark reference uses harness commit `37b6a1e`.
The later full-assembly cleanup repair is application commit `28b1e84`.
The [machine-readable measurements](measurements/splink5-20260922.json) retain
individual timings, resource fingerprints, image digests, input hashes, quality
counts, rejected variants and validation coverage. Raw command logs and samples
are retained locally under `artifacts/performance/pipeline-splink5/`.

The final image completes a separate million-record initial load in 91.33 seconds
and a full rescore after the 10,000-record delivery in 49.77 seconds, including
delivery ingestion. Its input hashes, base and post-delivery counts, memberships
and complete quality metrics match the optimized incremental run exactly.
Post-delivery output has 395,680 golden records and 2,374,080 lineage rows, with
the stale merge losers removed. This single correctness run is excluded from the
timing medians above.

## Operation

Use the [migration runbook](runbook.md#migrating-an-existing-lake-to-splink-5) for existing lakes. The pin is a prerelease and the evidence format changes from Bayes factors to log2 weights. The runbook covers the required full resolution and rollback to a compatible image/model/TF snapshot.

Run an initial load and incremental delivery:

```sh
uv run python benchmarks/full_pipeline.py --scale 1m --local --with-incremental
```

Compare existing images using separate output directories and shared inputs:

```sh
uv run python benchmarks/full_pipeline.py --scale 1m --local \
  --image er-perf:baseline --image-source BASELINE_COMMIT \
  --corpus-root artifacts/bench/shared-inputs --out artifacts/bench/baseline-1
uv run python benchmarks/full_pipeline.py --scale 1m --local \
  --with-incremental --image er-perf:candidate --image-source CANDIDATE_COMMIT \
  --corpus-root artifacts/bench/shared-inputs --out artifacts/bench/candidate-1
```

Repeat three times with new output directories, reversing image order on the second
pair. These commands measure baseline initial loads and candidate initial loads plus
incremental deliveries. Measure the slow baseline incremental path separately with
an explicit time budget; a stopped run provides only a lower bound.

Supplied images are retained. Use `--prediction-matrix` on a separate run for storage/chunk comparisons, `--profile` for separate SQL diagnostics, and `--scale 10m` for an initial load at ten million records. `--config` tests an explicit training variant. Combine `--with-incremental` with
`--batch-mode full` for a full-rescore quality reference after the delivery, retaining
the initial model and frozen TF snapshot. Its time is labeled separately from
incremental processing. Keep original manifests, command logs, input hashes, fingerprints and resource samples when comparing results.
