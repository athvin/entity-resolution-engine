# Configuration

*Engine-layer configuration (one tenant's lake). Under the multi-tenant control
plane, config is versioned and published per tenant — [backend-design.md](backend-design.md) §6.*

Start with [configs/default.yaml](../configs/default.yaml). The checked-in
[test configuration](../configs/test.yaml) supplies the fixture and Compose defaults.
Pass `--config PATH` after a command or set `ER_CONFIG` to select a file.
## Source data

Deliver CSV or Parquet files in `<drop-root>/<source-name>/`. For example,
`er ingest --source crm --path /app/drop` reads `/app/drop/crm/`. The path names
the drop root, not a single file or the source subdirectory.

Each source declares its adapter, ID column, update timestamp column, date format,
priority and business-column mapping. The unified `stg_records` staging SQL renders
one union arm per declared source, so adding a new source name is a config change
alone — no staging code accompanies it.

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
| `survivorship_groups` | Field sets that survive TOGETHER from one record, with an optional `controlling` field the rules rank on. `address` is the built-in instance; declaring it overrides the default. Omitted when empty, so an existing config's `config_hash` is unchanged |
| `training` | Deterministic rules, sampling seed and EM sessions |
| `storage` | Data path, delivery root and model object prefix |
| `versions` | Standardization, address-parser and survivorship versions |
| `generator` | Synthetic fixture seed |
| `clustering` | Iteration limit and protected-edge threshold |
| `coherence` | Current scorer: `noop` |
| `correction_pass` | Cron cadence. Under the control plane it is synced into a schedule on config publish (backend-design §5); standalone, an external scheduler invokes it |

Configuration is validated by Pydantic and cross-field rules before processing.
All blocks and their exact constraints are in [S6](../DesignDoc.md#s6).

## Extra source fields

Fields outside the nine canonical mappings automatically become `metadata`; no
extra mapping or survivorship rule is needed. The source ID, update timestamp and
reserved fixture `persona_id` are excluded. Names and values retain their delivered
spelling, including blanks and nulls (CSV/Parquet adapters render values as text/null).

Staging and `int_std_records` store the record's extra fields directly. Golden
records collect all current members under source and record ID:

```json
{
  "crm": {
    "123": {"loyalty_tier": "gold"},
    "456": {"loyalty_tier": "silver"}
  },
  "billing": {
    "A789": {"payment_terms": "net30"}
  }
}
```

The record ID layer preserves conflicts within one source. Records with no extra
fields are omitted, and an entity with no metadata receives `{}`. Metadata never
enters blocking, matching comparisons, training features or winner selection.
Metadata-only changes are ingested as new versions and refresh touched golden rows
even when membership is unchanged. Merges, splits and deletions rebuild metadata
from the current membership rather than accumulating historical values.

For example, a client can retrieve the original CRM detail directly:

```sql
SELECT entity_id,
       metadata -> 'crm' -> '123' ->> 'loyalty_tier' AS loyalty_tier
FROM lake.main.golden_records;
```

For a lake created before this feature, bump `versions.std_version` and
`versions.survivorship_version` and run the planned full rebuild (`er run-all
--mode full`). This backfills metadata from retained raw payloads. The default
configuration now uses version `2` for both; the fixture configuration retains its
existing version labels. Identical historical deliveries still follow the engine's
existing replay/anti-join rules.

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

`ER_DUCKDB_THREADS` is the pipeline's parallelism control: matching, training and
standardization are DuckDB queries that scale across the thread count, so raising
it (with enough `ER_DUCKDB_MEMORY_LIMIT` headroom for each worker's buffers) speeds
those stages without changing results — scoring is deterministic across thread
counts. Under the control plane, an operator sets it per tenant with
`PATCH /v1/orgs/{org}/resources` (Admin → tenant detail → **Pipeline resources**);
the value lands in the environment the dispatcher injects into that tenant's next
job. Reconciliation is a Python graph step and does not parallelize with threads.

For separate tenants, use separate metadata schemas and object prefixes as well
as distinct configs. Changing `tenant` alone does not provision another lake.

Engine execution settings are recorded in benchmark fingerprints and do not change
`config_hash`. `dedupe-core` keeps its `__dedupe__*` intermediates in the session's
native temp catalog, outside DuckLake, and releases them on completion or failure.

## Training large corpora

The normal config uses exact deterministic-prior estimation, a target of 1,000,000
random pairs for u, no u early stopping, and uncapped EM sessions. u estimation
samples records with a deterministic bucket filter — `hash(record_key || '_' ||
u_seed) % 1000000 < threshold`, where `threshold` comes from the pair budget —
and estimates u over that sample without resampling it, so the same `u_seed` and
budget always select the same records. The record sample is approximate, so its
actual pair count can exceed the target. `training.u_sampling_method` is still accepted
and recorded for `config_hash` stability, but `dedupe-core` implements the single
deterministic-hash path and does not branch on it. All training options are stored
with the model.

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
