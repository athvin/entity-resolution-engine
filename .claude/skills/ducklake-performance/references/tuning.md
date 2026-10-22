# Storage and tuning rules

## Contents

- Check the installed version first
- Storage boundaries: three different things
- Sorted insertion
- Partitioning
- Partitioning plus ordering
- Other candidates before more threads
- Budget each subsystem independently
- Primary references

## Check the installed version first

The workspace pins `duckdb` with an `=` pin, and the DuckLake extension's
supported syntax and automatic maintenance vary by version. So:

- Get the current pin from the rust-conventions skill's generated crate map, or
  from `rust/Cargo.toml` directly.
- Probe the baked extension's capabilities before proposing SQL that depends on
  a feature.
- **Never download an extension or a model during a benchmark or a production
  run.** It perturbs the measurement and it fails closed when the registry is
  unreachable. Extensions are baked into the image.

External deployment defaults from managed DuckDB vendors are context, not
settings to copy.

## Storage boundaries: three different things

Keep these distinct in any recommendation. Conflating them is the most common
error, because advice written for one is often wrong for another.

**1. DuckLake tables** — Parquet data with a SQL catalog.

- **Ordinary DuckDB ART indexes are unsupported.** `CREATE INDEX` is rejected.
  **Do not propose an ART index on a DuckLake table.**
- The read mechanisms that *do* apply are partition and file statistics, and
  Parquet row-group min/max pruning.
- DuckLake's internal catalog indexes are a separate topic. Do not modify the
  DuckLake-managed catalog schema without upgrade-safe guidance and
  catalog-side evidence.

**2. Local DuckDB scratch tables** — the native temp catalog where every
`__dedupe__*` intermediate lives.

- These have zone maps, and an ART index **is** a candidate here for selective
  point lookups — but only if the actual plan can use it.
- Index creation costs time, memory and writes. Broad scans and joins do not
  automatically benefit.

**3. The catalog database** (Postgres in deployment).

- Catalog optimization needs **catalog-side evidence**. Native DuckDB profile
  timings, where available, do not measure the catalog's query plans.

## Sorted insertion

Placing similar values in the same row group narrows min/max ranges, which
improves both pruning and compression. It does **not** require or create an ART
index.

- An `ORDER BY` aligned with the filters queries actually use may help.
- Ordering a whole table for a broad join may simply add sort and spill cost.
- Multiple sort columns primarily favour predicates on the **leading** key.
- Later incremental files weaken clustering over time, so a benefit measured
  right after a full write is an optimistic reading.
- **Do not assume `ORDER BY` survives every writer.** Inspect output footers
  and the actual scan pruning rather than trusting the write path.

## Partitioning

Partitioning helps **only** when predicates prune partitions or files.

- Avoid high-cardinality record or entity keys. The result is a very large
  number of tiny files, and per-file overhead costs more than the scan saved.
- Changes apply to **new writes**. Rewriting existing files is a separate
  decision with its own cost.
- Supported DuckLake sorted-table syntax and automatic maintenance vary by
  extension version — check before proposing.

## Partitioning plus ordering

The combination is often the right answer: partition on the coarse filter,
order within the partition on the selective one.

Because the arms interact, a layout proposal must measure **four** arms:

```
baseline │ ordering alone │ partitioning alone │ both
```

and must include, for each: full-load write cost, incremental deletes and
updates, files opened, bytes read, sort spill, compaction, and total time.

Reporting only the read improvement is how a layout change ships and then
makes the incremental workload slower.

## Other candidates before more threads

Investigate these before raising thread or memory limits — they are usually the
larger win and they do not trade memory for speed:

- repeated lake scans or CTEs that could be materialized once,
- over-wide projections (the no-`SELECT *` rule exists partly for this),
- join fan-out,
- premature materialization between stages.

Prototype a set-based DuckDB transformation in bounded scratch state and compare
semantic outputs before adopting it.

**Hard limits on what a speed change may do:** do not cross tenant or
writer-lock boundaries, and do not alter blocking, model or TF generations to
gain speed. A faster path that changes a score is a wrong path — INV-SCORE, see
the er-matching skill.

## Budget each subsystem independently

DuckDB, the catalog and the object store have separate budgets. Mixing them
produces a total that explains nothing.

One counter-intuitive consequence worth knowing: **increasing threads can
increase total memory use and spill**, because each thread holds concurrent
buffers — so a thread increase can exceed the container's memory limit even
with `memory_limit` set, since that limit governs DuckDB's own accounting and
not the container's.

## Primary references

- [DuckDB profiling](https://duckdb.org/docs/current/dev/profiling)
- [DuckDB profiling metrics](https://duckdb.org/docs/current/dev/metrics)
- [DuckDB performance overview](https://duckdb.org/docs/lts/guides/performance/overview)
- [DuckDB indexing and ordered data](https://duckdb.org/docs/current/guides/performance/indexing)
- [DuckLake unsupported features](https://ducklake.select/docs/stable/duckdb/unsupported_features)
- [DuckLake partitioning](https://ducklake.select/docs/stable/duckdb/advanced_features/partitioning)
- [DuckLake sorted tables](https://ducklake.select/docs/stable/duckdb/advanced_features/sorted_tables)
- [DuckLake logging](https://ducklake.select/docs/stable/duckdb/advanced_features/logging)

Verify every knob against the installed DuckDB version and this repo's own
session options (`SessionOptions` in `dedupe-core`), not against the docs'
defaults.
