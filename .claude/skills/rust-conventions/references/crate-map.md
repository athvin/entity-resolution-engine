# The crate map

## Contents

- How to read it
- The crates, the pins, and the workspace lints (generated)

## How to read it

Generated from the tree by `scripts/skills-check.sh regen`. Do not hand-edit
below the marker — `make skills-check` regenerates it and fails on drift. Each
purpose is the crate's own leading `//!` sentence, and each lint's reason is the
comment beside it in `rust/Cargo.toml`, so both are the source's own words
rather than a second copy maintained here.

Reading it: the crates divide into the engine (`dedupe-*`), the pipeline stages
(`er-ingest`, `er-standardize`, `er-match`, `er-reconcile`, `er-golden`), the
entity framework (`er-entity-*`, `er-entities`, `er-sql-check`), the control
plane (`er-server`, `er-pg-tls`), the CLIs, and dependency-light primitives
(`er-ids`, `er-hash`, `er-schema`, `er-errors`, `er-env`, `er-obs`,
`er-versions`) that the rest can depend on without pulling a database into
their graph.

Three pins in the version block are not free choices — `duckdb`'s `=` pin and
the `arrow` and `datafusion` majors that follow another crate's. The rule and
the reason for each are in the skill body's *Dependencies* section
([../SKILL.md](../SKILL.md)); the values are below. One addition the body does
not carry: `datafusion` lives in `er-sql-check`'s manifest, not the workspace
table, so it does not appear in the generated pin block at all.

<!-- BEGIN GENERATED: crate-map -->
**33 crates.** The cargo workspace root is `rust/`, not the repo root.

