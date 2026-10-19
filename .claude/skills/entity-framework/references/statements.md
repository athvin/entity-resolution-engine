# Declared statements: the yaml schema, the E0xx rules, the idioms

## Contents
- The statement yaml, field by field
- Real examples from the tree
- The E0xx rules and their remedies
- SQL idioms that plan and bind
- What the emitter generates

## The statement yaml, field by field

Every statement is a `.sql` + `.yaml` pair under
`entities/<entity>/{create,read,update,delete}/<name>.{sql,yaml}`. The folder
is checked against the SQL's verb (E012): `read` holds SELECT and COPY TO;
`create` holds INSERT, COPY FROM and a MERGE that inserts; `update` holds
UPDATE and a MERGE; `delete` holds DELETE.

| Field | Required | Meaning |
|---|---|---|
| `kind` | yes | always `statement` |
| `operation` | yes | `create`/`read`/`update`/`delete`; must match the folder |
| `description` | yes | non-empty prose — the tree doubles as documentation |
| `params` | default `[]` | `{ name, type, nullable }`; **params default `nullable: false`** (an optional filter says `nullable: true` explicitly) |
| `returns` | yes | `entity` (the full column set, in order), `none` (a write), or a typed column list `{ name, type, nullable }` — **result columns default `nullable: true`** (non-null is an opt-in claim the analyser must prove and the generated `from_batches` enforces at runtime) |
| `mode` | default `full` | `partial` skips only the DataFusion *planner* (for MERGE, ON CONFLICT, SKIP LOCKED, DuckDB-only functions); parsing and the AST rules (E004/E005/E009/E010/E011/E012) still run, and the oracle supplies the output shape |
| `pin` | default `snapshot` | `live` opts a lake read out of the snapshot pin; requires `pin_reason`; rejected anywhere but a lake read |
| `pin_reason` | with `pin: live` | the written justification; forbidden otherwise |
| `reads` | default `[]` | entity NAMES (not relations) this statement reads beyond its own — enforced by E009, consumed by the differ's impact report |

Types are the closed eight-type vocabulary: `varchar`, `bigint`, `double`,
`boolean`, `date`, `timestamp`, `json`, `list<varchar>`. A ninth type is a
spec error, not a mapping to add.

## Real examples from the tree

Snapshot-pinned read with typed returns —
`entities/entity_membership/read/member_keys.{yaml,sql}`:

```yaml
kind: statement
operation: read
description: "The member record keys of one entity at a pinned snapshot, ascending (the unmerge precheck and the detail member list)."
params:
  - { name: snapshot, type: bigint }
  - { name: entity_id, type: varchar }
returns:
  - { name: record_key, type: varchar, nullable: false }
```
```sql
SELECT record_key
FROM lake.main.entity_membership AT (VERSION => $snapshot)
WHERE entity_id = $entity_id
ORDER BY record_key
```

Live read — `entities/resolved_entity/read/count_live.yaml`:

```yaml
kind: statement
operation: read
description: "How many entities have ever been minted, read live (the id-factory floor, so new ids never collide with persisted ones)."
pin: live
pin_reason: "er-cli composes this read inside the writer's own transaction; there is no committed snapshot to pin."
params: []
returns:
  - { name: n, type: bigint }
```

Partial-mode MERGE write — `entities/entity_membership/update/upsert_from_src.yaml`
(the SQL merges a session staging temp; temps are referenced unqualified and
are exempt from E009):

```yaml
kind: statement
operation: update
mode: partial
description: "MERGE the run's membership plan (the session's __mem_src staging) on the record identity."
params:
  - { name: assigned_at, type: varchar }
  - { name: run_id, type: varchar }
returns: none
```

Control (Postgres) statement — `entities/job/update/claim.yaml`: `mode:
partial` (FOR UPDATE SKIP LOCKED), `returns: entity` with an explicit 19-column
RETURNING list in the SQL (E011 in practice).

Cross-entity read — `entities/assertion/create/from_review.yaml` declares
`reads: [review]` because its INSERT..SELECT reads `lake.main.review_queue`.

## The E0xx rules and their remedies

