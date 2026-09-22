# Performance

## Million-record full pipeline benchmark

Run one initial load of **1,000,000 synthetic source records representing 400,000
people**, spread across CRM, billing and webforms (seed 42):

```sh
make benchmark-1m
```

The command builds the current checkout, starts a fresh disposable lake, and times
ingestion → cleaning/standardization → model training → matching → reconciliation
→ golden record assembly. Training is included in the first-load total. Generation,
image/stack setup, validation and teardown are outside that processing time. There
is no incremental delivery, and detailed SQL profiling is disabled. The existing
CI benchmark still measures its separate six-phase workload including incrementals.

`make benchmark-1m` uses **local mode**, which fits the resource limits to the current
Docker host without reducing the million-record input. It leaves two CPUs and at
least 1 GiB of Docker memory outside the pipeline limit, assigns at most two thirds
of container memory (capped at 4 GB) to DuckDB, and limits workers to one per 2 GiB
of DuckDB memory. Additional memory remains available for reconciliation's Python
heap. A 12-GiB Docker allocation selects **2 CPUs, 10 GiB container memory,
2 DuckDB threads and a 4 GB DuckDB limit**. Local mode checks for 2 GiB free disk at
startup; that is a startup guard, not a prediction of the run's disk consumption.

Without `--local`, the runner uses the standard `benchmarks/scales.yaml` envelope:
12 CPUs, 56 GiB container memory, a 40 GB DuckDB limit and 120 GiB free disk. These
are comparison settings, not minimum requirements for processing a million records.
The chosen resource profile is recorded in the manifest, verified against actual
container limits, and printed in the report. Compare timings only at the same limits.

Check the host, validate the runner with 1,000 records, or measure three fresh
million-record passes for a median and variation:

```sh
uv run python benchmarks/full_pipeline.py --scale 1m --local --check-only
uv run python benchmarks/full_pipeline.py --scale smoke --local
uv run python benchmarks/full_pipeline.py --scale 1m --local --repeat 3
```

Run from a checkout on the Docker host (Docker Desktop bind mounts also work).
Keep the host otherwise idle. Each pass owns a unique Compose project; its
containers, volumes and temporary image tag are removed afterward, including on
failure. Repeats reuse the same generated inputs, with a fresh stack for each pass.
Existing development stacks are not touched.

Use `--keep-failed` during resource tuning to preserve a failed lake for diagnosis
or recovery. Its Compose project and image are recorded as `retained_project` and
`retained_image` in `manifest.json`. After finishing that investigation, remove the
retained stack and image using those exact recorded names:

```sh
docker compose -p "<retained_project>" -f docker/compose.yaml \
  -f "<output-directory>/compose.json" --profile bench down -v --remove-orphans
docker image rm "<retained_image>"
```

Results go into a new `artifacts/bench/full-<scale>-<timestamp>/` directory. Use
`--out <new-directory>` to choose a different location; existing directories are
never overwritten. Keep this directory until the measurement has been reviewed:

- `report.md`: full-pipeline duration, input throughput, time per stage, bottleneck,
  CPU time, sampled pipeline memory, golden record count and lineage count.
- `results.json`: measured passes, output validation, match quality, input hashes,
  runtime fingerprints and aggregate statistics. One pass reports variation as
  unmeasured; multiple passes also compare counts and membership partitions.
- `run-*/`: command logs, raw timings, resource samples and generated run configuration.
  `manifest.json`, `working-tree.patch`, build and service logs identify the source,
  dependencies and execution environment. Large generated inputs remain under `inputs/`.

Validation requires exactly one membership for every standardized input, one
golden row per entity, and one lineage decision per golden attribute whose winning
record belongs to that entity. Synthetic truth also produces blocking, edge and
cluster quality metrics, outside the processing timer. CPU and memory cover the
pipeline container and its descendants; the database and object store are separate
services. Memory includes page cache and is sampled every 250 ms.

