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

`ER_CPU_LIMIT` and `ER_MEM_LIMIT` configure Compose's container limits;
Compose derives DuckDB's thread count from the CPU limit. The image includes the
pinned DuckDB extensions. The supplied Compose stack is a development/test setup
with local credentials and ephemeral service storage.

For separate tenants, use separate metadata schemas and object prefixes as well
as distinct configs. Changing `tenant` alone does not provision another lake.

## Changing rules

Configuration and model fingerprints guard incremental runs. After changing
standardization or survivorship behavior, bump its version and perform a full
rebuild; use `--reason std_version_bump` or `--reason survivorship_version_bump`
to record the intent. Address-parser changes also require a standardization version
bump. Retraining activates a new model and requires a full rescore before reconciliation.

Use [the runbook](runbook.md#full-rebuilds-and-correction) for the command sequence.