| Rule | Checks | When it fires, do this |
|---|---|---|
| E001 | the statement parses and plans against the store catalog | fix the SQL, or declare `mode: partial` if DataFusion's *planner* genuinely cannot take it (MERGE, ON CONFLICT, DuckDB-only functions) — never to silence other errors |
| E002 | every `$param` is declared with a compatible type; **no `list<varchar>` param on the lake** | declare the param; for lake lists use the `chr(31)` idiom below |
| E003 | output = declaration: names, order, compatible types, nullability no looser than declared (both `returns: entity` and typed columns) | project exactly the declared columns; if the plan can't prove non-null (outer join, expression, aggregate), declare the column nullable |
| E004 | an INSERT (incl. MERGE insert arms) supplies every NOT NULL column without a `default`/`serial`; positional inserts by arity | name the missing column or supply every column positionally |
| E005 | assigned columns exist; **a key column is never assigned** | never rewrite identity — delete-and-insert if the key must change |
| E006 | lake reads carry `AT (VERSION => $snapshot)` XOR declare `pin: live` + `pin_reason` | pin it; go `live` only for the writer's own in-transaction reads, and say why |
| E009 | qualified references resolve to same-store entities; foreign entities are named under `reads:` | add the entity NAME to `reads:`; cross-store references are never allowed |
| E010 | a string literal meeting an `accepted_values` column (compared, assigned, or inserted) is in the set | use a value from the entity's declared set |
| E011 | no `SELECT *` anywhere (projections, subqueries, CTEs) | name every column, so an added column can never change a shape silently |
| E012 | the SQL's verb matches its folder | move the file or change the statement |
| E007 / E008 / E013 | documented in the design doc, deliberately not implemented yet (control WHERE-presence, COPY TO, the `functions/` yaml tree) | nothing — do not claim them in reviews |

Partial-mode statements get no plan, so E003's type proof is replaced by the
oracle: `er-sql-check/tests/oracle.rs` DESCRIBEs every lake read against a real
DuckDB built from the generated DDL.

## SQL idioms that plan and bind

- **Optional filter**: declare the param `nullable: true` and write
  `($after IS NULL OR entity_id > $after)`. `LIMIT $limit` binds fine.
- **Variable-arity IN list**: one `varchar` param of `chr(31)`-joined values:
  `WHERE record_key IN (SELECT unnest(string_split($keys, chr(31))))` —
  requires `mode: partial`; the caller joins with `'\u{1f}'`. (duckdb-rs cannot
  bind LIST values; E002 enforces this.)
- **Snapshot value**: callers pin `session.latest_snapshot_version()` (or a
  keyset cursor's embedded snapshot, so pages stay stable against a moving lake).
- **Snapshot pin**: exactly one `AT (VERSION => $snapshot)`; it is spliced into
  the SQL text at runtime (DuckLake rejects a parameter there), every other
  param binds positionally (`$name` → `$1`, in yaml order; a repeated `$name`
  reuses its number).
- **Lake write values**: timestamps and JSON cross as `varchar` params with
  explicit `$n::timestamp` / `$n::json` casts in the SQL.
- **Aggregates**: `count(*)` is safe; wrap anything else in
  `CAST(.. AS BIGINT)` (DuckDB's `sum(BIGINT)` is HUGEINT — the oracle fails
  it) and declare the result column nullable.
- **Tuple keyset cursor**: expand it —
  `($a IS NULL OR k1 > $a OR (k1 = $a AND k2 > $b))` (DataFusion does not plan
  row-value comparisons).

## What the emitter generates

Per statement, one Rust fn in `rust/crates/er-entities/src/<entity>.rs` under
`pub mod <op>`:

- Lake `returns: entity` → `fn(conn, …) -> duckdb::Result<Vec<RecordBatch>>`.
- Lake typed columns → a `<Name>Row` struct (serde-serializable; JSON columns
  are `serde_json::Value`; timestamps serialize in DuckDB's `to_json` text
  form) + `from_batches` (shape/type/null violations are `Err`, never panic,
  via the generated `er_entities::batch` module) + `fn -> Vec<Row>`.
- Lake writes → `fn(conn, …) -> duckdb::Result<usize>`.
- Control statements → `async fn(client: &tokio_postgres::Client, …)`
  returning `Vec<Entity>` (entity reads / RETURNING) or `u64` (`returns: none`).
