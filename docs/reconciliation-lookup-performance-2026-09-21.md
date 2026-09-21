# 100k reconciliation lookup performance: 2026-09-21 UTC

Five matched 100,000-record runs per version reduced median reconciliation time
from **205.37 to 17.79 seconds (91.3%)**. Median processing time for the complete
initial load fell from **273.06 to 84.80 seconds (68.9%)**, delivering **3.22×
throughput** and saving **188.26 seconds per run**.

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Median reconciliation | 205.37 s | 17.79 s | −91.3% |
| Median complete processing | 273.06 s | 84.80 s | −68.9% |
| Median container CPU time | 281.50 CPU-s | 92.80 CPU-s | −67.0% |
| Median sampled memory peak | 3,612.9 MiB | 3,671.1 MiB | +1.6% |
| Processing time coefficient of variation | 0.82% | 1.77% | Both below 5% |

The target was at least 50% less reconciliation time, lower total processing time,
and a sampled memory peak below 6 GiB. All three performance targets passed. The
largest processing memory sample across all ten normal runs was **3.68 GiB**.

## Paired measurements

Pairs alternate baseline/candidate and candidate/baseline execution order.

| Pair | Baseline total (s) | Candidate total (s) | Baseline reconcile (s) | Candidate reconcile (s) |
|---|---:|---:|---:|---:|
| 1 | 270.643674 | 87.500085 | 204.013410 | 17.843946 |
| 2 | 273.063810 | 83.487663 | 205.372812 | 17.840961 |
| 3 | 273.532354 | 83.484047 | 208.024258 | 17.748328 |
| 4 | 276.685408 | 85.631690 | 210.832798 | 17.670309 |
| 5 | 270.687689 | 84.799599 | 205.094628 | 17.786628 |

The reconciliation ranges were 204.01–210.83 seconds for baseline and
17.67–17.84 seconds for the candidate. Component medians are:

| CLI stage | Baseline (s) | Candidate (s) |
|---|---:|---:|
| Ingestion, all three sources | 7.76 | 7.76 |
| Cleaning / standardization | 19.01 | 18.94 |
| Training | 4.47 | 4.42 |
| Matching | 26.38 | 26.43 |
| Reconciliation | 205.37 | 17.79 |
| Golden record assembly | 8.29 | 8.22 |

Component medians do not necessarily sum to the total median. Matching is now the
largest stage at about 26 seconds, followed by standardization at about 19 seconds.
These measurements establish performance for this initial-load workload and resource
envelope; the smaller gates separately verify incremental behavior.

## Implementation

Reconciliation now binds requested keys as one typed `VARCHAR[]` parameter with
`IN (SELECT unnest(?::VARCHAR[]))`, replacing scalar placeholders that grew with
the affected record set. The change covers membership expansion, current partition
loading, standardized-record existence checks, absent-record detection, and the
selection and deletion of tombstoned memberships.

Each helper materializes, deduplicates and sorts its iterable once. Empty input
returns before querying the lake. Membership and partition lookups still return
every member of each touched entity, while deletion remains scoped to requested
records absent from the standardized corpus. `_current_partition` also accepts a
single-use iterator correctly; previously it consumed that iterator twice.

Five helper spans expose these lookup costs when detailed tracing is enabled:
`reconcile.current_membership`, `reconcile.current_partition`,
`reconcile.standardized_records`, `reconcile.absent_records`, and
`reconcile.retract_tombstones`. Normal measurements disable detailed tracing.

## Diagnostic breakdown

The additional candidate pass took 86.16 seconds overall, including 18.00 seconds
for the reconciliation CLI. It completed all 216 spans without collector errors
and captured no native SQL profiles. The targeted lookups took **0.714 seconds
combined**:

| Helper span | Calls | Combined seconds |
|---|---:|---:|
| Current membership | 1 | 0.098 |
| Current partition | 2 | 0.206 |
| Standardized records | 1 | 0.139 |
| Absent records | 1 | 0.172 |
| Tombstone retraction | 1 | 0.099 |

Persistence now accounts for 12.07 seconds of the 15.53-second reconciliation
lifecycle span, including 3.91 seconds of event writes. Label propagation takes
0.89 seconds. These parent and child spans overlap and must not be added together.
The diagnostic has additional instrumentation and is excluded from performance
medians; its helper timings have no identically instrumented baseline counterpart.

## Method

