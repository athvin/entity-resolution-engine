---
name: entity-framework
description: Explains this repo's entity framework — the YAML-driven schema and SQL governance where every table is an entity.yaml, every persistent SQL statement is a declared .sql+.yaml file (or a plan-vector-tested generator), and er-entity-gen emits the Rust structs, DuckLake/Postgres DDL, Arrow schemas, TypeScript bindings and typed query functions. Use when adding or changing a table, column, or SQL statement; when adding a lake read or write; when `er-entity verify`, an E0xx diagnostic, the oracle, or a posterity gate (declared_names, freeform_sql, lake e2e, pg coverage) fails; or when asked how entities/, the generated er-entities crate, or schema changes work here. For the analyser's own internals — the rewrite step, `mode: partial`, the ff-analysis planner and its function stubs — see sql-analysis; for the engine's golden-governed model SQL see er-matching.
---

# The entity framework

One YAML per table drives everything: the Rust struct, both stores' DDL, the
Arrow schema, the TypeScript binding/reader, and the typed query functions.
A generator (`er-entity-gen`) and a DataFusion-based analyser (`er-sql-check`)
share one catalog, so what the code compiles against, what the database holds,
and what each statement returns can never drift apart.

**The one rule:** no schema and no persistent SQL is hand-written outside the
declared authorities. Every persistent statement is one of three governed kinds:

1. **A declared statement** — `entities/<entity>/<op>/<name>.{sql,yaml}`,
   planned by the analyser, emitted as a typed Rust function
   (`er_entities::<entity>::<op>::<name>`).
2. **A plan-vector-tested generator** — config/tenant-driven SQL (golden marts,
   survivorship/master election, merge plans, standardize stages) whose real
   output is planned in CI over representative config vectors
   (`rust/crates/er-sql-check/src/stage.rs` tests).
3. **The engine core's model plane** — dedupe-core's model-derived
   blocking/scoring/clustering SQL, governed by captured golden fixtures
   (an execution oracle), never by the static analyser.

The three visible exemptions: session-temp scratch reads
(`Session::query_scratch_strings`), `dedupe-cli`'s operator-named outputs, and
catalog introspection (`Session::lake_table_exists` / `relation_row_count`).
Everything else is gated — see [references/gates.md](references/gates.md).

## File map

| What | Where |
|---|---|
| Entity definition | `entities/<name>/entity.yaml` (the inventory is generated — [references/entities-index.md](references/entities-index.md)) |
| Declared statements | `entities/<name>/{create,read,update,delete}/<stmt>.{sql,yaml}` |
| Generated Rust (structs + query fns + `batch.rs` + `schema.rs` + `catalog.rs`) | `rust/crates/er-entities/src/` |
| Generated DDL | `rust/crates/er-entities/ddl/{lake,postgres}/` |
| Generated TypeScript | `frontend/src/lib/entities/{bindings,readers}/` |
| Generated ERD (never hand-edit) | `docs/erd.md` |
| Engine-internal lake relations (non-entity) | `rust/crates/er-schema/src/model.rs` (the S5 registry) |
| The analyser + emitter + E0xx rules | `rust/crates/er-sql-check/` |
| The generator + emitters | `rust/crates/er-entity-gen/` |
| The CLI (`er-entity gen/check/verify/diff/migrate/erd`) | `rust/crates/er-entity-cli/` |
| The reconciler (differ + planner + ledger, both stores) | `rust/crates/er-entity-diff/` |
| Full design rationale | `docs/entity-framework-design.md` |

Everything under the generated directories is owned by `er-entity gen` —
hand-edits are overwritten, and files gen did not emit fail `check` as orphans.

## Making a change: definition first

The order is always definition → regenerate → use. Copy this checklist:

```
- [ ] 1. Edit the definition (entity.yaml and/or statement .sql+.yaml)
- [ ] 2. Regenerate:  cd rust && cargo run -q -p er-entity-cli -- gen --root ..
- [ ] 3. Verify:      cargo run -q -p er-entity-cli -- verify --root ..
- [ ] 4. Write/update the Rust and frontend code that uses the generated API
- [ ] 5. Commit the definition AND the regenerated artifacts together
```

