# Performance & Scaling

This engine is a **SQL generator**: it builds a `__dedupe__*`-templated CTE
pipeline and executes it on a single DuckDB connection per [`Session`]. DuckDB
is the compute engine — it is columnar, vectorized, multi-threaded, and spills
to disk when a workload exceeds memory. So performance tuning is mostly about
**configuring DuckDB well and not getting in its way**, not hand-optimizing
Rust. The Rust side only builds SQL strings and shuttles small aggregates (EM
counts, medians) back and forth.

## How it scales across CPUs

DuckDB parallelizes a single query across worker threads automatically. Give it
more cores and the parallel-friendly stages get faster with no code change:

| Stage | Parallelism | Notes |
|-------|-------------|-------|
| TF tables, concat | Good | parallel scans + hash aggregation |
| Blocking join | **Output/skew-bound** | a join that emits tens of millions of pairs is limited by result volume and block skew, not CPU |
| Comparison vectors + scoring | **Excellent** | a wide vectorized projection (`CASE`/string funcs) over the blocked pairs — the part that scales nearly linearly |
| EM counts | Good | parallel aggregation; the M-step in Rust is negligible |
| Clustering | Partial | iterative connected-components SQL; each round parallelizes, the iteration is sequential |

The practical takeaway: **scoring dominates a realistic run and scales with
cores**, so adding CPUs speeds up predict end-to-end. Blocking that produces a
huge pair set is the stage to watch — tighten blocking rules (see
[blocking analysis](README.md)) rather than expecting cores to absorb it.

### Measured

8-core machine (Apple M-series, DuckDB 1.5.5 bundled), synthetic 40k-row
dedupe with deliberately loose blocking → **≈16M candidate pairs** so scoring
dominates. `cargo bench -p dedupe-core` (criterion median of 10 samples):

| threads | end-to-end predict | speedup | blocking only | speedup |
|--------:|-------------------:|--------:|--------------:|--------:|
| 1 | 3.41 s | 1.00× | 531 ms | 1.00× |
| 2 | 1.97 s | 1.74× | 513 ms | 1.03× |
| 4 | 1.33 s | 2.58× | 521 ms | 1.02× |
| 8 | 1.08 s | 3.17× | 518 ms | 1.03× |

The read: **predict scales ~3.2× on 8 cores** because scoring (the bulk of the
work on a large pair set) parallelizes. **Blocking is flat** — it is bound by
how many pairs it emits and by block skew, not CPU, so cores can't help it;
tighten blocking rules instead. On a *realistic* model with tight blocking the
pair set is small and predict is already sub-second single-threaded (the 50k
historical fixture predicts in ~0.1 s), so cores matter most exactly when the
candidate set is large.

Reproduce:

```
cargo bench -p dedupe-core -- predict_scaling
# or the lightweight harness (also the flamegraph target):
cargo run --release --example profile_predict -- historical_50k 1
cargo run --release --example profile_predict -- historical_50k 8
```

## Memory efficiency

Stable, bounded memory is what lets you scale by adding CPUs (or pods): peak
stays flat, throughput rises with cores. The levers:

- **Automatic spill directory** — every `Session` that isn't given a
  `temp_directory` creates its own unique one (and removes it on drop). An
  in-memory DuckDB otherwise keeps everything resident; a dedicated per-process
  dir means it can always evict to disk and no two processes ever share a spill
  dir (a known corruption hazard in multi-pod setups).
- **`memory_limit`** — hard cap on in-memory buffers; above it DuckDB spills
  rather than OOM-ing.
- **`preserve_insertion_order = false`** — drops row-order bookkeeping DuckDB
  keeps for large scans/joins (its own first suggestion in an OOM message). The
  CLI sets it automatically when running multi-threaded (already
  non-deterministic in float-sum order); `--threads 1` keeps it on so
  golden-exact reproduction is untouched. Value-safe either way — results are
  compared order-independently.
