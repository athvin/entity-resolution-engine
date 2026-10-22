# Research: Atlas, Alembic, and how this repo should migrate schema per tenant

*2026-10-22. Research and a proposal; nothing in §5–§7 is implemented. Companion
to [entity-framework-design.md](entity-framework-design.md) (the declarative
model, and the "Upgrades and API versions" section whose ten rules this doc
builds on), [infrastructure.md](infrastructure.md) §12 (the deploy) and
[backend-design.md](backend-design.md) (tenant lifecycle).*

The question this answers: Atlas is a schema-migration tool with a declarative
front end; this repo already has a declarative schema (`entities/*.yaml`). Do we
need something Atlas-shaped, something Alembic-shaped, or neither — and how does
whatever we pick cope with tenants that sign up at runtime, each with their own
DuckLake, so that every deploy leaves every tenant reconciled?

The short version: **the entity framework already *is* the declarative half of
Atlas, and the design doc already chose the versioned half; what is missing is
the versioned half itself, and a per-tenant fan-out that the deploy drives.**
Atlas the product is the wrong fit (no DuckDB, and the parts we would want are
paid), but four of its ideas are worth copying, and Alembic's one durable idea —
a migration is a reviewable, checksummed file with a journal in the database —
is the thing the current boot-replay model lacks.

## 1. What the code actually does today

Three stores, three unrelated mechanisms. Only one of them is versioned.

### 1.1 The BFF's `erweb` schema: Drizzle, versioned, Helm hook

`frontend/drizzle/` holds three numbered SQL files and their `meta/*_snapshot.json`
journal. `scripts/db-migrate.mjs` is drizzle-orm's migrator over postgres.js,
and the chart runs it as a `pre-install,pre-upgrade` hook Job
(`infra/k8s/charts/er-platform/templates/job-db-migrate.yaml`). Drizzle keeps its
own `__drizzle_migrations` table. This is the Alembic model exactly: ordered
files, a journal, applied before the app rolls. It is also the only store in
the repo whose live state is provably a function of committed files.

### 1.2 The control plane `public.*`: baseline replay at boot

`entities/*.yaml` with `stores: [postgres]` generate `POSTGRES_BASELINE`
(`rust/crates/er-entities/src/schema.rs`): one entry per entity, in FK order,
each `CREATE TABLE IF NOT EXISTS` plus `CREATE INDEX IF NOT EXISTS`, plus the
`er_history()` prelude and history companions. `pg::ensure_schema`
(`rust/crates/er-server/src/pg.rs:54`) replays the whole baseline inside one
transaction under an advisory lock, on every boot of the API and of the
dispatcher, with `er.ddl_log_skip` set so the replay does not spam the DDL
ledger. There is no migrations table and no migration file; the "version" of
the control database is whatever image booted last.

Two consequences, one good and one not:

- **A new table or a new index reaches production by itself.** `IF NOT EXISTS`
  makes the replay converge for those. That is why no PR in the history has ever
  needed a migration step for a new entity.
- **A new column on an existing table does not.** The baseline contains no
  `ALTER TABLE` (zero occurrences), so a nullable column added to an existing
  entity is created on fresh databases and silently absent on every database
  that existed before. The generated query functions name the column, so the
  first statement that touches it fails at runtime, not at deploy. The tool that
  closes the gap exists — `er-entity migrate --store postgres --apply`
  (`rust/crates/er-entity-diff/src/migrate.rs`) reads `information_schema`, plans
  safe `ADD COLUMN`/`CREATE TABLE` statements, refuses breaking ones — but
  nothing in the chart, the init path or CI calls it. It is an operator command
  that an operator has to remember.