| Crate | Purpose |
|---|---|
| `dedupe-cli` | CLI for dedupe-core: train/predict/cluster against a DuckLake catalog |
| `dedupe-core` | dedupe-core: a Fellegi-Sunter probabilistic record-linkage engine, narrowly scoped to DuckDB/DuckLake execution |
| `er-cli` | `er-cli` — orchestrate the ER pipeline stages over a real DuckLake lake (P2) |
| `er-compile` | Compile a tenant's declared `comparisons:` / `blocking:` into the settings document the engine loads |
| `er-config` | The S6 configuration document and its `config_hash` (port of `src/er/config/{schema,hashing,loader}.py`, DesignDoc.md S6/S6.1/S5.2) |
| `er-dag` | The compiled-DAG library of the batch-orchestration design, carried by the `er-engine-ops` skill as `references/batch-orchestration-design.md` |
| `er-entities` | The generated entity crate: one module per entity, each with its struct, `arrow_schema()` and (in later phases) its typed query functions |
| `er-entity-cli` | `er-entity` — the entity-framework CLI |
| `er-entity-diff` | `er-entity-diff` — does the live store match the entity catalog? |
| `er-entity-gen` | `er-entity-gen` — the emitters |
| `er-entity-lint` | `er-entity-lint` — the entity-convention rules as a registry |
| `er-entity-spec` | `er-entity-spec` — the parsed entity YAML model |
| `er-env` | The one reader of the `ER_*` environment set (port of `src/er/lake/env.py`) |
| `er-errors` | The S4.7 failure taxonomy and the S4.0 exit-code contract (Rust port of `src/er/errors.py`) |
| `er-eval` | Pairwise quality metrics — the S8.5 contract (Rust port of `src/er/eval/metrics.py`) |
| `er-golden` | The golden-record (survivorship) stage (port of the retired dbt models `marts/golden_*` + `macros/survivorship/*`, DesignDoc.md S4.6) |
| `er-hash` | `content_hash` — the normative S4.1 digest (Rust port of `src/er/ingest/hashing.py`) |
| `er-ids` | Identity primitives — record keys, pair canonicalisation, ULID minting, redirect resolution (Rust port of `src/er/entities/ids.py`) |
| `er-ingest` | The ingest stage (port of `src/er/ingest/*`, DesignDoc.md S4.1) |
| `er-k8s` | The Kubernetes half of the batch-orchestration design (§6, carried by the `er-engine-ops` skill): the [`PodApi`] seam the dispatcher launches and watches through (one production implementation over `kube`, one scripted test double in er-server's tests), |
| `er-match` | The matching stage (port of `src/er/matching/*`, DesignDoc.md S4.3) |
| `er-mcp` | `er-mcp` — an MCP server over the control API, for one tenant (O10) |
| `er-obs` | Observability: installing the subscriber, plus the S5.2 counter vocabulary and structured stage log line (ports of `src/er/obs/counters.py` and `src/er/obs/logging.py`) |
| `er-pg-tls` | `er-pg-tls` — the TLS connector for every Postgres client in the workspace |
| `er-profile` | The dataset profiler's findings pass |
| `er-reconcile` | The reconcile / entity-resolution stage (port of `src/er/entities/*` + `src/er/review/*`, DesignDoc.md S4.5) |
| `er-schema` | The S5 schema registry (Rust port of `src/er/lake/*`), dependency-free so `config`/`golden`/tests can consume it without pulling a DB into their graph |
| `er-server` | `er-server` — the Rust control plane (P3), an axum port of the Python `erserver` FastAPI app (`server/src/erserver/api.py`) |
| `er-span` | The span vocabulary the generated query functions call |
| `er-sql-check` | `er-sql-check` — the static analyser |
| `er-standardize` | The standardization stage (port of the retired dbt models `stg_records.sql` + `int_std_records.sql`, DesignDoc.md S4.2) |
| `er-std` | Standardization primitives (port of `src/er/std/*`, DesignDoc.md S4.2) |
| `er-versions` | The config-drift guard (drift-guard half of `src/er/versions.py`): the run fingerprint, the S4.0 mode-precondition decision, and the S5.1 rebuild-reason |

Pinned versions that carry a reason (from `rust/Cargo.toml`):

```toml
edition = "2024"
rust-version = "1.89"
duckdb = { version = "=1.20000.1", features = ["bundled"] }
sqlparser = { version = "0.63", features = ["visitor"] }
thiserror = "2"
anyhow = "1"
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls"] }
axum = "0.8"
tower = "0.5"
tokio-postgres = { version = "0.7", features = ["with-serde_json-1"] }
rustls = { version = "0.23", default-features = false, features = ["ring", "std", "tls12", "logging"] }
tokio-postgres-rustls = "0.14"
arrow = "60"
utoipa = { version = "6", features = ["chrono"] }
```

## Workspace lints

The policy, from `[workspace.lints]` and applied per crate via
`[lints] workspace = true`:

```toml
unsafe_code = "deny"
all = { level = "warn", priority = -1 }
pedantic = { level = "warn", priority = -1 }
```

Every `allow` and the comment that justifies it. The reason is the
comment in `Cargo.toml`, so an allow added without one shows up here as
a blank cell:

| Allowed | Why |
|---|---|
| `cast_possible_truncation`, `cast_precision_loss`, `cast_possible_wrap`, `cast_sign_loss` | Numeric engine: i64/usize/f64 conversions are pervasive and intentional. |
| `float_cmp` | We deliberately compare trained/fixed float params for exact equality. |
| `doc_markdown` | Doc prose mentions DuckDB, DuckLake, SQL — not code spans. |
| `missing_errors_doc`, `missing_panics_doc` | Public API documents errors/panics in prose, not //! sections. |
| `must_use_candidate`, `module_name_repetitions`, `too_many_lines`, `items_after_statements`, `struct_excessive_bools` | Style preferences that fight the shape of this codebase. |
| `unreadable_literal` | Determinism-critical float/hash constants (CPython float-repr thresholds, sampling moduli) — reformatting risks a transcription error for zero functional gain. |
| `needless_pass_by_value` | CLI command handlers and builder entry points legitimately take their args by value (they own the parsed config); borrowing would just add noise. |
<!-- END GENERATED: crate-map -->
