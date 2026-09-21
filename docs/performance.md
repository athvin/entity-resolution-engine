# Comparing pipeline performance

The [2026-09-20 results](performance-results-2026-09-20.md) record a 27.6% reduction
in median processing time across five matched 10k-record runs per version.

The comparison runner measures the complete CLI lifetimes for ingestion,
standardization, training, matching, reconciliation and assembly, followed by an
incremental cycle. It excludes environment initialization, corpus generation and
output verification. Normal stage logging and 250 ms container resource sampling
remain enabled. Native SQL profiling runs separately because it adds substantial
overhead to repeated statements.

## Implementation

- Ingestion, graph nodes/edges and incremental keys use bound, explicitly typed
  arrays with `INSERT ... SELECT UNNEST(...)`. Loads are bounded at 1,024 rows;
  ingestion order, transaction boundaries and all business rules are preserved.
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
setup/teardown, ledger operations and dbt launch/invocation. These diagnostic spans
must be identical in the baseline and candidate before making a comparison.

## Reproduce a comparison

Freeze the baseline and candidate in separate source directories, including any
uncommitted changes. Add only the common diagnostic spans to the baseline. Record
both original and measured source snapshots; a Git SHA alone cannot identify a
dirty working tree.

Build both versions using the same existing dependency image:

```sh
.venv/bin/python benchmarks/build_performance_image.py \
  --context /path/to/baseline --base-image er-pipeline:ci \
  --tag er-perf:baseline --out artifacts/performance/build-baseline
.venv/bin/python benchmarks/build_performance_image.py \
  --context . --base-image er-pipeline:ci \
  --tag er-perf:candidate --out artifacts/performance/build-candidate
.venv/bin/python benchmarks/performance.py \
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
.venv/bin/python benchmarks/performance.py \
  --out artifacts/performance/comparison --report
```

See [profiling.md](profiling.md) for trace semantics and standalone profiling.