This is a capacity measurement, separate from CI regression baselines. Running it
does not enable scheduled million-record jobs or promote a baseline.

### Ten-million-record capacity test

`make benchmark-10m` runs the same complete initial-load pipeline with **10,000,000
records representing 4,000,000 people**, using seed 42 and the same three sources.
It fits the CPU and memory limits to the local Docker host. The experimental `10m`
scale is available only in the full-pipeline runner; it adds no scheduled CI job.

The generator streams source CSV rows, and delivery directories share immutable
input files through hard links where supported. File hashing and membership
partition hashing stream their inputs. Quality validation counts pairs in DuckDB
and uses the shared metric formulas, so it does not retain all candidate pairs in
Python. Generation still retains personas and truth labels in memory. Lake data,
generated inputs and DuckDB spill files also need disk space beyond the startup
check; a 4 GB DuckDB limit is not a limit on the whole container or on disk use.

The ten-million-record pipeline completed on **2026-09-21**, producing
**3,898,183 golden records**, **10,000,000 memberships**, and **23,389,098 lineage
rows**. Reconciliation took **336.30 seconds (5m 36s)** and golden-record assembly
**310.64 seconds (5m 11s)**. Every standardized record has exactly one membership;
every entity has one golden record and six lineage decisions whose winning records
belong to that entity.

The recorded processing total is **7,843.46 seconds (2h 10m 43s)**. This is a
**resumed pipeline with code changes**, combining the preserved ingestion, cleaning
and training with repaired matching, review recovery, reconciliation and assembly.
It is not a fresh full-pipeline pass on one image. Matching includes its interrupted
836.38-second pass, which saved all scores, and 62.02 seconds to finish reviews after
a local object-store connection error. Earlier failed matching and reconciliation
attempts, generation, setup, debugging, validation and downtime are excluded.

| Stage | Seconds |
|---|---:|
| Ingestion | 185.12 |
| Cleaning/standardization | 706.10 |
| Model training | 5,406.89 |
| Matching and review recovery | 898.40 |
| Reconciliation | 336.30 |
| Golden-record assembly | 310.64 |

The run used **2 CPU cores, 2 DuckDB threads, a 4 GB DuckDB limit and a 10 GiB
pipeline-container limit**, with roughly 12 GiB allocated to Docker Desktop on the
local 24-GiB Mac. Reconciliation's sampled peak process RSS was **4.72 GiB** and its
peak container usage **5.17 GiB**. Whole-pipeline container memory reached **10 GiB**,
including reclaimable page cache; this excludes the catalog and object-store
services. These are tested settings, not measured minimum requirements.

Training remains the largest cost: **90m 7s, about 69% of processing time**. Its
surname/postcode training rule creates **491,624,467 pairs**. Observed training spill
reached at least **135.40 GiB**, in addition to persistent inputs and lake data; spill
was sampled occasionally, so this is a lower bound, not total disk required.

Accuracy was measured against the synthetic truth (4 million personas, seed 42):

| Metric | Result |
|---|---:|
| Blocking recall | 99.56% |
| Edge precision | 94.41% |
| Edge recall within blocked pairs | 97.10% |
| Edge F1 | 95.74% |
| Cluster pairwise precision | 82.74% |
| Cluster pairwise recall | 97.91% |
| Cluster pairwise F1 | 89.69% |

**Completion and integrity do not establish acceptable match accuracy.** Cluster
closure contains **1,633,894 false-positive pairs**; false merges need further work.
No matching threshold, training rule or model parameter was changed during recovery.
The preserved model still carries its original warnings about unobserved exact email
and phone u probabilities. This test does not establish that those warnings caused
the measured errors.

There are **567,184,685 distinct candidate pairs**, **8,451,869 saved scores**,
**8,192,404 automatic-match edges** and **259,465 gray-band reviews**. The review
coverage audit found no missing entries. All **3,898,183 created events** have unique
IDs and dense sequence values; all event hashes are correct, and replay reproduces
all ten million memberships exactly. Partition SHA-256:
`0e2fb7d2569f53be68f2d7635ecf33b67fa23070351af5e4f951eb78d95bf9c6`.

