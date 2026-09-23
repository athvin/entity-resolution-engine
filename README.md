# Entity Resolution Engine

Clean customer data, find duplicate records across sources, and maintain a golden
record for each resolved person. The engine runs as a Python CLI backed by Splink,
dbt, DuckDB and DuckLake, with a PostgreSQL catalog and S3-compatible storage.

The pipeline provides:

- Standardized names, email addresses, phones, addresses and birth dates.
- Probabilistic matching with automatic merge and manual review thresholds.
- Persistent entity IDs, merge redirects, split history, deletions and resurrection.
- Golden records with configurable survivorship and attribute-level lineage.
- Full and incremental processing, steward assertions, run history and recovery.

Each lake holds one tenant and permits one writer at a time. The current product is
a batch engine with a CLI; an API, web review interface and embedding-based
coherence scoring are not implemented.

## Try the complete pipeline

With Docker and Docker Compose available, run from the repository root:

```sh
bash scripts/ci/profile.sh --case smoke
```

This builds the image, generates 1,000 source records, and runs ingestion, cleaning,
training, deduplication, reconciliation and golden record assembly, followed by a
50-record incremental delivery. It validates the outputs and removes its temporary
Compose stack. The command prints the report directory under `artifacts/profile/`.

The processing container uses two CPUs and 6 GiB of memory; allow additional memory
for PostgreSQL and the object store, and at least 4 GiB of free disk for the smoke
run. To keep a development lake and inspect its golden records, follow the
[runbook](docs/runbook.md#development-lake).

Training is explicit: `er run-all` runs ingest → standardize → match → reconcile →
assemble using an existing model. It does not train a model automatically.

## Documentation

| Guide | Use it for |
|---|---|
| [Runbook](docs/runbook.md) | Initial load, incremental deliveries, reviews, recovery and maintenance |
| [Configuration](docs/configuration.md) | Source mappings, matching rules, survivorship and runtime settings |
| [Architecture](docs/architecture.md) | Pipeline stages, data ownership and entity lifecycle |
| [Table relationships](docs/table-relationships.md) | Mermaid diagrams, table purposes and the validated 100,000-record catalog inventory |
| [Performance](docs/performance.md) | Million-record benchmark, measured results and comparisons |
| [Profiling](docs/profiling.md) | Detailed traces, SQL profiles and artifact interpretation |
| [Contributing](CONTRIBUTING.md) | Local setup, tests and CI |
| [Technical specification](DesignDoc.md) | Numbered schemas, algorithms and invariants referenced by tests |

The measured 100k initial load took **84.80 seconds** after the reconciliation
optimization, down from 273.06 seconds in five matched runs per version. See the
[workload and measurement limits](docs/performance.md#measured-results) before
using these numbers for capacity planning.

## Repository

`src/er/` contains the CLI and engine; `dbt/` contains transformation models;
`configs/` contains reference configurations; `fixtures/` and `tests/` define
correctness checks; `benchmarks/` and `scripts/ci/` run measurements and validation.
Generated outputs belong in the ignored `artifacts/` directory.

Licensed under [MIT](LICENSE).