- **Explicit temp-table lifecycle** — `__dedupe__*` intermediates are native
  temp tables dropped as soon as a stage is done (clustering, for instance,
  drops per-iteration tables), so the live set stays near one stage's data.
- **Chunked / streamed predict** (below) — bounds the scoring working set, and
  with `--stream` the result too.

`SessionOptions::performance(temp_dir)` bundles the throughput defaults (all
cores, `preserve_insertion_order = false`, explicit spill dir). The CLI exposes
`--threads`, `--memory-limit`, and `--temp-dir`.

```rust
// Throughput: all cores, relaxed ordering, spill to disk.
let session = Session::open_in_memory(
    SessionOptions::performance("/fast/scratch").with_memory_limit("16GB"),
)?;
```

### Measured: flat memory, cores add throughput

Scoring the 64M-pair synthetic workload under a **fixed 200 MB** `memory_limit`
(DuckDB spilling to disk), varying only the thread count:

| threads | predict | vs 1 thread |
|--------:|--------:|------------:|
| 1 | 17.7 s | 1.00× |
| 2 | 11.0 s | 1.61× |
| 4 | 7.5 s | 2.37× |
| 8 | 5.7 s | 3.10× |

Memory stays capped at 200 MB throughout; adding cores just makes it finish
sooner. That is the whole point — a pod with a fixed memory budget scores an
arbitrarily large candidate set (spilling), and you buy speed with CPUs or more
pods (each a `--num-chunks` hash range), not with more RAM. Below the point
where the *result table* itself won't fit, switch on `--stream` so each pass
lands in the lake instead of a temp table.

### Chunked predict (bounded-memory scoring)

When the candidate-pair set is too large to block-and-score in one shot — even
with spilling — split predict into **hash chunks**:
`predict::run_predict_chunked(.., num_chunks)` / `Linker::predict_chunked(..)`
(CLI `dedupe predict --num-chunks N`). Pass `i` scores only the pairs whose
*right* record satisfies `hash(uid_r) % N == i`, and the passes are unioned.
Because the dedupe where-clause fixes `uid_l < uid_r`, every pair has one right
record, so each pair lands in exactly one chunk — **the result is byte-identical
to the single-shot path** (`golden_chunking.rs` asserts chunked == golden),
while the blocked-pairs and scoring intermediates for one pass are ~`1/N` of the
whole. The concat and TF tables are built once and reused across passes. `N = 1`
is the default and dispatches straight to the unchunked path, so results are
untouched until you opt into chunking.

By default the passes accumulate into one temp table, so the *final result*
still lands in memory (DuckDB spills it if needed). For **constant memory end to
end**, stream each pass straight to the lake instead:
`Linker::predict_chunked_to_lake(target, .., num_chunks, lake_opts)` (CLI
`dedupe predict --num-chunks N --stream`). Pass 0 creates the (optionally
partitioned) target and writes its rows; each later pass appends and is flushed
to Parquet — and freed — before the next begins, so peak memory is one pass, not
the whole result. The written table equals the single-shot output
(`streaming_chunked_predict_matches_direct` asserts it exactly). Committing is
what frees the memory, so a run of N chunks leaves N snapshots; compact them
afterwards with `dedupe maintenance --checkpoint`. Pair `--stream` with
`--partition-by` to land the output already partitioned for downstream reads.

## Everything runs inside DuckDB (no network, no Python UDFs)

A common question is whether moving work into DuckDB UDFs would cut network
overhead and speed things up. For engines that drive DuckDB from Python that
matters (Python UDFs are slow and cross a process boundary). It does **not**
apply here: DuckDB is
statically bundled into this Rust binary and runs **in-process**, and the engine
generates SQL that calls DuckDB's **native C++ built-ins**. There is no Python,
no RPC, no DuckDB server — the whole pipeline (blocking, comparison vectors,
scoring, TF, SQL connected-components) executes inside one process with zero
network. The only network is DuckLake object-storage I/O, which is storage, not
compute.

