# Rewrites, idioms, and what the emitter generates

## Contents

- The two-store catalog
- Every rewrite, and the rule it preserves
- Idioms that plan and bind
- The remedies for the diagnostics that fail most
- What the emitter generates
- Plan-vector tests for generators

## The two-store catalog

The analyser plans against a catalog assembled from two sources:

1. **The entity catalog** — every `entities/*/entity.yaml`, split by declared
   store (`duckdb` for lake entities, `postgres` for control-plane ones).
2. **The engine's relation registry** (`er-schema`) — the intermediate lake
   tables that are *not* entities (`int_std_records`, `golden_lineage`,
   `entity_membership`, `entities`), so stage SQL can be planned alongside the
   entity-owned marts.

A statement is planned against **its own store only**. That is what E009
enforces: a qualified relation must resolve to an entity of the statement's own
store, and any entity beyond the statement's own must be named under `reads:`
— and a `reads:` entry must itself name a same-store entity.

Unqualified names are session temps or CTEs and are governed elsewhere.

## Every rewrite, and the rule it preserves

The principle: a removal **records what it stripped**, so a later pass can
enforce the rule the construct stood for. Removal is not forgiveness.

| Construct | Rewrite | Rule preserved |
|---|---|---|
| `AT (VERSION => $snapshot)` | stripped, snapshot param recorded | **E006** — a lake read is snapshot-pinned unless it declares `pin: live` with a `pin_reason` |
| `name := value` | prefix dropped, args become positional | none — DataFusion rejects named args, and positions are all schema inference needs |
| `MERGE INTO` | split: source query planned; target assignments checked by name/type against the entity | **E005** (no key assignment), **E014** (`updated_at` assigned), **E015** (ON matches `natural_key`) |
| `RETURNING` | removed | output schema taken from the entity's columns |
| `ON CONFLICT DO NOTHING` / `DO UPDATE SET` | removed **after** the assignment list is checked | E005 / E014 on the update arm |
| `FOR UPDATE`, `SKIP LOCKED`, `NOWAIT` | removed | none — no schema effect |
| named params (`$name`) | rewritten to positional placeholders carrying declared types | **E002** — declared type must match the planner's inferred placeholder type |

`::jsonb` and `::text` casts are kept rather than stripped — they carry type
information the planner needs.

A statement that still will not parse after all of this falls back to text
heuristics, which is a much weaker check. That is the real cost of exotic SQL:
not that it is rejected, but that it is checked less.

## Idioms that plan and bind

**The IN-list idiom (E002).** `list<varchar>` parameters cannot bind on the
lake — duckdb-rs has no LIST binding. Pass one varchar of `chr(31)`-joined
values and split it in SQL:

```sql
WHERE col IN (SELECT unnest(string_split($keys, chr(31))))
```

This needs `mode: partial`. `chr(31)` (unit separator) is chosen because it
cannot appear in real data.

**Optional filters.** Use a nullable param and a disjunction rather than
building SQL conditionally:

```sql
AND ($since IS NULL OR updated_at > $since)
```

`LIMIT $limit` binds fine.

**Aggregates.** Wrap anything but `count(*)` in an explicit cast, because
DuckDB types `sum(BIGINT)` as `HUGEINT` and the oracle compares type names:

```sql
CAST(sum(n) AS BIGINT) AS total
```

Declare aggregate results **nullable** (they are plan-nullable) and
`unwrap_or(0)` at the call site.

**Timestamps and JSON as params.** They cross as text with explicit casts in
the SQL (`$at::timestamp`, `$doc::json`).

**Bulk append.** Never hand-build a multi-row `VALUES`. Set `append: true` in
the entity's `generate` block and use the generated
`append_rows(conn, &[AppendRow])`, which chunks and binds.

## The remedies for the diagnostics that fail most

| Failing | Usual cause and fix |
|---|---|
| **E003** | the declared output drifted from the plan. A `returns: entity` statement must project **every** column, in order — adding an entity column means updating every such statement |
| **E006** | a lake read with no `AT (VERSION => $snapshot)`. Add it, or declare `pin: live` + `pin_reason` — and a live read must then **not** carry a pin |
| **E011** | a `SELECT *` anywhere, including in a CTE or subquery. Name every column. No override exists |
| **E002** | a `list<varchar>` param, or a declared type the planner infers differently. Use the `chr(31)` idiom, or correct the declared type |
| **E005** | assigning a `key` column, a `natural_key` column, or `created_at`. Identity and insert-time values are immutable |
| **E014** | a mutation that does not `SET updated_at` |
| **E015** | a MERGE whose `ON` target columns are not exactly the declared `natural_key` |
| **E009** | reading another entity's relation without naming it under `reads:` |
| **E012** | the statement's verb does not match its folder. Upserts file under `create` by their leading verb; `COPY TO` under `read` |
| oracle mismatch | the declared type is not what DuckDB returns — usually an uncast aggregate |

## What the emitter generates

For each statement, a typed Rust function at
`er_entities::<entity>::<op>::<name>`:

| Declaration | Generated |
|---|---|
| `returns: entity` | a fn returning `Vec<RecordBatch>` (the Arrow wire path) |
| `returns: columns` | a bespoke `<Name>Row` struct plus a fn returning `Vec<Row>` |
| `returns: none` | a fn returning the affected-row count |

JSON columns come back as `serde_json::Value`; timestamps in DuckDB's text form.
The query modules are appended onto each entity's generated `.rs` file, so one
file carries the struct and its `read`/`create`/… modules.

Never hand-edit any of it. `er-entity verify` byte-compares the tree and also
reports **orphans** — files in the generated directories that this run did not
emit, left behind by a deleted entity or statement.

## Plan-vector tests for generators

SQL whose shape depends on runtime config or a trained model cannot be a
declared statement. It becomes a Rust generator function living beside its
domain crate, plus a **plan-vector test** that plans every shape the generator
can emit, over representative real config fixtures.

The vectors must be real configs, not minimal synthetic ones — the point is to
plan the SQL that tenants actually cause to be generated. A generator with one
vector is barely tested.

`dedupe-core`'s model SQL is the exception to the exception: it stays under its
golden fixtures. Do not add analyser coverage there.
