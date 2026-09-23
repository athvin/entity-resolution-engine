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

## Performance benchmark

**One million records is the standard performance workload.** Run the complete
initial load, including training and golden-record assembly, with:

```sh
make benchmark
```

This uses `configs/default.yaml`, the baseline generator profile and seed 42.
It reports total processing time, each stage's duration, CPU/memory use, and match
quality. `make benchmark BENCHMARK_REPEAT=3` repeats the load for a median;
`make benchmark-workloads` also measures a 10,000-record incremental delivery and
a correction separately. Resource limits fit the local Docker host and are
recorded with the results; see the [performance guide](docs/performance.md).

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

In a [single 1M hard-profile comparison](docs/performance-training-full-load.md),
the experimental 1M EM pair target reduced training from **10.53s to 7.43s** and
the complete initial load from **97.13s to 91.82s**, with no quality regression on
that corpus. The reference configuration retains uncapped EM; these measurements
are specific to the recorded corpus and resource limits.

## Repository

`src/er/` contains the CLI and engine; `dbt/` contains transformation models;
`configs/` contains reference configurations; `fixtures/` and `tests/` define
correctness checks; `benchmarks/` and `scripts/ci/` run measurements and validation.
Generated outputs belong in the ignored `artifacts/` directory.

Licensed under [MIT](LICENSE).
