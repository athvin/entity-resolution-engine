# The crate map

Generated from the tree by `scripts/skills-check.sh regen`. Do not hand-edit
below the marker — `make skills-check` regenerates it and fails on drift. Each
purpose is the crate's own leading `//!` sentence, so it is the crate's
self-description rather than a second one maintained here.

Reading it: the crates divide into the engine (`dedupe-*`), the pipeline stages
(`er-ingest`, `er-standardize`, `er-match`, `er-reconcile`, `er-golden`), the
entity framework (`er-entity-*`, `er-entities`, `er-sql-check`), the control
plane (`er-server`, `er-pg-tls`), the CLIs, and dependency-light primitives
(`er-ids`, `er-hash`, `er-schema`, `er-errors`, `er-env`, `er-obs`,
`er-versions`) that the rest can depend on without pulling a database into
their graph.

Three pins in the version block are not free choices:

- `duckdb` is an **`=` pin**, so a DuckDB change is always deliberate and
  reviewable — it can move emitted SQL and float output, which the goldens
  compare byte-for-byte.
- `arrow` must equal the major the `duckdb` crate pulls.
- `datafusion` (in `er-sql-check`, not the workspace table) must equal the
  major `ff-analysis` resolves.

Bumping `arrow` or `datafusion` alone can never go green; dependabot is
configured to leave them alone and the unblocker is upstream.

<!-- BEGIN GENERATED: crate-map -->
**28 crates.** The cargo workspace root is `rust/`, not the repo root.

| Crate | Purpose |
|---|---|
| `dedupe-cli` | CLI for dedupe-core: train/predict/cluster against a DuckLake catalog |
| `dedupe-core` | dedupe-core: a Fellegi-Sunter probabilistic record-linkage engine, narrowly scoped to DuckDB/DuckLake execution |
| `er-cli` | `er-cli` — orchestrate the ER pipeline stages over a real DuckLake lake (P2) |
| `er-compile` | Compile a tenant's declared `comparisons:` / `blocking:` into the settings document the engine loads |
| `er-config` | The S6 configuration document and its `config_hash` (port of `src/er/config/{schema,hashing,loader}.py`, DesignDoc.md S6/S6.1/S5.2) |
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
| `er-match` | The matching stage (port of `src/er/matching/*`, DesignDoc.md S4.3) |
| `er-obs` | Observability: the S5.2 counter vocabulary and structured stage log line (ports of `src/er/obs/counters.py` and `src/er/obs/logging.py`) |
| `er-pg-tls` | `er-pg-tls` — the TLS connector for every Postgres client in the workspace |
| `er-reconcile` | The reconcile / entity-resolution stage (port of `src/er/entities/*` + `src/er/review/*`, DesignDoc.md S4.5) |
| `er-schema` | The S5 schema registry (Rust port of `src/er/lake/*`), dependency-free so `config`/`golden`/tests can consume it without pulling a DB into their graph |
| `er-server` | `er-server` — the Rust control plane (P3), an axum port of the Python `erserver` FastAPI app (`server/src/erserver/api.py`) |
| `er-sql-check` | `er-sql-check` — the static analyser |
| `er-standardize` | The standardization stage (port of the retired dbt models `stg_records.sql` + `int_std_records.sql`, DesignDoc.md S4.2) |
| `er-std` | Standardization primitives (port of `src/er/std/*`, DesignDoc.md S4.2) |
| `er-versions` | The config-drift guard (drift-guard half of `src/er/versions.py`): the run fingerprint, the S4.0 mode-precondition decision, and the S5.1 rebuild-reason |

Pinned versions that carry a reason (from `rust/Cargo.toml`):

```toml
edition = "2024"
rust-version = "1.88"
duckdb = { version = "=1.10506.1", features = ["bundled"] }
sqlparser = { version = "0.63", features = ["visitor"] }
thiserror = "2"
anyhow = "1"
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls"] }
axum = "0.8"
tower = "0.5"
tokio-postgres = { version = "0.7", features = ["with-serde_json-1"] }
rustls = { version = "0.23", default-features = false, features = ["ring", "std", "tls12", "logging"] }
tokio-postgres-rustls = "0.13"
arrow = "58"
utoipa = { version = "6", features = ["chrono"] }
```
<!-- END GENERATED: crate-map -->