What actually crosses the Rust↔DuckDB boundary is a handful of **small in-process
aggregate pulls** (EM agreement-pattern counts, prior/u counts — all `O(levels)`
or `O(distinct patterns)`), plus `edge_bridges`, which pulls the thresholded
edge set to compute bridges with `petgraph` (DuckDB has no native bridge
algorithm). Nothing streams the candidate pairs out of DuckDB.

> **Future optimization (not worth it yet): `edge_bridges` data movement.**
> `edge_bridges` is the one place a non-trivial result leaves DuckDB. Measured
> on 5M edges (8 cores), the cost splits roughly: DuckDB→Rust string pull 1.17s
> (55%), string-keyed graph build 0.67s (31%), bridge DFS 0.38s (18%). The
> algorithm is *not* the bottleneck — 82% is data movement and string hashing,
> which exist only because the work left DuckDB.
>
> - A custom Rust **table-function extension** that ran bridges over the scan
>   in-process would remove the serialization boundary (~2–4× faster here), but
>   it would *not* speed up the algorithm itself, and it's a heavy thing to own
>   (per-release builds, signing, distribution) for a secondary QA metric.
> - A much cheaper in-Rust change captures most of the same win with no new
>   build system: map node keys to dense integer ids in SQL, add a typed
>   `(i64, i32, i32)` fetch instead of `Vec<Vec<String>>`, and build a
>   `UnGraphMap<u32>`. That drops the per-cell `String` allocations and replaces
>   string hashing with integer hashing (likely well under 1s). Guarded by the
>   `edge_bridges_match_golden` regression test.
>
> Either way this only touches `edge_bridges` (run on the confident-edge subset,
> opt-in), never the predict/cluster hot path — which is already fully in DuckDB
> with nothing to push down. Do the cheap version first, and only if this metric
> ever shows up as a real cost.

**Are the fuzzy-matching extensions faster?** Measured on 5M string pairs
(DuckDB 1.5.5): the `rapidfuzz` community extension is the *same speed* as the
built-ins for the functions we use — `jaro_winkler_similarity` 0.095s built-in
vs 0.097s rapidfuzz; `levenshtein` 0.376s vs 0.344s. DuckDB's built-ins are
already rapidfuzz-grade native C++, so there is no speedup to be had there.

**What the extensions do add is capability**, not speed — functions DuckDB
lacks natively, usable directly in comparison/blocking SQL so new logic stays
in-engine:

- `rapidfuzz` → `rapidfuzz_token_set_ratio` / `token_sort_ratio` /
  `partial_ratio` (order-insensitive / substring name matching).
- `splink_udfs` → `soundex`, `double_metaphone` (phonetic blocking keys).
- `lsh` → MinHash/LSH blocking.

Both `rapidfuzz` and `splink_udfs` load cleanly in the bundled build. Load them
via `SessionOptions { extensions, extension_directory, install_extensions }` or
the CLI `--extension rapidfuzz --extension-dir /opt/duckdb-ext`. Keep
`install_extensions` off in production and **bake the extension binaries into the
container image** (point `--extension-dir` at them) so loading needs no runtime
network — exactly the air-gapped-pod pattern. Extension names are validated as
bare identifiers, so they can't inject SQL.

The core Fellegi-Sunter metrics — `jaro_winkler_similarity`, `levenshtein`,
`damerau_levenshtein`, `jaccard`, `regexp_*` — are all built in and need no
extension.

## DuckLake layout: partitioning & "clustering keys"

DuckLake stores tables as Parquet files with a SQL catalog. It has **no
secondary indexes** — `CREATE INDEX` is rejected by design (columnar files, not
B-trees). The two physical-layout levers it *does* offer, both exposed via
[`LakeWriteOptions`] / `write_to_lake_with` and the CLI's `--partition-by` /
`--order-by`:

- **Partitioning** (`partition_by`) — writes one Parquet subtree per distinct
  value (`col=val/…`). A reader filtering on that column scans only matching
  files. Good for columns downstream queries filter on: `source_dataset`, a
  batch/ingest date, a cluster bucket.