The score writer and initial reconciliation now stage large intermediates in
DuckDB and consume bounded batches. Existing lifecycle rules, ID ordering and event
semantics are preserved. The bounded reconciliation path applies only to complete
initial loads without prior lifecycle/assertion state; incremental reconciliation
has not been measured at this scale. Benchmark validation also uses partitioned
counts and restricted pair joins to stay within the DuckDB limit.

The complete report and exact results are in
`artifacts/bench/full-10m-local-retry-20260921/completion-report.md` and
`completion-analysis.json`. Earlier failures remain archived separately in that
folder. Checks included 780 unit tests, targeted validation tests, matching and
reconciliation integration tests, and a one-million-record reconciliation check
under a 512 MB DuckDB limit. One resumed capacity run was measured; variation is
unmeasured. The benchmark services are stopped and the completed lake is retained.

### Optimized local million-record result, 2026-09-21

The optimized full pipeline processed **1,000,000 records in 207.79 seconds
(3 minutes 28 seconds)**, versus 1,532.33 seconds before: **7.37 times faster**
overall, with **86.4% less processing time**. Both passes used the same generated
inputs, dependency versions, 2 CPUs, 10 GiB container limit, 2 DuckDB threads and
4 GB DuckDB limit on the local Mac described below.

| CLI stage | Before, seconds | After, seconds |
|---|---:|---:|
| Ingestion, all sources | 21.47 | 20.47 |
| Cleaning/standardization | 81.98 | 84.15 |
| Model training | 11.06 | 12.05 |
| Matching | 1,192.38 | 19.25 |
| Reconciliation | 208.14 | 55.59 |
| Golden record assembly | 17.30 | 16.28 |
| **Total processing** | **1,532.33** | **207.79** |

Matching was **61.95 times faster**. The same 36,928 review records now use
37 insert batches, and the observed matching snapshot delta fell from 36,930 to
39. Reconciliation took **73.3% less time**. Cleaning/standardization is now the
largest stage, at 40.5% of processing time. Throughput rose from 653 to 4,813
input records/second.

Sampled pipeline memory fell **46.9%, from 8.51 to 4.52 GiB**. Within reconciliation,
the sampled peak fell from 8.51 to 2.37 GiB. Processing CPU time fell from
1,507.42 to 308.77 CPU-seconds. These memory figures include the pipeline's page
cache and child processes, exclude the catalog and object store, and use 250 ms
samples; they are not a hard cap on all Python and native allocations.

All 1,000,000 inputs retained memberships. Counts, the membership-partition hash,
input hashes and synthetic quality metrics matched the baseline exactly, including
**395,867 golden records** and **2,375,202 lineage rows**. Integrity validation
passed. A separate 10k initial-plus-incremental comparison also matched normalized
outputs, with a maximum score difference of `6.66e-16`. The million-record baseline
did not export every golden value and score, so those were not compared row by row
at that scale. All 742 unit/contract tests and 26 selected database integration
tests passed; a separate profiling-enabled smoke run also passed.

These are single exploratory passes; variation is unmeasured. Checks overlapped
baseline generation and early ingestion, and temporary image/cache cleanup
overlapped optimized generation and early processing. No tests ran during optimized
processing. This comparison demonstrates the observed improvement on this corpus,
not a controlled multi-run regression baseline.

The optimized evidence is retained under
`artifacts/bench/full-1m-optimized-20260921/`; the source manifest identifies the
uncommitted changes on `4ce5a5c`. Its full campaign took 5 minutes 27 seconds,
including setup, generation, validation and teardown. Stage memory, environment
checks and the before/after comparison are saved in
`artifacts/performance/matching-20260921/million-comparison.json`.