`verify` = the entity lint pass (V-rules) + the byte-exact staleness check +
orphan scan + the analyser's E0xx passes, in that order. The lint pass
**short-circuits on an error-severity finding**, because every later pass
assumes a conforming catalog. It is CI's gate (the `lint` job) and part of
`make check`. A change that skips step 2 fails CI with "N generated file(s) are
stale or missing".

`er-entity check` and `er-entity verify` are the **same code path** — both
match arms call one function. Treat them as aliases; `check` is not a narrower
staleness-only command, whatever its own help text suggests.

**Live databases are reconciled, not migrated by file.** There are no
migration files. The catalog is compiled into `er-server`
(`er_entities::catalog::catalog()`), and `er-entity-diff`'s reconciler compares
it with a live store's `information_schema`, applies only what is safe (create
an absent relation, add an absent nullable column), refuses everything else
with `ERR_SCHEMA_BREAKING`, and writes one `applied` row per statement to that
store's own `schema_migrations` ledger keyed by the catalog hash (a `verified`
row marks a new catalog version that found the store current). Where it runs:
`ensure_schema` at boot and the web-api init container (`public.*`); the start
of every lake job, provisioning, and the deploy's `er-server migrate-tenants`
sweep (every tenant's `lake.main.*`, with the outcome cached in
`public.tenant_schema_states`). Stage-materialised lake relations are skipped —
each run rebuilds them whole. A column the store carries that the catalog does
not declare is **ahead**: reported, never dropped, never refused (E011 makes
an old binary safe against it). From the command line, `er-entity diff` and
`er-entity migrate [--apply]` take `--store postgres --dsn <dsn>` or `--store
duckdb --lake-catalog <metadata> --lake-data-path <path>`; `--apply` is the
same reconciler, ledger included. A breaking lake difference means a rebuild
of that tenant; a breaking control difference means a hand-written migration,
verified by the next boot. Bump an entity's `version` only on a breaking
change (type change, rename, drop); an added nullable column is not one.

## Common workflows

**Add a column to an existing entity** — add the column to
`entities/<name>/entity.yaml` (columns default `nullable: true`; give every
column a `description`), regenerate, then fix whatever stops compiling: any
`returns: entity` statement must now project the new column too (E003 enforces
same columns, same order). Lake note: additive only; the lake has no defaults.

