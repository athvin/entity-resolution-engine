---
name: ducklake-lakehouse
description: Explains how this repo reaches DuckLake from Rust — the ATTACH options, the native-temp-catalog rule for intermediates, no secondary indexes and the two levers that replace them (partitioning and sort-on-write), the maintenance operations and their order, extension loading, session presets, and per-tenant lake isolation. Use when opening or attaching a lake, setting `partition_by` or `order_by` on a lake write, reclaiming space or expiring snapshots, adding a DuckDB extension, debugging a cross-catalog or transaction error, working on object-store or S3 credentials, or reasoning about tenant isolation. For whether a layout change is justified — mechanism here, measurement there — see ducklake-performance; for declaring a lake table or statement see entity-framework; for running stages and maintenance see er-engine-ops; for tenant isolation as an attack surface see security; for the lake bucket and its IAM see infra-platform.
---

# DuckLake from Rust

DuckLake is a lakehouse format: Parquet files in object storage, with a SQL
catalog holding the metadata. DuckDB executes; the catalog is a file locally and
a Postgres database in deployment. Everything in this repo goes through
`dedupe_core::session::Session`.

Three structural facts drive almost every rule below:

1. **DuckLake cannot host temp tables.** All intermediates live in DuckDB's
   native temp catalog; only final outputs go to the lake.
2. **DuckLake has no secondary indexes.** `CREATE INDEX` is rejected.
3. **The lake catalog is never `USE`d.** Every reference is fully qualified
   (`lake.main.golden_records`), so there is no ambient current-catalog state to
   get wrong.

## The attach, and why each option is pinned

`Session::attach_ducklake` runs `INSTALL ducklake; LOAD ducklake;` and then:

```sql
ATTACH 'ducklake:<metadata>' AS <alias> (
  DATA_PATH '<path>',          -- where the Parquet lives
  READ_ONLY,                   -- read paths only
  AUTOMATIC_MIGRATION false    -- always
);
-- read-write attach only:
CALL <alias>.set_option('data_inlining_row_limit', 0);
```

- **`AUTOMATIC_MIGRATION false`** pins the catalog format. Without it a newer
  DuckLake could migrate a tenant's catalog in place, on open, as a side effect
  of a deploy. A catalog format change must be a deliberate, reviewable event.
- **`data_inlining_row_limit = 0`** forces every row to Parquet instead of
  letting small writes live inline in the catalog. That is what makes output
  byte-deterministic, which the golden fixtures depend on.
- **`metadata`** is everything after `ducklake:`, verbatim — `meta.ducklake`,
  `sqlite:meta.db`, or `postgres:dbname=lake host=…`. It is a connection
  string, not a path, and is passed through untouched.

Field-by-field detail and the tenancy discussion:
[references/attach-and-tenancy.md](references/attach-and-tenancy.md).

## The shape of every run

```
lake catalog  --cross-catalog SELECT-->  native temp catalog  --dedicated txn-->  lake catalog
   (inputs)                                (__dedupe__* etc.)                       (outputs)
```

Writes go back in **dedicated single-catalog transactions**. If a write looks
oddly isolated from the reads that produced it, that is deliberate — mixing a
lake write and cross-catalog reads in one transaction is what it avoids.

`Session::write_to_lake` is the plain path; `write_to_lake_with` takes the
physical layout options below.

## There are no indexes — only layout

With `CREATE INDEX` unavailable, the only two levers are physical:

| Lever | What it does | Use for |
|---|---|---|
| `partition_by` | writes one Parquet subtree per distinct value (`col=val/…`) | a column downstream queries *filter* on, with low-to-moderate cardinality |
| `order_by` | sorts rows within each file, tightening Parquet zone-maps | a column queries range-scan or filter on, including high cardinality |

Partitioning on a high-cardinality column is the classic mistake: it produces
an enormous number of tiny files, and small-file overhead costs more than the
scan it saves. `order_by` is the safe default when unsure.

Neither is a correctness mechanism. DuckLake **does not enforce primary-key or
unique constraints** — logical keys are enforced by writers and tests. A layout
change can never fix a duplicate-row bug.

Sizing guidance and the maintenance operations that keep layout healthy:
[references/layout-and-maintenance.md](references/layout-and-maintenance.md).

## Extensions

`SessionOptions.extensions` names extensions to `LOAD`. Two rules:

- Each name is validated as a **bare identifier** (ASCII alphanumeric or `_`)
  before being interpolated into SQL. Keep that check if you touch the code —
  it is the only thing between a config value and `LOAD <arbitrary SQL>`.
- `install_extensions` defaults to **false**, and `INSTALL … FROM community`
  runs only when it is on. Production bakes extension binaries into the image
  and points `extension_directory` at them, because a container that installs
  from the network at startup needs egress and fails closed when the registry
  is unreachable.