A separate capacity check also completed all 1,000,000 records with a **6 GiB
pipeline container limit**, keeping 2 CPUs and the 4 GB DuckDB limit. Processing
took **198.50 seconds**, with a sampled peak of **4.81 GiB**; counts, membership
partitions and quality metrics matched the 10 GiB pass. This confirms the workload
can now finish under the container limit that previously failed in reconciliation.
Docker Desktop remained allocated 12 GiB during this check. It reused the generated
inputs and an audited image on a fresh lake; it is a capacity check, not a repeated
measurement of the 10 GiB configuration. Evidence is in
`artifacts/bench/full-1m-optimized-6g-retry-20260921/` and
`artifacts/performance/matching-20260921/capacity-comparison.json`.
An earlier 6 GiB attempt failed during ingestion with an object-store HTTP 400
`IncompleteBody` error. Its logs are retained separately under
`artifacts/bench/full-1m-optimized-6g-20260921/`; that attempt does not establish a
memory limit. All measurement stacks were removed after their logs were saved.

### Local million-record baseline, 2026-09-21

One complete pass on the local Apple Silicon Mac (8 logical CPUs, 24 GiB RAM)
processed all **1,000,000 records in 1,532.33 seconds (25 minutes 32 seconds)**,
about **653 input records/second**. Docker Desktop was allocated 12 GiB; the
pipeline used 2 CPUs, a 10 GiB container limit, 2 DuckDB threads and a 4 GB DuckDB
limit. The engine source was `4ce5a5c`, with the uncommitted benchmark runner
identified by the source hashes in the run manifest.

| CLI stage | Seconds | Share of processing |
|---|---:|---:|
| Ingestion, all sources | 21.47 | 1.4% |
| Cleaning/standardization | 81.98 | 5.3% |
| Model training | 11.06 | 0.7% |
| Matching | 1,192.38 | 77.8% |
| Reconciliation | 208.14 | 13.6% |
| Golden record assembly | 17.30 | 1.1% |

The run produced **395,867 golden records** and **2,375,202 lineage rows**. All
1,000,000 inputs were present in standardization and entity membership, and the
golden-record and lineage integrity checks passed. Synthetic pairwise cluster
precision was 90.34% and recall was 98.70%; successful integrity checks do not
imply perfect entity resolution.

Sampled pipeline memory peaked at **8.51 GiB**, including page cache, and processing
used **1,507.42 CPU-seconds**. An earlier attempt with a 6 GiB pipeline limit was
killed during reconciliation after matching completed. The larger container limit
allowed the same workload to finish without changing production engine code.

Matching was the largest stage. It added **36,928 review records**, and
live database observations showed repeated individual review lookups and writes.
The baseline review queue implementation handled these one subject at a time.
Together with the logged blocking and prediction times (1.33 s and 2.93 s), this
pointed to review-queue persistence as the dominant matching cost. Detailed SQL
profiling was disabled, so its exact share was not separately timed. The current
implementation batches these operations as described below.

This was one exploratory local run; runtime variation is unmeasured. Local checks
also ran during generation and early ingestion, so it is not an idle-host
regression baseline. The headline time excludes setup, generation, validation and
teardown. The complete campaign, including those activities, took 26 minutes
57 seconds. Raw evidence is retained locally under
`artifacts/bench/full-1m-local-10g-20260921/`, including `report.md`, `results.json`,
the source manifest and command/resource logs. The disposable stack and image tag
were removed after success.

## Historical controlled comparisons

These are historical controlled comparisons, not CI baselines or service-level
commitments. Both campaigns used five alternating baseline/candidate pairs,
identical synthetic inputs and pinned dependencies, fresh lakes, two CPUs, 6 GiB
container memory, two DuckDB threads and a 4 GB DuckDB memory limit. Detailed SQL
profiling was disabled for the headline measurements.