Alongside: `er-server migrate` (the binary's one subcommand) applies the baseline
and the event triggers with a master DSN, for the one-off install the `ddl_log`
ledger needs; and `ddl_log` itself records every non-replay DDL with actor,
phase, tag and statement text. The ledger is a good audit trail, but it is a
record of *what happened*, not of *which version the database is at*.

### 1.3 Each tenant's DuckLake: tables appear when the pipeline runs

There is no provisioning DDL for the lake. `provision.rs` creates a directory
(local mode) or a `CREATE DATABASE` for the DuckLake catalog (Postgres mode) and
nothing else; the stages create what they need as they go. Relations have two
owners (`er_schema::model::Owner`): `Ddl` relations are created by
`create_table_sql` as `CREATE TABLE IF NOT EXISTS lake.main.<x>` at the point the
stage first writes them (`raw_records`, `entities`, `match_scores`, `assertions`,
…), and `Stage` relations are rebuilt whole by `CREATE TABLE AS SELECT` on every
run (`int_std_records`, the golden marts). Every lake entity is also emitted as
`rust/crates/er-entities/ddl/lake/<relation>.sql`, but those files are a
generated artifact that `verify` byte-compares; the engine never reads them, and
the test `every_lake_entity_matches_the_registry` is what keeps the hand-built
`er-schema` registry and the YAML equal.

So for a tenant lake:

- A `Stage` relation migrates for free: the next run recreates it with the
  current columns. This is the reason the lake has been able to live without a
  migration story at all.
- A `Ddl` relation does not. `CREATE TABLE IF NOT EXISTS` on an existing table is
  a no-op, exactly as on Postgres, so a column added to `raw_records` or
  `entities` is absent on every tenant provisioned before the change. The
  skill's rule "lake changes must be additive" is a convention, not a check: the
  lake differ (`er-entity-diff/src/lake.rs`, `read_lake_columns`) is written and
  tested in-crate but `er-entity diff --store duckdb` is refused ("only
  `postgres` is wired today"), and no job runs it.
- Nothing records a schema version in the lake. `runs` carries `config_hash`,
  `std_version`, `survivorship_version`, `code_version` — the determinism
  fingerprint that decides whether an incremental run may proceed — but that is
  the *engine's* version, not the *table shape's*.

### 1.4 What the design doc specifies and the code does not have

The design doc's "Upgrades and API versions" section is a complete versioned
design: numbered migration files per store generated by `migrate plan`, a
`schema_migrations` table in `public` and in every `lake.main`, an init
container under `Recreate` replacing `ensure_schema`, every lake job migrating
its lake first under the writer lock, a post-boot sweep, read-path migration of
additive changes, cursor invalidation at a migration, `additive`/`breaking`
image labels, and a baseline so new tenants skip the history. None of that is
built. The doc also says, under that heading, "Atlas is not used", for two
reasons — no DuckDB or DuckLake, drift detection is paid — that §2 confirms are
still true. The remainder of this doc is what to build, and in which order, now
that tenants provision themselves (#257) and the per-tenant fan-out is no longer
hypothetical.

## 2. Atlas, read in this repo's terms

Atlas (Ariga, Go, v1.x) ships two workflows over one engine. Everything below is
from the official docs as of October 2026; sources are at the end.

**Declarative.** `atlas schema apply --to <desired> --url <db>`: inspect the live
database, diff it against the desired state (HCL, SQL, an ORM loader, or another
database), print a plan, apply on approval. That is `er-entity diff` plus
`er-entity migrate --apply`, with YAML as the desired state. Atlas's planner is
broader (it will plan drops, renames with `--auto-approve`, and views/triggers in
the paid build); this repo's planner is deliberately narrower — safe operations
only, `Severity::Breaking` refused — which is the right posture for a store you
cannot rebuild.

**Versioned.** `atlas migrate diff <name> --to <desired> --dir file://migrations
--dev-url <scratch db>`: replay the migration directory onto an empty scratch
database, diff the result against the desired state, write the delta as a new
timestamped SQL file. `atlas migrate apply` runs pending files one transaction
each and records them in `atlas_schema_revisions` (version, description,
statement counts, execution time, error text, hash, operator version). An
`atlas.sum` file carries a checksum per migration and one for the directory, and
`apply` refuses to run if the directory and the sum file disagree, so a
hand-edited migration is caught before it reaches a database. This is the
design doc's `migrate plan` → numbered files → `schema_migrations`, with two
additions worth taking: the **checksum** and the **error row**.

**The dev database.** Atlas insists on a real scratch database of the right
engine and version to normalise and validate schemas, because only the engine
can compile every expression. This repo already has that idea under another
name: the oracle in `er-entity verify` runs `DESCRIBE` on DuckDB and
`prepare_typed` on the CI Postgres for every declared statement. The one place
the idea is *not* applied is migrations themselves: nothing today proves that
"committed migrations replayed onto an empty store equal the baseline", because
there are no committed migrations.

**Linting.** `atlas migrate lint` classifies each migration file: destructive
(drop schema/table/column), data-dependent (adding `NOT NULL` or a unique index
on populated tables — may succeed or fail), backward-incompatible (renames),
naming, non-linear history, dialect-specific lock and rewrite hazards. Since
v0.38 (2025-10-28) the command is paid in the standard binary, though the
analyzers remain Apache-2 in the Community build. This repo's `Severity` enum
is the destructive + incompatible analyzers; the data-dependent class is the one
we do not have and should (a `NOT NULL` column added without a default is
"breaking" in the differ, which is the same answer by a blunter rule).

**Drift.** `atlas schema diff --from file://migrations --to <db>` is free and is
exactly `er-entity diff`. `atlas migrate drift` (2026-09) and the Cloud
"schema monitoring" agent that runs it on a schedule are paid.

**Multi-tenancy.** An `env` block takes `for_each` over a data source — the
documented pattern is `data "sql"` returning tenant schema names or URLs — and
`atlas migrate apply --env prod` then runs against every tenant, reporting each.
A `deployment`/`rollout` block adds waves: canary tenants first, then the rest,
`parallel` N, `on_error = FAIL|CONTINUE`. The per-target lock name that real
parallelism on schema-per-tenant needs is paid. This is the Atlas feature the
question is really about, and it is worth noticing how little it is: a loop
over a query result, a per-target journal, and a report. The repo's equivalent
data source is `SELECT name, env FROM orgs WHERE state = 'active'`.

**Kubernetes.** Atlas's own init-container and Helm-hook guides are now marked
"no longer recommended" in favour of the Atlas Operator (`AtlasSchema` and
`AtlasMigration` CRDs, paid, not in the Community build), which reconciles a CR
holding the desired state or a migration-directory tag against the database and
exposes health that Argo CD waits on before syncing the app. The principles the
guides state survive the deprecation and match the design doc: migrations run
as a discrete step before the new app version; one runner at a time; a failed
migration halts the deployment; running DDL at app start is discouraged because
replicas race and crash-loop. The design doc's init container under `Recreate`
satisfies all four at one replica.

**What Atlas does not do.** It does not speak DuckDB or DuckLake, has no plugin
mechanism for drivers (custom builds come from Ariga), and the useful operations
beyond diff/apply — lint, checkpoint, drift, the operator — are paid. The lake
is where most of this repo's schema lives (20 lake entities against 12 control
ones, by the generated DDL directories), so adopting Atlas would mean adopting
it for the smaller half and building the larger half anyway.

## 3. Alembic, in the same terms

Alembic is versioned-only. `alembic revision --autogenerate` compares the
SQLAlchemy metadata to a live database and writes a candidate Python file with
`upgrade()`/`downgrade()`; each file names its `down_revision`, which is how the
tool orders them; the first `upgrade` creates `alembic_version` and stores the
current head. Branches arise when two files share a parent; `alembic merge`
writes a file whose `down_revision` is a tuple.

What it gets right, and what the current repo lacks: a migration is a file a
reviewer reads in the PR, the database carries its own position in the history,
and a data migration (backfill a column from another) is ordinary code in the
same file as the DDL.

What it gets wrong for this repo:

- **Autogenerate is advisory.** The docs say it "is not intended to be perfect":
  it cannot detect renames of tables or columns, anonymous constraints, or some
  type changes, and the generated file "must be reviewed and edited by hand".
  That is tolerable when the model is the ORM, which is also the thing the
  application reads. Here the model is the YAML, and every consumer is
  generated from it, so a migration that is *also* a hand-maintained artifact is
  a second source of truth of exactly the kind the entity framework exists to
  eliminate.
- **Multi-tenancy is "DIY".** The cookbook recipe reads `-x tenant=<schema>`,
  sets `search_path` on the connection, overrides the dialect's default schema
  (a workaround the docs themselves call "non-supported" and "interim"), and
  leaves looping over tenants to the caller. Nothing records which tenants are
  at which revision except each tenant's own `alembic_version`.
- **DuckDB and DuckLake.** `duckdb_engine` gives SQLAlchemy a DuckDB dialect, but
  Alembic's DDL against a DuckLake catalog is untested territory, and the lake's
  eight-type vocabulary with `NOT NULL` as the only constraint needs almost none
  of Alembic's machinery.

So the honest answer to "I'm used to Alembic, is that a good way": the *shape*
(files, journal, forward-only, applied by the deploy) is right and is what the
design doc already specifies. The *authoring model* (write or hand-correct the
file) is wrong here, because the YAML is the truth and the migration should be
derived from it — which is Atlas's `migrate diff`, not Alembic's `autogenerate`.

## 4. Side by side

| | Atlas | Alembic | This repo today | Design doc target |
|---|---|---|---|---|
| Desired state | HCL / SQL / ORM / live DB | SQLAlchemy metadata | `entities/*.yaml` | same |
| Authoring a migration | `migrate diff` derives it from desired state; reviewed, not edited | `autogenerate` drafts it; hand-edited | none (`public`: boot replay; lake: implicit) | `migrate plan` derives it |
| Journal in the DB | `atlas_schema_revisions` per target, with hash and error | `alembic_version` per target (head only) | none (`ddl_log` is an audit, not a position) | `schema_migrations` per store |
| File integrity | `atlas.sum` checksums; `apply` refuses on mismatch | none | n/a | not specified |
| Drift check | `schema diff` (free), `migrate drift` + Cloud (paid) | none built in | `er-entity diff --store postgres` (manual) | the differ at boot and job start |
| Breaking classification | lint analyzers (paid command) | none | `Severity::Breaking` in the differ | same, plus image labels |
| Multi-tenant | `for_each` over a data source, waves, per-tenant report | caller's loop | nothing | job-start migrate + sweep + read path |
| Kubernetes | Operator (paid); init container / hook guides deprecated | caller's choice | Drizzle hook only; `public` at boot | init container under `Recreate` |
| DuckDB / DuckLake | unsupported, no plugin path | dialect exists; DuckLake untested | native | native |
| Cost | free core; lint/drift/operator/checkpoint paid | free | — | — |

## 5. The assumption, tested

> "My assumption is that it would create some sort of migration reconciliation
> thing."

It would, and the repo needs one, but it is two things rather than one, and the
useful lesson from Atlas is to keep them separate:

1. **Versioned artifacts** derived from the declarative source at PR time —
   numbered, checksummed files that CI proves reproduce the baseline. These give
   a reviewable diff, a position a database can record, a label a release can
   carry, and an answer to "what did tenant X get and when".
2. **Reconciliation** at deploy and at runtime: for each target (one `public`,
   N lakes), compare the recorded position with the image's, apply the pending
   additive files under that target's lock, record each one, report.

Boot replay today conflates the two: it re-asserts the desired state with no
record of what changed, which is why it cannot add a column and cannot say which
tenant is where. Alembic has (1) without a generator and leaves (2) to you.
Atlas has both and sells the fan-out, the lint and the drift schedule. The
design doc's ten rules are (1) + (2) built in-house, and the decision to build
rather than adopt stands on DuckDB alone.

## 6. Proposal: per-tenant registration, and reconciliation on every deploy

This fills in the design doc's rules with the mechanics the tenant-provisioning
work now makes concrete. The pieces are small and each lands on its own.

### 6.1 Two `schema_migrations` entities, declared like any other table

Declare `entities/schema_migration/entity.yaml` with `stores: [postgres, duckdb]`
so the same shape exists as `public.schema_migrations` and
`lake.main.schema_migrations` and the journal is itself governed by the
framework (V-rules, generated struct, typed statements). Columns, following
Atlas's revision table rather than Alembic's single row:

| column | type | note |
|---|---|---|
| `id` | varchar | the migration's version string, e.g. `20261022T1530_add_middle_name` |
| `checksum` | varchar | sha256 of the file as applied; the `atlas.sum` idea, one row at a time |
| `applied_at` | timestamp | |
| `applied_by` | varchar | the image's `code_version` (already stamped on `runs`) |
| `snapshot_id` | bigint, nullable | lake only: the DuckLake snapshot after apply, for the cursor rule (design rule 8) |
| `error` | varchar, nullable | the failure text when a file failed mid-way, so the next attempt and the operator see it |

Timestamp-prefixed names rather than the design doc's sequence numbers: two
PRs that each add `0007_` conflict only at merge, after both passed CI;
timestamps plus a committed checksum file (`ddl/<store>/migrations/SUM`) that
`verify` recomputes make the conflict visible in the PR. Ordering is lexical on
the name; "behind" is "has rows for a prefix of the committed list, with
matching checksums"; a mismatched checksum is a hard stop, as in Atlas.

### 6.2 Generation and the gate

`er-entity migrate plan` grows a `--write` mode that, instead of printing
statements, replays the committed migration files onto an empty store (CI's
Postgres; an in-memory DuckLake for the lake, which the lake e2e already builds),
diffs that against the catalog, and writes the delta as a new file. That is
Atlas's `migrate diff` with the oracle's databases as the dev database.
`er-entity verify` gains one check per store: *replaying every committed
migration onto an empty store produces the baseline*. A YAML change without a
migration fails `make check` with "N migration(s) missing; run `er-entity
migrate plan --write`", in the same place a stale struct does today.

The lake needs `--store duckdb` wired in the CLI; `read_lake_columns` already
exists. The planner for the lake handles `Ddl` relations only — a `Stage`
relation's migration is the empty file, because the next run's CTAS carries the
new shape — so the plan writer should consult `er_schema::model::Owner` and
emit a comment rather than DDL for those.

### 6.3 Control plane: the init container

Replace boot replay with `er-server migrate` as an init container on the
`web-api` pod, under the `Recreate` strategy the chart already uses. It applies
pending `public` migrations one transaction each, under the existing advisory
lock, recording each row; a failure records the `error` column, exits non-zero,
and the pod never starts — the Atlas principle "a failed migration halts the
deployment". Keep the baseline as the path for an *empty* database (fresh dev
stack, CI): if `schema_migrations` is absent, apply the baseline and insert
every committed migration as applied, which is Atlas's `--baseline` for a
database the generator provisioned. The event-trigger install stays a
documented one-off with the master DSN, exactly as now.

The Drizzle hook can stay a hook for now; it only touches `erweb`, which Rust
never reads. Folding it into the same init container is the design doc's open
question and is not blocking.

### 6.4 Tenants: register, reconcile, report

The tenant's lake is authoritative for its own position (a lake restored from a
backup is at the backup's version, whatever the control plane remembers), so
the `lake.main.schema_migrations` rows are the record, and the control plane
holds a cache for reporting: two nullable columns on `orgs`,
`lake_schema_version` and `lake_schema_checked_at`, written whenever anything
migrates or checks a lake.

Three places reconcile, in the order the design doc gives them, and the chart
drives the one that makes "every deploy reconciles every tenant" true:

1. **Every lake job migrates first** (rule 5). `run_job` in the dispatcher opens
   the lake, takes the writer lock it already takes, and calls the same
   `migrate_lake(session)` the others use before any stage runs. Correctness
   never depends on the sweep.
2. **The deploy fans out** (rule 6, made explicit). A `post-upgrade` Helm hook
   Job runs `er-server migrate-tenants`: `SELECT name, env FROM orgs WHERE state
   = 'active'`, and for each, try-lock the tenant's writer lock; if held, skip
   and report `busy` (rule 1 will catch it); otherwise apply pending files,
   record them, update the `orgs` cache, report `migrated N` or `current`. It
   prints one line per tenant, exits non-zero only on an *error* (not on
   `busy`), and the chart keeps the failed Job for `kubectl logs` as the Drizzle
   hook does. Post-upgrade rather than pre-upgrade because the files come from
   the new image, and because every auto-applied change is additive, so the old
   and new code both tolerate either state — the reason the design doc's
   objection to a pre-upgrade hook does not apply here. This is Atlas's
   `for_each` + `rollout` with `on_error = CONTINUE`, for free. Suspended orgs
   are skipped and migrate on resume by rule 1.
3. **The read path** (rule 7) applies additive migrations under a try-lock when
   it finds a lake behind, and answers 503 `lake_unavailable` while a job holds
   the lock.

**New tenants** (rule 10): provisioning gains a step after the lake is
reachable — apply the lake baseline DDL for every `Ddl` relation and insert
every committed migration as applied. Today the first job creates the tables; a
tenant must not be marked `active` with a lake that has no `schema_migrations`
table, or the sweep will read it as "empty, apply from the beginning" against
tables a later CTAS will have shaped differently.

**Drift as a schedule, not a product.** The same `migrate-tenants` binary with
`--check` runs the lake differ for every active org, writes the cache columns,
and exits non-zero naming any tenant whose live columns differ from the
catalog. A CronJob in the chart (daily is enough) is the free equivalent of
Atlas Cloud's drift monitoring, and `GET /v1/admin/schema` returns the `orgs`
cache so the admin UI can show every tenant's version against the image's
without opening twenty lakes.

### 6.5 Breaking changes and rollback

Unchanged from the design doc, now with the journal to hang them on: a
breaking lake change bumps the entity `version`, is refused by the planner, and
ships as an explicit `rebuild` job per tenant that the same fan-out enqueues;
`images.yml` labels the image `additive` or `breaking` from the classification
of the migrations it carries, and the deploy refuses an image rollback across a
`breaking` release. An `additive` rollback serves a lake that is ahead, because
no statement uses `SELECT *` (E011), and the journal says so explicitly: the
old image finds rows it does not know and reads "ahead", not "corrupt".

## 7. Order of work

Each step is a PR that `make check` gates on its own.

1. `schema_migration` entity, both stores; `--store duckdb` in the CLI; the
   `Owner`-aware lake planner.
2. `migrate plan --write`, the `SUM` file, and the `verify` replay check. From
   here a YAML change without a migration cannot merge.
3. `er-server migrate` applies files and records rows; init container in the
   chart; boot replay deleted (`ensure_schema` becomes the empty-database
   baseline path).
4. `migrate_lake` at job start; provisioning writes the baseline journal.
5. `er-server migrate-tenants` and the post-upgrade hook; the `orgs` cache
   columns; `--check` and the CronJob; the admin endpoint.
6. Image labels and the rollback gate in CD.

Steps 1–3 are the control plane's missing `ADD COLUMN` and are the ones with a
latent production bug behind them (§1.2). Steps 4–5 are the tenant fan-out this
doc was asked for. Step 6 is the design doc's rule 9 and can wait for a prod
environment to exist.

## 8. Open questions

- **Hand-written breaking migrations on `public` before prod exists.** The
  design doc allows a person to write one and the differ to verify it. Until
  there is a production database, a breaking control-plane change could instead
  be "drop and re-baseline" in dev. Worth deciding before step 3 so the applier
  does not grow a path nobody uses.
- **Whether the Drizzle migrations move into the init container** (design doc
  open question). Two journals in one database is tolerable; two hook
  mechanisms with different failure behaviour is less so.
- **Parallelism in the fan-out.** Serial is fine at tens of tenants; at
  hundreds, `migrate-tenants --parallel N` with the per-tenant lock already
  providing isolation is the Atlas `rollout` block, and needs nothing paid.

## Sources

- This repo: `rust/crates/er-server/src/pg.rs` (`ensure_schema`),
  `rust/crates/er-entities/src/schema.rs` (`POSTGRES_BASELINE`),
  `rust/crates/er-entity-diff/src/{lib,migrate,lake,postgres}.rs`,
  `rust/crates/er-entity-cli/src/main.rs` (`Diff`, `Migrate`),
  `rust/crates/er-server/src/{provision,dispatcher}.rs`,
  `rust/crates/er-schema/src/model.rs` (`Owner`, `create_table_sql`),
  `entities/{ddl_log,org,run}/entity.yaml`, `frontend/drizzle/`,
  `frontend/scripts/db-migrate.mjs`,
  `infra/k8s/charts/er-platform/templates/{job-db-migrate,deployment-web-api}.yaml`,
  `docs/entity-framework-design.md` ("Schema diff", "Upgrades and API versions").
- Atlas: [declarative vs versioned](https://atlasgo.io/concepts/declarative-vs-versioned),
  [migrate diff](https://atlasgo.io/versioned/diff),
  [migrate apply and `atlas_schema_revisions`](https://atlasgo.io/versioned/apply),
  [directory integrity (`atlas.sum`)](https://atlasgo.io/concepts/migration-directory-integrity),
  [dev database](https://atlasgo.io/concepts/dev-database),
  [schema diff](https://atlasgo.io/declarative/diff),
  [migrate drift](https://atlasgo.io/changelog/migrate-drift),
  [lint analyzers](https://atlasgo.io/lint/analyzers),
  [v0.38: lint leaves the free plan](https://atlasgo.io/blog/2025/10/28/v038-analyzers-pii-and-migration-hooks),
  [features by tier](https://atlasgo.io/features), [pricing](https://atlasgo.io/pricing),
  [community edition](https://atlasgo.io/community-edition),
  [supported databases](https://atlasgo.io/databases),
  [custom drivers](https://atlasgo.io/custom-drivers),
  [project files and `for_each`](https://atlasgo.io/atlas-schema/projects),
  [database-per-tenant: deploying](https://atlasgo.io/guides/database-per-tenant/deploying),
  [database-per-tenant: rollout](https://atlasgo.io/guides/database-per-tenant/rollout),
  [Kubernetes operator: declarative](https://atlasgo.io/integrations/kubernetes/declarative),
  [Kubernetes operator: versioned](https://www.atlasgo.io/integrations/kubernetes/versioned),
  [operator v0.8](https://atlasgo.io/blog/2026/10/01/atlas-operator-v0-8),
  [deploying: principles](https://atlasgo.io/guides/deploying/intro),
  [init container (deprecated)](https://atlasgo.io/guides/deploying/k8s-init-container),
  [Helm hook (deprecated)](https://atlasgo.io/guides/deploying/helm),
  [Argo CD flow](https://atlasgo.io/guides/deploying/k8s-argo),
  [Atlas vs Alembic (vendor)](https://atlasgo.io/guides/atlas-vs-alembic).
- Alembic: [tutorial](https://alembic.sqlalchemy.org/en/latest/tutorial.html),
  [autogenerate and its limits](https://alembic.sqlalchemy.org/en/latest/autogenerate.html),
  [branches](https://alembic.sqlalchemy.org/en/latest/branches.html),
  [cookbook: multi-tenant](https://alembic.sqlalchemy.org/en/latest/cookbook.html).
