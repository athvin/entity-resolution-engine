# The gates: what runs them, what failing means, the fix

## Contents
- The verify gate (staleness + orphans + analyser)
- The oracle
- The posterity gates (static)
- The posterity gates (live)
- The generator plan-vector tests
- Running everything locally

## The verify gate (staleness + orphans + analyser)

**Command:** `cd rust && cargo run -q -p er-entity-cli -- verify --root ..`
**Runs in:** CI's `lint` job (`.github/workflows/rust.yml`), `make check`
(target `entities-check`).

Three checks in one:
1. **Staleness** — regenerates the whole tree in memory and byte-compares every
   generated file (incl. `docs/erd.md`). Failure: `er-entity check: N generated
   file(s) are stale or missing` → run gen and commit the diff.
2. **Orphans** — any file in a generated directory that gen did not emit.
   Failure names the files → delete them (a removed entity/statement leaves
   its output behind; gen never deletes).
3. **Analyse** — loads every statement, plans full-mode ones via DataFusion,
   runs the E0xx passes. Failure: `[E0xx] path: message` → the remedy table in
   [statements.md](statements.md).

## The oracle

**Test:** `er-sql-check/tests/oracle.rs::oracle_lake_reads_match_their_declared_shape`
(plain `cargo test -p er-sql-check`).

For every lake read statement it asks a real DuckDB `DESCRIBE <statement>`
(each `$param` substituted with a typed NULL literal; the snapshot pin stripped)
and compares name+type against the declaration. It catches
DuckDB-vs-DataFusion divergence the analyser cannot — classically
`sum(BIGINT)` → HUGEINT. Fix: `CAST` in the SQL or correct the declared type.
This is the only shape check a `mode: partial` read gets.

## The posterity gates (static)

**`er-schema/tests/declared_names.rs::every_lake_main_literal_names_a_declared_relation`**
— any `lake.main.<name>` literal anywhere in the workspace must name a relation
the S5 registry (or an entity) declares. Fix: declare the relation first;
inline DDL cannot ship. (`dedupe-cli` is exempt: operator-named outputs.)

**`er-schema/tests/freeform_sql.rs`** — two scans:
- `query_strings_has_no_callers_outside_the_engine_core` — product code never
  calls `Session::query_strings`. A session-temp scratch read uses the visibly
  exempt `Session::query_scratch_strings`; everything persistent is a declared
  statement.
- `er_server_builds_no_lake_sql` — `er-server/src` names no `lake.main.<x>` at
  all; its lake access is generated statements plus the er-golden merge-plan
  generator. Fix: declare a statement, never assemble SQL in er-server.

**`er-schema/tests/model_registry.rs`** + `model_registry_golden.json` — the
registry is byte-pinned. A registry change must update the golden
(`json.dumps(data, indent=1, ensure_ascii=False) + '\n'`, byte-exact).

## The posterity gates (live)

**`er-cli/tests/lake_e2e.rs::the_lake_contains_only_declared_relations`**
(needs `DEDUPE_E2E_DUCKLAKE=1`) — after a full pipeline run, the live lake
holds only declared relations. Catches dynamically-built names the static scan
cannot see.

**`er-server/tests/pg_schema_coverage.rs::public_schema_contains_only_declared_tables`**
(needs `ER_TEST_PG_DSN`) — after boot provisioning, `public.*` holds only the
tables the generated `POSTGRES_BASELINE` declares.

**The DuckLake e2e tiers** (`DEDUPE_E2E_DUCKLAKE=1 DEDUPE_E2E_EXTENSIONS=1`,
`make check-e2e`) — the base_10 committed goldens must stay byte-identical;
this is the determinism proof every framework change rides on.

## The generator plan-vector tests

In `rust/crates/er-sql-check/src/stage.rs` (`cargo test -p er-sql-check`):
the golden-record and lineage mart assembly (person + account), the
standardize stg/int_std SQL (person + account), and the merge-plan generator
(every policy × protect × since = 20 vectors). A generator change that emits an
un-plannable shape fails here, not at runtime. New config-driven SQL must add
its vectors alongside these. (If planning needs a new DuckDB function stub,
that lives in the pinned `ff-analysis` — bump requires a feather-flow tag.)

## Running everything locally

```bash
make check          # fmt + clippy + er-entity verify + workspace tests
cd rust && DEDUPE_E2E_DUCKLAKE=1 DEDUPE_E2E_EXTENSIONS=1 \
  cargo test -q -p er-cli --test lake_e2e   # the determinism tier
```

Postgres-gated suites expect a disposable DB, e.g.
`docker run -d --rm --name er-pg-test -e POSTGRES_USER=er -e POSTGRES_PASSWORD=er \
 -e POSTGRES_DB=er -p 5455:5432 postgres:16-alpine` then
`ER_TEST_PG_DSN='host=localhost port=5455 user=er password=er dbname=er'`.
Tear the container down afterwards.
