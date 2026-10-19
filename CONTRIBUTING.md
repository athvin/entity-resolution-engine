# Contributing

The backend is the Rust workspace under `rust/` (toolchain pinned by
`rust/rust-toolchain.toml`, MSRV 1.85); the frontend is a standalone pnpm
project under `frontend/` (pnpm pinned via corepack in its `package.json`).

```sh
make check
```

`make check` is the PR-equivalent backend gate, mirroring CI
(`.github/workflows/rust.yml`): `cargo fmt --check`, clippy with `-D warnings`,
the entity-framework gate (`er-entity verify` — regenerate every artifact from
`entities/*.yaml`, fail on stale committed files, then plan every statement and
run the E0xx passes), and `cargo test --workspace`. It needs no lake.

## Gated end-to-end tiers

```sh
make check-e2e ER_TEST_PG_DSN='host=localhost port=5433 user=er password=er dbname=er'
```

Runs the DuckLake + extensions pipeline tier and the Postgres control-plane
queue/dispatch tier. Needs the DuckDB extensions and a local Postgres (see the
`ducklake-e2e` job in `rust.yml` for the service definition).

## Frontend

```sh
make frontend
```

Installs with a frozen lockfile, runs `pnpm check` (lint + format + types +
unit), builds, and runs the mock-backed Playwright tier. Visual baselines
compare on Linux only; see `frontend/README.md`.

## Making a change

Use a branch and a pull request. Explain the behavior changed and the
validation performed. Tests should exercise observable behavior, including
relevant merge, split, deletion or incremental cases when changing resolution
logic.

Table shapes are declared once: entity-owned relations in `entities/*.yaml`
(run `cd rust && cargo run -p er-entity-cli -- gen --root ..` after editing and
commit the regenerated artifacts), the remaining engine-internal lake relations
in `er-schema`'s registry (`rust/crates/er-schema/src/model.rs`, gated by
`tests/model_registry_golden.json`). Update
[the reference specification](DesignDoc.md) when a contract changes.

**There is no third place.** A persistent table that none of those authorities
declares cannot ship: `er-schema`'s `declared_names` test fails on any
`lake.main.<name>` literal the registry doesn't know, the DuckLake e2e fails if
an undeclared relation survives a pipeline run, the Postgres e2e fails if
`public.*` holds a table the generated baseline didn't create, and the entity
analyser refuses statements against unknown relations. New control-plane tables
enter through an `entities/*/entity.yaml` (never hand DDL — er-server boots only
the generated `POSTGRES_BASELINE`); new lake relations enter through the
registry or an entity; the frontend's `erweb` schema changes only through
drizzle migrations. `docs/erd.md` is generated from the same authorities and CI
fails when it is stale.

**And no free-form SQL.** Every persistent statement — read or write, both
stores — is one of three governed kinds: a **declared statement**
(`entities/<entity>/<op>/<name>.{sql,yaml}`, planned by the analyser, emitted as
a typed function; lake reads carry `AT (VERSION => $snapshot)` or a justified
`pin: live`), a **plan-vector-tested generator** (the golden marts,
survivorship/masters, merge plans, the standardize stages — config/tenant-driven
SQL whose real output the analyser plans over representative vectors), or the
**engine core's execution-oracle SQL** (dedupe-core's model-derived blocking/
scoring/clustering, proven against the captured Splink golden fixtures).
`er-schema`'s `freeform_sql` test enforces the boundary: `query_strings` has no
callers outside the engine core, and `er-server` names no lake relation in
source. The deliberate exemptions, each visible at the call site: session-temp
scratch tables and their reads (`Session::query_scratch_strings`, e.g. the
`__preview_src` match preview), `dedupe-cli`'s operator-named outputs, and
catalog introspection (`Session::lake_table_exists` / `relation_row_count` —
catalog metadata, not a data contract).

**And one identity shape.** Every entity's primary key is `id` (`key: [id]`),
every entity carries `created_at`/`updated_at`, and foreign keys are named
`<singular(relation)>_id` referencing `id` — validated at catalog load (V1–V10)
and at statement analysis (E005 immutables, E014 updated_at, E015 MERGE-on-
natural-key). How an id is minted depends on who creates the row:

| mint class | recipe | examples |
| --- | --- | --- |
| application, one-shot | `er_ids::new_v7()` (wall-clock UUIDv7 text) | org, job, api_key, schedule, webhook, staged_action |
| application, rerun-reproducible | `er_ids::DeterministicV7Factory` (`deterministic_v7(run clock, mint counter)`) | reconcile's entity ids |
| SQL/CTAS, content-derived | `sha256('<relation>:' \|\| k1 \|\| ':' \|\| …)` (`er_ids::natural_id` is the Rust twin) | int_std/lineage marts, membership, match_scores, reviews, events, raw_records (batch-salted) |
| database-minted ledgers | `bigint` + `serial: true` | audit_log, ddl_log |

`natural_key:` declares the upsert/MERGE identity (a second registry LogicalKey
on the lake, `UNIQUE` on the control plane); domain linkage columns keep their
names (`record_key`, `run_id`, `entity_id`-as-FK, …). Control-plane entities may
opt into `history: true` (a generated `<relation>_history` jsonb row-image
companion), and every DDL statement lands in the `ddl_log` ledger via event
triggers. The full conventions: the "Identity conventions" section of
[docs/entity-framework-design.md](docs/entity-framework-design.md).

The fixture format is documented in
[fixtures/static/FORMAT.md](fixtures/static/FORMAT.md). Benchmark and profiling
changes should follow [performance.md](docs/performance.md) and
[profiling.md](docs/profiling.md).

## Keeping the checkout small

```sh
make clean
```

This removes the Cargo build cache (`rust/target`). Raw logs, exports,
generated corpora and temporary scripts stay out of Git; keep durable
measurements and methodology in the performance guide.
