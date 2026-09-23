# Storage and tuning rules

## Version and storage boundaries

The manifest and `er doctor` own installed versions. Consult official docs for those
versions and probe the baked extension's capabilities before proposing unsupported
SQL. Never download an extension or model during a benchmark or production run.

- **DuckLake** stores Parquet data and PostgreSQL metadata. Ordinary DuckDB ART indexes
  are unsupported on DuckLake tables. Partition/file statistics and Parquet row-group
  min/max pruning are the relevant read mechanisms. Internal catalog indexes are a
  separate topic; do not modify DuckLake-managed PostgreSQL schema without supported
  upgrade-safe guidance and measured catalog evidence.
- **Local DuckDB scratch tables** have zonemaps; an ART index is a candidate for
  selective point lookups if the actual plan can use it. Index creation, memory and
  writes cost time. Broad scans/joins do not automatically benefit.
- **Sorted insertion** can place similar values in the same row group, narrowing
  min/max ranges and improving pruning/compression. It does not require or create an
  ART index. An `ORDER BY` aligned with common filters may help; ordering a whole table
  for a broad join may simply add sort/spill cost. Later incremental files can weaken
  clustering. Multiple sort columns primarily favor predicates on the leading key.
- **Partitioning** helps only when predicates prune partitions/files. Avoid high
  cardinality record/entity keys and partitions that create many tiny files. Changes
  apply to new writes; assess rewriting existing files separately. Supported DuckLake
  sorted-table syntax and automatic maintenance vary by extension version.
- **Partitions plus ordering**: order within a useful partition by common selective
  filters. Measure baseline, ordering alone, partitioning alone and both, including
  full-load writes and incremental deletes/updates, files opened, bytes read, sort
  spill, compaction and total time. Do not assume `ORDER BY` survives every writer;
  inspect output footers and actual scan pruning.

## Pushdown and other candidates

Investigate repeated lake scans/CTEs, over-wide projections, join fan-out and premature
Python materialization before increasing threads/memory. Prototype a set-based DuckDB
transformation in bounded scratch state and compare semantic outputs. Do not cross
tenant/writer-lock boundaries or alter blocking/model/TF generations to gain speed.

Budget DuckDB, Python, dbt/ONNX subprocesses, catalog and object store independently.
Increasing threads can increase concurrent buffers and spill or exceed container
memory despite `memory_limit`. Native profile request timings, if available, do not
measure PostgreSQL execution plans. Catalog optimization needs catalog-side evidence.

## Primary references

- [DuckDB profiling](https://duckdb.org/docs/current/dev/profiling)
- [DuckDB profiling metrics](https://duckdb.org/docs/current/dev/metrics)
- [DuckDB performance overview](https://duckdb.org/docs/lts/guides/performance/overview)
- [DuckDB indexing and ordered data](https://duckdb.org/docs/current/guides/performance/indexing)
- [DuckLake unsupported features](https://ducklake.select/docs/stable/duckdb/unsupported_features)
- [DuckLake partitioning](https://ducklake.select/docs/stable/duckdb/advanced_features/partitioning)
- [DuckLake sorted tables](https://ducklake.select/docs/stable/duckdb/advanced_features/sorted_tables)
- [DuckLake logging](https://ducklake.select/docs/stable/duckdb/advanced_features/logging)
- [Splink DuckDB performance](https://moj-analytical-services.github.io/splink/topic_guides/performance/optimising_duckdb.html)

The Splink guide may target a different major version. Verify knobs against the
installed Splink version and this repository's `er.matching.runtime` implementation.
The Go profiling API is not this Python implementation's interface. Ascend/MotherDuck
deployment defaults are context, not settings to copy into this Compose stack.