| Workload | Before | After | Reduction |
|---|---:|---:|---:|
| 10k initial load + 100 incremental records, 2026-09-20 | 90.46 s | 65.46 s | 27.6% |
| 100k initial load, 2026-09-21 | 273.06 s | 84.80 s | 68.9% |
| Reconciliation within that 100k load | 205.37 s | 17.79 s | 91.3% |

The 100k comparison used 100,000 records representing 40,000 generated people
(seed 42). The baseline was commit `679dbf9`; the optimized code merged as
`e13535a`. Median CPU time fell from 281.50 to 92.80 CPU-seconds. Median sampled
memory peak changed from 3,612.9 to 3,671.1 MiB; the largest sample was 3.68 GiB.
Runtime variation was 0.82% before and 1.77% after.

| 100k CLI stage | Baseline median | Optimized median |
|---|---:|---:|
| Ingestion, all sources | 7.76 s | 7.76 s |
| Standardization | 19.01 s | 18.94 s |
| Training | 4.47 s | 4.42 s |
| Matching | 26.38 s | 26.43 s |
| Reconciliation | 205.37 s | 17.79 s |
| Golden assembly | 8.29 s | 8.22 s |

Component medians need not sum to the total median. Matching became the largest
stage. A separate diagnostic attributed 12.07 seconds to reconciliation persistence,
including 3.91 seconds of event writes; the five targeted lookup helpers totaled
0.714 seconds. Nested spans overlap, and the diagnostic is excluded from the medians.

All 16 normalized output comparisons in the 100k campaign passed. Each large run
produced 100,000 memberships, 39,995 golden records and 239,970 lineage rows.
The maximum probability difference was `1.89e-14`, below the `1e-10` tolerance.
The 100k workload included only the initial load; small fixture/smoke runs separately
checked incremental behavior. It used a smaller CPU/memory envelope than the
standard `100k` scale in `benchmarks/scales.yaml`.

Processing times include complete CLI lifetimes, normal stage logging and 250 ms
resource sampling. They exclude stack setup, corpus generation, validation, export
and teardown. Memory includes page cache. These results do not establish capacity
for arbitrary source distributions or concurrent workloads.

The raw local campaigns, exports and temporary 100k measurement harness were
removed during repository cleanup. This summary retains the measured results and
methodology; the deleted raw runs cannot be regenerated into reports. The supported
comparison runner below measures 10k plus incremental processing, so it does not
recreate the historical 100k initial-only experiment exactly.

## Implementation

- Ingestion, graph nodes/edges and incremental keys use bound, explicitly typed
  arrays with `INSERT ... SELECT UNNEST(...)`. Loads are bounded at 1,024 rows;
  ingestion order, transaction boundaries and all business rules are preserved.
- Full and incremental matching consume the persisted scores in batches of 1,024
  Python rows, with running counters and evidence JSON decoded only for review
  candidates. The native result is completed before interleaving review writes,
  preventing a later statement from discarding unread result chunks. It stays on
  the caller's connection so uncommitted scores remain visible.
- Review candidates accumulate into batches of up to 1,024. Each batch uses one
  joined lookup plus bulk inserts and refreshes, with null-safe subject keys.
  Repeated subjects preserve the first payload, and settled reviews remain unchanged.
- Entity transitions, memberships and events load temporary DuckDB tables in
  bounded column batches. Each destination still receives one final merge or insert;
  the temporary tables are removed afterward. This avoids constructing SQL and
  Python parameter lists with millions of scalar values, and preserves the caller's
  transaction boundaries.
- Reconciliation membership, corpus and tombstone lookups bind each requested key
  set as one `VARCHAR[]` parameter using `IN (SELECT unnest(?::VARCHAR[]))`.
  Membership lookups still expand to every member of each touched entity, and
  tombstone deletion remains limited to the requested records absent from the corpus.
- A `LakeSession` reuses the native connection only within one locked CLI invocation.
  Failed leases discard the connection, and each stage clears its in-memory Splink
  schema. The connection closes before a dbt subprocess starts and reopens on demand.