The core string metrics (`jaro_winkler_similarity`, `levenshtein`,
`damerau_levenshtein`, `jaccard`) are built in and need no extension. What is
available, and what each extension adds:
[references/extensions.md](references/extensions.md).

## Session presets

| Preset | Setting | For |
|---|---|---|
| `SessionOptions::deterministic()` | `threads = 1` | goldens, parity oracles, and every read path — byte-stable output |
| `SessionOptions::performance(temp_dir)` | threads/memory tuned, explicit spill dir | pipeline runs |

Every in-memory session without an explicit `temp_directory` gets its own spill
directory under the system temp dir, named with the process id, and removes it
on `Drop`. Two sessions therefore never share a spill dir — which matters
because a shared one turns a concurrent run into a silent corruption rather
than an error.

`threads = 1` is not just determinism theatre: parallel DuckDB execution can
reorder rows within a result, and the goldens compare bytes.

## Tenant isolation is physical

Three nouns appear below and they are not interchangeable: an **org** is the
control-plane row that carries the configuration, **tenant** is the engine-side
name for that same subject, and a **dataset** is the unit actually isolated —
one metadata schema plus one data path, several of which can share one org's
database.

A tenant's data plane is separated by **where its lake is**, not by a predicate
in its queries. Lake read statements carry no `org_id` filter, so there is no
second line of defence: if two orgs ever resolve to the same catalog and data
path, it is silent and total cross-tenant exposure.

The org's stored `env` supplies the location:

| Key | Becomes |
|---|---|
| `ER_LAKE_CATALOG` | the post-`ducklake:` metadata connection string |
| `ER_LAKE_DATA_PATH` | `DATA_PATH` |
| `ER_LAKE_CATALOG_SCHEMA` | DuckLake's **`METADATA_SCHEMA`** attach option — the schema of this lake's own catalog tables inside a shared Postgres metadata database; unset = omitted (file catalogs always omit it) |
| `ER_LAKE_METADATA_SCHEMA` | the **user-table** schema (`lake.<schema>.*`), default `main` |

**Read that last row carefully — the name is misleading.** Despite being called
`…_METADATA_SCHEMA`, it sets `DuckLakeConfig.schema`, which is the schema your
tables live in. DuckLake's own `METADATA_SCHEMA` attach option (where DuckLake
keeps its catalog tables) is the *other* key, `ER_LAKE_CATALOG_SCHEMA`. Anyone
reasoning about tenancy from the variable names alone will get this wrong.
Each metadata schema is an independent DuckLake catalog pinned to its own
recorded `DATA_PATH` (re-attach under a different one is refused) — which is
how several datasets share one `er_t_<org>` database as fully disjoint lakes.

**A lake's data path may be `s3://`.** The object-store credential path
exists (#792): an attach whose `DATA_PATH` is an `s3://` URL `LOAD`s the `aws`
extension and emits a session-scoped `CREATE OR REPLACE SECRET` ahead of the
ATTACH. The secret's shape comes from `ObjectStoreConfig`, and there is no
mode flag — the credential's shape is the mode: **no explicit key selects
`PROVIDER CREDENTIAL_CHAIN`** (the pod's own identity in-cluster; the Pod
Identity role must be enrolled for that org’s lake prefix — see infra-platform),
while an explicit
`ER_LAKE_S3_KEY_ID`/`ER_LAKE_S3_SECRET` pair is for MinIO-class stores and CI,
which have no chain to consult. The `ER_LAKE_S3_*` variables are host-level
process env, never org-row env — the credential filter in `er_server::pg`
refuses credential-shaped keys (`*_SECRET`, `*_KEY_ID`, `*_TOKEN`, …) there
with a 422, because an org row is projected into job params and travels
further than process env. Details:
[references/attach-and-tenancy.md](references/attach-and-tenancy.md).

## Common workflows

**Adding a lake read or write.** It is a declared statement, not hand-written
SQL — see the entity-framework skill. Every lake read pins a snapshot (`AT
(VERSION => $snapshot)`), which is why snapshot expiry has to outlive your
longest cursor.

**"Transaction conflict" / "cannot mix catalogs".** Check that the write is in
its own single-catalog transaction and that intermediates were materialized in
the temp catalog rather than the lake.

**A query got slow.** Measure before changing layout. The evidence discipline
and what each profiling artifact means belong to the ducklake-performance
skill; guessing a partition key from an operator name is the failure mode it
exists to prevent.

**Needing an `s3://` lake.** Point `ER_LAKE_DATA_PATH` at the `s3://` URL
and give the host its credential shape — nothing at all in-cluster (the pod
identity is the credential, with the org enrolled in the tenant-runner registry),
the `ER_LAKE_S3_*` process env elsewhere. The
catalog metadata is unchanged: still a Postgres DSN or a local file.
