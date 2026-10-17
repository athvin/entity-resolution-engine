# Entity Resolution Engine

A multi-tenant SaaS that cleans customer data, finds duplicate records across
sources, and maintains a golden record for each resolved person. One shared web
application and control plane serve every tenant (a tenant is a client); each
tenant's data is resolved and stored in isolation. The long-term moat is a
source-agnostic resolution engine; the first market is Salesforce deduplication,
where it competes directly with [Cloudingo](docs/research-cloudingo.md).

The engine resolves:

- Standardized names, email addresses, phones, addresses and birth dates.
- Probabilistic matching (Splink) with automatic-merge and manual-review thresholds.
- Persistent entity IDs, merge redirects, split history, deletions and resurrection.
- Golden records with configurable survivorship and attribute-level lineage.
- Full and incremental processing, steward assertions, run history and recovery.

## Architecture at a glance

The system is four layers. One shared web/control-plane layer oversees work for
every tenant; the heavy resolution runs per tenant in an isolated lake.

```mermaid
flowchart TB
  subgraph clients[" "]
    SRC["Salesforce · CSV · S3<br/>(a client's data)"]
  end
  subgraph web["Web & control plane — shared across all tenants (server/, frontend/)"]
    UI["Next.js UI + BFF<br/>login, per-tenant access"]
    API["FastAPI control plane<br/>orgs · jobs · config · read path"]
    CPDB[("Multi-tenant Postgres<br/>orgs · users · jobs · config · audit")]
  end
  subgraph conn["Connector seam — ingress/egress, CRM-agnostic (docs/connectors.md)"]
    C["drop-dir ingress · webhooks egress · Salesforce (planned)"]
  end
  subgraph engine["Resolution engine — single-tenant per lake (src/er/)"]
    RUN["ingest → standardize → match → reconcile → assemble"]
  end
  subgraph lake["DuckLake — one lake per tenant"]
    CAT[("Per-tenant Postgres catalog<br/>snapshots → parquet map")]
    S3[("S3 parquet<br/>s3://…-lake/{tenant}/")]
  end

  SRC <--> C
  UI --> API
  C --> API
  API --> CPDB
  API -->|"enqueue one job per run"| RUN
  RUN -->|"DuckDB + dbt, one writer/lake"| CAT
  RUN --> S3
  API -->|"snapshot-pinned reads (MVCC)"| CAT
```

1. **Web & control plane** (`server/`, `frontend/`) — a single shared FastAPI
   control plane and Next.js UI/BFF. A login belongs to one or more tenants with a
   role; the BFF translates that membership into the right per-tenant credential on
   every request. Access is gated at the tenant level: a login sees the tenants it
   belongs to, and everything within them. A **multi-tenant Postgres** database
   holds the web/config state — orgs, users, memberships, jobs, config versions,
   schedules, webhooks, audit — so configuration is fast and centralized rather than
   per-tenant infrastructure.
2. **Resolution engine** (`src/er/`) — the batch pipeline, run as a library. It is
   single-tenant *per lake* and permits one writer at a time (a Postgres advisory
   lock). The product runs many such lakes behind the one control plane.