**Add a new entity (table)** — create `entities/<name>/entity.yaml` with
`kind/version/name/relation/description/key/generate/columns` (see
[references/entities-and-generation.md](references/entities-and-generation.md)
for every field), regenerate, commit. Control-plane tables boot from the
generated `POSTGRES_BASELINE`; lake tables are created in every tenant lake by
the reconciler at the next job start, provisioning or deploy sweep; never
hand-write DDL. A lake relation that is engine-internal (not an entity) goes in
the `er-schema` registry instead — those are the only two doors ("there is no
third place", CONTRIBUTING.md). A lake *entity* must also appear in that
registry (`er_schema::model::ddl_specs`, with the golden JSON updated), because
the declared-names gates read the registry.

**Add a lake read** — a `.sql` + `.yaml` under `entities/<entity>/read/`.
Two shapes:
- `returns: entity` → the full column set in order; the generated fn returns
  `Vec<RecordBatch>` (the Arrow-wire path).
- `returns:` a typed column list (`{ name, type, nullable }`, nullable defaults
  true) → generates a `<Name>Row` struct + fn returning `Vec<Row>`; JSON
  columns come back as `serde_json::Value`; timestamps serialize in DuckDB's
  text form.

Every lake read carries `AT (VERSION => $snapshot)` (E006). The writer's own
in-transaction reads instead declare `pin: live` with a mandatory `pin_reason`
— and must then NOT carry the pin. Field-level detail and real examples:
[references/statements.md](references/statements.md).

**Add a lake write** — under `create/` (INSERT, MERGE-that-inserts) or
`update/` (UPDATE, MERGE), always `returns: none`; the generated fn returns the
affected-row count. A MERGE needs `mode: partial` (DataFusion cannot plan
MERGE; partial skips only the planner — parsing and the AST rules still run).
Timestamps and JSON cross as text params with `::timestamp` / `::json` casts
in the SQL.

**Bulk append (variable row count)** — never build multi-row VALUES by hand.
Set `append: true` in the entity's `generate` block; gen emits a typed
`AppendRow` struct + `append_rows(conn, &[AppendRow])` (chunked, bound
parameters). `raw_record` is the model.

**Tenant/config-driven SQL** (shape depends on runtime config or the trained
model) — never a statement. Write a Rust generator fn (see
`er_golden::merge_plans_sql` or `er_standardize::int_std_records_sql`) and add
a plan-vector test in `rust/crates/er-sql-check/src/stage.rs` that plans every
shape the generator can emit (vectors are real `er_config` fixture configs —
see `TEST_CONFIG` there; the generator fn lives beside its domain crate, the
way merge plans live in er-golden and standardize in er-standardize). Model-derived SQL inside dedupe-core stays under
its golden fixtures — do not add analyser coverage there.

## The rules that bite

- **E006**: lake reads pin a snapshot, or justify `pin: live`. Never both.
- **E011**: no `SELECT *` anywhere — name every column.
- **E005**: a key column is never assigned; identity is never rewritten.
  `natural_key` columns and `created_at` are immutable too (written at insert).
- **E014/E015**: every mutation SETs `updated_at`, and a MERGE matches on
  exactly the declared `natural_key`. Every entity follows the identity
  conventions: `key: [id]` (varchar UUIDv7/sha256, or bigint+serial for
  DB-minted ledgers), `created_at`/`updated_at` always, FKs named
  `<singular(relation)>_id` referencing `id`.
- **E002 (lake)**: `list<varchar>` params cannot bind (duckdb-rs has no LIST
  binding). The IN-list idiom: pass one varchar of `chr(31)`-joined values and
  `WHERE col IN (SELECT unnest(string_split($keys, chr(31))))` (`mode: partial`).
- **E009**: reading another entity's relation requires naming it under `reads:`.
- **Aggregates**: wrap lake aggregates other than `count(*)` in explicit
  `CAST(.. AS BIGINT)` — DuckDB types `sum(BIGINT)` as HUGEINT and the oracle
  will fail the statement. Aggregate results are plan-nullable: declare them
  nullable and `unwrap_or(0)` at the call site.
- **Optional filters**: `($p IS NULL OR col > $p)` with a `nullable: true`
  param; `LIMIT $limit` binds fine.

The full rule table with remedies: [references/statements.md](references/statements.md).
Note that **`E007`, `E008` and `E013` are documented but never implemented** —
do not rely on them firing. The analyser's own internals, the rewrite step and
the generated list of which codes are really live belong to the sql-analysis
skill.

Those are the **statement** rules (`E0xx`, in `er-sql-check`). The **entity**
rules (`V0xx`, in `er-entity-lint`) are a separate family and documented
differently: every rule carries its own reasoning, so rather than summarising
them here, ask the tool. Codes are append-only — a retired rule keeps its code,
so the next free code is always one past the highest, never a gap.

```
er-entity lint --explain V4    # why the rule exists, plus worked examples
er-entity lint --list          # the whole registry
```

[`docs/entity-lint-rules.md`](../../../docs/entity-lint-rules.md) is the same
content, generated from the registry and byte-compared by `verify` so it cannot
drift. Adding a rule: [references/gates.md](references/gates.md).

## When a gate fails

| Failure | Meaning | Fix |
|---|---|---|
| `er-entity lint: [V0xx] …` | an `entity.yaml` breaks a convention | `er-entity lint --explain <code>` |
| `… stale or missing` | definition changed without regen | run gen, commit the diff |
| `… orphaned file(s)` | a deleted entity/statement left output behind | delete the named files |
| `er-entity analyse: [E0xx] …` | a statement breaks a rule | see the rule's remedy in references/statements.md |
| oracle (`oracle_lake_reads_match_their_declared_shape`) | DuckDB's real result type ≠ declaration | fix the declared type, or CAST in SQL |
| `declared_names` / `the_lake_contains_only_declared_relations` | an undeclared `lake.main.<x>` | declare it (entity or registry) first |
| `freeform_sql` | `query_strings` caller or `lake.main.` literal in er-server | make it a statement (or `query_scratch_strings` for session temps) |
| `pg_schema_coverage` | `public.*` holds an undeclared table | the table must come from an entity.yaml |

Details, invocations and env vars: [references/gates.md](references/gates.md).
