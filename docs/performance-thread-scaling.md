# Pipeline thread scaling (work in progress)

DuckDB's thread count (`ER_DUCKDB_THREADS`, equal to `ER_CPU_LIMIT`) is the
pipeline's parallelism control. Matching, training and standardization compile to
DuckDB SQL, which parallelizes a single query across the configured threads; Splink
5 removed its own `salting_partitions` API, so thread count is the intended
mechanism. The prediction `num_chunks_*` knobs run chunks sequentially and are
memory control, not parallelism (see [performance-splink5.md](performance-splink5.md)).

**These are work-in-progress numbers, not committed baselines.** They were measured
locally with `benchmarks/full_pipeline.py` overriding the envelope on one developer
machine (Apple Silicon, 8-CPU Docker VM, ~11.7 GiB), a single pass per arm. They are
not comparable across machines and do not clear the S10.4 bar for a committed
baseline (which requires the CI runner classes and `--repeat 3` medians). Scoring is
deterministic across thread counts, so the point of the comparison is wall-clock, not
quality.

## One million records

Same immutable corpus, same image; the only change is the envelope.

| Stage | 2 threads / 4 GB | 6 threads / 6 GB | Change |
|---|---:|---:|---|
| ingest | 11.08 | 9.81 | −11% |
| standardize | 19.51 | 14.83 | −24% |
| train | 10.65 | 7.50 | −30% |
| match | 18.65 | 8.36 | −55% |
| reconcile | 23.48 | 24.26 | +3% |
| assemble | 12.85 | 10.30 | −20% |
| **Initial load** | **96.22** | **75.06** | **−22%** |
| Incremental delivery (10k) | 30.76 | 26.68 | −13% |

Golden/lineage counts (395,867 / 2,375,202) and every edge/cluster precision and
recall metric are identical between the two arms. Peak container memory 4.06 → 4.26
GiB; total CPU-seconds 137.9 → 165.6 (idle cores traded for wall-clock).

## Ten million records

6 threads held DuckDB at 4 GB to match the previously recorded 2-thread reference
envelope, isolating thread count as the only change.

| Stage | 2 threads (recorded) | 6 threads | Change |
|---|---:|---:|---|
| ingest | 50.04 | 35.25 | −30% |
| standardize | 117.40 | 118.15 | ~0% |
| train | 5,155.33 | 2,855.58 | −45% |
| match | 737.21 | 460.73 | −37% |
| reconcile | 241.81 | 276.79 | +14% |
| assemble | 156.41 | 110.92 | −20% |
| **Initial load** | **6,458.21 (1h 47m)** | **3,857.41 (1h 04m)** | **−40%** |

Training is ~74% of the ten-million-record load, and it is DuckDB-bound EM, so it
takes the largest absolute cut. Outputs match the recorded reference exactly:
3,898,183 golden records, 23,389,098 lineage rows. Peak container memory 9.14 GiB of
the 10 GiB limit.

## Reading the result

- Matching and training scale nearly linearly with threads; standardization and
  assembly gain meaningfully; ingestion is I/O-bound.
- Reconciliation is a Python graph step, does not parallelize with DuckDB threads,
  and becomes the largest single stage at one million records once matching shrinks.
  It is the next place to look for a speedup, and it is not addressed here.
- Raising threads needs proportional `ER_DUCKDB_MEMORY_LIMIT` headroom — each worker
  holds its own buffers, and staging at scale can exhaust memory before spilling
  helps. The `local_scale` fitter deliberately caps threads at `duckdb_gib // 2` for
  this reason.

## Reproducing

```sh
# Fit the host, then raise only the parallelism knobs (no scales.yaml edit):
uv run python benchmarks/full_pipeline.py --scale 1m --local --with-incremental \
  --cpu-limit 6 --duckdb-memory-limit 6GB \
  --corpus-root artifacts/bench/shared-1m --out artifacts/bench/threads6-1m

# Ten million reuses the 1m envelope; keep DuckDB at 4GB to match the reference:
uv run python benchmarks/full_pipeline.py --scale 10m --cpu-limit 6 \
  --mem-limit 10g --duckdb-memory-limit 4GB --out artifacts/bench/threads6-10m
```

Under the control plane, an operator sets the same knobs per tenant with
`PATCH /v1/orgs/{org}/resources` (Admin → tenant detail → **Pipeline resources**).
The value is merged into the environment the dispatcher injects into that tenant's
next job; a running job keeps the environment it was spawned with.