- **Sort-on-write** (`order_by`) — physically orders rows so each Parquet
  file's min/max statistics (zone maps) are tight, letting range/equality
  filters skip whole files. This is DuckLake's **clustering-key equivalent**:
  no index object, just well-ordered data plus statistics.

Important scope: these speed up **reads from / writes to the lake**, not the
in-engine blocking join. Blocking runs on native temp tables (DuckLake cannot
host temp tables), so partition the *inputs* you filter on (incremental
batches) and the *outputs* consumers query, not the scratch space.

## DuckLake maintenance

Read and write performance degrade as small files, delete files, and old
snapshots accumulate — so maintenance is a first-order performance lever, not an
afterthought. `dedupe maintenance --catalog <…>` wraps DuckLake's maintenance
functions (each also on `Session`):

| Flag | Action |
|------|--------|
| `--checkpoint` | full `CHECKPOINT`: flush inlined → expire → merge → rewrite → cleanup → delete orphans |
| `--flush-inlined` | flush catalog-resident rows out to Parquet |
| `--merge-adjacent [--max-file-size N] [--max-compacted-files N]` | compact small files (bounded, for tiered compaction) |
| `--rewrite-table T` | rewrite a table's delete-heavy files |
| `--expire-older-than '7 days'` / `--delete-older-than '2 days'` / `--target-file-size 512MB` | set catalog options, then act |
| `--set KEY=VALUE` | set an arbitrary catalog option (e.g. `parquet_compression=zstd`) |
| `--cleanup-old-files` / `--delete-orphaned-files` | reclaim unreferenced / orphaned files |
| `--list-options` | print catalog options and exit |

Run it as a **single scheduled job**, never from every writer. A typical split:
a frequent bounded `--merge-adjacent` + `--flush-inlined`, and a nightly
`--checkpoint` with `--expire-older-than` set well above your longest
transaction. The command prints per-table `files`/`delete_files` before→after so
you can see compaction take effect.

```sh
# Nightly full maintenance
dedupe maintenance --catalog meta.ducklake --data-path ./data \
  --expire-older-than '7 days' --delete-older-than '2 days' \
  --target-file-size 512MB --checkpoint
```

## Profiling

Two complementary views:

1. **DuckDB query profiler** — per-operator timings and cardinalities for the
   generated SQL (where the time actually goes):

   ```sql
   PRAGMA enable_profiling = 'json';
   PRAGMA profiling_output = '/tmp/profile.json';
   ```

   or `EXPLAIN ANALYZE <query>` for a single statement.

2. **Sampled flamegraph** of the whole process (Rust + native DuckDB frames)
   with [`samply`](https://github.com/mstange/samply):

   ```
   cargo build --release --example profile_predict
   samply record -- target/release/examples/profile_predict 'synthetic:60000' 8
   ```

   **Finding:** a sampled flamegraph of this engine is almost entirely
   *unsymbolicated native DuckDB frames* spread across ~22 worker threads (the
   bundled DuckDB library is stripped), with no single Rust hot spot — the Rust
   side just assembles SQL strings. That confirms the compute lives in DuckDB's
   parallel executor, so the **DuckDB query profiler (view 1) is the actionable
   tool**, not the flamegraph. On the ≈16M-pair synthetic predict it attributes
   the scoring statement to: two `HASH_JOIN`s (blocked pairs → record columns),
   the `mw_*` scoring `PROJECTION`s, and the row sink — all parallel operators,
   which is why the stage scales (above). The `profile_predict` example will
   dump that per-operator tree when run with
   `DEDUPE_PROFILE_SQL=/tmp/p.json`.

## CI

`cargo bench -p dedupe-core --no-run` builds the benches on every CI run so they
never bit-rot; the benches themselves run manually (they take minutes). See
`.github/workflows/ci.yml`.

[`Session`]: crates/dedupe-core/src/session/mod.rs
[`SessionOptions`]: crates/dedupe-core/src/session/mod.rs
[`LakeWriteOptions`]: crates/dedupe-core/src/session/ducklake.rs