- Golden assembly uses separate preparation and finalization connection scopes
  around dbt. No native connection lease spans the subprocess.
- Schema preflight reads live column metadata once. Heavy matching modules load only
  when needed; the blocking configuration used by dbt has no Splink dependency.
- The first standardization builds the nickname seed and dependent models in one
  dbt invocation. Later standardizations retain their existing model selection.

Detailed traces additionally identify CLI import/execution, preflight, connection
setup/teardown, ledger operations and dbt launch/invocation. Use the same tracing
settings on both versions for timed runs. Compare detailed span timings only when
both versions carry the same instrumentation.

## Reproduce a comparison

Freeze the baseline and candidate in separate source directories, including any
uncommitted changes. Add only the common diagnostic spans to the baseline. Record
both original and measured source snapshots; a Git SHA alone cannot identify a
dirty working tree.

Build both versions using the same existing dependency image:

```sh
uv run python benchmarks/build_performance_image.py \
  --context /path/to/baseline --base-image er-pipeline:ci \
  --tag er-perf:baseline --out artifacts/performance/build-baseline
uv run python benchmarks/build_performance_image.py \
  --context . --base-image er-pipeline:ci \
  --tag er-perf:candidate --out artifacts/performance/build-candidate
uv run python benchmarks/performance.py \
  --out artifacts/performance/comparison \
  --baseline-image er-perf:baseline --candidate-image er-perf:candidate --repeat 5
```

The image builder verifies installed dependency pins, hashes the source overlay and
records immutable image IDs. The campaign pins those IDs, saves a hashed copy of
the measurement harness, and mounts that same copy for both versions. Each trial starts its own Compose stack,
catalog and object store, with two CPUs, 6 GiB container memory, two DuckDB threads
and a 4 GB DuckDB limit. Teardown removes only that trial's stack and volumes.

The sequence is:

1. Detailed baseline and candidate runs on the 10-person fixture (23 source records
   and six incremental records), checking its committed expected output.
2. Detailed baseline and candidate smoke runs (1,000 records and 50 incremental).
3. At least five alternating baseline/candidate pairs with SQL profiling disabled,
   using 10,000 source records representing 4,000 people, then 100 incremental records.
   The sequence extends to at most ten pairs if either runtime CV exceeds 5%.
4. One detailed 10k run per version, retaining all SQL and nested transformation logs.

Generated inputs are reused across variants and repeats, with their hashes checked.
Output verification compares standardized values, blocking keys and candidate
pairs, threshold classifications, membership partitions, golden records and lineage.
Generated IDs and timestamps are normalized; probabilities allow only absolute
roundoff up to `1e-10`. Dependency and resource fingerprints must also match.

Keep the host otherwise idle during timing. The runner requires at least 4 GiB
free disk for small gates and 8 GiB for large trials. Source snapshots and logs
remain under the output directory. Use `--resume` only with the identical images,
harness and settings; successful trials are retained. Preserve a failed trial
outside `trials/` before retrying it.

## Results

`comparison.md` and `comparison.json` contain runtime medians, ranges, coefficient
of variation, stage deltas, CPU time, sampled memory peaks and output equality
checks. Individual trial reports retain transformation counts, throughput, CPU,
memory, slow SQL, complete command logs and fingerprints. The separate detailed
runs quantify profiling overhead, SQL execution counts, connection opens and dbt
invocations. Raw artifacts are the evidence; do not judge the optimization using
only a profiled wall-clock improvement.

CPU totals use cgroup counter deltas at processing command boundaries. The resource
series samples memory every 250 ms. Memory includes container page cache; the
comparison reports the median of each trial's sampled peak. Very short detailed
spans may have no memory sample, which remains unavailable rather than borrowing
another stage's peak. Parent and child timings overlap and must not be summed.

Regenerate the comparison offline without a database:

```sh
uv run python benchmarks/performance.py \
  --out artifacts/performance/comparison --report
```

See [profiling.md](profiling.md) for trace semantics and standalone profiling.

