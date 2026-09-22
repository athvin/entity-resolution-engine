# Configuration

Start with [configs/default.yaml](../configs/default.yaml). The checked-in
[test configuration](../configs/test.yaml) supplies the fixture and Compose defaults.
Pass `--config PATH` after a command or set `ER_CONFIG` to select a file.

## Source data

Deliver CSV or Parquet files in `<drop-root>/<source-name>/`. For example,
`er ingest --source crm --path /app/drop` reads `/app/drop/crm/`. The path names
the drop root, not a single file or the source subdirectory.

Each source declares its adapter, ID column, update timestamp column, date format,
priority and business-column mapping. The shipped dbt staging models cover
`crm`, `billing` and `webforms`. Map their input headers in the YAML. Adding a new
source name also requires a staging model and corresponding tests.

```yaml
sources:
  crm:
    adapter: csv
    priority_rank: 1
    record_id_column: crm_id
    updated_at_column: last_modified
    date_format: "%Y-%m-%d"
    # Keep the complete columns mapping from configs/default.yaml.
```

This is an excerpt, not a complete configuration. Source record IDs are strings;
colons are reserved for the composed record key. Keep keys stable across deliveries.
Ordinary ingest appends changed records. `--full-refresh-keys` means the supplied
delivery is the complete key set for that source: absent keys become tombstones.

## Configuration blocks

| Block | Controls |
|---|---|
| `tenant` | Namespace identity and writer locking |
| `standardization` | Email handling, placeholder values and default phone region |
| `sources` | Input adapters, mappings and source priority |
| `blocking` | Candidate-generation expressions shared with matching |
| `comparisons` | Comparison levels and term-frequency adjustments |
| `thresholds` | `review_low` and `auto_merge`, with `0 < review_low < auto_merge <= 1` |
| `survivorship` | Ordered rules for each golden attribute |
| `training` | Deterministic rules, sampling seed and EM sessions |
| `storage` | Data path, delivery root and model object prefix |
| `versions` | Standardization, address-parser and survivorship versions |
| `generator` | Synthetic fixture seed |
| `clustering` | Iteration limit and protected-edge threshold |
| `coherence` | Current scorer: `noop` |
| `correction_pass` | Desired cron cadence; an external scheduler must invoke it |

Configuration is validated by Pydantic and cross-field rules before processing.
All blocks and their exact constraints are in [S6](../DesignDoc.md#s6).

## Runtime environment

The Compose file supplies these values for its disposable development lake. For
another lake, provide the complete environment to the container and mount the
selected config and delivery directories where the process can read them.

| Variable | Value |
|---|---|
| `ER_CONFIG` | Absolute config path inside the runtime |
| `ER_CATALOG_DSN` | PostgreSQL connection URI |
| `ER_LAKE_METADATA_SCHEMA` | Unique catalog schema for the lake |
| `ER_LAKE_DATA_PATH` | S3 data prefix, consistent with the configured storage path |
| `ER_LAKE_ALIAS` | `lake` |
| `ER_S3_ENDPOINT` | Host and port, without `http://` or `https://` |
| `ER_S3_ACCESS_KEY_ID`, `ER_S3_SECRET_ACCESS_KEY` | Object-store credentials |
| `ER_S3_REGION` | Object-store region |
| `ER_S3_URL_STYLE` | `path` or `vhost` |
| `ER_S3_USE_SSL` | `true` or `false` |
| `ER_DUCKDB_THREADS` | Integer worker count |
| `ER_DUCKDB_MEMORY_LIMIT` | DuckDB buffer limit, below the container memory limit |
| `ER_SPLINK_NUM_CHUNKS_LEFT`, `ER_SPLINK_NUM_CHUNKS_RIGHT` | Positive integers; full prediction defaults to `1` × `1` |
| `ER_SPLINK_MATERIALISATION` | `table` (default) or `parquet` for Splink scratch results |
| `ER_SPLINK_WORK_DIR` | Required absolute local directory for `parquet`; unset for `table` |

`ER_CPU_LIMIT` and `ER_MEM_LIMIT` configure Compose's container limits;
Compose derives DuckDB's thread count from the CPU limit. The image includes the
pinned DuckDB extensions. The supplied Compose stack is a development/test setup
with local credentials and ephemeral service storage.

For separate tenants, use separate metadata schemas and object prefixes as well
as distinct configs. Changing `tenant` alone does not provision another lake.

Splink execution settings are recorded in benchmark fingerprints and do not change
`config_hash`. Chunk controls apply to full prediction; the pinned incremental APIs
do not expose chunk arguments. Parquet scratch is local, outside DuckLake, and files
created by Splink are removed on completion or failure. Use a directory dedicated
to this job, with sufficient disk space. A process killed by the operating system
cannot run cleanup; remove that job's abandoned directory before retrying.

## Training large corpora

The normal config uses exact deterministic-prior estimation, at most 1,000,000
random pairs for u, no u early stopping, and uncapped EM sessions.
`training.u_sampling_method: bernoulli` preserves the previous ordered, seeded
DuckDB record sample; Splink 5 estimates u over that sample without resampling it.
The opt-in `hash` method uses native Splink 5 sampling, which can change quality
even with the same seed and pair budget. All training
options and the exact Splink version are stored with the model.

The shipped training defaults are:

```yaml
training:
  # Retain the deterministic rules, EM blocking rules, recall and seed.
  u_max_pairs: 1000000
  u_sampling_method: bernoulli
  u_min_count_per_level: null
  u_num_chunks: 10
  em:
    fix_u_probabilities: true
    max_pairs: null
```

Early stopping at 100 observations per level reduced measured precision on the
migration corpus. An EM cap of 1,000,000 pairs missed two additional true cluster
pairs on the million-record corpus. Both remain disabled; no sampled-training
preset is shipped.

EM's `max_pairs` is an approximate cap implemented by sampling records before
blocking. These options can change fitted probabilities, so evaluate precision and
recall on representative labeled data before enabling them. Set the cap and
early-stop count to `null` to retain conservative training. Do not change thresholds
or blocking to compensate for a quality regression without a separate evaluation.

## Changing rules

Configuration and model fingerprints guard incremental runs. After changing
standardization or survivorship behavior, bump its version and perform a full
rebuild; use `--reason std_version_bump` or `--reason survivorship_version_bump`
to record the intent. Address-parser changes also require a standardization version
bump. Retraining activates a new model and requires a full rescore before reconciliation.

Use [the runbook](runbook.md#full-rebuilds-and-correction) for the command sequence.
