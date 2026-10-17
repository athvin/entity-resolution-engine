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
drizzle migrations. Session-temp scratch tables and `dedupe-cli`'s
operator-named outputs are the two deliberate exemptions. `docs/erd.md` is
generated from the same authorities and CI fails when it is stale.

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
