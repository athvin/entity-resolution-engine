# Working in this repo

## The gate is `make check`, not raw cargo

Before calling backend work done, run **`make check`** from the repo root. It is
the PR-equivalent gate and mirrors `.github/workflows/rust.yml`:

```
make check   # = fmt + lint + entities-check + test
```

`cargo fmt` / `cargo clippy` / `cargo test` on their own are **not** sufficient —
they skip `entities-check`, which is the entity-framework gate (`er-entity
verify`: regenerate every artifact from `entities/*.yaml` and fail if a
committed file is stale, then plan every statement and run the E0xx passes).
A change that passes clippy and tests can still break that gate.

Some installed third-party skills (notably `rust-best-practice`, which symposium
syncs into `rust/.claude/skills/`) prescribe a generic finish line of `cargo fmt`
+ `cargo clippy --all --workspace` + `cargo test --all --workspace`. In this repo
that checklist is incomplete; `make check` supersedes it.

Other gates, each run on its own because they need external services:

| Command | What it covers | Needs |
|---|---|---|
| `make check` | the backend PR gate | — |
| `make check-e2e` | DuckLake pipeline + Postgres control plane | DuckDB extensions, a local Postgres via `ER_TEST_PG_DSN` |
| `make frontend` | lint + types + unit + mock-backed Playwright | corepack pnpm |
| `make entities-check` | just the entity-framework gate | — |

## Layout

The backend is the Rust workspace under `rust/` (26 crates); the Python/dbt
implementation has been removed. **The cargo workspace root is `rust/`, not the
repo root** — bare `cargo` commands must run from `rust/`, which is why the
Makefile targets all `cd rust` first.

- `rust/crates/` — engine, CLI, control plane (`er-server`), entity tooling
- `entities/` — one YAML per table; the source of truth for schema
- `frontend/` — standalone pnpm project with its own gate
- `infra/` — terraform + helm charts
- `docs/` — design docs; `docs/architecture.md` is the entry point

## Conventions worth knowing

**Schema changes go through `entities/`.** Every table is an `entity.yaml` and
every persistent SQL statement is a declared `.sql` + `.yaml` pair;
`er-entity gen` emits the Rust structs, DDL, Arrow schemas, and TypeScript
bindings. Never hand-edit a generated file. See the `entity-framework` skill and
`docs/entity-framework-design.md`.

**Errors must not interpolate their own source.** A `#[error("...")]` string must
not include the `#[from]`/`#[source]` field (or a field named `source` —
thiserror treats that name as the source automatically). Reporters walk
`.source()` separately, so interpolating double-prints:

```rust
#[error("reading {path:?}: {source}")]   // wrong — double-prints
#[error("reading {path:?}")]             // right
```

This is enforced, not just documented: `er-errors/tests/display_no_source.rs`
scans every product source in the workspace and fails `make check` on a
reintroduction, naming the file, line and placeholder.

The flip side: `e.to_string()` alone now yields only the outermost message. Where
an error is flattened into a `String` rather than propagated — `er-server` does
this for job-failure text and HTTP bodies — walk the chain at that point:

- `anyhow::Error` → `format!("{e:#}")` (its alternate formatter walks the chain)
- a `thiserror` type → `er_server::errfmt::chain(&e)`

**The UI is strictly black and white.** Both themes are grayscale — no brand
accent, no colored chips or badges, no palette (`docs/frontend-design.md` §3.3).

**Use `cargo add`** to add dependencies or features rather than editing
`Cargo.toml` by hand.

## Agent tooling

`make symposium` installs the repo-local [Symposium](https://github.com/symposium-dev/symposium)
toolchain: the `cargo-agents` binary into `.tools/bin`, its config/cache into
`.symposium`, and dependency-matched skills into `rust/.claude/skills/`. Nothing
is written to `~`. Those nested skills load lazily — only once a file under
`rust/` is read or edited — and the hooks symposium writes to
`rust/.claude/settings.json` are inert, because Claude Code reads settings only
from the repository root. The root `.claude/settings.json` carries the one
bridging hook that actually runs.

Repo skills: `entity-framework`, `security`, `pentest` (and `/pentest` for the
autonomous security campaign).
