# Entities and generation: entity.yaml, the type map, and what lands where

## Contents
- The entity.yaml, field by field
- The eight-type map
- What each generate flag emits, and where
- The generated crate's extras
- The S5 registry (the non-entity lake relations)
- The ERD

## The entity.yaml, field by field

One file per table: `entities/<name>/entity.yaml`.

| Field | Required | Meaning |
|---|---|---|
| `kind` | yes | always `entity` |
| `version` | yes | schema version; bumped only on a breaking change |
| `name` | yes | the entity (directory) name, snake_case singular (`golden_record`) |
| `relation` | yes | the table name (`golden_records`) |
| `description` | yes | prose; the tree doubles as documentation |
| `key` | yes | logical key columns (the lake enforces only NOT NULL; keys are honoured by construction — MERGE/grouping — and E005 forbids assigning them). Identity convention: exactly `[id]` (V2) — `id` is varchar NOT NULL first column (V1/V3), minted as UUIDv7 (`er_ids::new_v7`/`deterministic_v7`), content-derived sha256 (`er_ids::natural_id`) for SQL/CTAS mints, or bigint+`serial` for database-minted ledgers |
| `natural_key` | no | the upsert/MERGE identity — what makes two rows the same thing. Published as a second registry LogicalKey on the lake (and drives the ERD's implied edges); renders `UNIQUE (…)` on the control plane. Columns must be real, NOT NULL, never id/created_at/updated_at (V10); MERGEs must match on exactly this set (E015) |
| `identity` | no | the retrofit ratchet: `legacy` skips V1–V8, E014, E015 and the E005 immutables. Default (absent) = enforced. Deleted entirely once the retrofit lands — never add it to a new entity |
| `generate` | yes | what to emit; an entity with no block is a spec error |
| `columns` | yes | declaration order is load-bearing everywhere. Identity convention: every entity carries `created_at` + `updated_at` (timestamp, NOT NULL; control = `default: now()` + `volatile: true`, lake = run-logical, written by statements, not volatile) (V4–V6); FK columns are named `<singular(relation)>_id` and reference `id` (V7/V8) |

The `generate` block:

| Field | Values | Effect |
|---|---|---|
| `stores` | `duckdb`, `postgres` | which DDL is rendered; also decides the statement store (lake vs control) |
| `languages` | `rust`, `typescript` | which readers/bindings are emitted |
| `wire` | `arrow`, `json` | `arrow` adds `arrow_schema()` + a TS Arrow reader; `json` emits the TS binding only |
| `append` | bool, default false | emits the typed bulk-append primitive (`AppendRow` + `append_rows`: chunked, bound-parameter INSERT..VALUES generated from the columns) — for variable-arity row batches, never hand-built VALUES |

Each column:

| Field | Default | Meaning |
|---|---|---|
| `type` | required | one of the eight types |
| `nullable` | **true** | columns default nullable; NOT NULL is explicit (the opposite of statement params) |
| `description` | required | prose |
| `serial` | false | database-supplied value (Postgres bigserial); E004 treats it as defaulted |
| `default` | none | control-plane SQL default expression |
| `volatile` | false | excluded from determinism comparisons (e.g. `assembled_at`) — still part of the shape and the wire |
| `accepted_values` | none | closed value set; rendered as a CHECK constraint and enforced on literals by E010 |
| `references` | none | control-plane foreign key `{ relation, column }`; drives FK-ordered boot DDL |

## The eight-type map

| yaml | Rust | DuckLake DDL | Postgres DDL | Arrow | TypeScript |
|---|---|---|---|---|---|
| `varchar` | `String` | `VARCHAR` | `text` | `Utf8` | `string` |
| `bigint` | `i64` | `BIGINT` | `bigint` | `Int64` | `number` |
| `double` | `f64` | `DOUBLE` | `double precision` | `Float64` | `number` |
| `boolean` | `bool` | `BOOLEAN` | `boolean` | `Boolean` | `boolean` |
| `date` | `chrono::NaiveDate` | `DATE` | `date` | `Date32` | `string` |
| `timestamp` | `chrono::DateTime<Utc>` | `TIMESTAMP` (naive, UTC by convention) | `timestamptz` | `Timestamp(µs, None)` | `string` |
| `json` | `serde_json::Value` | `JSON` | `jsonb` | `Utf8` (text on the wire) | object |
| `list<varchar>` | `Vec<String>` | logical `LIST(VARCHAR)`, rendered `VARCHAR[]` in DDL | `text[]` | `List(Utf8)` | `string[]` |

`timestamp` is the one per-store divergence (lake naive, control tz-aware).
`list<varchar>` cannot BIND as a lake statement parameter (duckdb-rs limit) —
see the `chr(31)` idiom in [statements.md](statements.md).

## What each generate flag emits, and where

`cd rust && cargo run -q -p er-entity-cli -- gen --root ..` writes, per entity:

| Flag | Output |
|---|---|
| `languages: [rust]` | `rust/crates/er-entities/src/<name>.rs` — struct (+ `KEY`/`VERSION`/`RELATION`/`STORE` consts, `columns()`, `arrow_schema()` when arrow-wire), with the statement query modules appended |
| `stores: [duckdb]` | `rust/crates/er-entities/ddl/lake/<relation>.sql` |
| `stores: [postgres]` | `rust/crates/er-entities/ddl/postgres/<relation>.sql` |
| `languages: [typescript]` + wire | `frontend/src/lib/entities/bindings/<Pascal>.ts`; `readers/<Pascal>.ts` additionally when `wire: [arrow]` |
| (always) | `docs/erd.md` |

These five directories are `er_entity_gen::GENERATED_DIRS`: gen owns every file
in them. `er-entity check` fails on byte-stale files AND on orphans (files gen
did not emit — e.g. a deleted entity's lingering module). Generated Rust is
rustfmt-canonicalised; the generated TS directory is exempt from the frontend's
strict lint, the generated crate from pedantic clippy.

## The generated crate's extras

- `src/schema.rs` — `POSTGRES_BASELINE`: every control table's DDL in
  FK-dependency order; er-server boots only this (never hand DDL).
- `src/batch.rs` — typed Arrow extraction for the `returns: columns` row
  structs: `check_columns` (structural; list child-field names ignored),
  per-type `opt_*` getters, `req` (NULL in a declared non-null column is an
  `Err`, never a panic), and `ser_duckdb_timestamp` (byte-compatible with
  DuckDB `to_json` text).
- `append: true` entities get `AppendRow` + `append_rows` in their module
  (`raw_record` is the model: ingest landing goes through it exclusively).

## The S5 registry (the non-entity lake relations)

Engine-internal lake relations that are not entities (the standardize
intermediates, shared stage tables) are declared in
`rust/crates/er-schema/src/model.rs`, gated byte-exactly by
`tests/model_registry_golden.json`. The registry's renderer also produces the
entity lake DDL, so there is exactly one DDL renderer. "There is no third
place": a persistent table enters through an entity.yaml or the registry —
nothing else ships (see [gates.md](gates.md)).

## The ERD

`docs/erd.md` is generated (Mermaid, all three stores: `public` from the
catalog, `erweb` from the latest drizzle snapshot, `lake.main` from the
registry). Never hand-edit it; it is part of the staleness gate, so any
schema change must re-run gen.