## Benchmark baselines

The weekly/manual smoke workflow currently has no measured baseline and reports
`NO_BASELINE`. The former JSON baseline was a copy of a synthetic test fixture and
has been removed. `10k`, `100k` and `1m` remain defined workloads; their dispatch
options stay disabled until reviewed measurements exist.

Run a smoke benchmark on a dedicated, disposable Compose project:

```sh
COMPOSE_PROJECT_NAME=er-benchmark bash scripts/ci/bench.sh
```

The report is written to `artifacts/bench/latest.json` and `report.md`. Review its
quality, variation and environment fingerprint before promoting it:

```sh
uv run python benchmarks/report.py \
  --compare artifacts/bench/latest.json --scale smoke \
  --baselines-dir benchmarks/baselines --write-baseline
```

The command refuses a `NON_COMPARABLE` run. Commit the generated baseline with its
`baseline_committed` flag in `benchmarks/scales.yaml`; for larger scales, also enable
`dispatchable` and the matching workflow choice. `OK` means within the threshold,
`REGRESSION` means a comparable phase exceeded it, `NON_COMPARABLE` means the
measurement cannot be judged, and `NO_BASELINE` means no comparison was possible.
Historical optimization results above are separate from these CI baselines.

## SQL data-flow comparisons

`benchmarks/data_flows.py` compares native execution with the retained collection
interfaces on deterministic inputs. Each case runs in a fresh process with two
DuckDB threads, a 512 MB DuckDB buffer limit and a private spill directory. Input
generation and output fingerprinting are outside the timer. Peak RSS includes the
whole process and input preparation; it is not a Python-heap measurement.

```sh
uv run python benchmarks/data_flows.py --case ingest --mode reference --records 1000000
uv run python benchmarks/data_flows.py --case ingest --mode sql --records 1000000
```

Other cases are `tf`, `review`, and `reconcile`. Review selects 40% of scores for
the gray band. Reconciliation changes four-record groups to five-record groups and
includes lifecycle planning, persistence and event serialization, excluding graph
clustering. TF uses real Splink registration. No training runs in these cases.

An initial single-pass measurement on macOS ARM64, Python 3.12 and DuckDB 1.5.5:

| Case | Input rows | Reference seconds | SQL seconds | Reference peak RSS MiB | SQL peak RSS MiB |
|---|---:|---:|---:|---:|---:|
| ingest | 100,000 | 0.860 | 0.125 | 124.9 | 183.3 |
| tf | 100,000 | 0.080 | 0.016 | 142.6 | 124.0 |
| review | 100,000 | 0.996 | 0.220 | 179.6 | 194.6 |
| reconcile | 100,000 | 47.997 | 0.923 | 235.4 | 208.8 |
| ingest | 1,000,000 | 8.566 | 0.726 | 322.3 | 650.5 |
| tf | 1,000,000 | 0.664 | 0.068 | 392.0 | 194.5 |
| review | 1,000,000 | 11.569 | 1.762 | 554.5 | 661.0 |
| reconcile | 1,000,000 | >174 (stopped) | 10.309 | not recorded | 616.9 |

Completed reference/SQL pairs produced the same count and aggregate fingerprint.
The million-record reference reconciliation was still running after 174 seconds
and was stopped; no completed reference fingerprint exists for that case. The SQL
case emitted 300,000 events. Small randomized partition tests independently check
assignments, transitions and ordered event details against the pure planner.

These are initial stage measurements, not repeated regression baselines or complete
pipeline benchmarks. Native ingestion and review were faster while using **more**
total RSS at this buffer limit; TF registration used less. Removing Python data
round trips is not a guarantee of lower whole-process memory. SQL intermediate
relations, parallel readers and buffer allocations remain part of capacity planning.
Exact-format serialization and graph exceptions are documented in
[Python processing exceptions](python-processing-exceptions.md). Re-run both modes
on an idle host before making deployment capacity or regression-threshold decisions.