3. **DuckLake storage** (per tenant) — the big processing runs on
   [DuckLake](https://ducklake.select): a **dedicated Postgres catalog database per
   tenant** maps table snapshots to **parquet files in S3** under the tenant's own
   prefix. DuckDB executes the SQL and dbt owns the transforms; snapshot/MVCC reads
   let the API serve point-in-time results without ever blocking the writer.
4. **Connectors** (`docs/connectors.md`) — a CRM-agnostic ingress/egress seam. The
   engine never learns about any specific source: connectors land deliveries as
   files and apply merge-backs through a small verb set. Drop-dir ingress and signed
   webhooks exist today; the Salesforce data connector (sync + writeback) is the
   first one being built.

**Tenancy, concretely:** a tenant = a dedicated catalog Postgres database
(`t_{org}_…`) + an S3 prefix, provisioned behind the shared control plane. It is a
namespace, not a row discriminator — no table carries a `tenant` column, and the
advisory writer lock is taken on the tenant's own database, so cross-tenant access
is structurally impossible rather than a per-query concern.

## Status

- **Built and tested:** the resolution engine (full + incremental, review,
  survivorship, golden records); the multi-tenant control plane in `server/` (jobs
  queue with per-tenant serialization, API-key auth + roles, versioned config with
  tiered publish, schedules, the snapshot-pinned read path, steward writes, drop-dir
  imports, signed webhooks, merge-plan export); and the web UI foundations in
  `frontend/`. See [backend design](docs/backend-design.md) and
  [frontend design](docs/frontend-design.md) for what each layer does.
- **In progress:** the Salesforce data connector (ingress + writeback,
  [connectors](docs/connectors.md)); human SSO (direct OIDC in the BFF); and the
  production AWS environment ([infrastructure](docs/infrastructure.md), tracked in
  [plan-to-aws](docs/plan-to-aws.md)). The dev cloud environment is built and
  live-probed; production is gated deliberately.

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
[runbook](docs/runbook.md#development-lake). To stand up the full dev stack
(control plane + UI + a lake) in one command, see `make run`.

Training is explicit: `er run-all` runs ingest → standardize → match → reconcile →
assemble using an existing model. It does not train a model automatically.

## Performance benchmark

**One million records is the standard performance workload.** Run the complete
initial load, including training and golden-record assembly, with:

```sh
make benchmark
```

This uses `configs/default.yaml`, the baseline generator profile and seed 42, and
reports total time, per-stage durations, CPU/memory use, and match quality. The
current medians are **91.70s** for the 1M reload and **39.48s** for a 100K
incremental on the documented local envelope; a 10M reload is ~1h46m. The tuning
history, evidence-linked backlog, and rejected experiments are consolidated in the
[performance record](docs/performance.md), with the profiling campaign and native
query traces described in the [profiling guide](docs/profiling.md) and the
[analysis skill](.agents/skills/ducklake-performance/SKILL.md).

## Documentation

| Layer | Guide | Use it for |
|---|---|---|
| Product | [Backend design](docs/backend-design.md) | The multi-tenant control plane: orgs, jobs, config service, read path, isolation |
| Product | [Frontend design](docs/frontend-design.md) | Cloud UI: personas, access model, screens, BFF, phased plan |
| Product | [Connectors](docs/connectors.md) | The agnostic ingress/egress seam and the Salesforce connector |
| Product | [Cloudingo research](docs/research-cloudingo.md) | Competitor and market analysis driving the parity roadmap |
| Engine | [Architecture](docs/architecture.md) | Pipeline stages, data ownership and entity lifecycle |
| Engine | [ERD](docs/erd.md) | Generated entity-relationship diagram of every store (control plane, erweb, lake); regenerate with `er-entity gen` |
| Engine | [Technical specification](DesignDoc.md) | Numbered schemas, algorithms and invariants referenced by tests |
| Engine | [Configuration](docs/configuration.md) | Source mappings, matching rules, survivorship and runtime settings |
| Ops | [Runbook](docs/runbook.md) | Running the engine: initial load, deliveries, reviews, recovery, maintenance |
| Ops | [Performance](docs/performance.md) / [Profiling](docs/profiling.md) | Benchmark results, measured findings, traces and artifacts |
| Ops | [Infrastructure](docs/infrastructure.md) / [AWS plan](docs/plan-to-aws.md) | AWS/EKS design and the ordered build-out tracker |
| Contributing | [Contributing](CONTRIBUTING.md) | Local setup, tests and CI |

## Repository

`src/er/` contains the CLI and resolution engine; `dbt/` contains transformation
models; `configs/` contains reference configurations; `fixtures/` and `tests/`
define correctness checks; `benchmarks/` and `scripts/ci/` run measurements and
validation. `server/` is the multi-tenant control plane and `frontend/` the web
UI/BFF — each a standalone sub-project with its own lockfile (see `server/README.md`
and `frontend/README.md`). Generated outputs belong in the ignored `artifacts/`
directory.

Licensed under [MIT](LICENSE).
