# Attaching a lake, and how tenants are kept apart

## Contents

- `DuckLakeConfig`, field by field
- The two schemas people confuse
- Where a tenant's location comes from
- Why isolation has no second line of defence
- Object storage: the `s3://` credential path
- In flight

## `DuckLakeConfig`, field by field

| Field | Meaning |
|---|---|
| `metadata` | everything after `ducklake:` in the ATTACH string, **verbatim** — `meta.ducklake`, `sqlite:meta.db`, `postgres:dbname=lake host=…` |
| `data_path` | the `DATA_PATH` attach option: where Parquet files live |
| `metadata_schema` | the `METADATA_SCHEMA` attach option: the schema inside the metadata database where DuckLake keeps **its own** catalog tables. `None` omits the option. How several lakes share one Postgres database as disjoint catalogs; never set for a file catalog |
| `alias` | the catalog alias, default `lake`. Never `USE`d — all references are fully qualified |
| `schema` | the schema **your tables** live in, default `main`, so `lake.main.golden_records` |
| `read_only` | adds `READ_ONLY` to the attach |
| `object_store` | the S3 configuration, consulted only when `data_path` is `s3://` — empty resolves the `ER_LAKE_S3_*` process env at attach time, an explicit config always wins |

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

`ER_LAKE_METADATA_SCHEMA`, despite its name, is read into **(1)**. The field
for (2) is `DuckLakeConfig.metadata_schema`, read from the deliberately
differently-named **`ER_LAKE_CATALOG_SCHEMA`** — the schema inside the shared
metadata database that holds one lake's catalog tables. Each metadata schema is
an independent DuckLake catalog with its own recorded `DATA_PATH`; the first
attach creates the schema, and re-attaching it under a **different**
`DATA_PATH` is refused by DuckLake
(`er-server/tests/pg_lake_catalog_schema.rs` pins all three properties).

If you are reasoning about tenancy, read the code rather than the variable
name. `er-server`'s `read::open_org_lake` is the authority for the read path,
and `schema::open_lake_env` for the write path.

## Where a tenant's location comes from

An org row carries an `env` JSON object. The read path takes four keys out of
it:

| Key | Used as | Default |
|---|---|---|
| `ER_LAKE_CATALOG` | `metadata` | required — absence is an error surfaced as 503 lake-unavailable |
| `ER_LAKE_DATA_PATH` | `data_path` | none |
| `ER_LAKE_CATALOG_SCHEMA` | `metadata_schema` (DuckLake's own catalog tables) | unset — omit the option (file catalogs always omit it) |
| `ER_LAKE_METADATA_SCHEMA` | `schema` (the user-table schema) | `main` |

The read path always opens **read-only** and with the deterministic session
preset.

## Why isolation has no second line of defence

Lake read statements carry **no `org_id` predicate**. There is nothing to
filter on — a tenant's tables are in a tenant's catalog. That design is simple
and fast, and it has one consequence that deserves to be stated plainly:

> If two orgs ever resolve to the same `(catalog, catalog schema, data path)`
> triple, one tenant reads another tenant's data, silently and completely, and
> no query predicate exists that would have caught it.

One edge of that triple is now self-defending: a metadata schema records its
own `DATA_PATH` at creation and DuckLake refuses to re-attach it under a
different one, so a mis-derived data path against an existing catalog surfaces
as an attach error rather than as silent cross-writing. The catalog and schema
derivation remain pure convention — derive them from immutable identifiers.

So the derivation of those three values is a security boundary, not
configuration plumbing. Two orgs differing in any way must differ here, and a
tenant's location must not be something a caller can influence. The security
skill treats this as its headline tenant-isolation invariant.

A related trap: a tenant's lake location should be derived from an immutable
identifier, not from a reusable routing handle like a name. If a name can be
released and re-registered, a new tenant could land on a purged tenant's
prefix.

## Object storage: the `s3://` credential path

Landed in #792. When `data_path` is an `s3://` URL, `attach_ducklake` `LOAD`s
the `aws` extension (baked into the image alongside `httpfs`, so this works
offline in-cluster) and emits one session-scoped secret before the ATTACH:

```sql
CREATE OR REPLACE SECRET er_lake_s3 (TYPE S3, PROVIDER CREDENTIAL_CHAIN);
-- or, with an explicit key pair: (TYPE S3, KEY_ID '…', SECRET '…', …)
```

Three design points, and each is load-bearing:

- **The credential's shape is the mode.** No explicit key →
  `PROVIDER CREDENTIAL_CHAIN`: the pod's own identity in-cluster (the Pod
  Identity role already carries the lake-bucket grants), an instance profile,
  `AWS_*` on a developer machine. An explicit `key_id` + `secret` pair → a
  static secret, for MinIO-class stores and CI, which have no chain to
  consult. One without the other is an error rather than a silent chain
  fallback. A mode flag would be a second thing that can disagree with the
  credential.
- **S3 configuration is host-level process env, never per-org.** The
  `ER_LAKE_S3_*` set (`ENDPOINT`, `REGION`, `URL_STYLE`, `USE_SSL`, `KEY_ID`,
  `SECRET`) resolves inside the attach when `object_store` is empty — one
  mechanism for every lake-open seam, so no seam can forget it. The
  credential filter in `er_server::pg` refuses credential-shaped keys on
  every org/dataset env write (a 422 naming the key): an org row is projected
  into job params and travels further than process env, so nothing
  credential-shaped persists there, ever.
- **The names are NOT the banned pair.** `ER_S3_ACCESS_KEY_ID` /
  `ER_S3_SECRET_ACCESS_KEY` stay banned by `tf-static-credentials-check.sh`
  rule 2 (a credential-shaped name nothing should consume in deployed envs),
  so the gate needed no narrowing.

History: this was a regression from the retired Python implementation — its
`secret://` indirection was deleted with the Python tree and nothing put in
its place — which is why it landed as a feature, not a reversal.

The proving test is `rust/crates/er-cli/tests/s3_lake.rs` (rustfs in the
lake-e2e CI legs; the local recipe is in the file's header): an `s3://` lake
attaches, writes and reads, through both the explicit key and the process-env
fallback.

## In flight

Both halves of the earlier in-flight shape have **landed**: `METADATA_SCHEMA`
(`ER_LAKE_CATALOG_SCHEMA`, the datasets program's per-dataset lake), and the
object-store credential path (#792 — the `ObjectStoreConfig` above and the
credential filter, both described in the section above).

Nothing is currently in flight on the attach surface itself. The follow-on
work is operational: point a real lake at `s3://` (which retires the lake
service user — see the infra-platform skill's iam-and-exceptions reference).
