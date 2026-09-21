# Performance

## Measured results

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
