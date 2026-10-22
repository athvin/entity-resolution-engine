---
name: ducklake-lakehouse
description: Explains how this repo reaches DuckLake from Rust — the ATTACH options and why each is pinned, the native-temp-catalog rule for intermediates, DuckLake's lack of secondary indexes and the two physical levers that replace them (partitioning and sort-on-write), extension loading, session presets, and how a tenant's lake is isolated. Use when opening or attaching a lake, choosing a partition or sort key, adding a DuckDB extension, debugging a cross-catalog or transaction error, working on object-store or S3 credentials, or reasoning about per-tenant lake isolation. For analysing profiling campaigns see ducklake-performance; for declaring a lake table or statement see entity-framework; for running pipeline stages see er-engine-ops.
---

# DuckLake from Rust

DuckLake is a lakehouse format: Parquet files in object storage, with a SQL
catalog holding the metadata. DuckDB executes; the catalog is a file locally and
a Postgres database in deployment. Everything in this repo goes through
`dedupe_core::session::Session`.

Three structural facts drive almost every rule below:

1. **DuckLake cannot host temp tables.** All intermediates live in DuckDB's
   native temp catalog; only final outputs go to the lake.
2. **DuckLake has no secondary indexes.** `CREATE INDEX` is rejected — tables
   are columnar files, not B-trees.
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

A tenant's data plane is separated by **where its lake is**, not by a predicate
in its queries. Lake read statements carry no `org_id` filter, so there is no
second line of defence: if two orgs ever resolve to the same catalog and data
path, it is silent and total cross-tenant exposure.

The org's stored `env` supplies the location:

| Key | Becomes |
|---|---|
| `ER_LAKE_CATALOG` | the post-`ducklake:` metadata connection string |
| `ER_LAKE_DATA_PATH` | `DATA_PATH` |
| `ER_LAKE_METADATA_SCHEMA` | the **user-table** schema (`lake.<schema>.*`), default `main` |

**Read that last row carefully — the name is misleading.** Despite being called
`…_METADATA_SCHEMA`, it sets `DuckLakeConfig.schema`, which is the schema your
tables live in. It is *not* DuckLake's own `METADATA_SCHEMA` attach option
(where DuckLake keeps its catalog tables). Anyone reasoning about tenancy from
the variable name alone will get this wrong.

**A lake on `main` is a filesystem path.** There is no object-store credential
path in the Rust engine today — no `httpfs`/`aws` load, no `CREATE SECRET`, no
`ER_S3_*` reader. A lake at a filesystem path works, which is every test; a
lake at `s3://` cannot currently be reached from Rust. The Python server's
`secret://` indirection was deleted with the rest of it and nothing has
replaced it yet. Do not write code that assumes an `s3://` lake opens, and do
not document one as working. Status and what is in flight:
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

**Needing an `s3://` lake.** It does not work from Rust on `main` yet (see
above). Use a filesystem path, and treat adding the credential path as the
feature it is rather than a config tweak.
