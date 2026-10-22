# Attaching a lake, and how tenants are kept apart

## Contents

- `DuckLakeConfig`, field by field
- The two schemas people confuse
- Where a tenant's location comes from
- Why isolation has no second line of defence
- Object storage: not wired up on `main`
- In flight

## `DuckLakeConfig`, field by field

| Field | Meaning |
|---|---|
| `metadata` | everything after `ducklake:` in the ATTACH string, **verbatim** — `meta.ducklake`, `sqlite:meta.db`, `postgres:dbname=lake host=…` |
| `data_path` | the `DATA_PATH` attach option: where Parquet files live |
| `alias` | the catalog alias, default `lake`. Never `USE`d — all references are fully qualified |
| `schema` | the schema **your tables** live in, default `main`, so `lake.main.golden_records` |
| `read_only` | adds `READ_ONLY` to the attach |

`metadata` being a connection string rather than a path is worth internalising:
the same field holds a filename locally and a Postgres DSN in deployment, and
it is passed through untouched. There is no parsing or validation of it on the
way in.

Always set on every attach: `AUTOMATIC_MIGRATION false`. Additionally on a
read-write attach: `CALL <alias>.set_option('data_inlining_row_limit', 0)`.
Both are explained in the SKILL body — format pinning and byte-determinism
respectively.

## The two schemas people confuse

DuckLake has two distinct schema concepts, and this repo's environment variable
naming actively works against telling them apart:

1. **The user-table schema** — where your tables are. `DuckLakeConfig.schema`.
   This is what every statement qualifies with.
2. **DuckLake's `METADATA_SCHEMA` attach option** — where DuckLake keeps *its
   own* catalog tables.

`ER_LAKE_METADATA_SCHEMA`, despite its name, is read into **(1)**. On `main`,
`DuckLakeConfig` has no field for (2) at all, so the attach never sets
`METADATA_SCHEMA` and DuckLake uses its default.

If you are reasoning about tenancy, read the code rather than the variable
name. `er-server`'s `read::open_org_lake` is the authority for the read path.

## Where a tenant's location comes from

An org row carries an `env` JSON object. The read path takes three keys out of
it:

| Key | Used as | Default |
|---|---|---|
| `ER_LAKE_CATALOG` | `metadata` | required — absence is an error surfaced as 503 lake-unavailable |
| `ER_LAKE_DATA_PATH` | `data_path` | none |
| `ER_LAKE_METADATA_SCHEMA` | `schema` (the user-table schema) | `main` |

The read path always opens **read-only** and with the deterministic session
preset.

## Why isolation has no second line of defence

Lake read statements carry **no `org_id` predicate**. There is nothing to
filter on — a tenant's tables are in a tenant's catalog. That design is simple
and fast, and it has one consequence that deserves to be stated plainly:

> If two orgs ever resolve to the same `(catalog, data_path, schema)` triple,
> one tenant reads another tenant's data, silently and completely, and no query
> predicate exists that would have caught it.

So the derivation of those three values is a security boundary, not
configuration plumbing. Two orgs differing in any way must differ here, and a
tenant's location must not be something a caller can influence. The security
skill treats this as its headline tenant-isolation invariant.

A related trap: a tenant's lake location should be derived from an immutable
identifier, not from a reusable routing handle like a name. If a name can be
released and re-registered, a new tenant could land on a purged tenant's
prefix.

## Object storage: not wired up on `main`

State of the tree today:

- No `INSTALL httpfs` / `LOAD httpfs`, no `aws` extension load.
- No `CREATE SECRET` of any kind.
- No reader for `ER_S3_*` settings.

A lake at a **filesystem path** works — that is what every test uses, and what
a development lake is (a directory; DuckLake keeps its catalog in the
`.ducklake` file and its Parquet under the data path). A lake at **`s3://`
cannot be reached** from the Rust engine.

This is a regression from the retired Python implementation, not a deliberate
omission: that server resolved a `secret://` indirection in its own secrets
module, and it was deleted along with the rest of the Python tree with nothing
put in its place.

Practical consequences:

- Do not write or review code on the assumption that an `s3://` lake opens.
- Do not document S3 as supported, in a skill or anywhere else.
- Treat adding the credential path as a feature with a design, not as a config
  value someone forgot to set.

## In flight

A branch adds the missing object-store path and changes the tenancy model. It
is **not on `main`**, so nothing above assumes it. Expected shape, recorded here
so this file can be updated rather than rediscovered:

- An object-store config whose fields are all optional, producing a
  `CREATE OR REPLACE SECRET`, where the **absence of an explicit key selects
  the credential chain** (the pod's own identity in-cluster) rather than a mode
  flag choosing it. An explicit key is for MinIO and CI, which have no chain to
  consult.
- A per-tenant DuckLake **`METADATA_SCHEMA`** becoming the real tenant
  boundary — each metadata schema is an independent DuckLake catalog with its
  own recorded `DATA_PATH`, so many tenants can share one Postgres metadata
  database and still own separate prefixes. Re-attaching a metadata schema with
  a different `DATA_PATH` is refused by DuckLake, and that refusal is the
  guarantee: a tenant's data location is pinned by its own catalog.
- A credential filter so nothing credential-shaped is ever persisted into an
  org row, because an org row is projected into job params and travels further
  than process env does.

When that merges, update this file and the SKILL body's tenancy section
together; the `…_METADATA_SCHEMA` naming trap above is what it resolves.