The campaign uses the retained seed-42 corpus of 100,000 source records representing
40,000 generated people. Each large trial starts with an empty lake and performs
ingestion, cleaning/standardization, training, matching, reconciliation, and golden
record assembly. It contains no incremental follow-up. The tiny and smoke gates
also exercise their existing incremental phases.

Five baseline/candidate pairs run serially, reversing order on every other pair.
Both immutable images inherit the same pinned dependencies and use the same frozen
measurement harness and input files. Each fresh Compose stack has a separate
catalog and object store. The processing container has two CPUs, 6 GiB memory, two
DuckDB threads, and a 4 GB DuckDB memory limit.

Processing time sums complete CLI lifetimes, including normal stage logs and
250 ms resource sampling. It excludes stack initialization, corpus generation,
output validation, export, and teardown. CPU time uses command-boundary cgroup
counter deltas. Memory is the sampled peak within processing windows, including
page cache. Native SQL profiling is disabled throughout the 100k campaign. One
additional candidate run enables nested spans and exports outputs; that diagnostic
is excluded from the paired medians.

## Correctness checks

Comparisons normalize generated IDs and timestamps, then check standardized values,
blocking keys and candidate pairs, score classifications, entity partitions,
golden records, and lineage. Score probabilities may differ by at most `1e-10` in
absolute value. Input, semantic configuration, dependency and resource fingerprints
must match. Each 100k run also checks 12 output integrity conditions and expects
100,000 assigned records, 39,995 golden records, and 239,970 lineage rows.

All **15 pipeline passes** succeeded: four tiny/smoke gates, ten normal 100k runs,
and one diagnostic 100k run. All **16 cross-run comparisons** matched, with a
maximum score probability difference of **1.89×10⁻¹⁴**. Every 100k run passed all
12 integrity checks and produced the expected output counts.

The seven new unit cases cover large key sets, duplicates, absent keys, quotes,
Unicode, single-use iterators, full-entity expansion, scoped tombstone removal and
empty inputs without lake relations. Validation passed all 727 unit tests and all
39 affected integration checks, including deletion/resurrection, reconciliation,
replay, assertion cuts, persistence and profiled failure/resume. Ruff lint/format,
strict mypy over 71 source files, and whitespace checks also passed.

## Provenance and reproduction

The retained local campaign is
`artifacts/reconcile-lookups/20260921t024920z/` in the main checkout. It contains
`comparison.json`, per-trial results and logs, the frozen harness, retained inputs,
source manifests, the candidate diff, and validation receipts. The baseline is
commit `679dbf9ea4a498d2ec568d21b97325c6dbcb1a44`; the candidate is that revision
plus the three lookup modules and their new unit test. The image manifests also
record a differing ignored dbt anonymous usage ID, which is not pipeline configuration.

The diagnostic retains eight Parquet tables and `golden_records.csv` under
`trials/100k-candidate-spans-1/run/outputs/`. The exports were read back successfully;
the CSV contains 39,995 data rows. `output-manifest.json` records file sizes, hashes
and row counts; `diagnostic.json` records helper timings, and `final-validation.json`
records the final checks. All campaign containers, networks and volumes were removed.

The 100k per-trial report introductions were corrected after the campaign to state
that these runs contain only the initial load. This reporting-only correction did
not change the frozen harness, source, timings or output comparisons.

| Snapshot | SHA-256 |
|---|---|
| Baseline image | `13fb964a285831fd304fcff6b121d0cb3d3d59999eccaa125d5782976d5657ab` |
| Candidate image | `6d978d5aefce0deb4e36481577cf35184f0df8c13322ea5878f686e7788c9163` |
| Baseline source overlay | `a5a9ef9ec77ccd366785598b753d215d3eaf04c570c9429fda1be82dc70255aa` |
| Candidate source overlay | `e925f0350fa19ca5f5d71c49af4c94de642747f93878c2680db454070c837fed` |

The artifact-local `campaign.py` records the exact sequence and commands. It uses
`worker.py` and a hashed copy of the benchmark harness with initial-only and
spans-only options. These experiment options do not change the pipeline CLI or
configuration schema. Successful existing trials are reused only with the same
frozen harness; a fresh campaign needs its own output directory and settings.

Regenerate the aggregate report offline from the main checkout:

```sh
python3 artifacts/reconcile-lookups/20260921t024920z/comparison.py
```

See [performance.md](performance.md) for building immutable source images and
[profiling.md](profiling.md) for instrumentation semantics.
