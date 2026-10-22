# Working in this repo

## The gate is `make check`, not raw cargo

Before calling backend work done, run **`make check`** from the repo root. It is
the PR-equivalent gate and mirrors `.github/workflows/rust.yml`:

```
make check   # = fmt + lint + entities-check + skills-check + test
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
| `make frontend` | the frontend PR gate: lint + format + types + authz + unit/component with the coverage floor, then the mock-backed Playwright tier | corepack pnpm |
| `make frontend-audit` | the frontend supply chain: HIGH/CRITICAL advisories + the license allowlist | network |
| `make entities-check` | just the entity-framework gate | — |
| `make skills-check` | just the agent-skill gate (authoring rules + generated references) | — |

**The frontend gate blocks merges now.** `lint + types + unit`, `playwright
functional tier` and `pnpm audit + licenses` are required status checks on `main`
alongside the four Rust ones — before 2026-10-20 the frontend CI ran and was then
ignored at merge time. `.github/workflows/frontend.yml` therefore has no `paths:`
filter: it uses the same `detect`-job pattern `rust.yml` does, because a workflow
a path filter skips never reports, and a required check that never reports blocks
every backend-only PR forever.

`docs/frontend-testing.md` is the frontend standard — which test tier a change
belongs in, what each gate is for, and the rule that every gate must be observed
*failing* before it is trusted.

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

**The entity conventions are a lint registry, not prose.** Every rule an
`entity.yaml` must hold lives in `rust/crates/er-entity-lint` as a value carrying
its own code, reasoning and worked examples:

```
er-entity lint                 # run the pass alone (verify runs it first)
er-entity lint --list          # the registry as a table
er-entity lint --explain V4    # one rule, with why it exists
```

`docs/entity-lint-rules.md` is **generated** from that registry and byte-compared
by `er-entity verify`, so the reference cannot drift from what is enforced. Adding
a rule is one file in `er-entity-lint/src/rules/`, one line in the registry, and
one fixture directory under `tests/fixtures/` — nothing else changes. Codes are
append-only; a retired rule leaves its code behind.

A consequence worth knowing: `Catalog::load_dir` now checks only structural
coherence. Anything about to write a generated artifact should load through
`er_entity_lint::load_linted` instead.

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

This is enforced, not just documented: a `no-restricted-syntax` rule in
`frontend/eslint.config.mjs` rejects Tailwind palette utilities
(`bg-emerald-500`, `text-red-600`, …) in string and template literals, so
`make frontend` fails on a reintroduction. Use the semantic tokens in
`globals.css` — all chroma-0 in both themes — and signal state with weight,
scale, spacing or motion rather than hue. The techniques that replaced the 47
colour utilities this rule found are tabulated in `docs/frontend-testing.md`,
along with the one known exception (`--destructive` is still red).

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

### Repo skills

Fifteen first-party skills in `.claude/skills/`, organised by what you are
doing rather than by crate. **[`docs/agent-skills.md`](docs/agent-skills.md) is
the standard** — the authoring rules, why everything sits at the root, and how
the gate works.

| Doing | Skill |
|---|---|
| adding a table, column or SQL statement | `entity-framework` |
| an E0xx failure, a statement that will not plan | `sql-analysis` |
| blocking, comparisons, training, term frequency, scores | `er-matching` |
| running the pipeline with the `er` CLI | `er-engine-ops` |
| attaching a lake, partition/sort choices, maintenance | `ducklake-lakehouse` |
| a slow stage or query, a layout experiment | `ducklake-performance` |
| an endpoint, guard, job or the dispatcher | `er-server-control-plane` |
| changing a request/response shape | `api-contract` |
| authoring or reviewing any Rust | `rust-conventions` |
| a BFF route handler, sessions, the browser/server line | `frontend-bff` |
| a frontend test or a red frontend job | `frontend-testing` |
| Terraform, Helm, EKS, IAM | `infra-platform` |
| a CI job failing for no diff-related reason, dependency bumps | `ci-and-supply-chain` |
| reviewing something for a vulnerability class | `security` |
| the autonomous campaign (`/pentest`, manual only) | `pentest` |

Each carries an `evals.md` of three hand-run scenarios. `make skills-check`
enforces the mechanical rules; counts and version numbers live in generated
reference blocks, never in prose, because a hand-written count in this repo
goes stale within weeks.

The symposium-synced skills under `rust/.claude/skills/` are third-party and
gitignored. One of them, `rust-best-practice`, prescribes a finish line of
`cargo fmt` + `cargo clippy` + `cargo test`; that checklist is **incomplete
here** (it skips `entities-check`), and `rust-conventions` overrides it.
