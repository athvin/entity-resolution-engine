# Entity Resolution Platform — Technical Specification

**Version:** 1.1
**Stack:** Python · Splink 4 (DuckDB backend) · dbt-duckdb · DuckLake (Postgres catalog + S3-compatible object store) · Docker Compose · GitHub Actions

This document is the sole specification. It is self-contained: every algorithm, schema, threshold and invariant an implementer needs is stated here, not delegated. **MUST** marks a normative requirement; a conforming implementation satisfies all of them. Named invariants (INV-PERM, INV-EQ, INV-SCORE, CONTRADICTION-1) are defined once, at the section indicated, and cited by name everywhere else.

---

<a id="s1"></a>
## 1. Scope & Goals

Build the entity resolution and golden record pipeline as a testable, benchmarkable codebase:

- **G1 — Correctness under test.** Every pipeline stage has automated tests that run in CI on every PR, against realistic fixtures, inside Docker Compose.
- **G2 — Entity permanence proven, not assumed.** CI includes scenario tests that assert entity IDs survive merges, splits, deletions and full re-resolution, per **INV-PERM** (S4.5).
- **G3 — Incremental processing proven, with stated preconditions.** CI includes a test that processes a base corpus, then an incremental batch, and asserts (a) that under the **INV-EQ** preconditions (S4.5) the incremental run yields the *same set-partition of current members* as a from-scratch full run — entity IDs may differ between the two universes and are compared as set-partitions — and (b) that the incremental run touched only the affected subgraph. G3 is **not** an unqualified equivalence claim: where an INV-EQ precondition is violated, the divergence MUST be exhibited by a test, bounded, and repaired by the periodic correction pass (S4.3) rather than asserted away.
- **G4 — Benchmarkable.** A benchmark harness measures per-stage throughput and match quality at multiple corpus scales. Runnable locally, on demand via CI (`workflow_dispatch`) and on a weekly schedule (S9.2), never on the PR path.

**Tenancy.** v1 is **single-tenant per lake**. `tenant` names the lake namespace — the DuckLake catalog metadata schema and the S3 prefix — and is **not** a row-level discriminator. No table has a `tenant` column. Two tenants means two lakes, two catalog schemas and two prefixes; cross-tenant matching is impossible by construction rather than by predicate.

**In scope for v1 and new since 1.0:** record deletion, retraction and supersession (`raw_records.is_deleted`/`deleted_at`, `er ingest --full-refresh-keys` tombstone derivation, edge invalidation, `member_removed` / `split` / `retired` emission, and resurrection of a re-appearing key). The `entity_events` and `entities` status domains are therefore fully reachable.

**Out of scope for v1:** the embedding coherence *implementation* (S11 ships the interface and a `NoopScorer` only), LLM-assisted review triage, a multi-tenant serving API, and Kubernetes manifests (Compose is the test/dev substrate; k8s deployment is a separate specification). Concurrent writers against one lake namespace are an explicit non-guarantee: v1 is a single-writer batch model, and the advisory lock that enforces it is defined in S4.0b.

---

<a id="s2"></a>
## 2. Technology Choices

| Concern | Choice | Notes |
|---|---|---|
| Language | Python 3.12 | Splink is Python-native; Rust ports of hot paths are a later optimization once benchmarks identify them |
| Package/env manager | `uv` | Lockfile-based (`uv.lock` committed, `uv sync --frozen` everywhere), fast in CI |
| Matching | Splink 4, DuckDB backend | Exact pin `splink==4.0.16` (S2.1). The Splink 3 API is incompatible; Splink 5 removes `find_matches_to_new_records`, on which incremental scoring (S4.3) depends — migration is tracked as an S13 risk and is confined to `src/er/matching/` |
| Transformations | dbt-core + dbt-duckdb | Standardization, blocking keys, golden assembly as dbt models; dbt runs as a subprocess (S4.0b) |
| Storage format | DuckLake | Catalog = Postgres in Compose; object store = S3-compatible (MinIO in Compose). DuckLake enforces `NOT NULL` only — see S5.0 for the key model this forces |
| Query engine | DuckDB, one version by construction | dbt-duckdb executes **in-process against the installed `duckdb` wheel**, so `uv.lock` already guarantees a single engine version — there is no dbt-vs-Python skew axis to police. The pins that actually matter are (a) the `ducklake` / `postgres` / `httpfs` **extension binaries matched to that engine build**, baked at Docker build time into `/opt/duckdb_extensions` with `autoinstall_known_extensions=false` at runtime, and (b) a DuckDB version at or above dbt-duckdb's DuckLake floor, since dbt-duckdb branches on `duckdb_version` for ALTER/RENAME workarounds and MERGE availability. `er doctor` asserts both |
| Orchestration (v1) | Python CLI (`typer`) invoking stages in order | No Airflow/Dagster in v1; the CLI contract (S4.0), including its exit codes, is the interface orchestrators wrap later |
| Containerization | Docker Compose | One image for the pipeline; services for catalog + object store; images pinned by digest (S2.1) |
| CI/CD | GitHub Actions | static → unit → integration (Compose) → dbt tests; benchmark as a manual/weekly workflow, never on the PR path |
| Lint/type | `ruff` (lint + `ruff format --check`), `mypy --strict src/er` | The same command string in S2, S8.1 and S9.1 — no `core/` directory exists and none is referenced |
| Testing | `pytest`, `pytest-xdist` (unit layer only), dbt data tests | Integration tests run single-process against a namespaced ephemeral lake (S8.1) |
| Config | Pydantic-validated YAML | One config document per lake: the fourteen S6 config blocks (`tenant`, `thresholds`, `standardization`, `sources`, `blocking`, `comparisons`, `survivorship`, `training`, `storage`, `versions`, `generator`, `clustering`, `coherence`, `correction_pass`). `config_hash` is defined normatively in S5.2 |

---

<a id="s2-1"></a>
## 2.1 Pinned Versions

Every version below is a **literal pin**, not a floor. `er doctor` (S4.0) runs, at runtime, **every check below whose *Asserted by* cell names `er doctor`**, plus the six runtime assertions enumerated in T-DOCTOR-1 (S8.3); it prints one line per check and exits `1` if any check fails (`1` — not `3` — because a pin mismatch is a check failure under the S4.0 exit-code table, and exit `3` is reserved there for the five named precondition failures). The integration job runs `er doctor` as its first step. The three **Compose service images** are pinned by `@sha256:` **digest**, never by mutable tag; the Dockerfile's build-stage base images are pinned by tag (S7.3).

| Component | Pin | Why it is pinned | Asserted by |
|---|---|---|---|
| Python | `3.12` (`requires-python = ">=3.12,<3.13"`) | Splink 4 and dbt-core support matrix; the runtime image and CI runner MUST agree | `er doctor`: `sys.version_info[:2] == (3, 12)`; `pyproject.toml`; the `python:3.12-slim` base image tag in `docker/Dockerfile` (S7.3) |
| Splink | `splink==4.0.16` | Splink 5 removes `find_matches_to_new_records`, the primitive incremental pass 1 (S4.3) is built on; Splink 3's API is incompatible. Exact, not `>=` | `er doctor`: `splink.__version__`; `uv.lock` |
| DuckDB (Python wheel) | `duckdb==1.5.5` | The single engine for both Python and dbt-duckdb; extension binaries are built per engine version | `er doctor`: `duckdb.__version__`; `uv.lock` |
| dbt-core | `dbt-core==1.12.2` | Adapter/protocol compatibility with dbt-duckdb; `on_schema_change` and contract semantics (S4.2) are version-sensitive | `er doctor`: `dbt.version.__version__`; `uv.lock` |
| dbt-duckdb | `dbt-duckdb==1.11.0` | Must be at or above the DuckLake floor; it branches on `duckdb_version` for ALTER/RENAME and MERGE behaviour | `er doctor`: adapter version + `dbt debug --target mem`; `uv.lock` |
| dbt-adapters / dbt-common | `dbt-adapters==1.24.5`, `dbt-common==1.39.0` | Transitive, but they carry the contract-enforcement and column-type machinery S5.0 relies on | `uv.lock` |
| DuckDB extension `ducklake` | `d8a1881e` | The lake format itself; a mismatched extension against the pinned engine fails at ATTACH, not at query time | `er doctor`: `duckdb_extensions()` + `SELECT * FROM lake.snapshots()` |
| DuckDB extension `postgres` | `41223e5`, reported by `duckdb_extensions()` under the name **`postgres_scanner`** | Catalog connectivity; accepts `postgresql://` DSNs directly. `INSTALL postgres` is the install name but `postgres_scanner` is the registered name — `er doctor` MUST assert the latter, or the check silently finds no row and passes | `er doctor`: `duckdb_extensions()` filtered on `postgres_scanner`; catalog round-trip |
| DuckDB extension `httpfs` | `827222f` | S3 data-file I/O for `ER_LAKE_DATA_PATH` | `er doctor`: `duckdb_extensions()`; write/read round-trip to `DATA_PATH` |
| `uv` | `uv==0.11.3` | The resolver produces the lockfile every other pin derives from; a different `uv` can resolve a different lock. This is the version that produced the committed `uv.lock` | `er doctor`: `uv --version`; CI `astral-sh/setup-uv` pinned to a commit SHA with `version:` set; the `ghcr.io/astral-sh/uv:0.11.3` tag in `docker/Dockerfile` |
| `ruff` | `ruff==0.16.3` | Lint and format output changes between minors; an unpinned formatter turns every PR into a diff | `er doctor`; CI static job |
| `mypy` | `mypy==2.3.0` | `--strict` gains checks between releases; the gate MUST be reproducible | `er doctor`; CI static job (`mypy --strict src/er`) |
| `pytest` | `pytest==9.1.1`, `pytest-xdist==3.8.0` | Fixture-scope and `--junitxml` schema stability across the CI artifact contract | `er doctor`; CI unit + integration jobs |
| `hypothesis` | `hypothesis==6.165.7` | Property-based normalizer tests (S8.4); shrinking behaviour and the deadline default change between minors | CI unit job |
| `actionlint` | `actionlint-py==1.7.7.23` — the wheel ships the `actionlint` binary, which is why `scripts/actionlint.py` downloads nothing | The workflow linter that enforces SHA-pinned `uses:` (S9). A binary fetched at CI time is an unpinned dependency whose findings change under the gate it is guarding | `er doctor`: `actionlint --version`; `uv.lock`; CI static job |
| `typer` / `pydantic` / `python-ulid` | `typer==0.27.1`, `pydantic==2.13.4`, `python-ulid==4.0.1` | The CLI contract (S4.0), config validation (S6) and every minted identifier (S5.0) | `er doctor`; `uv.lock` |
| `boto3` | `boto3==1.43.72` | Object-store prefix listing and deletion for `er lake reset` and the S8.1 harness teardown. DuckDB's `httpfs` can read and write objects but cannot enumerate or delete a prefix, and the S4.0b connection model forbids `lake/objectstore.py` from opening a DuckDB connection at all | `er doctor`; `uv.lock` |
| `psycopg` | `psycopg[binary]==3.3.4` | The tenant advisory lock (S4.0b) and catalog-schema teardown must run on a Postgres connection the DuckLake attachment does not own, since the lock outlives every DuckDB connection in the process | `er doctor`; `uv.lock` |
| Catalog image | `postgres:16@sha256:11a9d238fbb48bab14599c57e41123254452b1a2d93c6c8595bce96f346bd082` | DuckLake catalog; a floating `:16` tag silently changes the catalog engine under a committed lake. The digest is the multi-arch index digest, so it resolves on both amd64 CI and arm64 developer machines | `er doctor`: `server_version`; `docker/compose.yaml` |
| Object store image | `minio/minio:RELEASE.2025-09-07T16-13-09Z@sha256:14cea493d9a34af32f524e538b8346cf79f3321eff8e708c1e2960462bd8936e` | S3-compatible data-file store. **This image is EOL and unmaintained** — upstream archived it at this release and it receives no further security fixes. Retained as the v1 test/dev substrate only, pinned by digest so the substrate cannot drift; migration to a maintained S3-compatible store is an S13 risk row | `er doctor`: bucket round-trip; `docker/compose.yaml` |
| Object-store init image | `minio/mc:RELEASE.2025-08-13T08-35-41Z@sha256:a7fe349ef4bd8521fb8497f55c6042871b2ae640607cf99d9bede5e9bdf11727` | Bucket creation in `objectstore-init`; same EOL caveat as above | `docker/compose.yaml` |

Rules governing this table:

- Every pin MUST be a literal before the document is released. For DuckDB extensions a literal is the `extension_version` commit hash reported by `duckdb_extensions()` against the pinned engine build, not a semver. `scripts/lint_spec.py` declares the placeholder patterns and fails if any of them survives in S2.1, so an unresolved pin cannot ship in a released doc.
- `uv.lock` is the authority for Python-package pins; S2.1 restates them so `er doctor` and reviewers have one table to read. The static job cross-checks this table against `uv.lock` and `docker/compose.yaml` and fails on disagreement; **S9.1 owns the list of what that lint enforces** and this bullet does not restate it.
- GitHub Actions are pinned to commit SHAs, not tags (S9).
- Adding a dependency means adding a row here. `er doctor` iterates this table; a component with no row is not asserted and therefore is not pinned.

---

<a id="s3"></a>
## 3. Repository Layout

```
entity-resolution-engine/
├── DesignDoc.md                      # this specification (the only design document)
├── pyproject.toml                    # uv-managed; every dependency pinned per S2.1
├── uv.lock                           # committed; `uv sync --frozen` everywhere
├── docker/
│   ├── Dockerfile                    # multi-stage: builder runs `uv sync --frozen`; runtime copies the venv
│   │                                 #   and bakes ducklake/postgres/httpfs into /opt/duckdb_extensions
│   └── compose.yaml                  # catalog (postgres), objectstore, objectstore-init, catalog-init,
│                                     #   pipeline, benchmark; profiles: test, bench
├── configs/
│   ├── default.yaml                  # reference config: all fourteen S6 config blocks
│   └── test.yaml                     # config used by fixtures and CI; `tenant: test`
├── src/er/
│   ├── __init__.py
│   ├── cli.py                        # `er` CLI (S4.0): init, doctor, ingest, standardize, train, match,
│   │                                 #   reconcile, assemble, run-all, correct, assert, review,
│   │                                 #   lake maintain, lake reset
│   ├── errors.py                     # error taxonomy → exit codes 0/1/2/3/10; retryable vs terminal
│   ├── versions.py                   # std_version / survivorship_version / address_parser_version /
│   │                                 #   code_version resolution; version-compat guards for run-all
│   ├── dbt_runner.py                 # subprocess wrapper for dbt (select, vars, target, log capture,
│   │                                 #   snapshot-range capture); no Python DuckDB connection spans it
│   ├── config/
│   │   ├── schema.py                 # Pydantic models for the S6 config document
│   │   ├── loader.py                 # load/validate from --config or $ER_CONFIG; fail fast, exit 2
│   │   └── hashing.py                # config_hash, defined normatively in S5.2
│   ├── lake/
│   │   ├── ducklake.py               # INSTALL/LOAD, CREATE SECRET, ATTACH lake, DETACH, snapshot helpers
│   │   ├── catalog.py                # Postgres catalog access: metadata schema, advisory lock on tenant
│   │   ├── objectstore.py            # S3 client: DATA_PATH round-trip, model artifact put/get, prefix reap
│   │   ├── ddl.py                    # CREATE TABLE IF NOT EXISTS for ddl.py-owned relations only;
│   │   │                             #   the owner split is normative in S5.0
│   │   ├── columns.py                # VOLATILE_COLUMNS — the single definition, excluded from every
│   │   │                             #   determinism comparison (S5.0)
│   │   ├── model.py                  # model artifact + model_registry: version allocation, upload to
│   │   │                             #   storage.model_uri_prefix, active/superseded pointer, tf_snapshot_id
│   │   └── maintain.py               # merge_adjacent_files → expire_snapshots → cleanup_old_files
│   ├── ingest/
│   │   ├── sources.py                # source adapters (v1: CSV/Parquet drop folder)
│   │   ├── hashing.py                # content_hash: NFC, 0x1f-joined, declared column order (S4.1)
│   │   └── landing.py                # anti-join append into raw_records; tombstone derivation for
│   │                                 #   --full-refresh-keys; ingest_batches manifest row
│   ├── matching/
│   │   ├── model.py                  # Splink 4 settings builder + blocking_rules_from_config()
│   │   ├── train.py                  # full-corpus training; writes model JSON + tf_lookup + registry row
│   │   ├── incremental.py            # two-pass incremental scoring (new-vs-corpus, new-vs-new)
│   │   ├── full.py                   # corpus-wide predict + cluster; also the correction pass
│   │   └── tf.py                     # tf_lookup materialization and register_term_frequency_lookup
│   ├── entities/
│   │   ├── cluster.py                # affected node+edge set, label propagation to fixpoint, cut_edges
│   │   ├── reconcile.py              # overlap-matrix cluster→entity mapping (INV-PERM), merge/split
│   │   ├── ids.py                    # record_key, pair canonicalisation, IdFactory, ULIDs, resolve()
│   │   └── events.py                 # entity_events append + replay fold
│   ├── golden/
│   │   └── assemble.py               # touched/retired sets → er_touched_entities → dbt marts → reap
│   ├── review/
│   │   ├── assertions.py             # always/never application, precedence, CONTRADICTION-1
│   │   └── queue.py                  # review_queue upsert, resolution → assertions in one transaction
│   ├── eval/
│   │   ├── __init__.py
│   │   └── metrics.py                # pairwise_metrics(): blocking recall, edge-level, cluster-level —
│   │                                 #   the one implementation used by both tests and benchmarks
│   └── embeddings/                   # phase 2: interface only in v1
│       └── coherence.py              # CoherenceScorer protocol + NoopScorer
├── dbt/
│   ├── dbt_project.yml
│   ├── profiles/profiles.yml         # targets: `lake` (DuckLake-attached) and `mem` (:memory:, no attach)
│   ├── models/
│   │   ├── staging/
│   │   │   ├── stg_crm.sql
│   │   │   ├── stg_billing.sql
│   │   │   └── stg_webforms.sql
│   │   ├── intermediate/
│   │   │   ├── int_std_records.sql   # carries record_key, content_hash, std_version
│   │   │   └── int_blocking_keys.sql # macro-generated from the config blocking payload
│   │   ├── marts/
│   │   │   ├── golden_records.sql
│   │   │   ├── golden_lineage.sql
│   │   │   └── golden_display.sql    # presentation casing only; never read by the matching layer
│   │   └── schema.yml                # contract: {enforced: true} on every dbt-owned model + data tests
│   ├── tests/                        # singular tests: record_key has no ':', pair canonical ordering,
│   │                                 #   one current std row per record, membership references active entity
│   ├── macros/
│   │   ├── std/                      # lowercase_trim, email_norm, phone_e164, name_norm, null_semantics
│   │   ├── blocking/                 # int_blocking_keys UNION ALL generator
│   │   └── survivorship/             # source_priority, recency, frequency, completeness, validated
│   └── seeds/
│       └── nickname_variants.csv
├── models/                           # local staging dir for model JSON written by `er train` before upload
│                                     #   to storage.model_uri_prefix; git-ignored except .gitkeep
├── fixtures/
│   ├── generator/                    # seeded synthetic generator, shared by fixtures and benchmarks
│   │   ├── personas.py               # ground-truth persons with realistic name/email frequency skew
│   │   ├── corruptions.py            # typos, nicknames, format drift, missingness, stale addresses
│   │   └── emit.py                   # per-source record emission
│   └── static/                       # small, hand-authored, committed fixture sets (S8.2)
│       ├── model_test_v1.json        # committed frozen Splink model; scenario tests load it, never train
│       ├── base_10/                  # every scenario has the S8.2.1 shape:
│       │   ├── base/                 #   per-source input CSVs (crm.csv, billing.csv, webforms.csv)
│       │   ├── tf_flip_pairs.csv     #   scenario-root auxiliary file: the T-TF-1 flip bound
│       │   └── expected/
│       │       └── base/             #   membership.csv, golden.csv, events.csv, std_hashes.csv,
│       │                             #     assertions.csv — symbolic entity labels (S8.2.1)
│       ├── incremental_batch/
│       │   ├── base/
│       │   ├── batch/                # incremental delivery
│       │   ├── parity_pairs.csv      # the derived pair set T-INC-3 scores through both paths
│       │   ├── tf_flip_pairs.csv     # the T-INC-1b divergence bound
│       │   └── expected/{base,batch}/
│       ├── merge_scenario/
│       │   ├── base/
│       │   ├── batch/
│       │   └── expected/{base,batch}/
│       ├── split_scenario/
│       │   ├── base/
│       │   ├── batch/
│       │   ├── assertions.csv        # input assertions with their phase column (S8.2.1)
│       │   └── expected/{base,batch}/
│       ├── assertions_scenario/
│       │   ├── base/
│       │   ├── batch/
│       │   ├── assertions.csv
│       │   └── expected/{base,batch}/
│       ├── deletion_scenario/
│       │   ├── base/
│       │   ├── refresh/              # --full-refresh-keys delivery from which tombstones are derived
│       │   ├── resurrect/            # ordinary delivery re-appearing one tombstoned key (S8.2.1)
│       │   └── expected/{base,refresh,resurrect}/
│       └── supersession_scenario/
│           ├── base/
│           ├── batch/                # same keys, changed content_hash
│           └── expected/{base,batch}/
├── tests/
│   ├── conftest.py                   # namespaced ephemeral DuckLake per session (S8.1)
│   ├── helpers/
│   │   ├── compare.py                # assert_partition_equal / assert_ids_stable / assert_golden_equal
│   │   └── expected.py               # expected-file loaders, symbolic-label binding, null token
│   ├── unit/                         # normalizers, reconciler, ids, blocking generator, and:
│   │   ├── test_config.py            #   the fifteen S6.1 validators, one test each
│   │   └── test_fixture_lint.py      #   expected-file sort order, null token, header literals (S8.2.1)
│   ├── integration/                  # full `er run-all` against Compose services; the S8.3 scenarios,
│   │   ├── test_fixture_integrity.py #   base_10 truth counts recomputed from the committed CSVs
│   │   └── test_invariants.py        #   the node id T-INV-1's autouse finalizer reports under
│   └── fixtures/                     # test-owned reference material (e.g. prior spec revisions)
├── benchmarks/
│   ├── run_benchmark.py
│   ├── scales.yaml                   # smoke / 10k / 100k / 1m definitions
│   ├── scales.py                     # reads scales.yaml; --scale S --field F for the S9.2 preflight
│   ├── report.py                     # --run / --compare / --write-baseline / --repeat
│   └── baselines/                    # committed baseline JSON per scale, updated deliberately via PR
│       ├── smoke.json
│       ├── 10k.json
│       └── 100k.json
├── artifacts/                        # bind-mounted to /app/artifacts: junit.xml, dbt logs, run manifests,
│                                     #   bench/latest.json; git-ignored except .gitkeep
├── scripts/
│   ├── board.py                      # ticket board read/write over docs/implementation/BOARD.md
│   ├── gates.sh                      # the local gate chain: ruff → mypy --strict → unit → dbt parse
│   ├── run-loop.sh                   # autonomous implementation loop driver
│   ├── lint_spec.py                  # spec lint; S9.1 is the authority for what it enforces
│   ├── lint_board.py                 # board lint: ticket ids, states, dependency closure
│   ├── lint_metrics.py               # fails on a second precision/recall definition (S8.5, S9.1)
│   ├── actionlint.py                 # runs the actionlint binary shipped by the pinned
│   │                                 #   actionlint-py wheel (S2.1); downloads nothing (S9.1)
│   └── ci/                           # helper scripts invoked by the workflows
├── .claude/
│   └── skills/                       # repo-scoped agent skills used by the implementation loop
├── docs/
│   ├── gap-report-v1.0.md            # the v1.0 review this revision closes
│   └── implementation/
│       └── BOARD.md                  # ticket board (ER-NNN): state, dependencies, exit criteria
└── .github/
    ├── dependabot.yml                # ecosystem: github-actions; proposes SHA bumps (S9)
    └── workflows/
        ├── ci.yaml                   # PR path: static → unit → integration
        └── benchmark.yaml            # workflow_dispatch + weekly schedule
```

Layout rules that are normative:

- `models/` and `artifacts/` are working directories, not sources of truth: `models/` stages a model JSON before it is uploaded to `storage.model_uri_prefix`, and `er match` MUST load the model from the registry URI, never from the local path. `artifacts/` MUST be a Compose bind mount so CI can upload it.
- `expected/` lives **inside each scenario directory**. There is no top-level `fixtures/expected/`.
- Every relation named in S5 is owned by exactly one of `src/er/lake/ddl.py` or a file under `dbt/models/` (S5.0). A relation appearing in both is a defect that `scripts/lint_spec.py` fails on.

---

---

<a id="s4"></a>
## 4. Pipeline Stages — Component Specs

The CLI is the orchestration contract. Every stage is idempotent.

A stage MAY commit **many** DuckLake snapshots — dbt-backed stages commit one per model, and the label-propagation and scoring loops commit one per persisted statement. Each stage therefore records the snapshot **range** it produced in `run_stages(snapshot_start, snapshot_end)`; **the range is the unit of time travel and rollback**. No stage may claim "exactly one snapshot per stage", and no test may assert on snapshot counts. A no-op run may legitimately produce empty snapshots.

Idempotency is defined per stage against a concrete key. Re-executing a stage with the same key MUST leave the lake logically unchanged (row counts and non-`VOLATILE_COLUMNS` content identical) and MUST exit `10` when it has nothing to do.

| Stage | Idempotency key | Re-execution behaviour |
|---|---|---|
| `ingest` | `(source_system, source_record_id, content_hash)` | Anti-join append; a re-delivered identical file appends 0 rows, writes an `ingest_batches` row with `new_count = 0` and `changed_count = 0`. |
| `standardize` | `ingest_batch_id` (staging), `(source_system, source_record_id)` (intermediate) | dbt incremental predicates below; no duplicate staged rows, intermediate rows replaced in place. |
| `train` | `(config_hash, corpus_snapshot)` | Allocates a new `model_version` on every invocation unless `--if-changed` is passed, which exits `10` when the active `model_registry` row already carries this key. |
| `match` | `(model_version, tf_snapshot_id, rec_a_key, rec_b_key)` | `MERGE INTO lake.main.match_scores` on that key; re-scoring a pair whose two endpoint content hashes are unchanged rewrites the same row with the same probability (INV-SCORE). |
| `reconcile` | `(run_id, entity_id, event_type, details_hash)` for events; the partition itself for membership | A re-run over an unchanged edge set and an unchanged `P_old` mints no ULID, rewrites no membership row, and emits zero events. |
| `assemble` | `entity_id` | `delete+insert` over the touched set plus the explicit reap step; untouched entities keep their prior `assembled_at`. |

Every stage writes exactly one `run_stages` row on entry (`status='running'`) and updates it on exit with `status`, `ended_at`, `rows_in`, `rows_out`, `snapshot_start`, `snapshot_end`, `error_class`, `error_detail`, and a `counters JSON` payload holding the named counters listed at the end of each subsection. The same record is emitted to **stderr** as exactly one JSON line keyed by `run_id` (S5.2 owns the logging contract); `--json` switches *stdout* from the human summary to JSONL and never moves the stderr line.

<a id="s4-0"></a>
### 4.0 CLI contract

Global flags, accepted by **every** command: `--config PATH` (default `$ER_CONFIG`), `--run-id ULID` (default: minted; `run-all` mints once and threads it to every child stage), `--json` (emit machine-readable JSONL to stdout instead of the human summary). Config is Pydantic-validated at process start; an invalid document exits `2` before any connection is opened.

Exit codes are uniform across all commands:

| Code | Meaning |
|---|---|
| `0` | Success. |
| `1` | Stage failure (an operation was attempted and failed). |
| `2` | Config or validation error (bad YAML, failed Pydantic validation, unknown source, unknown survivorship rule, `0 < review_low < auto_merge <= 1` violated). |
| `3` | Precondition failure: lake not initialised, no `status='active'` model, advisory lock not acquired, mixed `model_version` above `review_low`, breaking (non-additive) schema change detected. |
| `10` | Nothing to do (empty delivery, no changed records, no touched entities, no pending assertions). |

| Command | Flags (defaults) | Required env | Exit codes | stdout |
|---|---|---|---|---|
| `er init` | `--force` (false) | `ER_CATALOG_DSN`, `ER_S3_*`, `ER_LAKE_DATA_PATH`, `ER_LAKE_ALIAS`, `ER_LAKE_METADATA_SCHEMA` | `0`; `3` if `DATA_PATH` disagrees with the catalog; `1` on DDL failure | One line per relation: `created` / `exists`; with `--json`, `{relation, action}` |
| `er doctor` | none | all lake env | `0` all checks pass; `1` any check fails | Check table: name, expected, actual, verdict |
| `er ingest` | `--source NAME` (required), `--path DIR` (required), `--full-refresh-keys` (false) | `ER_CONFIG`, lake env | `0`; `10` when `new=0 and changed=0 and tombstoned=0`; `2` unknown source **or** an empty `--full-refresh-keys` delivery (the S4.1.1 guard); `1` unreadable/unparsable input | Manifest line: `source, files, new, changed, unchanged, tombstoned, resurrected, ingest_batch_id` |
| `er standardize` | `--changed-only` (false) | `ER_CONFIG`, lake env | `0`; `10` when no batch is unprocessed and `--changed-only`; `1` dbt non-zero | dbt model result summary, one line per model |
| `er train` | `--if-changed` (false) | `ER_CONFIG`, lake env | `0`; `10` with `--if-changed` and unchanged `(config_hash, corpus_snapshot)`; `1` EM failure; `2` incomplete `training:` block | `model_version, params_path, tf_snapshot_id, corpus_snapshot, metrics` |
| `er match` | `--mode incremental\|full` (required), `--model-version V` (default: the `status='active'` row), `--new-tf-snapshot` (false; `--mode full` only, used by `er correct`) | `ER_CONFIG`, lake env | `0`; `3` no active model; `10` incremental mode with no unscored records; `1` scoring failure | `mode, model_version, tf_snapshot_id, candidate_pairs, pairs_scored, pairs_above_auto_merge, review_queue_added` |
| `er reconcile` | none | `ER_CONFIG`, lake env | `0`; `1` non-convergence or CONTRADICTION-1; `3` mixed `model_version` above `review_low`; `10` empty affected set | `affected_entities, entities_created, entities_merged, entities_split, entities_retired, edges_cut` |
| `er assemble` | `--touched-only` (false) | `ER_CONFIG`, lake env | `0`; `10` `--touched-only` with an empty `er_touched_entities` set for this `run_id`; `1` dbt non-zero | `entities_rebuilt, entities_reaped, lineage_rows` |
| `er run-all` | `--mode incremental\|full` (required), `--source NAME`, `--path DIR`, `--skip-ingest` (false), `--allow-escalate` (false), `--resume RUN_ID` | `ER_CONFIG`, lake env | `0` when every stage returns `0` or `10` — `10` is "nothing to do", not a failure, and never aborts the chain; otherwise the first child exit code **other than `0` or `10`** propagates; `3` on the config-drift guard | One line per stage plus a final run summary |
| `er correct` | none | `ER_CONFIG`, lake env | `0`; `3` no active model; `10` when the pass changes no edge and no membership row; `1` scoring, clustering or assembly failure | `tf_snapshot_id, pairs_scored, entities_changed, entities_rebuilt` |
| `er assert` | `add --a KEY --b KEY --kind always\|never --by USER [--note TEXT]` \| `remove --assertion-id ID --by USER` \| `load --path FILE` | `ER_CONFIG`, lake env | `0`; `2` malformed key or unknown kind; `1` rejected conflicting insert | `assertion_id, rec_a_key, rec_b_key, kind, active` |
| `er review` | `list [--status open] [--limit 100]` \| `resolve --review-id ID --as match\|no_match\|dismiss --by USER` | `ER_CONFIG`, lake env | `0`; `10` empty list; `2` unknown `review_id` | Rows of `review_id, subject_type, keys, match_probability, status` |
| `er lake maintain` | `--retain-days N` (7) | lake env | `0`; `3` lock not acquired; `1` maintenance failure | `files_merged, snapshots_expired, files_deleted` |
| `er lake reset` | `--confirm-tenant NAME` (required) | lake env | `0`; `2` tenant mismatch; `3` lock not acquired | `dropped_schema, deleted_prefix` |

**`er init`** installs/loads extensions, creates the S3 secret, ATTACHes the lake, issues `CREATE TABLE IF NOT EXISTS` for the **`ddl.py`-owned relations only** — the owner split it obeys is normative in S5.0 — and DETACHes. It is idempotent and single-writer. If the catalog's recorded `DATA_PATH` differs from `$ER_LAKE_DATA_PATH`, `er init` exits `3` with the literal message `lake DATA_PATH immutable: catalog=<a> env=<b>; use 'er lake reset --confirm-tenant <tenant>' to destroy and recreate this namespace`.

**`er lake reset`** destroys the namespace: it takes the same advisory lock every other writer takes (S4.0b), drops the catalog metadata schema and deletes the `DATA_PATH` prefix. It is a writer like any other and is recorded like one — `runs.mode='reset'` and one `run_stages` row with `stage='reset'` (both enum values are declared in S5's DDL). It exits `2` when `--confirm-tenant` does not match `tenant` in the config, so the destructive path cannot be reached by a typo.

**`er run-all` stage chains** (it **NEVER** trains; `er train` is always an explicit, separate invocation):

```
--mode incremental :  er ingest --source S --path P        # skipped with --skip-ingest
                   →  er standardize --changed-only
                   →  er match --mode incremental
                   →  er reconcile
                   →  er assemble --touched-only

--mode full        :  er ingest --source S --path P        # skipped with --skip-ingest
                   →  er standardize
                   →  er match --mode full
                   →  er reconcile
                   →  er assemble
```

**`er correct` — the periodic correction pass.** It is the CLI verb for the pass S4.3.3, S4.5.6 and S13 refer to, scheduled at `correction_pass.cadence` (S6). It **never trains**. Its chain is:

```
er correct        :  er match --mode full --new-tf-snapshot   # rebuilds tf_lookup under a new
                  →  er reconcile                             #   tf_snapshot_id at the ACTIVE model_version
                  →  er assemble
```

It writes `runs.mode='correction_pass'` and `runs.rebuild_reason='correction_pass'`, and every event it emits carries `details.reason='correction_pass'`. `--new-tf-snapshot` is accepted by `er match` only in `--mode full` and only from `er correct`; it is the only path that mints a `tf_snapshot_id` outside `er train` (D4).

**What it recomputes, and which invariant it restores.** `er correct` regenerates candidate pairs over the **whole** corpus rather than a batch, re-materializes `tf_lookup` under a new `tf_snapshot_id` from the current corpus, re-scores every regenerated candidate pair at the **active** `model_version`, re-clusters the full assertion-adjusted edge set, and re-assembles every entity the pass touches. Those are exactly the two INV-EQ loss vectors named in S4.5.6 — incremental candidate generation, which can never pair two records that were both already in the corpus, and corpus-dependent term frequency — so the invariant the pass restores is **INV-EQ**: once it completes, the incrementally-maintained universe holds the same set-partition of current members a from-scratch full run would produce. It restores nothing else and repairs nothing else. Because it never trains, `model_version` and every fitted m/u value are unchanged across it, so INV-PERM still governs the entity ids it rewrites: entities whose membership is unchanged keep their `entity_id` and emit no event. T-CORR-1 is the acceptance test for the candidate-generation arm and T-INC-1b for the TF arm.

`--source`/`--path` MUST be supplied unless `--skip-ingest` is set; supplying neither and omitting `--skip-ingest` exits `2`. A child stage returning `10` does not abort the chain; downstream stages run and will themselves return `10` if they in turn have nothing to do. `er run-all --mode incremental` refuses to proceed (exit `3`) when `(config_hash, model_version, std_version)` differ from the last successful run for this tenant; `--allow-escalate` promotes the run to `--mode full` instead of failing.

<a id="s4-0b"></a>
### 4.0b Connection & writer model

- The DuckDB **primary database is `:memory:`**. DuckLake is ATTACHed as the alias `lake` (`$ER_LAKE_ALIAS`). No stage ever makes DuckLake the default catalog.
- Splink receives that same connection via `DuckDBAPI(connection=<conn>, output_schema='splink_scratch')`, so no `__splink__` relation ever lands in DuckLake. Input frames are read fully qualified (`lake.main.int_std_records`) or materialized into local temp tables first; **only final scored pairs are written to `lake.main.match_scores`, in a single write statement** — the `MERGE INTO` of S4.3.4.
- The same rule binds the S4.5 label-propagation loop: iterations run in the in-memory database and only the final labelling is written to the lake, so the loop cannot commit one snapshot per iteration.
- **dbt runs as a subprocess.** No Python DuckDB connection spans a dbt invocation: the Python connection is closed before `dbt` is spawned and reopened after it exits. dbt's `threads` is pinned to `1` in `profiles.yml`.
- **One writer per `run_id`**, enforced by a Postgres advisory lock keyed on `tenant`, taken on `$ER_CATALOG_DSN` for the lifetime of the process and released on exit including on failure. Failure to acquire exits `3`. v1 is a single-writer batch model; concurrent writers to one namespace are an explicit non-guarantee. This bullet is the definition site: every other mention of the lock cites S4.0b.
- Tenancy is namespace-only, as defined in S1. The namespace this process attaches is the pair (`$ER_LAKE_METADATA_SCHEMA`, `$ER_LAKE_DATA_PATH`), and the lock above is keyed on the `tenant` that names it.

Extensions are installed at **Docker build time** into `/opt/duckdb_extensions`; at runtime autoinstall is disabled. Every DuckDB connection `lake/ducklake.py` opens executes exactly this statement sequence, in this order, with every `{…}` position substituted **in Python before the statement reaches DuckDB**:

```sql
SET extension_directory = '/opt/duckdb_extensions';
SET autoinstall_known_extensions = false;
SET autoload_known_extensions   = false;
INSTALL ducklake;  LOAD ducklake;      -- INSTALL is a no-op against the baked directory
INSTALL postgres;  LOAD postgres;
INSTALL httpfs;    LOAD httpfs;

SET threads       = {ER_DUCKDB_THREADS};        -- integer literal
SET memory_limit  = {ER_DUCKDB_MEMORY_LIMIT};   -- quoted string literal

CREATE OR REPLACE SECRET er_s3 (
    TYPE s3,
    KEY_ID     {ER_S3_ACCESS_KEY_ID},
    SECRET     {ER_S3_SECRET_ACCESS_KEY},
    ENDPOINT   {ER_S3_ENDPOINT},        -- host:port, NO scheme
    URL_STYLE  {ER_S3_URL_STYLE},
    USE_SSL    {ER_S3_USE_SSL},         -- unquoted true / false
    REGION     {ER_S3_REGION}
);

ATTACH IF NOT EXISTS {DUCKLAKE_URI} AS lake (
    DATA_PATH       {ER_LAKE_DATA_PATH},
    METADATA_SCHEMA {ER_LAKE_METADATA_SCHEMA}
);
```

**One substitution mechanism, and it is Python string building.** This is forced by the engine, not chosen for convenience. Verified against the pinned `duckdb==1.5.5` Python API:

- **`getenv()` does not exist.** It is a DuckDB *CLI* function; `duckdb_functions()` on a Python connection returns no `getenv`, and no setting exposes it (`enable_external_access` and `allow_unsigned_extensions` cannot be changed on a running database, and a connection opened with them in `config=` still does not have the function). Any `getenv()` in this block would raise `Catalog Error: Scalar Function with name getenv does not exist!`.
- **`ATTACH` takes a bare string literal only.** Not a function call, not a `||` concatenation (`Parser Error: syntax error at or near "||"`), and not a bound `?` parameter (`Parser Error: syntax error at or near "?"`). So the full `ducklake:postgres:<dsn>` URI MUST be assembled in Python and emitted as one literal — that is what `{DUCKLAKE_URI}` is.
- `CREATE SECRET` option values *do* accept expressions, but since `getenv()` is unavailable there is nothing useful to express; they are emitted as literals for consistency with the `ATTACH`.

Substitution goes through **one helper in `lake/ducklake.py`**, which reads the variable from `os.environ`, raises `ERR_ENV_MISSING: <name>` (exit `2`) if it is absent or empty rather than emitting an empty literal, and renders it as a single-quoted SQL string with embedded quotes doubled. Integer and boolean positions are validated and emitted unquoted. No value reaches DuckDB unescaped, and no shell-style `$VAR` form may appear anywhere in the rendered text: in DuckDB SQL a bare `$NAME` is a **prepared-statement parameter marker** and `'$NAME'` is the literal characters `$NAME`, so a block that mixed those forms would execute without error while creating a secret whose `KEY_ID` is the string `$ER_S3_ACCESS_KEY_ID` and attaching a lake whose `DATA_PATH` is the string `$ER_LAKE_DATA_PATH` — both failing far from their cause. `er doctor`'s `DATA_PATH` write/read round-trip (T-DOCTOR-1) is the backstop that catches a lake attached to the wrong place.

`postgresql://` URIs are accepted verbatim by the postgres extension and need **no** translation. `DATA_PATH` is written into the catalog on first attach and is **immutable thereafter**; a mismatch is a hard error, not a silent re-point (see `er init`).

**The dbt side of the same contract.** A dbt profile is YAML and cannot emit a SQL block; dbt-duckdb expresses the identical three parts as profile keys, and `dbt/profiles/profiles.yml`'s `lake` target MUST carry values equal to the block above, field for field:

```yaml
lake:
  type: duckdb
  path: ':memory:'                       # the primary database, per the first bullet of S4.0b
  threads: 1                             # dbt's own concurrency, pinned above; NOT DuckDB's threads
  extensions: [ducklake, postgres, httpfs]
  settings:                              # emitted as SET <key> = <value>
    extension_directory: /opt/duckdb_extensions
    autoinstall_known_extensions: false
    autoload_known_extensions: false
    threads: "{{ env_var('ER_DUCKDB_THREADS', '2') | int }}"
    memory_limit: "{{ env_var('ER_DUCKDB_MEMORY_LIMIT', '4GB') }}"
  secrets:                               # emitted as CREATE OR REPLACE SECRET
    - name: er_s3
      type: s3
      key_id: "{{ env_var('ER_S3_ACCESS_KEY_ID', '') }}"
      secret: "{{ env_var('ER_S3_SECRET_ACCESS_KEY', '') }}"
      endpoint: "{{ env_var('ER_S3_ENDPOINT', '') }}"
      url_style: "{{ env_var('ER_S3_URL_STYLE', 'path') }}"
      use_ssl: "{{ env_var('ER_S3_USE_SSL', 'false') }}"
      region: "{{ env_var('ER_S3_REGION', 'us-east-1') }}"
  attach:                                # emitted as ATTACH ... AS <alias> (...)
    - path: "ducklake:postgres:{{ env_var('ER_CATALOG_DSN', '') }}"
      alias: lake
      options:
        data_path: "{{ env_var('ER_LAKE_DATA_PATH', '') }}"
        metadata_schema: "{{ env_var('ER_LAKE_METADATA_SCHEMA', '') }}"
```

The fields that MUST agree with the SQL block are: the five `settings:` keys against the five `SET` statements; the `secrets:` entry's `name`, `type` and six option values against `CREATE OR REPLACE SECRET er_s3`; and `attach:`'s `path`, `alias`, `data_path` and `metadata_schema` against the `ATTACH`. Substitution here is Jinja `env_var()`, evaluated by dbt before DuckDB sees the value. That is the same discipline as the Python side — the value is resolved by the host and reaches the engine as a literal — and not a second mechanism inside one statement. Every `env_var()` call supplies a default so `dbt parse` runs on a bare runner with no services (S9.1); the defaults are deliberately empty or inert, because a *wrong* default would attach a different lake, which `er init`'s `DATA_PATH` immutability check exists to catch.

<a id="s4-1"></a>
### 4.1 Ingest (`src/er/ingest/`)

- Reads CSV/Parquet from `storage.drop_dir/<source>/` (v1 adapter; the `SourceAdapter` interface allows DB adapters later). The source's canonical→source column mapping, `record_id_column`, `updated_at_column` and `date_format` come from `sources.<name>` in S6.
- Writes to `raw_records` as **append-only version history** (Decision 6). The logical key is `(source_system, source_record_id, content_hash)`; a corrected record ADDS a row and never overwrites one. The write is an anti-join append against that key — never an upsert — so re-delivering an identical file appends zero rows.
- `payload JSON` holds the full source row verbatim; `ingest_batch_id` (ULID) and `ingested_at` are supplied by the writer.

**`content_hash` (normative).** `content_hash` is the SHA-256, hex-encoded lowercase, of the UTF-8 concatenation of the source columns named in `sources.<name>.columns`, **in the declared order**, each value NFC-normalized and joined by the `0x1f` unit separator, with NULL encoded as the empty string. It excludes `ingested_at`, `ingest_batch_id` and `std_version`. Two implementations that disagree on this definition make T-IDEM-1a and T-IDEM-1 non-reproducible; it is therefore pinned here and computed by exactly one function, `er.ingest.landing.content_hash(row, columns)`.

**Batch manifest.** Every invocation persists one row to `ingest_batches`; **S5 owns that relation's column list** and this section does not restate it. What this section owns is what the counts mean: `new_count` = keys not previously seen; `changed_count` = known key with a new `content_hash`; `unchanged_count` = known key with a known hash (skipped, counted, not appended); `tombstone_count` and `resurrected_count` are defined in S4.1.1. The five counts are also the five stage-counter names (S4.1.1) and are printed on the S4.0 stdout manifest line under the shorter labels `new`, `changed`, `unchanged`, `tombstoned`, `resurrected` — the same five numbers under three spellings, and no sixth counter exists.

<a id="s4-1-1"></a>
#### 4.1.1 Deletion (in scope for v1)

`raw_records` carries `is_deleted BOOL` and `deleted_at`. Deletion is expressed as a **tombstone version row**, preserving append-only semantics.

- **`er ingest --source S --path P --full-refresh-keys`** treats the delivery as the **complete key set** for source `S`. Every key currently live in `raw_records` for `S` and absent from the delivery gets a tombstone row appended with `is_deleted=true`, `deleted_at` = the batch's `ingested_at`.
- **Tombstone sentinel hash.** A tombstone row carries the sentinel `content_hash = '0' * 64` and `payload = NULL`. The sentinel is never produced by the hash function above (which always hashes at least one separator-joined value), so tombstones can never collide with content versions.
- **Empty-delivery guard.** If a `--full-refresh-keys` delivery parses to zero records, the stage MUST refuse to tombstone the source: it exits **`2`** with `empty full-refresh delivery: refusing to tombstone <n> live keys for source <S>`. The code is `2`, not `10`, because this is a *refusal to destroy* — the input is rejected as invalid rather than accepted as a no-op, and `10` (“nothing to do”) does not abort an `er run-all` chain while a validation error must. T-DEL-1a asserts exit `2`. A genuine full deletion of every key in a source is performed by an operator with `er lake reset --confirm-tenant <tenant>` followed by re-ingestion (S4.0, S4.7), never by delivering an empty file.
- **Resurrection.** Re-appearance of a tombstoned key in any later delivery appends an ordinary content version whose `ingested_at` is greater than the tombstone's; the supersession rule in S4.2 then makes the record live again with no special case. `resurrected_count` — live keys in this delivery whose current version in `raw_records` was a tombstone — is persisted on the `ingest_batches` row (S5) and printed as `resurrected` on the S4.0 manifest line.
- Tombstones are consumed by S4.2 (exclusion from `int_std_records`) and S4.5 (the retraction path: invalidate incident edges, widen the affected set, emit `member_removed` / `split` / `retired`).

**Counters** — `run_stages`: `rows_in` = source rows read, `rows_out` = `raw_records` rows appended; `counters = {files, new_count, changed_count, unchanged_count, tombstone_count, resurrected_count, ingest_batch_id, duration_ms}` — the five count names are the `ingest_batches` column names of S5, deliberately, so no reader has to map `tombstone_count` onto a differently-spelled counter.

<a id="s4-2"></a>
### 4.2 Standardization (dbt staging + intermediate)

One `stg_<source>` model per source maps source columns → the canonical schema. v1's three models (`stg_crm`, `stg_billing`, `stg_webforms`) are hand-written and read their column mapping from the `sources` var the CLI passes, so adding a source is a config change plus one model file. Macros applied, all in `dbt/macros/std/`:

| Macro | Contract |
|---|---|
| `lowercase_trim(col)` | NFC-normalize, `trim`, `lower`; empty string → NULL. |
| `email_norm(col)` | `lowercase_trim`, strip plus-addressing when `standardization.email_strip_plus_addressing`, then **null every address in `standardization.email_placeholders`** (e.g. `test@test.com`); emits `email` and `email_valid BOOL`. Placeholder nulling belongs to `email_norm` and to no other macro — `null_semantics` handles only the sentinel vocabulary below, which contains no email addresses. |
| `phone_e164(col)` | Digits-only extraction, default region `standardization.phone_default_region`, E.164 render; emits `phone_e164` and `phone_valid BOOL`. |
| `null_semantics(col)` | Maps the sentinel vocabulary (`''`, `'NULL'`, `'N/A'`, `'-'`, `'unknown'`) to NULL. |
| `name_norm(col)` | `lowercase_trim`, punctuation strip, diacritic fold; emits `given_name` / `family_name` and `name_variants LIST(VARCHAR)` joined from the `nickname_variants` seed. **The normalized `given_name` is ALWAYS element 0 of its own `name_variants` array on every record** — the symmetry guarantee that makes `variant_match` orientation-independent. |
| `address_parse(cols)` | Regex/`usaddress`-based componentizer behind the `AddressParser` interface, versioned by `versions.address_parser_version`; emits `addr_number, addr_street, addr_unit, addr_city, addr_region, addr_postal`. The fixture generator emits only patterns the v1 parser handles; a libpostal container can replace it later without a model change. |
| `parse_date(col, fmt)` | Emits exactly one column, `birth_date`. It computes a precision (`day`, `month` or `year`) **internally** to decide the value — a `year`-precision parse yields NULL, because a year-only DOB is not usable matching evidence — and does not persist that precision. v1 has no consumer for a precision column: no comparison level, no blocking key, no survivorship rule and no `golden_records` column reads it, and a stored column no rule consumes is a column that silently drifts. Should review display need it later, it is an additive column under S5.1. |

`int_std_records` unions the staged sources and materializes `record_key`, `content_hash`, `std_version` (from the `--vars` override the CLI passes; `dbt_project.yml` holds only a fallback), and `updated_at_source`. `int_blocking_keys` materializes `(key_type, key_value, record_key, source_system, source_record_id)`.

**Supersession rule (normative).** `int_std_records` holds **exactly one current row per `(source_system, source_record_id)`**: the row derived from the `raw_records` version with the **greatest `ingested_at`** for that key (ties broken by `ingest_batch_id DESC` — the ULID is time-ordered, so `DESC` selects the *most recent* batch, which is what "current" means; `ASC` would let the older version win). Rows whose winning version has `is_deleted = true` are **excluded** from `int_std_records` entirely. A dbt `unique` test on `record_key` and a `dbt_utils.unique_combination_of_columns` test on `(source_system, source_record_id)` enforce this.

**Incremental configuration (normative, one line per model family).**

```sql
-- stg_crm / stg_billing / stg_webforms
{{ config(materialized='incremental', incremental_strategy='append',
          on_schema_change='sync_all_columns') }}
select ... from {{ source('lake','raw_records') }}
{% if is_incremental() %}
  where ingest_batch_id not in (select distinct ingest_batch_id from {{ this }})
{% endif %}
```

```sql
-- int_std_records, int_blocking_keys
{{ config(materialized='incremental', incremental_strategy='delete+insert',
          unique_key=['source_system','source_record_id'],
          on_schema_change='sync_all_columns') }}
```

`on_schema_change` MUST be `sync_all_columns` on every incremental model. The dbt-duckdb default of `ignore` would silently swallow the columns a `std_version` bump introduces, defeating the whole `std_version` mechanism. `delete+insert` on `(source_system, source_record_id)` is correct for `int_blocking_keys` even though it holds many rows per record: the strategy deletes all rows for a touched record before re-inserting its full key set. No model declares `indexes` (DuckLake has none), and no **dbt** model uses `incremental_strategy='merge'` (DuckLake's MERGE supports a single `when_matched` action, which dbt's merge strategy cannot express). This scopes to dbt materializations only: the Python writers to `match_scores` (S4.3.4) and `entity_membership` (S4.5.3) do issue `MERGE INTO` statements directly. Every dbt-owned model declares `contract: {enforced: true}` in `schema.yml`, per the ownership rule in S5.0.

`--changed-only` limits the run to `--select staging+ intermediate` with the incremental predicates active. Without it the same models run over the full selection **with the incremental predicates still active** — the absence of the flag widens the selection, it never implies `--full-refresh`. `--full-refresh` is added by the CLI only for a planned rebuild (a `std_version` or `survivorship_version` bump, S5.1); an unconditional full refresh here would make every `er run-all --mode full` a corpus rebuild and would defeat the incremental predicates entirely.

**Blocking generation direction (normative).** `configs/*.yaml` `blocking:` is the **single source of truth**. One generator produces both consumers:

```python
blocking_rules_from_config(cfg) -> tuple[dbt_var_payload, list[BlockingRuleCreator]]
```
in `src/er/matching/model.py`. The CLI passes `dbt_var_payload` to dbt as a var; the `int_blocking_keys` model is macro-generated from it, emitting one `UNION ALL` branch per `key_type`:

```sql
select '<key_type>' as key_type, <expr> as key_value,
       record_key, source_system, source_record_id
from {{ ref('int_std_records') }}
where <expr> is not null and <expr> <> ''
```

Splink receives `block_on('<expr>')` for the **identical** `expr` string. **NULL/empty policy:** a NULL or empty-string key value is never emitted and never blocks — on either side. The dbt-derived pair set MUST be computed with `SELECT DISTINCT` over canonicalised `(rec_a_key, rec_b_key)`: Splink deduplicates across rules via preceding-rule exclusion, a key-table self-join does not, and without `DISTINCT` the two candidate sets differ by multiplicity alone.

**T-BLK-1** is the parity check: on `base_10`, the DISTINCT canonicalised pair set derived from `int_blocking_keys` equals Splink's blocked pair set exactly. It is the only thing converting "mirrored" from aspiration into a checked invariant.

**Counters** — `run_stages`: `rows_in` = `raw_records` rows read, `rows_out` = `int_std_records` current rows; `counters = {models_run, stg_rows_appended, std_rows_current, blocking_rows, blocking_keys_by_type, tombstones_excluded, duration_ms}`.

<a id="s4-3"></a>
### 4.3 Matching (`src/er/matching/`)

**Thresholds and units.** All config thresholds are **probabilities**. Where a Splink call takes a match **weight**, pass `log2(p/(1-p))`; never rely on Splink's `-4` default. The gray band is **half-open**: `review_low <= p < auto_merge`. The clustering threshold **is** `auto_merge` and is passed explicitly to `cluster_pairwise_predictions_at_threshold(threshold_match_probability=auto_merge)` — omitting it treats every supplied edge as a match.

**Record identity and pair ordering.** Both are defined in S5.0. This stage's only obligations against them are that `record_key` is what the settings builder passes as Splink's `unique_id_column_name`, and that every pair this stage writes has already been through the S5.0 canonicalisation helper.

<a id="s4-3-1"></a>
#### 4.3.1 Comparison levels (`model.py`)

The settings builder maps each token in `comparisons.<attr>.levels` to exactly one Splink construct:

| Token | Splink construct / generated SQL |
|---|---|
| `exact` | `ExactMatchLevel(col)` |
| `jaro_winkler:T` | `JaroWinklerLevel(col, T)` |
| `null` | `NullLevel(col)` |
| `username_exact` | `CustomLevel("split_part(email_l,'@',1) = split_part(email_r,'@',1)")` |
| `variant_match` | `ArrayIntersectLevel('name_variants', min_intersection=1)` |
| `dob_same_year_month` | `CustomLevel("date_trunc('month', birth_date_l) = date_trunc('month', birth_date_r)")` |

Normative emission rules: the builder **ALWAYS** emits `NullLevel` first and `ElseLevel()` last — without the else level Splink emits a `CASE … END` with no `ELSE`, yielding gamma NULL and a NULL match weight for any pair matching nothing. `tf: true` maps to `.configure(term_frequency_adjustments=True)` and applies **only to exact-match levels** on the named column. `name_variants` is `LIST(VARCHAR)`; `variant_match` is orientation-independent only because of S4.2's symmetry guarantee. There is no `phonetic` level: it is deleted from the spec.

<a id="s4-3-2"></a>
#### 4.3.2 Training (`train.py`)

`er train` runs full-corpus EM. The call sequence is fixed, and every argument comes from the `training:` block in S6, which Pydantic rejects if incomplete:

```python
linker.training.estimate_probability_two_random_records_match(
    deterministic_matching_rules=cfg.training.deterministic_rules,
    recall=cfg.training.recall,
)
linker.training.estimate_u_using_random_sampling(
    max_pairs=cfg.training.u_max_pairs,
    seed=cfg.training.u_seed,          # required; no default — an unseeded u estimate is not reproducible
)
for rule in cfg.training.em_blocking_rules:        # min 2 sessions; m is not estimated for blocked columns
    linker.training.estimate_parameters_using_expectation_maximisation(
        blocking_rule=rule,
        fix_u_probabilities=cfg.training.em.fix_u_probabilities,
    )
```

The whole `training:` block is persisted verbatim into `model_registry.metrics` alongside the fitted m/u values.

**Model artifact lifecycle.**

1. Write the settings JSON to `{storage.model_uri_prefix}model_v{N}.json` (e.g. `s3://lake/models/test/model_v0003.json`). `storage.model_uri_prefix` already ends in `/` and already names the tenant (V14, S6); the tenant MUST NOT be interpolated a second time. **Object first, registry row second** — a registry row never points at a missing object.
2. `model_version` is allocated as zero-padded `max+1` **inside the registry insert transaction**.
3. `model_registry` carries `status ∈ {active, superseded}`, `params_path` (the S3 URI), `tf_tables_path`, `tf_snapshot_id`, `corpus_snapshot` (the DuckLake snapshot version the model was trained against), `trained_at`, `metrics JSON`. Activating vN+1 sets the previous active row to `superseded` in the same transaction.
4. `er match` loads the single `status='active'` row unless `--model-version` overrides it.
5. **Activation guard:** activating a new `model_version` requires `er match --mode full` before the next `er reconcile`. `er reconcile` fails with exit `3` if `match_scores` holds more than one distinct `model_version` among active rows with `match_probability >= review_low`. Assertion-sourced edges cannot confound the check: they are never persisted to `match_scores` (S4.4).
6. Scenario tests never train: they load the committed `fixtures/static/model_test_v1.json`. EM over 23 records is degenerate.

<a id="s4-3-3"></a>
#### 4.3.3 Term frequency policy

TF values are **not** in the model JSON; Splink otherwise computes them from whatever corpus is registered at predict time, which would make the same pair score differently in a base run, an incremental run and a full re-resolution. v1 freezes TF:

- `er train` materializes and persists `tf_lookup(model_version, tf_snapshot_id, column_name, value, tf_value)` for every column with `tf: true`, and records `tf_snapshot_id` + `tf_tables_path` on the `model_registry` row.
- Incremental and full runs call `linker.table_management.register_term_frequency_lookup(...)` for **every** TF column and **NEVER** call `linker.table_management.compute_tf_table(...)`.
- Only the **periodic correction pass** — the `er correct` verb (S4.0), scheduled at `correction_pass.cadence` in S6 — rebuilds TF under a **new** `tf_snapshot_id` and re-scores the corpus. It never retrains; its events carry `details.reason='correction_pass'`.
- `match_scores` carries `tf_snapshot_id` alongside `model_version`, so a mixed-TF corpus is detectable after the fact.

> **INV-SCORE** — `match_probability` is a pure function of `(model_version, tf_snapshot_id, rec_a_key, rec_b_key, rec_a_content_hash, rec_b_content_hash)`.

The two endpoint content hashes are part of the key, and MUST be: a record's standardized attributes are the scoring inputs, so when an endpoint's `content_hash` changes the pair is a **different** scoring problem. Without them the invariant would be false across exactly the case S4.5.5 handles by invalidating the edge and re-scoring. Stated with them, invalidation is INV-SCORE working rather than INV-SCORE breaking: the prior row is marked `is_active=false` and the new hashes get their own score. Both hashes are stored on the row (S5), so the claim is checkable after the fact.

INV-SCORE is what makes `match_scores` a cumulative, re-derivable table and what allows S4.5's edge set to mix rows written by different runs.

<a id="s4-3-4"></a>
#### 4.3.4 Incremental scoring — two passes, unioned

Splink 4's inference surface (`deterministic_link` / `predict` / `find_matches_to_new_records` / `compare_two_records`) accepts no precomputed pair table; every entry point regenerates pairs from blocking rules. Incremental scoring is therefore **two Splink passes over the same frozen model JSON and the same registered TF tables**:

```python
# Pass 1 — new vs corpus
new_vs_corpus = linker_corpus.inference.find_matches_to_new_records(
    records_or_tablename='batch_std',
    blocking_rules=blocking_rules_from_config(cfg)[1],
    match_weight_threshold=log2(review_low / (1 - review_low)),
)

# Pass 2 — new vs new  (find_matches_to_new_records does NOT pair new records with each other)
linker_batch = Linker(batch_std, settings=frozen_settings_with(link_type='dedupe_only'),
                      db_api=DuckDBAPI(connection=conn, output_schema='splink_scratch'))
register_tf_tables(linker_batch, tf_snapshot_id)
new_vs_new = linker_batch.inference.predict(threshold_match_probability=review_low)
```

UNION the two results, drop self-pairs, canonicalise to `rec_a_key < rec_b_key`, `DISTINCT`, and persist to `lake.main.match_scores` in a single `MERGE INTO` (below) carrying `model_version`, `tf_snapshot_id`, `rec_a_content_hash`, `rec_b_content_hash`, `is_active = true`, `run_id`, `evidence JSON`, `scored_at`. Pass 2 is what makes the "2 new records form a new entity" case in the `incremental_batch` fixture reachable at all.

`--mode full` is one corpus-wide `linker.inference.predict(threshold_match_probability=review_low)`; it is used by training runs and by the correction pass.

**Splink regenerates its own candidate pairs; `int_blocking_keys` is NOT an input to scoring.** It is (a) the blocking-rule generation source, (b) the driver for the S4.5 touched-subgraph computation, and (c) the `candidate_pair_count` benchmark metric.

`match_scores` is **cumulative and is never truncated per run**. Writes are `MERGE INTO` on `(model_version, tf_snapshot_id, rec_a_key, rec_b_key)` — that tuple is therefore also the relation's logical key (S5.0), with **at most one row per key regardless of `is_active`**. Invalidation (S4.5.5) is an in-place `UPDATE` setting `is_active=false`, `invalidated_at` and `invalidated_run_id`; it never leaves a second row behind, and re-scoring the same key under new endpoint content hashes overwrites the row and sets `is_active=true` again.

<a id="s4-3-5"></a>
#### 4.3.5 Gray band → `review_queue`

Pairs with `review_low <= p < auto_merge` are written to `review_queue` and are **not** clustered. The write is an **upsert**: it refreshes `last_seen_run_id` on an existing `open` row, inserts with `status='open'`, `first_seen_run_id = last_seen_run_id = run_id`, `subject_type='pair'`, `reason='gray_band'` for a new pair, and **skips pairs already resolved** (`resolved_match`, `resolved_no_match`, `dismissed`). That skip is what makes the stage idempotent and stops a dismissed pair resurfacing every run. `waterfall JSON` retains the `gamma_*` comparison-vector columns and per-comparison Bayes factors from `predict()`; they MUST be retained rather than projected away. Resolving a row to `match` / `no_match` writes the corresponding `assertions` row in the same transaction.

**Counters** — `run_stages`: `rows_in` = records scored, `rows_out` = pairs persisted; `counters = {mode, model_version, tf_snapshot_id, candidate_pairs, pairs_scored, pairs_above_auto_merge, pairs_in_gray_band, review_queue_added, review_queue_refreshed, duration_ms}`.

<a id="s4-4"></a>
### 4.4 Assertions (`src/er/review/assertions.py`)

Assertions are steward-authored constraints applied identically in incremental and full modes — this is what makes corrections survive re-runs.

**Lifecycle.** `assertions(assertion_id ULID, rec_a_key, rec_b_key, kind ∈ {always, never}, active BOOL, created_by, created_at, retracted_by, retracted_at, note)`. Pairs are stored canonically per S5.0. A dbt uniqueness test enforces one `active` row per `(rec_a_key, rec_b_key)`. `er assert remove` sets `active=false` and stamps `retracted_by`/`retracted_at`; rows are never deleted, so the assertion delta between runs is computable.

**Precedence.** **`never` dominates `always` for the same pair.** A conflicting insert is rejected at write time (exit `1`), not silently ordered around. Edge adjustment applies `always` first and `never` second, so the two orderings cannot disagree.

**Edge adjustment (pre-clustering).** For each `active` assertion: `always` inserts an edge with `match_probability = 1.0` and `evidence = {"source":"assertion","assertion_id":…}`; `never` removes the edge between the endpoints regardless of score. **Assertion edges are never persisted to `match_scores`** — they are materialized into the in-memory clustering edge set only, by the assertion-adjustment step S4.5.1 names in the trailing annotation under its query (`-- then assertion-adjusted per S4.4`). That step is not a SQL clause and is not part of the `select`: the query loads the scored edges, and the adjustment — minus active `never` pairs, plus active `always` pairs at `p = 1.0` — is applied to the loaded result in memory. `assertions` is their durable record. Consequently every `match_scores` row is scored: `model_version` and `tf_snapshot_id` are `NOT NULL` there (S5), and no consumer needs a NULL-`model_version` branch.

<a id="s4-4-1"></a>
#### 4.4.1 CONTRADICTION-1

> **CONTRADICTION-1** — before clustering, compute the connected components of the `active` `always` edges (the *always-closure*). If any `active` `never` pair has both endpoints inside one component, the run **FAILS**: exit `1`, no snapshot committed by the reconcile stage, no events emitted, and the offending `assertion_id`s plus the closure component are named in `run_stages.error_detail`.

The constraint set is unsatisfiable as a partition (`always(a,b) ∧ always(b,c) ∧ never(a,c)`), and no cutting strategy can repair it. It is a hard, deterministic pre-clustering failure — never a warning.

<a id="s4-4-2"></a>
#### 4.4.2 Partition-level `never_match`

Deleting edge `(a,b)` does not prevent `a–c–b` from re-linking the pair under connected components. v1 therefore enforces `never` at the **partition** level, after clustering:

1. For every `active` `never` pair whose endpoints are **co-clustered** in the clustering output, find the shortest path between them in the clustering edge set. The path is chosen by `(hop_count ASC, then the lexically smallest vertex sequence)` — a total order over paths.
2. Cut the **minimum-probability** edge on that path. The cut choice total order is `(match_probability ASC, rec_a_key ASC, rec_b_key ASC)`.
3. **Protection.** An edge whose `match_probability >= clustering.cut_protect_probability` is **PROTECTED** and is never cut. `clustering.cut_protect_probability` **defaults to `1.0`**, which protects exactly the assertion-sourced edges (score `1.0`). Setting it to the `auto_merge` value yields the stricter reading in which no ordinary edge is cuttable.
4. **Escalation.** When every path between the endpoints consists solely of protected edges, the pair is **not** cut; it is escalated as a `review_queue` row with `subject_type='pair'`, `reason='never_unsatisfiable'`, `status='open'`. *Stated honestly:* at the `1.0` default, a path made entirely of protected edges is a path of `always`-assertion edges — which is precisely CONTRADICTION-1 and has already failed the run before clustering. The escalation branch is therefore a **narrow residual**, reserved for future non-assertion protected edges (e.g. a raised `cut_protect_probability`, or human-confirmed edges). It is specified and implemented, not exercised at the default setting.
5. Persist every cut to `cut_edges` (S5 owns its column list; the run is recorded in `cut_run_id`, not `run_id`), emit an `edge_cut` event on the affected entity, and **re-run connected components** over the reduced edge set.
6. Repeat to a **bounded fixpoint**: at most `clustering.max_iterations` rounds. **Failure mode:** if `never` violations remain after `max_iterations`, the stage fails (exit `1`), no snapshot is committed, no events are emitted, and the surviving violating pairs plus the iteration count are logged. Cutting is monotone (each round removes at least one edge from a finite set), so non-termination indicates a protected-edge cycle or a bug, not slow convergence.

**`cut_edges` rows are excluded from the clustering edge set on every subsequent run.** Without that exclusion, every cut is silently re-merged on the next run from the cumulative `match_scores` table, and `never` becomes a no-op with a one-run half-life. A cut is invalidated only when its assertion is retracted, or when either endpoint's `content_hash` changes.

**Counters** — `run_stages`: `rows_in` = active assertions applied, `rows_out` = edges added + edges removed; `counters = {always_applied, never_applied, assertion_delta_since_last_run, contradiction_pairs, edges_cut, cut_iterations, never_unsatisfiable_escalations, duration_ms}`.

<a id="s4-5"></a>
### 4.5 Clustering + Reconciliation (`src/er/entities/`)

<a id="s4-5-1"></a>
#### 4.5.1 Affected node set and affected edge set

**Affected NODE set (formula, not prose).**

```
seed  = records in this run's ingest batches
      ∪ records whose content_hash changed since the last successful run
      ∪ records tombstoned or resurrected since the last successful run
      ∪ records referenced by assertions created or retracted since the last successful run
      ∪ records referenced by review_queue rows resolved since the last successful run

partners        = { r' : an assertion-adjusted edge (r, r') exists with p >= auto_merge, r ∈ seed }
affected_entities = current entity of each record in (seed ∪ partners), where assigned
affected_nodes  = all current members of affected_entities ∪ seed ∪ partners
```

`er reconcile` reads the pending assertion delta itself; this is what makes `split_scenario` executable in incremental mode when a `never` is asserted on two records that appear in no batch.

**Affected EDGE set (normative).** For every entity in the affected set, load **ALL currently-active edges among its members** — not merely this run's scored pairs. Concretely, the clustering edge set is:

```sql
select rec_a_key, rec_b_key, match_probability
from lake.main.match_scores
where model_version  = :run_model_version
  and tf_snapshot_id = :run_tf_snapshot_id
  and is_active                                                        -- excludes edges invalidated by S4.5.5
  and match_probability >= :auto_merge
  and rec_a_key in (select record_key from affected_nodes)
  and rec_b_key in (select record_key from affected_nodes)
  and (rec_a_key, rec_b_key) not in (select rec_a_key, rec_b_key from lake.main.cut_edges where active)
  and rec_a_key in (select record_key from lake.main.int_std_records)   -- excludes tombstoned
  and rec_b_key in (select record_key from lake.main.int_std_records)
-- then assertion-adjusted per S4.4: minus active `never` pairs, plus active `always` pairs at p = 1.0
```

`match_scores` is **cumulative and is never truncated per run**. An implementer who loads only this run's scored pairs will spuriously fragment every touched entity; loading all currently-active edges is the only correct reading, and it is the one specified here.

<a id="s4-5-2"></a>
#### 4.5.2 Clustering

- **Incremental:** iterative label propagation in DuckDB SQL over the affected subgraph. `label(v) = min(record_key)` over the closed neighbourhood of `v`, propagated to fixpoint. Iterations run in the in-memory database (S4.0b); only the final labelling is written to the lake.
- **Failure:** if the labelling has not converged within `clustering.max_iterations` (S6, default 50), the stage **fails** — no snapshot committed, no events emitted, exit `1`, with the unconverged component's size and its minimum `record_key` logged.
- **Full:** `linker.clustering.cluster_pairwise_predictions_at_threshold(threshold_match_probability=auto_merge)` over the same assertion-adjusted, cut-edge-excluded edge set. Both paths consume the identical edge set; **T-INV-1** asserts at the end of every integration scenario that `entity_membership` equals the connected components of the current edge set, which is the only guard against the two implementations drifting apart.

Splink's `cluster_id` is the minimum `unique_id` in a cluster — a record-derived label that changes whenever membership changes. It is **never** an entity id. Reconciliation owns the raw-cluster → entity mapping.

<a id="s4-5-3"></a>
#### 4.5.3 Reconciliation — the overlap matrix

> **INV-PERM** — let `P_old` be the current membership partition of `affected_nodes` and `P_new` the clustering output over the same nodes. For every group in `P_new` that is **set-equal** to a group in `P_old`, every member retains its existing `entity_id`, no ULID is minted, and no event is emitted. Otherwise, build the overlap matrix `|g_new ∩ g_old|` between `P_new` and `P_old` and apply:
> - **Claimant.** Each new group's claimant is the old entity with the largest overlap, tiebroken by `min member record_key ASC` within the overlap; if a new group overlaps no old entity, it mints a new `entity_id`.
> - **Merge.** Old entities that lose **all** their members to a claimant are merged into it: `status='merged'`, `merged_into = claimant`, one `merged` event, and their `entity_membership` rows are rewritten to the claimant **in the same snapshot**.
> - **Split.** Old entities left holding a **strict subset** of their prior members are split per the fragment ordering below: one `split` event per departing fragment.
> - **Retire.** An old entity holding zero members after the mapping gets `status='retired'` and one `retired` event.

This single mapping subsumes the merge, split, extend and mint cases and — unlike a four-branch list keyed on the number of distinct prior entities per cluster — it correctly covers a cluster that is **simultaneously a merge of two entities and a split of a third**.

**Fragment ordering (total order).** Fragments of a split old entity are ranked by `(member_count DESC, min member record_key ASC)`. Rank 1 retains the `entity_id`; every other fragment mints a new `entity_id` and emits a `split` event. A fragment of size 0 retires the entity. A record leaving all clusters becomes a singleton entity. This is a total order for every input including a 2–2 split — the entity-level tiebreaks (`created_at`, lexical ULID) are properties every fragment of one entity shares and are therefore useless here.

**`entity_membership` is CURRENT STATE:** exactly one row per `(source_system, source_record_id)`, maintained by `MERGE INTO`; `assigned_at` is the most recent (re)assignment. All history lives in `entity_events`, folded in `(occurred_at, seq)` order. `entities.merged_into` exists **only** for external ID resolution via `ids.resolve()` (with a cycle guard) and is **never** used to resolve current membership.

<a id="s4-5-4"></a>
#### 4.5.4 Determinism

- **D1** — the output partition and the cluster→`entity_id` assignment **MAP** are deterministic given identical inputs. All tiebreaks are total orders.
- **D2** — output is byte-identical **modulo minted identifiers**.
- **Mint order:** new clusters are minted in **ascending order of their minimum member `record_key`**, expressed as an explicit `ORDER BY` in the reconcile SQL — never an incidental scan order, which DuckDB does not guarantee.
- `reconcile()` takes an `IdFactory` dependency (production: ULID; tests: monotonic counter), which is what makes the reconciler testable as a pure function.
- `VOLATILE_COLUMNS` are excluded from every determinism comparison. The set is listed once, in S5.0, and imported from `src/er/lake/columns.py`; this section does not restate its members.
- Events are idempotent: an event is written at most once per `(run_id, entity_id, event_type, details_hash)`; a re-run producing identical output writes zero events.

<a id="s4-5-5"></a>
#### 4.5.5 Retraction path (deletion & supersession)

When a record is tombstoned (S4.1.1) or its `content_hash` changes:

1. Its incident `match_scores` edges for the prior content are **invalidated** — excluded from the edge set and, for tombstones, permanently (the record is absent from `int_std_records`).
2. Its entity enters the affected set, which widens through the affected-edge rule to that entity's full membership.
3. Re-clustering emits `member_removed` for the departed record, `split` for any fragment the removal disconnects, and `retired` when the entity empties.
4. A resurrected record re-enters as an ordinary new record: it is in `seed`, gets scored, and is re-clustered by the same code path with no special case.

<a id="s4-5-6"></a>
#### 4.5.6 INV-EQ — when incremental equals full

> **INV-EQ** — for a pinned `model_version` with a pinned `tf_snapshot_id`, an append-only corpus (no `content_hash` changes to previously-ingested records, no deletions), an unchanged `std_version` and `config_hash`, and an identical active assertion set, **incremental reconcile produces the same set-partition of current members as full reconcile.**

*Proof sketch.* (1) Connected components are monotone under edge insertion, and under the stated preconditions every edge that differs between the two runs is incident to at least one batch record — so no pre-existing component can change except by absorbing a batch record. (2) The affected-node closure contains, for every batch record, its scored partners **and all current co-members of those partners' entities**, hence both endpoint components of every new edge in full; loading all currently-active edges among those nodes reproduces exactly the sub-graph full clustering would see, so the components computed over it are identical.

The two loss vectors when the preconditions do **not** hold are (a) incremental **candidate generation** — two pre-existing records can never become a pair, since neither is in the batch — and (b) corpus-dependent TF, which a new `tf_snapshot_id` can move across `auto_merge`. Both are repaired by the periodic full correction pass; neither is a defect of incremental clustering, which is provably sufficient.

**Counters** — `run_stages`: `rows_in` = affected nodes, `rows_out` = membership rows written; `counters = {affected_entities, affected_edges, label_prop_iterations, clusters_out, entities_created, entities_merged, entities_split, entities_retired, members_added, members_removed, edges_cut, review_queue_added, events_emitted, duration_ms}`. `review_queue_added` is in this stage's list because the S4.4.2 escalation path writes `review_queue` rows during reconcile, not only during match.

<a id="s4-6"></a>
### 4.6 Golden Assembly (dbt marts + `src/er/golden/`)

**Survivable column set.** S5 owns the column list (materialised as `GOLDEN_SURVIVABLE_COLUMNS` in S5.0) and S6.1 V2 owns the validator that keeps it set-equal to the `survivorship:` key set; neither is restated here. This stage owns one rule the other two do not state: when `address` wins, all **six** `addr_*` columns MUST come from the **single winning contributing record**, never assembled field-by-field across records. `golden_lineage.attribute` vocabulary is exactly `{email, phone_e164, given_name, family_name, address, birth_date}`.

**Survivorship dispatch.** `golden_records.sql` builds, per attribute, a window over the entity's member rows in `int_std_records` and takes the rank-1 row. Each rule name in the config chain dispatches to one macro emitting one literal `ORDER BY` fragment, concatenated in chain order:

| Rule | Literal `ORDER BY` fragment |
|---|---|
| `source_priority` | `sources[source_system].priority_rank ASC` |
| `recency` | `COALESCE(updated_at_source, ingested_at) DESC` |
| `frequency` | `count(*) OVER (PARTITION BY entity_id, value) DESC` |
| `completeness` | `(value IS NOT NULL) DESC, length(value) DESC` |
| `validated` | `<attr>_valid DESC NULLS LAST` |
| *(terminal, mandatory)* | `record_key ASC` |

`record_key ASC` is a **MANDATORY terminal element of every chain**, which makes every chain a total order. When that element decides the winner, `golden_lineage.rule = 'tiebreak_deterministic'`. Survivorship tiebreaks are total orders; this is load-bearing for T-INC-1 and T-GOLD-1, because without it the winner depends on physical row order, which differs between the touched-subset and full-corpus materialisations. `int_std_records` carries `phone_valid BOOL` as well as `email_valid BOOL`, so `validated` has an input for both attributes. Pydantic rejects a chain containing no rule able to separate two records from the same source.

**`golden_lineage`** emits one row per `(entity_id, attribute)`: `source_system`, `source_record_id`, `record_key`, the winning `rule`, `survivorship_version`, `assembled_at`. `survivorship_version` is a config field under `versions:` in S6 — not a dbt var.

**`golden_display`** is a separate model applying presentation transforms (proper-casing, phone formatting) on top of `golden_records`. It is **presentation casing only and is never read by the matching layer**, so matching-layer data is never re-cased. It carries no `survivorship_version` of its own: it is one row per `entity_id` derived from `golden_records`, so its provenance is read by joining `golden_records` (and `golden_lineage`) on `entity_id`, which is why it is rebuilt and reaped in lockstep with them.

**Touched-only assembly.** `assemble.py` computes the touched set as a formula over this run's `entity_events` — `{entity_id : an event of type created, member_added, member_removed, merged, split, retired or edge_cut exists with this run_id}` — and writes it to `er_touched_entities(run_id, entity_id, disposition)` with `disposition ∈ {rebuild, retire}`. `retire` covers merge losers, emptied split fragments and retired entities; everything else is `rebuild`. dbt is invoked with **only** `--vars '{run_id: <ulid>}'`; the marts join `er_touched_entities` filtered on that `run_id`. Passing the ULID list itself as a var is forbidden: at the 1m scale tens of thousands of 26-char ULIDs exceed Linux's 128 KB per-argv-element limit and the invocation hard-fails with `E2BIG`.

**Explicit reap step (mandatory).** dbt's `delete+insert` deletes only keys **present in the freshly built batch**. A merge loser or an emptied fragment produces zero rows, so its key is absent, nothing is deleted, and its stale golden row survives forever. Therefore, **after** the marts run, `assemble.py` explicitly deletes from `golden_records`, `golden_lineage` **and `golden_display`** every `entity_id` in `er_touched_entities` with `disposition='retire'`. `golden_display` is reaped with the other two: an orphan display row is the same defect, and it is the row a consumer is most likely to read. History remains available through the DuckLake snapshot range recorded in `run_stages`. Mart config: `incremental_strategy='delete+insert', unique_key='entity_id', on_schema_change='sync_all_columns'`.

**`assembled_at`** is the **run's `started_at`**, written only for entities in the touched set. Untouched entities keep their prior value untouched, which is exactly what T-INC-2 asserts. dbt tests: every `golden_records.entity_id` has `entities.status='active'`; every active entity with ≥1 member has exactly one golden row.

**Counters** — `run_stages`: `rows_in` = touched entities, `rows_out` = golden rows written; `counters = {entities_touched, entities_rebuilt, entities_reaped, lineage_rows, tiebreak_deterministic_count, duration_ms}`.

<a id="s4-7"></a>
### 4.7 Failure semantics & recovery

**Error taxonomy.** Every failure is classified into `run_stages.error_class`, and the class determines the operator action:

| Class | Examples | Exit | Retryable |
|---|---|---|---|
| `transient_io` | S3 5xx, catalog connection reset, DNS failure | `1` | **Yes** — re-run the same `run_id` with `--resume` |
| `lock_conflict` | tenant advisory lock held by another writer | `3` | **Yes** — after the other run completes |
| `precondition` | lake not initialised, no active model, mixed `model_version`, breaking schema change | `3` | No — fix the precondition, then re-run |
| `config` | Pydantic validation, unknown source, unknown survivorship rule | `2` | No |
| `contradiction` | CONTRADICTION-1 | `1` | No — retract an assertion first |
| `non_convergence` | label propagation or cut fixpoint exceeded `clustering.max_iterations` | `1` | No — investigate the logged component |
| `data` | unparsable source file, `source_record_id` containing `':'`, uniqueness test failure | `1` | No |

**What a mid-run failure leaves committed.** Stages commit independently; there is **no cross-stage transaction**. A failure at stage *k* leaves stages `1..k-1` fully committed with `status='succeeded'` and their snapshot ranges recorded, and stage *k* with `status='failed'`, a partial or empty snapshot range, and populated `error_class`/`error_detail`. The `runs` row is `status='failed'`. Because every stage is idempotent against the keys in the S4 preamble, re-executing a failed stage over the same inputs is always safe.

Two stages additionally guarantee **all-or-nothing at the logical level**: `reconcile` commits membership and events in one snapshot after clustering succeeds — a CONTRADICTION-1, non-convergence or mixed-model failure emits **no events and writes no membership**; and `assemble`'s reap step runs only after the marts return zero.

**`--resume <run_id>`.** `er run-all --resume <run_id>` reads `run_stages` for that run, identifies the first stage whose `status` is not `succeeded`, and restarts the chain from there with the original `run_id`, `config_hash` and `model_version`. It refuses (exit `2`) if the config on disk hashes differently from the `runs.config_hash` of the run being resumed, or (exit `3`) if the run is already `succeeded`. Resume re-executes the failed stage in full; it never resumes mid-stage.

**Single writer.** The advisory-lock model is defined in S4.0b. Its recovery-relevant surface is: failure to acquire exits `3` with the literal message `writer lock held for tenant <t> by run <run_id>` and nothing is written, and `er lake maintain` / `er lake reset` take the same lock, so maintenance can never run concurrently with a pipeline stage.

**Non-goals (explicit).** There are **no automatic retry loops** anywhere in the CLI — a transient failure surfaces as a non-zero exit and is retried by the caller or the operator. There is **no snapshot rollback**: DuckLake time travel over the `run_stages` snapshot range is the recovery tool, and reading a prior state (`SELECT * FROM lake.main.golden_records AT (VERSION => :snap)`) is the supported operation. Snapshot expiry in `er lake maintain` never reaps a snapshot referenced by a `run_stages` row inside the retention window. Correcting a bad run means running the pipeline forward — a corrected ingest, a retracted assertion, or a full correction pass — never mutating history.

**Counters** — `run_stages` for the terminal state of every stage: `status`, `error_class`, `error_detail`, `started_at`, `ended_at`, `snapshot_start`, `snapshot_end`; the `runs` row additionally carries `status`, `ended_at`, and its own `snapshot_start`/`snapshot_end` spanning every stage.

---

<a id="s5"></a>
## 5. Data Model

Every relation lives in the attached DuckLake catalog under the alias `lake`, schema `main`. Tenancy is namespace-only, as defined in S1; that is why no relation below carries a tenant column.

Relations have exactly two owners — `src/er/lake/ddl.py` (Python `CREATE TABLE IF NOT EXISTS`) or dbt (`contract: {enforced: true}` in `schema.yml`). The ownership table, the logical keys, and the constraint model are normative in S5.0. The DDL below is the review-time authority for both owners: a dbt-owned relation's enforced contract MUST list exactly the columns and types written here.

Types used: `VARCHAR`, `BIGINT`, `DOUBLE`, `BOOLEAN`, `DATE`, `TIMESTAMP`, `JSON`, `LIST(VARCHAR)`. `NOT NULL` is the only constraint any statement may declare (S5.0).

**`ddl.py`-owned relations.**

```sql
CREATE TABLE IF NOT EXISTS lake.main.raw_records (
  source_system      VARCHAR   NOT NULL,
  source_record_id   VARCHAR   NOT NULL,
  payload            JSON      NOT NULL,
  content_hash       VARCHAR   NOT NULL,  -- SHA-256 hex; sentinel '0' * 64 when is_deleted (S4.1.1)
  is_deleted         BOOLEAN   NOT NULL,
  deleted_at         TIMESTAMP,           -- NULL unless is_deleted
  ingest_batch_id    VARCHAR   NOT NULL,
  ingested_at        TIMESTAMP NOT NULL
);

CREATE TABLE IF NOT EXISTS lake.main.match_scores (
  rec_a_key            VARCHAR   NOT NULL,  -- rec_a_key < rec_b_key, always
  rec_b_key            VARCHAR   NOT NULL,
  match_probability    DOUBLE    NOT NULL,
  model_version        VARCHAR   NOT NULL,  -- every row is scored; assertion edges are never persisted (S4.4)
  tf_snapshot_id       VARCHAR   NOT NULL,
  rec_a_content_hash   VARCHAR   NOT NULL,  -- content_hash of each endpoint at scoring time (INV-SCORE)
  rec_b_content_hash   VARCHAR   NOT NULL,
  evidence             JSON      NOT NULL,  -- gamma_* vector and per-comparison Bayes factors
  is_active            BOOLEAN   NOT NULL,
  invalidated_at       TIMESTAMP,
  invalidated_run_id   VARCHAR,
  run_id               VARCHAR   NOT NULL,
  scored_at            TIMESTAMP NOT NULL
);

CREATE TABLE IF NOT EXISTS lake.main.entity_membership (   -- CURRENT STATE, one row per record
  source_system      VARCHAR   NOT NULL,
  source_record_id   VARCHAR   NOT NULL,
  record_key         VARCHAR   NOT NULL,
  entity_id          VARCHAR   NOT NULL,
  assigned_at        TIMESTAMP NOT NULL,
  run_id             VARCHAR   NOT NULL
);

CREATE TABLE IF NOT EXISTS lake.main.entities (
  entity_id     VARCHAR   NOT NULL,
  status        VARCHAR   NOT NULL,   -- {active, merged, retired}
  merged_into   VARCHAR,              -- non-NULL iff status='merged'; external ID resolution only
  created_at    TIMESTAMP NOT NULL,
  updated_at    TIMESTAMP NOT NULL,
  created_run_id VARCHAR  NOT NULL,
  updated_run_id VARCHAR  NOT NULL
);

CREATE TABLE IF NOT EXISTS lake.main.entity_events (
  event_id      VARCHAR   NOT NULL,   -- monotonic ULID from ids.py
  seq           BIGINT    NOT NULL,   -- 1-based, dense, per run_id; replay orders by (occurred_at, seq)
  run_id        VARCHAR   NOT NULL,
  entity_id     VARCHAR   NOT NULL,
  event_type    VARCHAR   NOT NULL,   -- {created, member_added, member_removed, merged, split, retired, edge_cut}
  details       JSON      NOT NULL,
  details_hash  VARCHAR   NOT NULL,   -- SHA-256 of the canonicalised details document
  occurred_at   TIMESTAMP NOT NULL
);

CREATE TABLE IF NOT EXISTS lake.main.assertions (
  assertion_id  VARCHAR   NOT NULL,   -- ULID
  rec_a_key     VARCHAR   NOT NULL,   -- rec_a_key < rec_b_key, always
  rec_b_key     VARCHAR   NOT NULL,
  kind          VARCHAR   NOT NULL,   -- {always, never}
  active        BOOLEAN   NOT NULL,
  created_by    VARCHAR   NOT NULL,
  created_at    TIMESTAMP NOT NULL,
  retracted_by  VARCHAR,
  retracted_at  TIMESTAMP,
  note          VARCHAR
);

CREATE TABLE IF NOT EXISTS lake.main.review_queue (
  review_id          VARCHAR   NOT NULL,   -- ULID
  subject_type       VARCHAR   NOT NULL,   -- {pair, entity}
  rec_a_key          VARCHAR,              -- non-NULL iff subject_type='pair'
  rec_b_key          VARCHAR,
  entity_id          VARCHAR,              -- non-NULL iff subject_type='entity'
  reason             VARCHAR   NOT NULL,   -- {gray_band, never_unsatisfiable, coherence}
  match_probability  DOUBLE,
  waterfall          JSON,                 -- gamma_* comparison vector + per-comparison Bayes factors
  status             VARCHAR   NOT NULL,   -- {open, resolved_match, resolved_no_match, dismissed}
  first_seen_run_id  VARCHAR   NOT NULL,
  last_seen_run_id   VARCHAR   NOT NULL,
  resolved_by        VARCHAR,
  resolved_at        TIMESTAMP
);

CREATE TABLE IF NOT EXISTS lake.main.model_registry (
  model_version    VARCHAR   NOT NULL,   -- zero-padded 'v0001'; allocated max+1 at insert
  status           VARCHAR   NOT NULL,   -- {active, superseded}
  trained_at       TIMESTAMP NOT NULL,
  corpus_snapshot  BIGINT    NOT NULL,   -- DuckLake snapshot version trained against
  params_path      VARCHAR   NOT NULL,   -- {storage.model_uri_prefix}model_v{N}.json (S4.3.2)
  tf_tables_path   VARCHAR   NOT NULL,
  tf_snapshot_id   VARCHAR   NOT NULL,
  config_hash      VARCHAR   NOT NULL,
  metrics          JSON      NOT NULL,   -- verbatim training: block + fit metrics
  run_id           VARCHAR   NOT NULL
);

CREATE TABLE IF NOT EXISTS lake.main.tf_lookup (
  model_version   VARCHAR NOT NULL,
  tf_snapshot_id  VARCHAR NOT NULL,
  column_name     VARCHAR NOT NULL,
  value           VARCHAR NOT NULL,
  tf_value        DOUBLE  NOT NULL
);

CREATE TABLE IF NOT EXISTS lake.main.cut_edges (
  cut_id             VARCHAR   NOT NULL,   -- ULID
  rec_a_key          VARCHAR   NOT NULL,   -- rec_a_key < rec_b_key, always
  rec_b_key          VARCHAR   NOT NULL,
  match_probability  DOUBLE    NOT NULL,   -- probability of the edge at cut time
  model_version      VARCHAR,
  tf_snapshot_id     VARCHAR,
  assertion_id       VARCHAR   NOT NULL,   -- the active never assertion the cut satisfies
  active             BOOLEAN   NOT NULL,
  cut_run_id         VARCHAR   NOT NULL,
  cut_at             TIMESTAMP NOT NULL,
  released_run_id    VARCHAR,              -- set when the never assertion is retracted
  released_at        TIMESTAMP
);

CREATE TABLE IF NOT EXISTS lake.main.runs (
  run_id               VARCHAR   NOT NULL,   -- ULID
  tenant               VARCHAR   NOT NULL,
  mode                 VARCHAR   NOT NULL,   -- {incremental, full, train, init, maintain, reset, correction_pass, stage}
                                             -- 'stage' covers every standalone stage invocation
                                             -- (er ingest / standardize / match / reconcile / assemble /
                                             --  assert / review / doctor run outside a run-all chain);
                                             -- run_stages then holds exactly one row naming which stage it was
  status               VARCHAR   NOT NULL,   -- {running, succeeded, failed}
  started_at           TIMESTAMP NOT NULL,
  ended_at             TIMESTAMP,
  config_hash          VARCHAR   NOT NULL,
  model_version        VARCHAR,
  tf_snapshot_id       VARCHAR,
  std_version          VARCHAR   NOT NULL,
  survivorship_version VARCHAR   NOT NULL,
  code_version         VARCHAR   NOT NULL,   -- git describe --always --dirty
  rebuild_reason       VARCHAR,              -- {std_version_bump, survivorship_version_bump, correction_pass, operator}
  snapshot_start       BIGINT,
  snapshot_end         BIGINT
);

CREATE TABLE IF NOT EXISTS lake.main.run_stages (
  run_id             VARCHAR   NOT NULL,
  stage              VARCHAR   NOT NULL,   -- {init, ingest, standardize, train, match, reconcile, assemble, maintain, reset}
  seq                BIGINT    NOT NULL,   -- 1-based, dense, per run_id
  status             VARCHAR   NOT NULL,   -- {running, succeeded, failed}
  started_at         TIMESTAMP NOT NULL,
  ended_at           TIMESTAMP,
  snapshot_start     BIGINT,
  snapshot_end       BIGINT,
  rows_in            BIGINT,
  rows_out           BIGINT,
  candidate_pairs    BIGINT,
  pairs_above_auto_merge BIGINT,
  entities_created   BIGINT,
  entities_merged    BIGINT,
  entities_split     BIGINT,
  entities_retired   BIGINT,
  edges_cut          BIGINT,
  review_queue_added BIGINT,
  duration_ms        BIGINT,
  counters           JSON,                -- the stage's free-form named counters (S4, S5.2)
  error_class        VARCHAR,
  error_detail       VARCHAR
);

CREATE TABLE IF NOT EXISTS lake.main.ingest_batches (
  ingest_batch_id  VARCHAR   NOT NULL,   -- ULID
  run_id           VARCHAR   NOT NULL,
  source_system    VARCHAR   NOT NULL,
  path             VARCHAR   NOT NULL,
  new_count        BIGINT    NOT NULL,
  changed_count    BIGINT    NOT NULL,
  unchanged_count  BIGINT    NOT NULL,
  tombstone_count  BIGINT    NOT NULL,
  resurrected_count BIGINT   NOT NULL,   -- S4.1.1; the fifth count, persisted, not manifest-only
  full_refresh_keys BOOLEAN  NOT NULL,
  created_at       TIMESTAMP NOT NULL
);

CREATE TABLE IF NOT EXISTS lake.main.er_touched_entities (
  run_id       VARCHAR   NOT NULL,
  entity_id    VARCHAR   NOT NULL,
  disposition  VARCHAR   NOT NULL,   -- {rebuild, retire}
  created_at   TIMESTAMP NOT NULL
);
```

**dbt-owned relations.** Declared here as typed column lists; the physical DDL is emitted by dbt and its `schema.yml` contract MUST match this listing column-for-column and type-for-type. Ownership — including what `ddl.py` may never touch — is normative in S5.0.

```sql
-- stg_crm, stg_billing, stg_webforms (one model per source in sources:, identical shape)
stg_<source>(
  source_system VARCHAR NOT NULL, source_record_id VARCHAR NOT NULL,
  content_hash VARCHAR NOT NULL, std_version VARCHAR NOT NULL,
  given_name VARCHAR, family_name VARCHAR, name_variants LIST(VARCHAR) NOT NULL,
  email VARCHAR, email_valid BOOLEAN, phone_e164 VARCHAR, phone_valid BOOLEAN,
  addr_number VARCHAR, addr_street VARCHAR, addr_unit VARCHAR,
  addr_city VARCHAR, addr_region VARCHAR, addr_postal VARCHAR,
  birth_date DATE, updated_at_source TIMESTAMP,
  ingest_batch_id VARCHAR NOT NULL, ingested_at TIMESTAMP NOT NULL)

int_std_records(
  record_key VARCHAR NOT NULL,                -- source_system || ':' || source_record_id
  source_system VARCHAR NOT NULL, source_record_id VARCHAR NOT NULL,
  content_hash VARCHAR NOT NULL, std_version VARCHAR NOT NULL,
  given_name VARCHAR, family_name VARCHAR, name_variants LIST(VARCHAR) NOT NULL,
  email VARCHAR, email_valid BOOLEAN, phone_e164 VARCHAR, phone_valid BOOLEAN,
  addr_number VARCHAR, addr_street VARCHAR, addr_unit VARCHAR,
  addr_city VARCHAR, addr_region VARCHAR, addr_postal VARCHAR,
  birth_date DATE, updated_at_source TIMESTAMP,
  ingest_batch_id VARCHAR NOT NULL, ingested_at TIMESTAMP NOT NULL)

int_blocking_keys(
  key_type VARCHAR NOT NULL, key_value VARCHAR NOT NULL,
  record_key VARCHAR NOT NULL,
  source_system VARCHAR NOT NULL, source_record_id VARCHAR NOT NULL)

golden_records(
  entity_id VARCHAR NOT NULL,
  given_name VARCHAR, family_name VARCHAR,
  email VARCHAR, phone_e164 VARCHAR,
  addr_number VARCHAR, addr_street VARCHAR, addr_unit VARCHAR,
  addr_city VARCHAR, addr_region VARCHAR, addr_postal VARCHAR,
  birth_date DATE,
  survivorship_version VARCHAR NOT NULL, assembled_at TIMESTAMP NOT NULL)

golden_lineage(
  entity_id VARCHAR NOT NULL,
  attribute VARCHAR NOT NULL,              -- {email, phone_e164, given_name, family_name, address, birth_date}
  record_key VARCHAR NOT NULL,
  source_system VARCHAR NOT NULL, source_record_id VARCHAR NOT NULL,
  rule VARCHAR NOT NULL,                   -- the rule that decided, or 'tiebreak_deterministic'
  survivorship_version VARCHAR NOT NULL, assembled_at TIMESTAMP NOT NULL)

golden_display(
  entity_id VARCHAR NOT NULL,
  display_name VARCHAR, display_email VARCHAR, display_phone VARCHAR,
  display_address VARCHAR,
  assembled_at TIMESTAMP NOT NULL)
```

The **eleven** columns from `given_name` through `birth_date` are the **survivable column set** of `golden_records`; `survivorship_version` and `assembled_at` are provenance stamps, not survivable attributes, and `entity_id` is the key. Normative: that survivable column set is exactly the key set of `survivorship:` in S6, with `address` expanding to the six `addr_*` columns (S6.1 V2 enforces the equality; S4.6 owns the rule that those six come from one winning record). `golden_display` is presentation casing only and MUST never be read by the matching layer.

<a id="s5-0"></a>
### 5.0 Ownership & key model

**Constraint model.** DuckLake enforces `NOT NULL` only. It supports no `PRIMARY KEY`, `UNIQUE`, `FOREIGN KEY`, `CHECK`, no indexes, no sequences, no non-literal `DEFAULT`s, and no `ENUM`. Therefore:

- Every key in this section is a **logical** key, enforced by dbt `unique` / `dbt_utils.unique_combination_of_columns` tests and by `MERGE INTO` on write.
- All IDs are ULIDs minted in Python (`src/er/entities/ids.py`); there are no sequences and no generated columns.
- All timestamps are supplied by the writer; there is no `DEFAULT now()`.
- Every `in {…}` domain in the DDL is a plain `VARCHAR` validated by a dbt `accepted_values` test.
- `JSON` and `LIST(VARCHAR)` are supported and retained; fixed-size `ARRAY` is not and MUST NOT be used.

**Owners.** Exactly two. `ddl.py` never issues DDL against a dbt-owned relation; dbt-owned models set `contract: {enforced: true}` in `schema.yml` so that a column or type drift fails `dbt build` and S5 stays the review-time authority.

| Relation | Owner | Logical key | Enforced by |
|---|---|---|---|
| `raw_records` | ddl.py | `(source_system, source_record_id, content_hash)` | `unique_combination_of_columns` |
| `match_scores` | ddl.py | `(rec_a_key, rec_b_key, model_version, tf_snapshot_id)` — unfiltered; it is the `MERGE INTO` key of S4.3.4, and invalidation updates `is_active` in place rather than adding a row | `unique_combination_of_columns` |
| `entity_membership` | ddl.py | `(source_system, source_record_id)` | `unique_combination_of_columns` |
| `entities` | ddl.py | `entity_id` | `unique` |
| `entity_events` | ddl.py | `event_id`; idempotency key `(run_id, entity_id, event_type, details_hash)` | `unique` + `unique_combination_of_columns` |
| `assertions` | ddl.py | `assertion_id`; `(rec_a_key, rec_b_key)` where `active` | `unique` + filtered `unique_combination_of_columns` |
| `review_queue` | ddl.py | `review_id`; `(subject_type, rec_a_key, rec_b_key, entity_id, reason)` where `status='open'` — `reason` is in the tuple because one pair can be open for two independent reasons (S4.3.5 `gray_band` and S4.4.2 `never_unsatisfiable`), and collapsing them would drop a steward task | `unique` + filtered `unique_combination_of_columns` |
| `model_registry` | ddl.py | `model_version`; at most one row with `status='active'` | `unique` + singular test |
| `tf_lookup` | ddl.py | `(model_version, tf_snapshot_id, column_name, value)` | `unique_combination_of_columns` |
| `cut_edges` | ddl.py | `cut_id`; `(rec_a_key, rec_b_key)` where `active` | `unique` + filtered `unique_combination_of_columns` |
| `runs` | ddl.py | `run_id` | `unique` |
| `run_stages` | ddl.py | `(run_id, seq)`; `(run_id, stage)` | `unique_combination_of_columns` |
| `ingest_batches` | ddl.py | `ingest_batch_id` | `unique` |
| `er_touched_entities` | ddl.py | `(run_id, entity_id)` | `unique_combination_of_columns` |
| `stg_crm` / `stg_billing` / `stg_webforms` | dbt | `(source_system, source_record_id, content_hash)` | contract + `unique_combination_of_columns` |
| `int_std_records` | dbt | `record_key` | contract + `unique` |
| `int_blocking_keys` | dbt | `(key_type, key_value, record_key)` | contract + `unique_combination_of_columns` |
| `golden_records` | dbt | `entity_id` | contract + `unique` |
| `golden_lineage` | dbt | `(entity_id, attribute)` | contract + `unique_combination_of_columns` |
| `golden_display` | dbt | `entity_id` | contract + `unique` |

**Record identity.** `record_key VARCHAR := source_system || ':' || source_record_id` is the canonical scalar record identity. It is materialized on `int_std_records` and `int_blocking_keys`, and is Splink's `unique_id_column_name`. `source_record_id` MUST NOT contain `':'` — enforced by a dbt singular test on `int_std_records`. `entity_membership.record_key` is a materialized copy and MUST equal `source_system || ':' || source_record_id` on every row (dbt singular test).

**Canonical pair ordering.** Every pair row in `match_scores`, `assertions`, `review_queue` (`subject_type='pair'`) and `cut_edges` MUST satisfy `rec_a_key < rec_b_key` lexically. Canonicalisation happens in one shared helper in `src/er/entities/ids.py` at write time; readers never perform a two-sided join. A dbt singular test asserts `rec_a_key < rec_b_key` on all four relations. This invariant is the precondition that makes assertion application correct and is part of the reconciliation determinism total order.

**VOLATILE_COLUMNS.** Defined once as a frozen set in `src/er/lake/columns.py` and excluded from every determinism comparison (content hashes, `expected/` diffs, T-STD-1, T-INC-1, T-IDEM-1):

```python
VOLATILE_COLUMNS = frozenset({
    "ingest_batch_id", "ingested_at", "assembled_at", "scored_at",
    "assigned_at", "occurred_at", "run_id", "event_id", "seq",
})
```

No comparison helper may hard-code its own exclusion list; all three helpers in `tests/helpers/compare.py` import this set.

**GOLDEN_SURVIVABLE_COLUMNS.** Also defined once in `src/er/lake/columns.py`, as the ordered tuple of `golden_records` columns produced by survivorship — that is, every column of `golden_records` except `entity_id`, `survivorship_version` and `assembled_at`:

```python
GOLDEN_SURVIVABLE_COLUMNS = (
    "given_name", "family_name", "email", "phone_e164",
    "addr_number", "addr_street", "addr_unit", "addr_city", "addr_region", "addr_postal",
    "birth_date",
)
```

These are exactly the eleven columns S5 names, in DDL order. `email_valid` and `phone_valid` are deliberately absent: they exist on `int_std_records` as the inputs to the `validated` survivorship rule (V4), they are not columns of `golden_records`, and they are not survivable.

The normative rule of S4.6 is checkable against this tuple: expanding the `survivorship:` keys of S6, with `address` expanding to the six `addr_*` columns, MUST yield exactly this set. A unit test asserts the equality in both directions, so adding a `golden_records` column without a survivorship rule — or a rule without a column — fails at config-validation time rather than producing a silently NULL golden attribute.

<a id="s5-1"></a>
### 5.1 Schema evolution

- **Idempotent application.** `er init` applies `ddl.py` as `CREATE TABLE IF NOT EXISTS` for ddl.py-owned relations only, then reconciles columns. Running it on an initialised lake is a no-op that exits `0`; running it on a lake whose `DATA_PATH` disagrees with the catalog exits `3`.
- **Additive only.** The only schema change `ddl.py` may perform on an existing relation is `ALTER TABLE … ADD COLUMN <name> <type>` with no `NOT NULL` and no `DEFAULT` (DuckLake permits neither on an added column). New columns are backfilled by an explicit `UPDATE` in the same stage or left NULL.
- **Breaking changes.** Dropping a column, renaming a column, narrowing or otherwise changing a column type, and removing a value from an `accepted_values` domain are **breaking**. `er init` and every stage's preflight detect a breaking difference between `ddl.py` and the live catalog and abort with exit code `3` and the named message `ERR_SCHEMA_BREAKING: <relation>.<column>: <live_type> -> <declared_type>`. No snapshot is committed. The operator path is an explicit migration script plus a full rebuild; there is no automatic destructive migration.
- **dbt-owned relations** evolve through their enforced contract. A contract violation fails `dbt build`, which fails the stage with exit code `1`. Every incremental dbt model sets `on_schema_change='sync_all_columns'`, so an additive column introduced by a `std_version` bump propagates instead of being silently dropped.
- **Version bumps are planned full rebuilds.** Changing `versions.std_version` or `versions.survivorship_version` invalidates the derived corpus. The affected stages MUST run non-incrementally (`er standardize` without `--changed-only` / `er assemble` without `--touched-only`), the run is recorded with `runs.rebuild_reason` set to `std_version_bump` or `survivorship_version_bump`, and such a run is **explicitly outside T-INC-2's accounting** — T-INC-2 asserts `rewritten ∪ reaped == touched` only for runs whose `rebuild_reason` is NULL. A version bump is exactly the drift the S4.0 config-drift guard catches; that guard and `--allow-escalate` are defined there.
- **Time travel** is supported across additive changes only. A snapshot taken before an `ADD COLUMN` reads back with the column absent; a query written against the current schema MUST therefore project explicitly rather than `SELECT *`. Time travel across a breaking migration is not supported and MUST NOT be relied on by any test.

<a id="s5-2"></a>
### 5.2 Run & batch metadata

This is the observability layer. Four relations give `run_id` a referent, make snapshot ranges addressable, and persist the ingest manifest and the touched set. Their DDL is in S5; their semantics are here.

**`runs`** — one row per CLI invocation that mints or receives a `run_id`. `run-all` mints one `run_id` and threads it through every stage. `config_hash` is **SHA-256 over the canonicalised Pydantic-validated config document**: the config is loaded, validated into the Pydantic model, dumped with keys sorted, no aliases, UTF-8, compact separators, and hashed; the hex digest is `config_hash`. It is computed once at CLI start and written to `runs`, `model_registry` and every stage's log line. `snapshot_start` is the lake's current snapshot version immediately before the first stage begins; `snapshot_end` is the version after the last stage commits.

**`run_stages`** — one row per stage per run, keyed `(run_id, seq)` with `seq` dense and 1-based. It is the persistence target for the snapshot **range** each stage produced, in `(snapshot_start, snapshot_end)`; the range-not-count rule itself is normative in the S4 preamble, and its test-side consequence in S8.1.

**Counter vocabulary.** Counters live in two places, and the split is normative.

1. **Promoted counters** — the closed set that is a *typed column* of `run_stages`: `rows_in`, `rows_out`, `candidate_pairs`, `pairs_above_auto_merge`, `entities_created`, `entities_merged`, `entities_split`, `entities_retired`, `edges_cut`, `review_queue_added`, `duration_ms`. Each is a nullable `BIGINT`; a stage writes NULL for counters that do not apply to it (e.g. `er ingest` writes `rows_in`, `rows_out`, `duration_ms` and NULL elsewhere). This list is closed: promoting a new counter to a column is a schema change under S5.1.
2. **Per-stage counters** — the free-form named counters each S4 subsection ends with (`files`, `models_run`, `blocking_keys_by_type`, `pairs_in_gray_band`, `label_prop_iterations`, `tiebreak_deterministic_count`, …). They are written as a JSON object into the `run_stages.counters` column. Stages MAY define their own names there; a stage MUST NOT invent a *column*.

**Completeness (normative).** A stage's `counters` payload is the **union** of (a) the per-stage names listed at the end of its S4 subsection and (b) every promoted counter that stage writes to a typed column. The S4 lists enumerate only the stage-specific names — they deliberately omit `rows_in` and `rows_out`, which the same paragraph states separately — and the writer adds the promoted ones, so neither list has to restate the other and the JSON object alone is still a complete record of the stage.

**Structured logging.** On completion, every stage MUST emit the same record as **exactly one JSON line on stderr**, keyed by `run_id`:

```json
{"run_id":"01J...","stage":"match","seq":4,"status":"succeeded","started_at":"2026-01-01T00:00:00Z","ended_at":"2026-01-01T00:04:12Z","snapshot_start":118,"snapshot_end":121,"config_hash":"a1b2…","model_version":"v0001","tf_snapshot_id":"01J…","rows_in":10000,"rows_out":8421,"candidate_pairs":184203,"pairs_above_auto_merge":7310,"entities_created":null,"entities_merged":null,"entities_split":null,"entities_retired":null,"edges_cut":null,"review_queue_added":112,"duration_ms":252118,"error_class":null,"error_detail":null}
```

**stdout is reserved for command output** (manifests, query results, `er doctor` reports) so a caller can pipe stdout without parsing telemetry. A failed stage writes the same line with `status:"failed"` plus `error_class` and `error_detail`, and the process exits `1`.

**`ingest_batches`** — the persistence target for S4.1's batch manifest: one row per `(source_system, path)` delivery, carrying `new_count`, `changed_count`, `unchanged_count`, `tombstone_count`, `resurrected_count` and `full_refresh_keys`. T-IDEM-1a asserts `new_count = 0 AND changed_count = 0 AND tombstone_count = 0` on a re-ingest.

**`er_touched_entities`** — the run's touched set, written by `assemble.py` before the marts run so the marts join it on `run_id` instead of receiving a `--vars` payload that would exceed the argv limit. `disposition='rebuild'` entities are re-assembled; `disposition='retire'` entities (merge losers, emptied fragments, retired entities) are deleted from `golden_records`, `golden_lineage` and `golden_display` after the marts run, because `delete+insert` cannot remove a key absent from the incoming batch. History for reaped entities remains readable via the snapshot range in `run_stages`.

---

<a id="s6"></a>
## 6. Configuration (`configs/*.yaml`)

One Pydantic-validated YAML document per tenant. It is the single source of truth for every tunable in the pipeline: blocking, comparisons, survivorship, training arguments, storage locations and versions. Validation runs at CLI start before any lake connection is opened; a validation failure exits `2` with the offending JSON pointer. `configs/test.yaml` below is the file the fixtures and CI use verbatim.

```yaml
tenant: test

thresholds:
  auto_merge: 0.95
  review_low: 0.60

standardization:
  email_strip_plus_addressing: false
  email_placeholders: ["test@test.com", "noreply@example.com"]   # nulled by email_norm (S4.2)
  phone_default_region: US                                       # default region for phone_e164 (S4.2)

sources:
  crm:
    adapter: csv
    priority_rank: 1
    record_id_column: crm_id
    updated_at_column: last_modified
    date_format: "%Y-%m-%d"
    columns:
      given_name:   first_name
      family_name:  last_name
      email:        email_address
      phone:        phone
      address_line: street_address
      addr_city:    city
      addr_region:  state
      addr_postal:  zip
      birth_date:   dob
  billing:
    adapter: csv
    priority_rank: 2
    record_id_column: account_no
    updated_at_column: updated_ts
    date_format: "%m/%d/%Y"
    columns:
      given_name:   fname
      family_name:  lname
      email:        billing_email
      phone:        contact_phone
      address_line: addr1
      addr_city:    addr_city
      addr_region:  addr_state
      addr_postal:  postal_code
      birth_date:   birth_date
  webforms:
    adapter: csv
    priority_rank: 3
    record_id_column: submission_id
    updated_at_column: submitted_at
    date_format: "%Y-%m-%d"
    columns:
      given_name:   name_first
      family_name:  name_last
      email:        email
      phone:        phone_number
      address_line: address
      addr_city:    city
      addr_region:  region
      addr_postal:  postcode
      birth_date:   date_of_birth

blocking:
  - { key_type: email_exact, expr: "email" }
  - { key_type: phone_exact, expr: "phone_e164" }
  - { key_type: name_postal, expr: "substr(family_name,1,4) || '|' || addr_postal" }
  - { key_type: dob_name,    expr: "birth_date || '|' || substr(given_name,1,3)" }

comparisons:
  given_name:  { levels: [exact, jaro_winkler:0.90, variant_match, null], tf: true }
  family_name: { levels: [exact, jaro_winkler:0.90, null], tf: true }
  email:       { levels: [exact, username_exact, null], tf: true }
  phone_e164:  { levels: [exact, null] }
  birth_date:  { levels: [exact, dob_same_year_month, null] }
  addr_postal: { levels: [exact, null] }

survivorship:
  email:       [validated, source_priority, recency]
  phone_e164:  [validated, source_priority, recency]
  given_name:  [source_priority, frequency, completeness]
  family_name: [source_priority, frequency, completeness]
  address:     [recency, source_priority]
  birth_date:  [frequency, source_priority]

training:
  deterministic_rules:
    - "l.email = r.email"
    - "l.phone_e164 = r.phone_e164"
    - "l.family_name = r.family_name and l.birth_date = r.birth_date"
  recall: 0.85
  u_max_pairs: 1000000
  u_seed: 20260101
  em_blocking_rules:
    - "l.email = r.email"
    - "l.family_name = r.family_name and l.addr_postal = r.addr_postal"
  em:
    fix_u_probabilities: true

storage:
  data_path: "s3://lake/er/"
  drop_dir: "/app/drop"
  model_uri_prefix: "s3://lake/models/test/"

versions:
  std_version: "1"
  survivorship_version: "1"
  address_parser_version: "1"

generator:
  seed: 42

clustering:
  max_iterations: 50
  cut_protect_probability: 1.0

coherence:
  scorer: noop

correction_pass:
  cadence: "0 3 * * 0"     # weekly, Sunday 03:00 UTC
```

`address_line` is the raw single-line address the v1 parser componentizes into `addr_number`, `addr_street` and `addr_unit`; `addr_city`, `addr_region` and `addr_postal` map directly. `phone` maps to the source column the `phone_e164` macro normalizes.

Threshold units, the half-open gray band `review_low <= p < auto_merge`, and the rule that the clustering threshold **is** `auto_merge` are defined normatively in S4.3; this block only supplies the two numbers.

`config_hash` — SHA-256 over the canonicalised Pydantic-validated config document — is defined normatively in S5.2 and is written to `runs.config_hash`, `model_registry.config_hash` and every stage log line. It is not itself a config field.

**`std_version` is passed to dbt by the CLI as a `--vars` override on every dbt invocation** (`--vars '{std_version: <value>, survivorship_version: <value>, run_id: <ulid>}'`) so that this file is the single source of truth. `dbt_project.yml` declares the same vars only as fallbacks for a bare `dbt parse` / `dbt compile --target mem`; a divergence between the two is invisible to the pipeline because the override always wins.

<a id="s6-1"></a>
### 6.1 Validation rules

Pydantic rejects the document — exit code `2`, no lake connection opened — unless **all** of the following hold. Each has a unit test in `tests/unit/test_config.py`.

| # | Validator | Failure message key |
|---|---|---|
| V1 | `0 < thresholds.review_low < thresholds.auto_merge <= 1` | `thresholds.ordering` |
| V2 | `set(survivorship.keys()) == {email, phone_e164, given_name, family_name, address, birth_date}` — the survivorship key set MUST equal the survivable column set of `golden_records` (S5), with `address` expanding to the six `addr_*` columns | `survivorship.keyset` |
| V3 | Every rule name in every chain is one of `source_priority`, `recency`, `frequency`, `completeness`, `validated` | `survivorship.unknown_rule` |
| V4 | A chain containing `validated` for attribute `<attr>` requires a matching `<attr>_valid` column on `int_std_records`; only `email` (`email_valid`) and `phone_e164` (`phone_valid`) qualify | `survivorship.validated_missing_column` |
| V5 | Every chain contains at least one rule that can separate two records **from the same source** — i.e. a chain consisting solely of `source_priority` is rejected, since `record_key ASC` would then decide every contest | `survivorship.not_separating` |
| V6 | Every column referenced by `blocking[].expr` and every key of `comparisons` exists in the `int_std_records` column list of S5; `variant_match` additionally requires `name_variants` | `columns.unknown` |
| V7 | `blocking[].key_type` values are unique | `blocking.duplicate_key_type` |
| V8 | Every level token in `comparisons[*].levels` is one of `exact`, `jaro_winkler:<T>` with `0 < T <= 1`, `null`, `username_exact`, `variant_match`, `dob_same_year_month` | `comparisons.unknown_level` |
| V9 | `training.em_blocking_rules` has **at least 2** entries; `training.deterministic_rules` has at least 1 | `training.em_blocking_rules.min_items` |
| V10 | `training.u_seed` is **REQUIRED** and has no default; `training.u_max_pairs >= 1`; `0 < training.recall <= 1` | `training.u_seed.required` |
| V11 | Every `sources.<name>.columns` maps every canonical attribute the standardization macros consume (`given_name`, `family_name`, `email`, `phone`, `address_line`, `addr_city`, `addr_region`, `addr_postal`, `birth_date`); `priority_rank` values are unique across sources and are positive integers | `sources.columns.incomplete` |
| V12 | `clustering.max_iterations >= 1`; `0 < clustering.cut_protect_probability <= 1` | `clustering.bounds` |
| V13 | `versions.std_version`, `versions.survivorship_version`, `versions.address_parser_version` are non-empty strings | `versions.required` |
| V14 | `storage.data_path` is an `s3://` URI ending in `/`; `storage.model_uri_prefix` is an `s3://` URI ending in `/`; `storage.drop_dir` is an absolute path | `storage.uri` |
| V15 | `correction_pass.cadence` parses as a 5-field cron expression | `correction_pass.cadence` |
| V16 | `standardization.email_placeholders` is a list (possibly empty) of lowercase, syntactically valid email addresses with no duplicates; `standardization.phone_default_region` is a 2-letter ISO 3166-1 alpha-2 code. Both are consumed by `email_norm` / `phone_e164` in S4.2 and by no other macro | `standardization.invalid` |

Normalization performed after validation, before `config_hash` is computed: the `null` token is removed from every `levels` list — the comparison builder ALWAYS emits `NullLevel(col)` first and `ElseLevel()` last regardless of what the list contains — and `record_key ASC` is appended as the mandatory terminal element of every survivorship chain, making each chain a total order. Both normalizations are part of the canonicalised document, so two configs that differ only in a redundant `null` token hash identically.

**Environment variables.** Config paths and secrets are not in the YAML. The complete set the CLI reads: `ER_CONFIG`, `ER_CATALOG_DSN`, `ER_S3_ENDPOINT` (host:port, **no** scheme), `ER_S3_ACCESS_KEY_ID`, `ER_S3_SECRET_ACCESS_KEY`, `ER_S3_REGION`, `ER_S3_URL_STYLE`, `ER_S3_USE_SSL`, `ER_LAKE_DATA_PATH`, `ER_LAKE_ALIAS`, `ER_LAKE_METADATA_SCHEMA`, `ER_DUCKDB_THREADS`, `ER_DUCKDB_MEMORY_LIMIT`. A missing required variable exits `2`; `ER_LAKE_DATA_PATH` disagreeing with the value recorded in the catalog exits `3`.

---

<a id="s7"></a>
## 7. Docker Compose Environment

`docker/compose.yaml` is the test/dev substrate and the only substrate CI uses. Profiles: `test` (PR path) and `bench` (workflow_dispatch / weekly). The stack is four services plus two one-shot initialisers; the one-shots are the reason the stack is driven with `docker compose run`, never `up` (S7.4).

<a id="s7-1"></a>
### 7.1 `docker/compose.yaml`

```yaml
name: er                                   # fixed project name; without it Compose derives `docker`
                                           # from the directory and the prebuilt image is never found

x-er-env: &er-env
  ER_CONFIG:                 /app/configs/test.yaml
  ER_CATALOG_DSN:            postgresql://er:erpassword@catalog:5432/ducklake
  ER_S3_ENDPOINT:            objectstore:9000        # host:port — NO scheme; DuckDB httpfs rejects a URL here
  ER_S3_ACCESS_KEY_ID:       erminio
  ER_S3_SECRET_ACCESS_KEY:   erminiopassword
  ER_S3_REGION:              us-east-1
  ER_S3_URL_STYLE:           path
  ER_S3_USE_SSL:             "false"
  ER_LAKE_DATA_PATH:         s3://lake/er/
  ER_LAKE_ALIAS:             lake
  ER_LAKE_METADATA_SCHEMA:   er_main
  ER_DUCKDB_THREADS:         "${ER_CPU_LIMIT:-2}"          # == the cpus quota below, always
  ER_DUCKDB_MEMORY_LIMIT:    "${ER_DUCKDB_MEMORY_LIMIT:-4GB}"

x-er-image: &er-image
  image: er-pipeline:ci          # built once by CI (or `docker compose build`) and consumed as-is
  pull_policy: never             # never attempt a registry pull for a local-only tag
  build: { context: .., dockerfile: docker/Dockerfile }

services:
  catalog:                                 # DuckLake catalog database
    image: postgres:16@sha256:11a9d238fbb48bab14599c57e41123254452b1a2d93c6c8595bce96f346bd082
    environment:
      POSTGRES_DB: ducklake
      POSTGRES_USER: er
      POSTGRES_PASSWORD: erpassword
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U er -d ducklake"]
      interval: 2s
      timeout: 3s
      retries: 30

  objectstore:                             # S3-compatible storage for DuckLake data files
    # EOL upstream; pinned by digest so the substrate cannot drift (S2.1, S13)
    image: minio/minio:RELEASE.2025-09-07T16-13-09Z@sha256:14cea493d9a34af32f524e538b8346cf79f3321eff8e708c1e2960462bd8936e
    command: server /data --console-address ":9001"
    environment:
      MINIO_ROOT_USER:     erminio         # >= 3 chars — MinIO refuses to start below the minimum
      MINIO_ROOT_PASSWORD: erminiopassword # >= 8 chars
    # NO healthcheck: the server image ships no `mc` binary and holds no `local` alias, so a
    # `mc ready local` probe can never pass, objectstore-init would never start, and the stack
    # would hang exactly as it did before. Readiness is proven by objectstore-init instead.

  objectstore-init:                        # create the bucket; same credentials as the server
    image: minio/mc:RELEASE.2025-08-13T08-35-41Z@sha256:a7fe349ef4bd8521fb8497f55c6042871b2ae640607cf99d9bede5e9bdf11727
    depends_on:
      objectstore: { condition: service_started }
    entrypoint:
      - /bin/sh
      - -c
      - >
        for i in $$(seq 1 60); do
          mc alias set local http://objectstore:9000 "$$MINIO_ROOT_USER" "$$MINIO_ROOT_PASSWORD" && break;
          sleep 1;
        done &&
        mc ready local &&
        mc mb -p local/lake
    environment:
      MINIO_ROOT_USER:     erminio
      MINIO_ROOT_PASSWORD: erminiopassword

  catalog-init:                            # one-shot lake bootstrap: extensions, secret, ATTACH, ddl.py tables
    <<: *er-image
    depends_on:
      catalog:          { condition: service_healthy }
      objectstore-init: { condition: service_completed_successfully }
    environment: *er-env
    command: ["er", "init"]                # idempotent; exit 0 on an already-initialised lake
    restart: "no"

  pipeline:
    <<: *er-image
    profiles: [test]
    depends_on:
      catalog-init: { condition: service_completed_successfully }
    environment: *er-env
    volumes:
      - ../artifacts:/app/artifacts        # junit, dbt logs, run manifests reach the host/runner
    command: ["pytest", "tests/integration", "-q", "--maxfail=3",
              "--junitxml=/app/artifacts/junit.xml", "--durations=20"]
    deploy: { resources: { limits: { cpus: "${ER_CPU_LIMIT:-2}", memory: "${ER_MEM_LIMIT:-6g}" } } }

  benchmark:
    <<: *er-image
    profiles: [bench]
    depends_on:
      catalog-init: { condition: service_completed_successfully }
    environment:
      <<: *er-env
      BENCH_SCALE: ${BENCH_SCALE:-smoke}
    volumes:
      - ../artifacts:/app/artifacts        # artifacts/bench/latest.json + report.md
      - benchdata:/app/.bench              # generated corpus; kept out of the artifacts mount
    command: ["python", "benchmarks/report.py", "--run",
              "--scale", "${BENCH_SCALE:-smoke}", "--repeat", "3",
              "--out", "/app/artifacts/bench/latest.json"]
    # The envelope is a property of the SCALE (S10.2), exported by the S9.2 preflight.
    # Defaults are the smoke/10k row, which is also the `test` profile's envelope.
    deploy: { resources: { limits: { cpus: "${ER_CPU_LIMIT:-2}", memory: "${ER_MEM_LIMIT:-6g}" } } }

volumes:
  benchdata:
```

Normative points on the file above:

- `catalog-init` declares no `profiles:` key, so it is enabled under every profile and both `pipeline` and `benchmark` gate on it with `service_completed_successfully`. `er init` is the only thing that ever creates `ddl.py`-owned tables (S4.0/S5).
- `pipeline` and `benchmark` carry the identical `ER_*` set; `ER_CONFIG` is mandatory on both. Every variable in the environment contract appears exactly once, in `x-er-env`.
- `deploy.resources.limits` is honoured by `docker compose` outside swarm mode. The three values `ER_CPU_LIMIT` / `ER_MEM_LIMIT` / `ER_DUCKDB_MEMORY_LIMIT` are the **resource envelope**, and the envelope is a property of the benchmark **scale**, not of this file: S10.2 owns the per-scale table and the S9.2 preflight exports the row before `docker compose` reads it. The defaults compiled in here — `2` / `6g` / `4GB` — are the `test` profile's envelope and are identical to S10.2's `smoke` and `10k` row, so the PR path fits a standard 2-vCPU / 8 GB hosted runner with RAM headroom for `catalog` and `objectstore` — the CPU value is a quota and deliberately reserves nothing for them (S10.2) — and a `100k` dispatch gets the larger envelope its larger runner can actually supply.
- **`cpus` is a quota, not a cpuset.** `deploy.resources.limits.cpus` sets the cgroup CPU *quota* (`cpu.max` = quota/period); it does not restrict which cores are visible, so in-container `nproc` keeps reporting the **host** core count no matter what the limit says. That is why DuckDB must be told the number explicitly — it reads neither cgroup CPU nor cgroup memory limits and otherwise plans against host RAM and host core count. `ER_DUCKDB_THREADS` and `ER_DUCKDB_MEMORY_LIMIT` MUST be applied with `SET threads` / `SET memory_limit` on every connection the process opens (S4.0b), and the benchmark marks a run NON_COMPARABLE when the *measured cgroup quota and limit* disagree with the scale's envelope — the rule, including why it reads `cpu.max` rather than `nproc`, is normative in S10.4.
- Every service image carries the literal `@sha256:` digest from the S2.1 pinned-versions table; a comment asserting that a digest ought to be there is not a pin. The object store image is unmaintained upstream; the pinned digest is deliberate and is tracked as a risk in S13.
- **Object-store readiness.** `objectstore` declares no healthcheck, because no probe that works exists inside that image: `mc` is not installed in the server image and the server exposes `/minio/health/live` but the image ships no HTTP client to call it. `objectstore-init` therefore retries `mc alias set` for up to 60 s from the `minio/mc` image — which does have `mc` — then runs `mc ready local` and creates the bucket. Everything downstream gates on `objectstore-init` with `service_completed_successfully`, so bucket existence, not a container liveness guess, is the readiness signal. A healthcheck that can never pass is worse than none: it hangs the whole stack behind `condition: service_healthy`.

<a id="s7-2"></a>
### 7.2 Lake attach sequence

`src/er/lake/ducklake.py` emits **the statement block in S4.0b, verbatim and in that order**, on every connection; the dbt `lake` target reaches the identical state through the dbt-duckdb `settings:` / `secrets:` / `attach:` profile keys that S4.0b specifies alongside the block, field for field. S4.0b is the single authority for both renderings; this section does not restate either, because two copies of an attach sequence drift and the sequence is what every service depends on.

- The DuckDB primary database is `:memory:`; the lake is only ever reached through the alias (S4.0b).
- `DATA_PATH` is written into the catalog on first attach and is **immutable** thereafter. A mismatch between `ER_LAKE_DATA_PATH` and the catalog value is a hard error (exit `3`) carrying the literal message given for `er init` in S4.0 (`lake DATA_PATH immutable: catalog=<a> env=<b>; …`), escapable only via `er lake reset`.
- Every scale/scenario namespace differs only in `METADATA_SCHEMA` and `DATA_PATH`; nothing else in the sequence varies.

<a id="s7-3"></a>
### 7.3 `docker/Dockerfile`

Multi-stage, single image, used by `pipeline`, `benchmark`, `catalog-init`, and later as the k8s job image.

```dockerfile
# ---- builder ----
FROM python:3.12-slim AS builder
COPY --from=ghcr.io/astral-sh/uv:0.11.3 /uv /bin/uv     # the S2.1 uv pin, by tag — never :latest
WORKDIR /app
COPY pyproject.toml uv.lock ./
RUN uv sync --frozen --no-install-project   # dependencies only — src/ is not in the context yet.
                                            #   NOT --no-dev: the runtime image runs `pytest`
COPY src/ src/
RUN uv sync --frozen                        # now install the project itself into /app/.venv
# Bake DuckDB extensions at BUILD time so no container ever downloads one at runtime.
ENV DUCKDB_EXTENSION_DIRECTORY=/opt/duckdb_extensions
RUN /app/.venv/bin/python -c "\
import duckdb, os; \
c = duckdb.connect(config={'extension_directory': os.environ['DUCKDB_EXTENSION_DIRECTORY']}); \
[c.execute(f'INSTALL {e}') for e in ('ducklake','postgres','httpfs')]"

# ---- runtime ----
FROM python:3.12-slim AS runtime
ENV PATH=/app/.venv/bin:$PATH \
    PYTHONDONTWRITEBYTECODE=1 \
    DBT_PROFILES_DIR=/app/dbt/profiles
COPY --from=builder /app/.venv /app/.venv
COPY --from=builder /opt/duckdb_extensions /opt/duckdb_extensions
WORKDIR /app
COPY src/ src/
COPY dbt/ dbt/
COPY configs/ configs/
COPY benchmarks/ benchmarks/
COPY fixtures/ fixtures/
COPY tests/ tests/
RUN mkdir -p /app/artifacts /app/.bench
ENTRYPOINT []
CMD ["er", "--help"]
```

- **The builder syncs twice on purpose.** `pyproject.toml` declares a `src/er` layout, so `uv sync --frozen` builds and installs the `er` project itself — which cannot work in a stage whose context holds only `pyproject.toml` and `uv.lock`. The first sync therefore passes `--no-install-project`: it resolves and installs the *dependencies*, which is the expensive layer and the one Docker should cache across source edits. `COPY src/` then invalidates only the second sync, which installs the project. `uv sync` installs the project in editable mode, which is why the runtime stage still copies `src/`.
- **The dev dependency group is installed on purpose.** The `pipeline` service's command is `pytest`, and `benchmarks/scales.py` is called from the S9.2 preflight, so `pytest`, `pytest-xdist` and `hypothesis` (all pinned in S2.1) MUST be present in the runtime image. `uv sync --frozen --no-dev` would produce an image that cannot run the integration suite it exists to run.
- Both base images (`python:3.12-slim`, `ghcr.io/astral-sh/uv:0.11.3`) are pinned by **tag**, matching the S2.1 Python and `uv` rows. The `@sha256:` digest requirement in S2.1 covers the three Compose *service* images, whose digests are literal in S7.1.
- `benchmarks/baselines/` and `benchmarks/scales.py` ship inside the image, so the in-image comparison in S9.2 needs no bind mount and no runner toolchain.
- `dbt` runs inside this image as a subprocess of the CLI, against `dbt/profiles/profiles.yml` (S9.1), which performs the S7.2 attach for its `lake` target.

<a id="s7-4"></a>
### 7.4 Invocation

The stack MUST be driven with `run`, not `up`:

```bash
docker compose -f docker/compose.yaml --profile test run --rm pipeline \
  pytest tests/integration -q --maxfail=3 \
  --junitxml=/app/artifacts/junit.xml --durations=20
docker compose -f docker/compose.yaml --profile test down -v --remove-orphans   # always, even on failure
```

`up --abort-on-container-exit --exit-code-from pipeline` MUST NOT be used: `objectstore-init` and `catalog-init` are one-shot containers, so the moment either exits successfully Compose tears the whole stack down and the reported exit code is a teardown signal rather than a test result. `run --rm <service>` starts dependencies, waits for their `depends_on` conditions, runs the command, and returns the command's own exit status.

`down -v --remove-orphans` runs before and after every CI invocation and is the documented local reset: it drops the catalog volume, the MinIO volume and `benchdata`, so a local run starts from the same empty state a fresh runner has. Because `artifacts/` is a bind mount it survives `down -v`; CI creates it with `mkdir -p artifacts` before the first `run`.

---

---

<a id="s8"></a>
## 8. Testing Strategy

<a id="s8-1"></a>
### 8.1 Layers

| Layer | Runs | Substrate | Scope |
|---|---|---|---|
| Static | every PR | GH runner, no services | `ruff check .`, `ruff format --check .`, `mypy --strict src/er`, `scripts/actionlint.py`, `scripts/lint_spec.py`, `scripts/lint_board.py`, `scripts/lint_metrics.py`, `report.py --validate-baselines` (from M5 only), `dbt deps`, `dbt parse` (no warehouse connection). **S9.1 is the authority for the exact step list**; this row summarises it |
| Unit | every PR | GH runner, no services | normalizer macros against a bare in-process DuckDB, `dbt compile --target mem`, reconciler as a pure function, `record_key`/pair-canonicalisation/`IdFactory`/`resolve()` logic, config validators, never-cut algorithm, `er.eval.pairwise_metrics` |
| Integration | every PR | Docker Compose (`run --rm pipeline`) | `er doctor`, then `er init` / `er run-all` against `fixtures/static/*`; every S8.3 scenario test |
| dbt data tests | every PR, inside the integration job | Compose | `dbt build` — `unique`, `not_null`, `accepted_values`, `dbt_utils.unique_combination_of_columns`, `relationships`, and the singular tests (`':'` in `source_record_id`, pair canonicality, one membership row per record) |
| Benchmark | `workflow_dispatch` + weekly cron | Compose `bench` profile | throughput and quality at scale; never on the PR path |

The Static layer MUST NOT invoke `dbt compile`: `compile` opens a warehouse connection, and no catalog or object store exists on a bare runner. `dbt parse` is the strongest check available service-lessly and MUST render `profiles.yml` through `env_var()` calls that all carry defaults. `dbt compile --target mem` (primary database `:memory:`, no `ATTACH`) belongs to the Unit layer; `dbt build` against the `lake` target belongs to Integration.

The static job also runs `scripts/lint_spec.py` against this file. S9.1 is the authority for everything that lint enforces — this specification is self-contained, and the forbidden patterns are declared in the linter, never in this text. There is exactly one mechanism, invoked in exactly one place.

**Test isolation contract.**

A session-scoped `pytest` fixture in `tests/conftest.py` implements the following, and every integration test MUST obtain its lake handle from it:

1. Derive `ns` = lowercased Crockford ULID minted at session start, suffixed with `PYTEST_XDIST_WORKER` when that variable is set (`ns = f"{ulid}_{worker}"`).
2. Export `ER_LAKE_METADATA_SCHEMA=er_test_<ns>`, `ER_LAKE_DATA_PATH=s3://lake/test/<ns>/`, `ER_LAKE_ALIAS=lake`, leaving `ER_CATALOG_DSN` and the `ER_S3_*` variables as Compose supplies them.
3. Run `er init` against that namespace. `er init` creates only `ddl.py`-owned relations; the dbt-owned relations are created by the first `dbt run` in the session.
4. On teardown: `CALL lake.expire_snapshots(older_than => now())`, `CALL lake.cleanup_old_files(cleanup_all => true)`, delete the `s3://lake/test/<ns>/` prefix, `DETACH lake`, and `DROP SCHEMA er_test_<ns> CASCADE` in the catalog. Teardown MUST run under `try/finally` so a failing test still reclaims the namespace.

Individual tests are function-isolated by a function-scoped fixture that `DELETE`s from every `ddl.py`-owned relation (`raw_records`, `match_scores`, `entity_membership`, `entities`, `entity_events`, `assertions`, `review_queue`, `model_registry`, `tf_lookup`, `cut_edges`, `runs`, `run_stages`, `ingest_batches`, `er_touched_entities`), drops the dbt-owned relations, and reloads the scenario fixture. A test that needs a second, independent universe inside one session (T-INC-1) requests the `sub_namespace` fixture, which repeats steps 1–4 under `er_test_<ns>_a` / `er_test_<ns>_b`.

Integration tests run single-process. `-n auto` applies to the unit layer only. v1 is a single-writer batch model: concurrent writers against one namespace are an explicit non-guarantee and are not tested, except by T-CONC-1, which asserts that the second writer is *refused* (exit code 3) rather than admitted.

Blanket rule: every snapshot-dependent test MUST capture its reference snapshot id at runtime from `run_stages.snapshot_start` / `run_stages.snapshot_end` for a named `(run_id, stage)`, and no test may reference an absolute snapshot version. Snapshot *counts* are not assertable at all, because a stage commits a range rather than a count — that rule is normative in the S4 preamble.

<a id="s8-2"></a>
### 8.2 Static fixtures (`fixtures/static/`)

Hand-authored, small, committed, with ground truth. Every scenario carries `persona_id` on every input row, which is the truth label for S8.5's metrics and never reaches the pipeline (the loader strips it before `er ingest`).

| Scenario | Contents | Guarantee exercised |
|---|---|---|
| `base_10` | 10 personas, 23 records, 3 sources | matching quality, standardization, survivorship, blocking parity |
| `incremental_batch` | 6 new records: 3 join existing entities, 2 form a new entity, 1 bridges two existing entities (forcing a merge) | G3, two-pass incremental scoring (the "2 form a new entity" case is reachable only via the new-vs-new pass) |
| `merge_scenario` | batch causing a 2-entity merge | survivor selection, redirect, `merged` event |
| `split_scenario` | a `never` assertion on a **bridge** edge severing a previously-merged entity; the asserted pair MUST be a bridge, so that cutting it disconnects the component | fragment ordering, `split` event |
| `assertions_scenario` | an `always` pair below `review_low`, a `never` pair above `auto_merge`, and a `never` pair whose endpoints remain connected through a third record | partition-level `never` semantics, `edge_cut`, `cut_edges` |
| `supersession_scenario` | one record re-delivered with corrected values (new `content_hash`) that moves it out of its entity | append-only `raw_records`, greatest-`ingested_at` current-row rule, edge invalidation |
| `deletion_scenario` | three phases: `base/`, then a `refresh/` `--full-refresh-keys` delivery omitting two keys (one of which is a bridge), then a `resurrect/` delivery re-appearing one of the two omitted keys | tombstones, retraction path, `member_removed` / `split` / `retired`, resurrection |

**`base_10` input headers.** Three CSV files under `base/`, one per source. Every field name below is the literal right-hand side of the corresponding `sources.<name>.columns` entry in S6 — **S6 owns the mapping and these headers are derived from it**, so a header edit without a matching config edit fails V11 and makes every scenario test unrunnable. The first column of each is the source's `record_id_column`; the second-to-last is its `updated_at_column`; `persona_id` is the truth column and is the last field of every row:

```
crm.csv
crm_id,first_name,last_name,email_address,phone,street_address,city,state,zip,dob,last_modified,persona_id

billing.csv
account_no,fname,lname,billing_email,contact_phone,addr1,addr_city,addr_state,postal_code,birth_date,updated_ts,persona_id

webforms.csv
submission_id,name_first,name_last,email,phone_number,address,city,region,postcode,date_of_birth,submitted_at,persona_id
```

Each source carries **exactly one address column** — the `address_line` of V11 — which `address_parse` componentizes into the six `addr_*` values. There is no second address column in the canonical set, so none is committed in the fixture; a unit-2 style value is authored inline in the single address line where a persona needs one.

`sources.crm.date_format` and `sources.webforms.date_format` are `%Y-%m-%d`; `sources.billing.date_format` is `%m/%d/%Y` (S6 is authoritative for all three). The two distinct formats exist so the `stg_*` date parsing is exercised rather than assumed.

**`base_10` designed traps.** Each is a named sub-assertion of T-MATCH-1a or T-MATCH-1b:

| Trap | Construction | Required outcome |
|---|---|---|
| Nickname pair | `crm` "Robert Chen", `webforms` "Bob Chen", same phone | merge, via `variant_match` on `name_variants` |
| Typo surname | `billing` "Krystal Nowakowski", `crm` "Krystal Nowakowsky", same DOB and postal | merge, via `jaro_winkler:0.9` on `family_name` |
| Shared household | two distinct personas at the same `address_line` (hence the same six `addr_*` values), different given/family names, different DOBs, no shared email or phone | MUST NOT merge — two entities, zero cross edges above `auto_merge` |
| Missing emails | 4 records with an empty `email` field | `NullLevel` fires; no record pairs on a null key; `int_blocking_keys` emits no `email_exact` row for them |
| Drifted phones | one persona's phone appears as `(415) 555-0132`, `415-555-0132`, `+14155550132` | all three normalize to `+14155550132` and block together |
| Placeholder email | `test@test.com` on two records of two different personas | nulled by `email_norm` from `standardization.email_placeholders` (S4.2 — `null_semantics` handles only the sentinel vocabulary and never sees this value); forms no blocking key and no component |
| Survivorship tie | one persona has two records from the **same** source, same `priority_rank`, identical `COALESCE(updated_at_source, ingested_at)`, both non-null and equal-length `given_name` values that differ | the mandatory terminal `record_key ASC` decides; `golden_lineage.rule = 'tiebreak_deterministic'` |
| Gray-band pair | exactly one pair scores `review_low <= p < auto_merge`, and it is a **cross-persona** pair: two records of two *different* personas that block together under `name_postal` (same `addr_postal`, same first four characters of `family_name`) with similar given names but different `birth_date`, no shared email and no shared phone | one `review_queue` row with `subject_type='pair'`, `reason='gray_band'`, `status='open'`; the pair is NOT clustered |

**Why the gray-band pair MUST be cross-persona (authoring constraint, normative).** T-MATCH-1b asserts cluster-level precision `== 1.0` **and** `entity count == 10` on this fixture, while T-REVIEW-1 asserts the gray-band pair is not co-clustered. A *same*-persona gray-band pair with no third record bridging its endpoints would leave that persona split across two entities: the entity count becomes 11 and T-MATCH-1b fails, with precision still 1.0 and nothing pointing at the cause. A cross-persona pair satisfies all three tests simultaneously, which is why the construction above is pinned here rather than left to the fixture author. The shared-household pair is a different trap and cannot serve as this one: its two records share no email, no phone, no family-name prefix and no `birth_date`, so no blocking rule pairs them at all and they have no score to fall in the band.

**Why a tolerated missed edge MUST lie inside a persona of three or more records (authoring constraint, normative).** T-MATCH-1a tolerates **at most one** true pair missed at `auto_merge`, while T-MATCH-1b asserts `entity count == 10` **and** cluster-level precision `== 1.0` on the same fixture. Those are jointly satisfiable only when the missed edge is *transitively recoverable*: inside a persona of three or more records the persona's remaining above-`auto_merge` edges still connect every member, so it is still exactly one entity and the pair is still co-clustered by the transitive closure. Inside a 2-record persona the missed edge is the persona's **only** edge — the persona splits into two entities, the entity count becomes 11, and T-MATCH-1b fails with precision still 1.0 and nothing pointing at the cause, which is the same failure shape the gray-band constraint above exists to prevent. `base_10` MUST therefore be authored so that every true pair scoring below `auto_merge` lies inside one of the three personas holding 4, 3 and 3 records, and so that deleting it leaves that persona's remaining above-`auto_merge` edges connected; no true pair of a 2-record persona and no true pair whose removal disconnects its persona may fall below `auto_merge`. Zero missed edges also satisfies both tests and is the preferred authoring, the tolerance existing only so a `jaro_winkler` boundary case is not a fixture rewrite.

**Ground truth.** `base_10`'s persona record counts are `4, 3, 3, 2, 2, 2, 2, 2, 2, 1` (23 records over 10 personas), giving exactly **18 true pairs** over a universe of `C(23,2) = 253`. This count is machine-checked: `tests/integration/test_fixture_integrity.py::test_base_10_truth_counts` recomputes it from the committed CSVs and asserts `records == 23`, `personas == 10`, `true_pairs == 18`. A fixture edit that changes the truth set therefore fails CI unless the count is updated deliberately.

<a id="s8-2-1"></a>
### 8.2.1 Fixture & expected-output format

Every scenario is one directory with the same shape. There is no top-level `fixtures/expected/`; `expected/` lives inside each scenario.

```
fixtures/static/<scenario>/
├── base/                     # first delivery: crm.csv, billing.csv, webforms.csv
├── batch/                    # incremental delivery (omit when the scenario has none)
├── refresh/                  # --full-refresh-keys delivery (deletion_scenario only)
├── resurrect/                # ordinary delivery re-appearing a tombstoned key (deletion_scenario only)
├── assertions.csv            # input assertions, applied before the phase named in its `phase` column
├── parity_pairs.csv          # optional: the pairs T-INC-3 scores through both code paths
├── tf_flip_pairs.csv         # optional: the edges T-INC-1b / T-TF-1 allow to cross auto_merge
└── expected/
    ├── base/                 # expected state after the base phase
    │   ├── membership.csv
    │   ├── golden.csv
    │   ├── events.csv
    │   ├── std_hashes.csv
    │   └── assertions.csv    # only where the scenario asserts on assertion state
    ├── batch/                # same five files, expected state after the batch phase
    ├── refresh/              # same five files, expected state after the refresh phase
    └── resurrect/            # same five files, expected state after the resurrect phase
```

**Phase vocabulary (normative).** A phase is one delivery plus the pipeline run over it, and the vocabulary is exactly `{base, batch, refresh, resurrect}`. `base` is always present and always first. `batch` is an ordinary incremental delivery; `refresh` is a `--full-refresh-keys` delivery; `resurrect` is an ordinary delivery whose purpose is to re-appear a key a preceding `refresh` tombstoned, and it exists because a scenario needs a *second* post-base delivery that is not itself a full refresh — reusing `refresh` for it would tombstone every key the resurrection delivery omits, which is the opposite of what T-DEL-1 asserts. Phases run in the order `base → batch → refresh → resurrect` over the phases the scenario has. The `phase` column of `assertions.csv` and each `expected/<phase>/` directory name are drawn from this same vocabulary and from no other.

A phase directory that does not exist means the scenario has no such phase; an `expected/<phase>/` file that does not exist means that phase makes no claim about that relation.

The three scenario-root files are inputs and bounds, not expectations, which is why they sit beside `base/` rather than under `expected/`. `assertions.csv` exists wherever a scenario asserts (`split_scenario`, `assertions_scenario`); `parity_pairs.csv` and `tf_flip_pairs.csv` exist only in the scenarios whose tests name them — `base_10` for T-TF-1, `incremental_batch` for T-INC-3 and T-INC-1b. `parity_pairs.csv` lives in `incremental_batch` and not in `base_10` because T-INC-3 exercises the incremental two-pass path, which has no input in a scenario with no `batch/` phase.

**`parity_pairs.csv` is DERIVED, not invented (normative).** Its contents are the canonicalised pair set

```
parity_pairs = { (a, b) : (a, b) is scored by the incremental two-pass path over `batch/` (S4.3.4)
                        ∧ (a, b) is scored by the corpus-wide `full.py` pass over base/ ∪ batch/ }
```

which — because `find_matches_to_new_records` plus a batch-only `dedupe_only` Linker can only produce pairs with **at least one endpoint in `batch/`**, and because both paths regenerate their own candidates from the same blocking rules at the same threshold — is exactly the set of pairs with at least one endpoint in `batch/` that clear `review_low`. Its cardinality is therefore a property of the fixture and its blocking rules, and **no test asserts a fixed pair count**. The file is committed so that a silent shrink of that set (a blocking regression, or a batch that stops linking) is visible as a diff rather than as a still-green parity test; T-INC-3 recomputes the set and asserts the file equals it and is non-empty before comparing probabilities.

**Headers (literal).**

```
assertions.csv                     # scenario root, an INPUT
phase,rec_a_key,rec_b_key,kind,created_by,note

parity_pairs.csv                   # scenario root, an INPUT
rec_a_key,rec_b_key

tf_flip_pairs.csv                  # scenario root, a BOUND
rec_a_key,rec_b_key,direction      # direction ∈ {up, down}: the side of auto_merge the edge may move to

expected/<phase>/membership.csv
persona_id,source_system,source_record_id,entity_label

expected/<phase>/golden.csv
entity_label,given_name,family_name,email,phone_e164,addr_number,addr_street,addr_unit,addr_city,addr_region,addr_postal,birth_date,survivorship_version

expected/<phase>/events.csv
entity_label,event_type,count

expected/<phase>/std_hashes.csv
source_system,source_record_id,std_hash

expected/<phase>/assertions.csv
rec_a_key,rec_b_key,kind,active
```

`entity_label` is a **symbolic** name drawn from `E1, E2, … En`, allocated in ascending order of the minimum `record_key` in the expected group. It MUST NEVER be a ULID. The helper resolves labels to actual `entity_id`s by that same rule at compare time, which is what makes the expected files stable across runs and across the two universes T-INC-1 builds.

`golden.csv` carries every `golden_records` column except `entity_id` (replaced by `entity_label`) and `assembled_at` (a `VOLATILE_COLUMNS` member). `std_hash` is the SHA-256 defined by T-STD-1 over the stable column list of `int_std_records`.

**Encoding rules, normative for both authoring and comparison:**

- **Null token:** the two-character sequence `\N`. An empty field is the empty string, which is a distinct value. Nulls are never written as an empty field.
- **Float tolerance:** `1e-9`, absolute. Applied to `match_probability` and to any numeric golden column. All other columns compare exactly, after both sides are read as `VARCHAR`.
- **Sort key:** every expected file is stored sorted ascending, byte-wise on the UTF-8 encoding of the full column tuple in header order. Comparison helpers re-sort both sides before comparing, so a mis-sorted committed file is a lint failure (`tests/unit/test_fixture_lint.py`), never a scenario-test failure.
- **Excluded columns:** the `VOLATILE_COLUMNS` set of S5.0 — imported, never re-listed here — never appears in an expected file and is dropped from both sides before any comparison.

**`tests/helpers/compare.py`** exposes exactly three functions and no others:

```python
def assert_partition_equal(actual: Iterable[tuple[str, str]],
                           expected_csv: Path) -> None:
    """ID-INSENSITIVE. Groups record_keys by entity into frozensets and compares
    the set of frozensets. Used by T-INC-1."""

def assert_ids_stable(actual: Iterable[tuple[str, str]],
                      expected_csv: Path,
                      label_map: dict[str, str]) -> None:
    """ID-IDENTICAL. Resolves entity_label -> entity_id through a label_map
    captured earlier in the same test and asserts the exact entity_id per
    record. Used by T-PERM-1, T-PERM-2, T-PERM-3, T-MODEL-1."""

def assert_golden_equal(actual: pd.DataFrame,
                        expected_csv: Path,
                        label_map: dict[str, str],
                        float_tol: float = 1e-9) -> None:
    """Value comparison of golden_records after label resolution and volatile-
    column removal. Used by T-GOLD-1, T-INC-1, T-SNAP-1."""
```

Two comparison semantics are required because the two guarantees are different: G3 compares two independently built universes whose minted ULIDs legitimately differ, so only the induced set-partition may be asserted, while G2 asserts that identifiers *survive*, so the identifier itself is the assertion and an ID-insensitive comparison would pass vacuously against a pipeline that re-mints every entity on every run.

<a id="s8-3"></a>
### 8.3 Scenario tests (integration)

Every row is a required test. `pytest node id` is relative to the repository root.

| id | guarantee | file path | pytest node id | asserts |
|---|---|---|---|---|
| T-DOCTOR-1 | environment matches S2.1 | `tests/integration/test_doctor.py` | `tests/integration/test_doctor.py::test_doctor_passes` | `er doctor` exits 0 having asserted **(a) every S2.1 row whose *Asserted by* cell names `er doctor`** — the interpreter version, `splink.__version__`, `duckdb.__version__`, dbt-core, dbt-duckdb, the three extension commit hashes (the `postgres` extension under its registered name `postgres_scanner`), `uv`, `ruff`, `mypy`, `pytest`, `actionlint`, `typer`/`pydantic`/`python-ulid`, and the catalog and object-store images — **and (b) these six runtime assertions**: `SELECT * FROM lake.snapshots()` succeeds; `duckdb_extensions()` reports all three extensions loaded; the dbt target database equals `$ER_LAKE_ALIAS`; a write/read round-trip to `DATA_PATH` succeeds; a catalog round-trip returns `server_version`; zero relations matching `__splink__%` exist in `lake`. A mismatch in either group exits `1`. Runs as the first step of the integration job |
| T-KEY-1a | logical keys on `ddl.py`-owned relations are enforced by tests, not by DuckLake | `tests/integration/test_keys.py` | `tests/integration/test_keys.py::test_ddl_owned_duplicate_key_fails_dbt_test` | insert a duplicate `(source_system, source_record_id, content_hash)` into `raw_records`; `dbt test --select tag:keys` exits non-zero and names `raw_records`. Also asserts that `CREATE TABLE … PRIMARY KEY` against the lake raises, documenting B2 in executable form. **Touches only `ddl.py`-owned relations, so it is runnable the moment `er init` exists** |
| T-KEY-1b | the same, for dbt-owned relations | `tests/integration/test_keys.py` | `tests/integration/test_keys.py::test_dbt_owned_duplicate_key_fails_dbt_test` | insert a duplicate `(source_system, source_record_id)` into `int_std_records`; `dbt test --select tag:keys` exits non-zero and names `int_std_records`. Requires the dbt-owned relations to exist, which is why it is a separate arm from T-KEY-1a |
| T-STD-1 | standardization determinism | `tests/integration/test_standardize.py` | `tests/integration/test_standardize.py::test_std_content_hash_stable` | run `er standardize` twice; for every row, `std_hash` = SHA-256 over the UTF-8 concatenation of `record_key, std_version, given_name, family_name, array_to_string(name_variants,'\x1f'), email, email_valid, phone_e164, phone_valid, addr_number, addr_street, addr_unit, addr_city, addr_region, addr_postal, birth_date, updated_at_source, content_hash` joined by `\x1f` with NULL as the empty string, is unchanged. Byte identity of the Parquet files is NOT asserted — Parquet output is not reproducible and `int_std_records` carries `VOLATILE_COLUMNS` |
| T-BLK-1 | blocking parity | `tests/integration/test_blocking.py` | `tests/integration/test_blocking.py::test_dbt_and_splink_pair_sets_match` | on `base_10`, the DISTINCT canonicalised (`rec_a_key < rec_b_key`) pair set derived from `int_blocking_keys` equals Splink's blocked pair set exactly — set equality in both directions, with the symmetric difference printed on failure. Both sides come from `blocking_rules_from_config(cfg)` |
| T-MATCH-SYM | scoring symmetry | `tests/integration/test_matching.py` | `tests/integration/test_matching.py::test_score_is_orientation_invariant` | for all `base_10` blocked pairs, `compare_two_records(a,b).match_probability == compare_two_records(b,a).match_probability` within `1e-12`; guards the `variant_match` symmetry requirement (normalized `given_name` is element 0 of its own `name_variants`) |
| T-MATCH-1a | edge-level quality (G1) | `tests/integration/test_matching.py` | `tests/integration/test_matching.py::test_edge_quality_base_10` | absolute counts against the 18 true pairs: blocking recall == 1.0 (all 18 true pairs are in the candidate set); at `auto_merge`, false-positive pairs == 0 and missed true pairs <= 1 — and a missed pair, if there is one, MUST satisfy the S8.2 authoring constraint that it lies inside a persona of three or more records, which the test asserts on the missed pair itself, because that is what keeps T-MATCH-1b's `entity count == 10` true alongside this tolerance. Named sub-assertions: the robert/bob pair is present; the typo-surname pair is present; the shared-household pair is absent; the two `test@test.com` records produce no edge |
| T-MATCH-1b | cluster-level quality (G1) | `tests/integration/test_matching.py` | `tests/integration/test_matching.py::test_cluster_quality_base_10` | over the transitive closure of `entity_membership`: cluster-level precision == 1.0, cluster-level recall >= 0.94 (>= 17 of 18 true pairs co-clustered); entity count == 10; the two household personas occupy two distinct entities; the singleton persona is its own entity |
| T-INC-3 | scoring parity across code paths | `tests/integration/test_incremental.py` | `tests/integration/test_incremental.py::test_incremental_and_full_scores_are_bit_equal` | on `incremental_batch` — **the scenario is named here because the incremental path needs a `batch/` phase and `base_10` has none**: load `base/`, then score `batch/` through the incremental two-pass path (S4.3.4) and score `base/ ∪ batch/` through `full.py`, both at the same pinned `model_version` and `tf_snapshot_id`. Recompute the parity pair set per its normative derivation in S8.2.1, assert it is non-empty and equals `fixtures/static/incremental_batch/parity_pairs.csv`, then assert bit-equal `match_probability` for every pair in it. No pair *count* is asserted — the cardinality follows from the blocking rules and the fixture. A T-INC-1 failure with T-INC-3 green localises the fault to clustering, not scoring |
| T-INC-1 | incremental == full (G3) | `tests/integration/test_incremental.py` | `tests/integration/test_incremental.py::test_incremental_equals_full` | two isolated sub-namespaces from the `sub_namespace` fixture: universe A runs `er run-all --mode full` on `base/` ∪ `batch/` at once; universe B runs `base/` then `batch/` incrementally. Both arms pin the same `model_version`, the same `tf_snapshot_id`, the same `config_hash` and `std_version`, and the same active assertion set — the INV-EQ preconditions, asserted explicitly before the comparison. Each universe is then compared **against the committed expectation**, which is what the S8.2.1 signatures accept: `assert_partition_equal(A, expected/batch/membership.csv)` and `assert_partition_equal(B, expected/batch/membership.csv)`, then `assert_golden_equal(A, expected/batch/golden.csv, label_map_A)` and `assert_golden_equal(B, expected/batch/golden.csv, label_map_B)`. Equality of both universes to one ID-insensitive expectation is equality to each other, so no fourth helper and no two-universe overload is needed. Entity IDs may differ between universes and are not compared |
| T-INC-1b | INV-EQ is a real precondition set, not decoration | `tests/integration/test_incremental.py` | `tests/integration/test_incremental.py::test_inc_eq_violation_diverges_then_repairs` | violate exactly one INV-EQ precondition (re-score universe B under a second `tf_snapshot_id`), assert the partitions diverge in the documented direction and by the documented magnitude (at most the pairs listed in `fixtures/static/incremental_batch/tf_flip_pairs.csv`), then run `er correct` and assert `assert_partition_equal` against `expected/batch/membership.csv` now holds again |
| T-INC-2 | touched-only assembly (G3) | `tests/integration/test_incremental.py` | `tests/integration/test_incremental.py::test_rewritten_plus_reaped_equals_touched` | after an incremental run, `{entity_id : assembled_at == run.started_at} ∪ {entity_id deleted from golden_records}` == `er_touched_entities` for that `run_id`; every untouched entity's `assembled_at` is byte-unchanged; zero golden rows exist for any `disposition='retire'` entity |
| T-PERM-1 | permanence under merge (G2) | `tests/integration/test_permanence.py` | `tests/integration/test_permanence.py::test_merge_preserves_survivor_id` | `merge_scenario`: survivor selected by the overlap-matrix claimant rule; `ids.resolve(loser)` returns the survivor; exactly one `merged` event for the `(run_id, entity_id)`; `assert_ids_stable` on the survivor's members; zero `entity_membership` rows reference the loser `entity_id` and `entities.status='merged'` on it. The reap of the loser's *golden* row is asserted by T-INC-2, which owns the golden relations |
| T-PERM-2 | permanence under split (G2) | `tests/integration/test_permanence.py` | `tests/integration/test_permanence.py::test_split_retains_id_for_rank_1_fragment` | `split_scenario`: fragments ranked `(member_count DESC, min record_key ASC)`; rank 1 retains the `entity_id` via `assert_ids_stable`; the minority fragment gets a newly minted ULID; one `split` event; a 2–2 split resolves by the `min record_key` element |
| T-PERM-3 | INV-PERM under full re-resolution (G2) | `tests/integration/test_permanence.py` | `tests/integration/test_permanence.py::test_full_reresolution_satisfies_inv_perm` | after incremental history, run `er match --mode full && er reconcile` at the **same** `model_version` and `tf_snapshot_id`; assert INV-PERM literally: for every group of `P_new` set-equal to a group of `P_old`, every member retains its `entity_id`, no ULID is minted, and no event is emitted; and every entity whose partition changed has a `merged` or `split` event in this `run_id` |
| T-ASSERT-1 | partition-level `never` | `tests/integration/test_assertions.py` | `tests/integration/test_assertions.py::test_never_pairs_never_co_cluster` | `assertions_scenario`, in both incremental and full mode: no active `never` pair shares an `entity_id`. The scenario includes a `never` pair whose endpoints are connected only through a third record — asserting the S4.5 cut fires, `cut_edges` holds the minimum-probability edge on the shortest path chosen by `(match_probability ASC, rec_a_key ASC, rec_b_key ASC)`, an `edge_cut` event is emitted, and a second full re-run does not undo the cut (the S4.4.2 exclusion rule, exercised end to end) |
| T-ASSERT-2 | contradiction is a hard failure | `tests/integration/test_assertions.py` | `tests/integration/test_assertions.py::test_contradiction_1_fails_the_run` | assert `always(a,b)`, `always(b,c)`, `never(a,c)`; `er reconcile` exits 1 naming CONTRADICTION-1 and the three `assertion_id`s; no snapshot is committed, no event is emitted, `entity_membership` is unchanged |
| T-TRAIN-1 | training is reproducible | `tests/integration/test_training.py` | `tests/integration/test_training.py::test_fixture_model_regenerates_byte_for_byte` | regenerate `fixtures/static/model_test_v1.json` and assert byte equality with the committed file. **Byte equality is a claim about a fully pinned input, so the corpus is named in this row rather than left to the test author:** the training corpus is the S10.1 generator's output at the **`10k`** scale (S10.2), generated under `configs/test.yaml`'s `generator.seed` — the corpus S12 pins for this artifact. Six things MUST be identical for the bytes to match, and the test asserts each of them before comparing so a failure names which one diverged: (1) the **corpus** — the same generator code emitting the same per-source CSVs; (2) the **seed** — `generator.seed` from `configs/test.yaml`; (3) the **scale** — the `10k` row of S10.2, since a different record count moves every fitted m/u value; (4) the **`model_version`** the committed artifact was allocated, pinned by the test rather than left to the `max+1` allocation of S4.3.2; (5) the **`training:` block** of `configs/test.yaml` verbatim, `u_seed`, `u_max_pairs`, `recall`, `deterministic_rules` and `em_blocking_rules` included; (6) the **Splink version** — `splink.__version__` equal to the S2.1 pin. Also assert the persisted `training:` block in `model_registry.metrics` matches the config verbatim |
| T-MODEL-1 | retrain preserves entities | `tests/integration/test_training.py` | `tests/integration/test_training.py::test_retrain_full_rescore_preserves_ids` | train v2, `er match --mode full`, `er reconcile`; `assert_ids_stable` for every partition unchanged between v1 and v2; assert that the S4.3.2 activation guard fires — `er reconcile` exits 3 on a mixed-`model_version` edge set |
| T-CFG-1 | config drift is caught, not absorbed | `tests/integration/test_cli_contract.py` | `tests/integration/test_cli_contract.py::test_incremental_refuses_on_config_drift` | mutate `thresholds.auto_merge`, run `er run-all --mode incremental`: exit 3, named message, no snapshot; re-run with `--allow-escalate`: promotes to full mode and succeeds |
| T-DEL-1a | tombstone derivation, ingest + standardize only | `tests/integration/test_deletion.py` | `tests/integration/test_deletion.py::test_full_refresh_keys_derives_tombstones` | `deletion_scenario`, phases `base → refresh → resurrect`: `er ingest --full-refresh-keys` over `refresh/` writes tombstone rows with the sentinel `content_hash`, `is_deleted=true` and `deleted_at` set for exactly the two omitted keys; `ingest_batches.tombstone_count == 2`; `int_std_records` excludes both after `er standardize`; an empty `--full-refresh-keys` delivery is refused (exit `2`) by the S4.1.1 guard; the `resurrect/` delivery appends an ordinary content version for the re-appearing key, `resurrected_count == 1`, and that key is present in `int_std_records` again. **Reads no matching or entity relation**, so it is runnable as soon as ingest and standardize exist |
| T-DEL-1 | deletion path end to end (G1) | `tests/integration/test_deletion.py` | `tests/integration/test_deletion.py::test_deletion_retracts_edges_and_resurrection_restores_membership` | `deletion_scenario`, same three phases, run through the full chain: incident `match_scores` edges of each tombstoned record are invalidated (`is_active=false`, `invalidated_run_id` set); `member_removed` is emitted for each departed record, `split` for the fragment the bridge removal disconnects, and `retired` for the entity that empties; after the `resurrect/` phase the resurrected record is scored, re-clustered and holds an `entity_membership` row again, and `expected/resurrect/membership.csv` matches |
| T-SUPER-1 | supersession: a changed `content_hash` invalidates edges and re-clusters (G1) | `tests/integration/test_supersession.py` | `tests/integration/test_supersession.py::test_superseded_record_invalidates_edges_and_leaves_its_entity` | `supersession_scenario`, phases `base → batch`, run through the full chain. The `batch/` delivery re-delivers one already-ingested key with corrected values: `er ingest` reports `new_count = 0` and `changed_count = 1` and appends one `raw_records` version row rather than overwriting one; after `er standardize`, `int_std_records` holds exactly one current row for that key — the greatest-`ingested_at` version (S4.2) — carrying the new `content_hash`. Then the S4.5.5 supersession arm: every `match_scores` row incident to that record and scored under its **prior** endpoint `content_hash` is invalidated in place (`is_active=false`, `invalidated_at` and `invalidated_run_id` set, and still exactly one row for its `(rec_a_key, rec_b_key, model_version, tf_snapshot_id)` key, per S4.3.4); the record's current entity enters the affected set and widens to that entity's **full membership even though none of its other members appears in this batch** (S4.5.1); re-clustering emits `member_removed` on the old entity for the departed record and the record lands in its own entity; `expected/batch/membership.csv` and `expected/batch/events.csv` match. This is the only deterministic construction in the suite that exercises the supersession arm — the deletion arm is T-DEL-1's — which is why the fixture has an owning test rather than being committed unread |
| T-CORR-1 | the correction pass finds what incremental cannot | `tests/integration/test_correction.py` | `tests/integration/test_correction.py::test_correction_pass_links_two_pre_existing_records` | build a corpus in which two **pre-existing** records become a true pair only after a third record shifts nothing about them (incremental candidate generation can never pair two pre-existing records); assert the incremental run misses the link, that `er correct` (S4.0) finds it under a new `tf_snapshot_id`, and that its events carry `details.reason='correction_pass'` |
| T-REVIEW-1 | gray band is captured, not clustered | `tests/integration/test_review.py` | `tests/integration/test_review.py::test_gray_band_pair_lands_open` | `base_10`'s single gray-band pair — cross-persona by construction (S8.2) — produces exactly one `review_queue` row with `subject_type='pair'`, `reason='gray_band'`, `status='open'`, a populated `waterfall` (all `gamma_*` columns and per-comparison Bayes factors), and `first_seen_run_id == last_seen_run_id`; the pair is NOT co-clustered; a second run refreshes `last_seen_run_id` and inserts no duplicate |
| T-REVIEW-2 | resolution closes the loop | `tests/integration/test_review.py` | `tests/integration/test_review.py::test_resolution_triggers_merge_without_new_records` | `er review resolve --review-id <review_id> --as match --by tester` (the S4.0 signature) writes the `always` assertion row in the same transaction; `er reconcile` with **zero** new records merges the pair — which also proves the assertion-delta arm of the affected-set computation |
| T-GOLD-1 | survivorship (G1) | `tests/integration/test_golden.py` | `tests/integration/test_golden.py::test_survivorship_values_and_rules` | `assert_golden_equal` on `expected/base/golden.csv`; and for every `(entity_label, attribute)` the `golden_lineage.rule` matches expectation — covering `source_priority`, `recency`, `frequency`, `completeness`, `validated`, and `tiebreak_deterministic` on the designed tie; the six `addr_*` columns all come from one contributing record |
| T-SNAP-1 | time travel | `tests/integration/test_snapshots.py` | `tests/integration/test_snapshots.py::test_time_travel_to_pre_incremental_golden` | read `run_stages.snapshot_end` for the pre-incremental `assemble` stage at runtime, then `SELECT * FROM lake.main.golden_records AT (VERSION => :snap)` and `assert_golden_equal` against `expected/base/golden.csv`. No absolute version is written in the test |
| T-IDEM-1a | idempotence of ingest + standardize | `tests/integration/test_idempotence.py` | `tests/integration/test_idempotence.py::test_reingesting_a_delivery_appends_no_rows` | re-run `er ingest` and `er standardize` on an already-processed delivery: the second `ingest_batches` row reports `new_count = 0 AND changed_count = 0 AND tombstone_count = 0`; zero rows appended to `raw_records`; the `int_std_records` row count is unchanged and the T-STD-1 `std_hash` set is byte-identical; `er ingest` exits `10`. **Reads no matching, entity or golden relation** |
| T-IDEM-1 | idempotence of the whole chain | `tests/integration/test_idempotence.py` | `tests/integration/test_idempotence.py::test_reprocessing_a_batch_changes_no_rows` | re-run `er run-all` on an already-processed delivery: the second `ingest_batches` row reports `new_count = 0 AND changed_count = 0 AND tombstone_count = 0` — the relation's column names, the same three T-IDEM-1a asserts, not the shorter stdout labels of the S4.0 manifest line; zero new `entity_events` rows; `entity_membership`, `golden_records` and `golden_lineage` row counts unchanged; the T-STD-1 `std_hash` set unchanged; every `golden_records.assembled_at` unchanged. Snapshot counts are NOT asserted — a no-op run may legitimately commit empty snapshots |
| T-TF-1 | TF drift is bounded and visible | `tests/integration/test_tf.py` | `tests/integration/test_tf.py::test_tf_refresh_flips_bounded_edge_count` | rebuild TF under a new `tf_snapshot_id` and assert the number of edges crossing `auto_merge` in either direction is at most the committed bound in `fixtures/static/base_10/tf_flip_pairs.csv`; assert INV-SCORE holds within each `tf_snapshot_id` |
| T-CONC-1 | single-writer enforcement | `tests/integration/test_cli_contract.py` | `tests/integration/test_cli_contract.py::test_second_concurrent_run_exits_3` | start `er run-all`, then start a second `er run-all` against the same `tenant`; the second exits 3 without writing a `runs` row |
| T-INV-1 | `entity_membership` means what the spec says | `tests/integration/conftest.py` | autouse finalizer, reported as `tests/integration/test_invariants.py::test_membership_equals_connected_components` | runs after **every** integration scenario, as an autouse session-and-function finalizer: recompute connected components over the current edge set (assertion-adjusted, `>= auto_merge`, at the run's `model_version`, minus `cut_edges`) and assert it equals the partition induced by `entity_membership`; assert every `entity_membership.entity_id` has `entities.status='active'`; assert exactly one membership row per `(source_system, source_record_id)`; assert every `match_scores`/`assertions`/`review_queue` pair satisfies `rec_a_key < rec_b_key`; assert zero `__splink__%` relations in `lake` |

T-INV-1 is the only standing invariant in the suite. It is what keeps the DuckDB label-propagation implementation and Splink's `cluster_pairwise_predictions_at_threshold` from drifting apart, which is the agreement G3 rests on. A scenario test that passes while T-INV-1 fails is reported as a T-INV-1 failure against that scenario's node id.

<a id="s8-4"></a>
### 8.4 Unit test highlights

- **Normalizers, property-based (`hypothesis`).** For each of `lowercase_trim`, `email_norm`, `phone_e164`, `null_semantics`, `name_norm`: idempotence `f(f(x)) == f(x)`; casing invariance `f(x) == f(x.upper())` where the normalizer lowercases; E.164 round-trip for every generated NANP number under the three drift formats; `email_strip_plus_addressing` honoured in both settings; the placeholder list maps to NULL, and NULL is distinct from the empty string in every output. Strategies draw from the Unicode ranges the NFC step must handle, so the `content_hash` NFC requirement is exercised rather than assumed.
- **`name_variants` symmetry.** For every generated name, the normalized `given_name` is element 0 of its own variants array — the precondition `ArrayIntersectLevel` needs for T-MATCH-SYM to be provable rather than empirical.
- **Reconciler as a pure function.** `reconcile(p_old, p_new, id_factory)` is tested with an injectable `IdFactory` (production: ULID; tests: monotonic counter) on synthetic partitions covering: unassigned-only cluster; extend; merge; split; **merge-and-split-at-once** (a cluster that is simultaneously the union of two old entities and a proper subset of a third); a 2–2 split resolved by `min record_key ASC`; empty fragment → `retired`; a record leaving all clusters → singleton entity; redirect chains three deep; a redirect cycle, which MUST raise rather than loop. Minting order is asserted to follow ascending minimum member `record_key`, so the cluster→`entity_id` map is reproducible.
- **Event replay.** Folding `entity_events` in `(occurred_at, seq)` order reproduces `entity_membership` exactly, on every synthetic partition above.
- **Config validators.** Unknown source name; unknown survivorship rule token; unknown comparison level token; `survivorship` key set not set-equal to the survivable column set of `golden_records`; `em_blocking_rules` with fewer than 2 entries; missing `u_seed`; a survivorship chain containing no rule able to separate two records from the same source; and the threshold validator `0 < review_low < auto_merge <= 1`. Each asserts the specific Pydantic error path, not merely that validation failed.
- **`config_hash`.** Reordering mapping keys, changing comment lines, and re-serialising the YAML leave `config_hash` unchanged; changing any value changes it.
- **Never-cut algorithm.** Pure-function tests over hand-built graphs: the cut choice tie (two edges with equal `match_probability` — resolved by `(rec_a_key ASC, rec_b_key ASC)`); the path tie (two shortest paths of equal `hop_count` — resolved by the lexically smallest vertex sequence); the 2–2 split induced by the cut; a path made entirely of edges at or above `clustering.cut_protect_probability`, which escalates to `review_queue` with `reason='never_unsatisfiable'` instead of cutting; and non-convergence within `clustering.max_iterations`, which MUST raise so the stage fails without committing.
- **`er.eval.pairwise_metrics`.** Degenerate inputs: empty predicted set; empty truth set; a universe smaller than the predicted set (MUST raise); one all-singletons partition; one all-in-one partition. Precision, recall and F1 are checked against hand-computed values.
- **Pair canonicalisation and `record_key`.** `canonical_pair(a, b) == canonical_pair(b, a)`; `record_key` rejects a `source_record_id` containing `':'`; `resolve()` cycle guard.

<a id="s8-5"></a>
### 8.5 Quality metric definitions

All match quality — in tests and in the benchmark — is computed by exactly one function:

```python
# src/er/eval/metrics.py
def pairwise_metrics(predicted: set[tuple[str, str]],
                     truth: set[tuple[str, str]],
                     universe: set[tuple[str, str]]) -> PairwiseMetrics:
    """All three arguments are sets of canonical (rec_a_key, rec_b_key) pairs
    with rec_a_key < rec_b_key. Returns precision, recall, f1, and the raw
    tp/fp/fn counts, evaluated strictly within `universe`; a predicted or truth
    pair outside `universe` raises."""
```

T-MATCH-1a, T-MATCH-1b and `benchmarks/run_benchmark.py` MUST call this function; no other precision/recall implementation may exist in the repository. That is enforced, not merely required: `scripts/lint_metrics.py` runs in the static job and fails on a second definition outside `src/er/eval/metrics.py` (the step and the lint's duty are declared in S9.1, which owns the static job's step list).

Three numbers are reported, always all three:

| Metric | `predicted` | `truth` | `universe` |
|---|---|---|---|
| **Blocking recall** | the DISTINCT canonical pair set from `int_blocking_keys` | true persona pairs | the full `C(n,2)` set over all current records |
| **Edge-level precision / recall** | `match_scores` rows with `match_probability >= auto_merge` | true persona pairs restricted to the blocked set | the DISTINCT canonical blocked pair set |
| **Cluster-level precision / recall** | the transitive closure of `entity_membership` | true persona pairs | the full `C(n,2)` set |

Cluster-level is the **headline**: clusters, not edges, are the product, and one bad edge chaining two 4-record clusters costs 1 false pair at the edge level and 16 at the cluster level. Edge-level alone hides a blocking regression entirely, because it is computed over the blocked universe — a blocking rule that stops emitting a key removes the pair from both `predicted` and `universe`, leaving edge-level recall at 1.0 while true pairs are silently lost; blocking recall over the full `C(n,2)` universe is the only number that can fall in that failure.

Quality is **reported, not gated**, in the benchmark; it is **gated** in T-MATCH-1a/1b as absolute counts on `base_10`. `blocking_recall` is a required key in `artifacts/bench/latest.json`.

---

<a id="s9"></a>
## 9. CI/CD (GitHub Actions)

Two workflows. `ci.yaml` is the PR path; `benchmark.yaml` never runs on the PR path (G4). Every `uses:` is pinned to the release commit SHA of the tag in its trailing comment; `actionlint` in the static job fails on any unpinned reference, and Dependabot (`.github/dependabot.yml`, ecosystem `github-actions`) proposes SHA bumps.

<a id="s9-1"></a>
### 9.1 `ci.yaml` — PR path

```yaml
name: ci
on:
  pull_request:
  push: { branches: [main] }

permissions:
  contents: read

concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: ${{ github.event_name == 'pull_request' }}

jobs:
  static:
    runs-on: ubuntu-latest
    timeout-minutes: 10
    steps:
      - uses: actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683   # v4.2.2
      - uses: astral-sh/setup-uv@f0ec1fc3b38f5e7cd3d55c029b73096c30f19b40 # v10.0.1
        with:
          enable-cache: true
          cache-dependency-glob: uv.lock
      - run: uv sync --frozen
      - run: uv run ruff check .
      - run: uv run ruff format --check .
      - run: uv run mypy --strict src/er
      - name: actionlint
        run: uv run python scripts/actionlint.py            # downloads nothing; wraps the pinned binary
      - name: Spec lint
        run: uv run python scripts/lint_spec.py DesignDoc.md
      - name: Board lint
        run: uv run python scripts/lint_board.py
      - name: Single metrics implementation
        run: uv run python scripts/lint_metrics.py       # the S8.5 second-definition grep
      # Added in M5, together with report.py, benchmark.yaml and the committed
      # baselines. It is NOT present in the ci.yaml that M1 delivers: the step
      # references three artefacts no earlier milestone produces, so adding it
      # before M5 would leave the static job — and therefore every PR — red for
      # three milestones with no way to satisfy it. See S12 M5.
      - name: Baseline/dispatch parity
        run: uv run python benchmarks/report.py --validate-baselines
             --baselines-dir benchmarks/baselines
             --workflow .github/workflows/benchmark.yaml
      - run: uv run dbt deps --project-dir dbt
      - run: uv run dbt parse --project-dir dbt --profiles-dir dbt/profiles --target mem

  unit:
    runs-on: ubuntu-latest
    timeout-minutes: 10
    steps:
      - uses: actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683   # v4.2.2
      - uses: astral-sh/setup-uv@f0ec1fc3b38f5e7cd3d55c029b73096c30f19b40 # v10.0.1
        with:
          enable-cache: true
          cache-dependency-glob: uv.lock
      - run: uv sync --frozen
      - run: mkdir -p artifacts
      - run: uv run dbt deps --project-dir dbt
      - run: uv run dbt compile --project-dir dbt --profiles-dir dbt/profiles --target mem
      - run: uv run pytest tests/unit -n auto -q --junitxml=artifacts/junit-unit.xml
      - if: always()
        uses: actions/upload-artifact@65c4c4a1ddee5b72f698fdd19549f0f0fb45cf08 # v4.6.0
        with:
          name: unit-artifacts
          path: artifacts/
          if-no-files-found: error

  integration:
    runs-on: ubuntu-latest
    timeout-minutes: 25
    needs: [static, unit]
    steps:
      - uses: actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683   # v4.2.2
      - uses: docker/setup-buildx-action@c47758b77c9736f4b2ef4073d4d51994fabfe349 # v3.7.1
      - name: Build pipeline image (cached)
        uses: docker/build-push-action@4f58ea79222b3b9dc2c8bbdd6debcef730109a75   # v6.9.0
        with:
          context: .
          file: docker/Dockerfile
          load: true
          tags: er-pipeline:ci
          cache-from: type=gha
          cache-to: "type=gha,mode=max"        # quoted: unquoted, the comma ends the flow-mapping entry
      - name: Reset substrate
        run: |
          mkdir -p artifacts
          docker compose -f docker/compose.yaml --profile test down -v --remove-orphans
      - name: Environment check (T-DOCTOR-1)
        run: docker compose -f docker/compose.yaml --profile test run --rm pipeline er doctor
      - name: Run integration suite
        run: docker compose -f docker/compose.yaml --profile test run --rm pipeline
             pytest tests/integration -q --maxfail=3
             --junitxml=/app/artifacts/junit.xml --durations=20
      - name: Tear down
        if: always()
        run: docker compose -f docker/compose.yaml --profile test down -v --remove-orphans
      - name: Collect artifacts (junit, dbt logs, run manifests)
        if: always()
        uses: actions/upload-artifact@65c4c4a1ddee5b72f698fdd19549f0f0fb45cf08 # v4.6.0
        with:
          name: integration-artifacts
          path: artifacts/
          if-no-files-found: error
```

- **Job graph.** `static` and `unit` run in parallel; only `integration` has `needs: [static, unit]`. The serial `static → unit → integration` chain paid for two `uv sync` runs ahead of the expensive job for no signal.
- **dbt targets.** `dbt/profiles/profiles.yml` defines exactly two targets: `lake` (in-container; performs the S7.2 attach; the default for every `er` invocation) and `mem` (`path: ':memory:'`, no attach, no extensions, no services). Every `env_var()` call in the profile MUST supply a default (`env_var('ER_CATALOG_DSN', '')`, `env_var('ER_LAKE_DATA_PATH', '')`, …) so a bare runner with no services set can parse and compile the project. `dbt parse` (static) and `dbt compile --target mem` (unit) never open a warehouse connection to DuckLake; `dbt build` runs only inside the integration suite against `--target lake`.
- **Lints owned by the static job.** This bullet is the authority for what each lint enforces; no other section adds duties to them.
  - `scripts/lint_spec.py` fails if `DesignDoc.md` cites a companion design document, carries an unresolved placeholder marker, leaves an S2.1 pin unfilled, has an `<a id="…">` anchor that is duplicated, missing before a heading, or inconsistent with its heading number, **or if any S2.1 row disagrees with `uv.lock` or with the image digests in `docker/compose.yaml`** — the pin table is a restatement, so a restatement that has drifted is worse than none. The forbidden patterns are declared in the linter, never in this document.
  - `scripts/lint_board.py` fails if any relation in S5 or any test id in S8.3 is unassigned to a milestone in S12, or if a milestone cites an id that does not exist.
  - `scripts/lint_metrics.py` fails if any file outside `src/er/eval/metrics.py` defines a second precision/recall implementation — the enforcement S8.5 requires, so that "exactly one implementation" is a gate rather than a wish.
  - `report.py --validate-baselines` fails if the set of `benchmarks/baselines/*.json` is not exactly the set of `workflow_dispatch` choice options in `benchmark.yaml`, **or if, for any dispatchable scale, `benchmark.yaml`'s `runs-on` expression and the envelope it exports disagree with that scale's row in `benchmarks/scales.yaml`** (S10.2) — the coupling that makes a `100k` baseline comparable to a `100k` run.
- **Machine-readable results.** Every pytest invocation writes junit XML into `artifacts/`; both uploads use `if-no-files-found: error`, so an empty artifacts directory is a CI failure rather than a silent success.
- **Budget.** The <10 min PR target is enforced by the timeouts: `static` and `unit` are capped at 10 minutes each and run concurrently, `integration` at 25. Branch protection on `main` requires all three jobs.

<a id="s9-2"></a>
### 9.2 `benchmark.yaml` — dispatch + weekly

```yaml
name: benchmark
on:
  workflow_dispatch:
    inputs:
      scale:
        type: choice
        options: [smoke, 10k, 100k]     # MUST equal the set of committed baselines (S10.2)
        default: smoke
      repeat:
        type: string
        default: "3"
  schedule:
    - cron: "0 6 * * 1"                 # weekly, Monday 06:00 UTC, smoke scale

permissions:
  contents: read

concurrency:
  group: benchmark-${{ inputs.scale || 'smoke' }}
  cancel-in-progress: false             # never cancel a scheduled or in-flight measurement

env:
  SCALE: ${{ inputs.scale || 'smoke' }}
  REPEAT: ${{ inputs.repeat || '3' }}

jobs:
  bench:
    # MUST agree with the runner column of S10.2; --validate-baselines checks it (S9.1)
    runs-on: ${{ (inputs.scale == '100k') && 'ubuntu-latest-8-cores' || 'ubuntu-latest' }}
    timeout-minutes: ${{ (inputs.scale == '100k') && 120 || 40 }}
    steps:
      - uses: actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683   # v4.2.2
      - uses: docker/setup-buildx-action@c47758b77c9736f4b2ef4073d4d51994fabfe349 # v3.7.1
      - name: Build pipeline image
        uses: docker/build-push-action@4f58ea79222b3b9dc2c8bbdd6debcef730109a75   # v6.9.0
        with:
          context: .
          file: docker/Dockerfile
          load: true
          tags: er-pipeline:ci
          cache-from: type=gha
          cache-to: "type=gha,mode=max"
      - name: Preflight (disk) and resource envelope for this scale
        run: |
          mkdir -p artifacts/bench
          docker compose -f docker/compose.yaml --profile bench down -v --remove-orphans
          free_gb=$(python3 -c "import os;s=os.statvfs('/var/lib/docker');print(s.f_bavail*s.f_frsize//2**30)")
          need=$(docker run --rm er-pipeline:ci python benchmarks/scales.py --scale "$SCALE" --field min_free_gb)
          echo "free=${free_gb}GiB need=${need}GiB host_nproc=$(nproc)"
          test "$free_gb" -ge "$need"
          # The envelope is a property of the scale (S10.2). Export it so Compose applies it and
          # so the measured cgroup values can be checked against it by the S10.4 rule.
          echo "ER_CPU_LIMIT=$(docker run --rm er-pipeline:ci python benchmarks/scales.py --scale "$SCALE" --field cpu_limit)" >> "$GITHUB_ENV"
          echo "ER_MEM_LIMIT=$(docker run --rm er-pipeline:ci python benchmarks/scales.py --scale "$SCALE" --field mem_limit)" >> "$GITHUB_ENV"
          echo "ER_DUCKDB_MEMORY_LIMIT=$(docker run --rm er-pipeline:ci python benchmarks/scales.py --scale "$SCALE" --field duckdb_memory_limit)" >> "$GITHUB_ENV"
      - name: Run benchmark
        run: docker compose -f docker/compose.yaml --profile bench run --rm benchmark
             python benchmarks/report.py --run --scale "$SCALE" --repeat "$REPEAT"
             --out /app/artifacts/bench/latest.json
        env:
          BENCH_SCALE: ${{ env.SCALE }}
      - name: Compare vs baseline (in-image)
        run: docker compose -f docker/compose.yaml --profile bench run --rm benchmark
             python benchmarks/report.py --compare /app/artifacts/bench/latest.json
             --baselines-dir /app/benchmarks/baselines --scale "$SCALE"
             --fail-threshold 1.25
        env:
          BENCH_SCALE: ${{ env.SCALE }}
      - name: Upload benchmark artifacts
        if: always()
        uses: actions/upload-artifact@65c4c4a1ddee5b72f698fdd19549f0f0fb45cf08 # v4.6.0
        with:
          name: bench-${{ env.SCALE }}-${{ github.sha }}
          path: artifacts/bench/
          if-no-files-found: error
      - name: Tear down
        if: always()
        run: docker compose -f docker/compose.yaml --profile bench down -v --remove-orphans
```

- **Runner, envelope and scale move together.** A larger scale is dispatched onto a larger runner *and* given a larger Compose envelope, from one table: S10.2 owns `runner`, `cpu_limit`, `mem_limit` and `duckdb_memory_limit` per scale; `runs-on` above encodes the runner and the preflight exports the other three into the environment `docker compose` reads. Without that coupling a `100k` run would land on an 8-core runner and then be squeezed into the 2-CPU default envelope — measurable, but not the machine anyone provisioned — and the S10.4 comparability check would have nothing coherent to compare against.
- **Everything Python runs in-image.** The disk figure comes from the runner's own `statvfs` (it is a property of the runner), but the threshold it is compared against comes from `benchmarks/scales.py` executed inside `er-pipeline:ci` with a bare `docker run` — no Compose services are started for a preflight. `report.py` likewise executes inside the image through `docker compose run`, so the job needs no `setup-uv`, no `uv sync`, and no runner-side Python environment beyond the preinstalled `python3`, and the S10.4 environment fingerprint provably comes from the measuring environment rather than the runner.
- **Upload after compare, `if: always()`.** A regression, a `NON_COMPARABLE` verdict, or a crash still publishes `latest.json` and `report.md`.
- **Dispatch options.** The `scale` choice list MUST equal the set of scales with a committed baseline under `benchmarks/baselines/`. Adding a scale to the dispatch list requires committing its baseline first; the static job's `--validate-baselines` step enforces the equality mechanically. `1m` is defined in `scales.yaml` but is not dispatchable until a `1m` baseline is committed.
- **Cadence.** Weekly on Monday at the `smoke` scale, plus on-demand dispatch. There is no nightly run and there is never a PR-path run.
- **Gate.** `--fail-threshold 1.25` fails the job when any phase median is more than 25% slower than the committed baseline (`REGRESSION`). Baselines change only through a reviewed PR that rewrites the JSON with `--write-baseline`.

---

<a id="s10"></a>
## 10. Benchmark Harness

<a id="s10-1"></a>
### 10.1 Data generation (`fixtures/generator/`)

Deterministic, seeded synthetic corpus generator, shared by fixtures and benchmarks. The seed is `generator.seed` in the tenant config (S6); nothing in the generator reads a clock or an unseeded RNG.

- `personas.py` generates ground-truth persons: names drawn from weighted frequency lists so term-frequency adjustments see realistic skew, plus emails, phones, addresses and DOBs.
- `corruptions.py` applies per-source corruption profiles: typo rate, nickname substitution, per-field missingness, format drift, stale addresses. Only address patterns the v1 parser handles are emitted.
- `emit.py` emits per-source CSVs under `storage.drop_dir` (the `benchdata` volume in Compose) with a configurable duplication factor (records per persona per source), matching the `sources.<name>.columns` mapping of the benchmark config exactly.
- Ground truth (`persona_id`) is written alongside, so every benchmark run reports match quality next to speed: a perf change that silently costs recall is visible in the same report.
- Corpus generation and `er init` are executed before measurement starts and are **not** included in any phase timing; the generated corpus is reused across the `--repeat` passes of a single run.

<a id="s10-2"></a>
### 10.2 Scales (`benchmarks/scales.yaml`)

| Scale | Personas | Records | Incremental batch | `min_free_gb` | Baseline committed | Dispatchable |
|---|---|---|---|---|---|---|
| `smoke` | 400 | 1,000 | 50 | 4 | yes | yes |
| `10k` | 4,000 | 10,000 | 100 | 8 | yes | yes |
| `100k` | 40,000 | 100,000 | 1,000 | 24 | yes | yes |
| `1m` | 400,000 | 1,000,000 | 10,000 | 120 | no | no |

`smoke` exists so the weekly cron and any first-run bootstrap complete inside a 40-minute job on a 2-vCPU runner. `min_free_gb` is the preflight threshold asserted in S9.2. A scale becomes dispatchable only when `benchmarks/baselines/<scale>.json` is committed.

**Resource envelope per scale (normative).** `scales.yaml` carries these four fields alongside the ones above, and they are the single source of truth for the runner a scale is dispatched onto and the container limits it is measured under:

| Scale | `runner` (S9.2 `runs-on`) | `cpu_limit` (Compose `cpus`, and `ER_DUCKDB_THREADS`) | `mem_limit` (Compose `memory`) | `duckdb_memory_limit` (`ER_DUCKDB_MEMORY_LIMIT`) |
|---|---|---|---|---|
| `smoke` | `ubuntu-latest` (2 vCPU) | `2` | `6g` | `4GB` |
| `10k` | `ubuntu-latest` (2 vCPU) | `2` | `6g` | `4GB` |
| `100k` | `ubuntu-latest-8-cores` | `6` | `24g` | `16GB` |
| `1m` | `ubuntu-latest-16-cores` | `12` | `56g` | `40GB` |

Three rules bind this table:

- **The envelope must fit the runner.** `mem_limit` MUST leave RAM on the runner for `catalog` and `objectstore`, because a container memory limit is a hard ceiling and the sum of the containers has to fit host RAM: `6g` of the 2-vCPU runner's 8 GiB, `24g` of 32 GiB, `56g` of 64 GiB. `cpu_limit` MUST NOT exceed the runner's vCPU count — a quota above the machine's capacity is unenforceable and leaves S10.4 comparing against a limit no run could reach — but it reserves nothing and is **not** required to leave cores behind: `deploy.resources.limits.cpus` is a CPU *quota*, not a cpuset and not a reservation (S7.1, S10.4), so `smoke` and `10k` set it to the runner's full `2` deliberately. The two service containers are I/O-bound and near-idle while a phase runs, and the kernel time-slices them against the pipeline rather than being locked out of it; a fractional quota there would only make the small scales slower without making them more comparable. `duckdb_memory_limit` is roughly 70% of `mem_limit`, because DuckDB's `memory_limit` bounds the buffer manager and not the Python heap or the dbt subprocess.
- **`ER_DUCKDB_THREADS` equals `cpu_limit`, always.** The Compose default (S7.1) is written as `${ER_CPU_LIMIT:-2}` precisely so the two cannot be set independently.
- **A run is measured against its own scale's row and no other.** Two scales are never comparable to each other, and a run whose measured cgroup quota or memory limit differs from its scale's row is `NON_COMPARABLE` (S10.4) — that is what makes a `100k` baseline mean something even though it was produced on a different class of machine than the `smoke` baseline.

The Compose defaults are the `smoke`/`10k` row, so the `test` profile and any local `docker compose run` reproduce the small envelope with no environment set. `ER_CPU_LIMIT` and `ER_MEM_LIMIT` are read by **Compose**, not by the CLI, and are therefore not part of S6's environment contract; `ER_DUCKDB_THREADS` and `ER_DUCKDB_MEMORY_LIMIT`, which the CLI does read, are derived from them inside `x-er-env` (S7.1).

<a id="s10-3"></a>
### 10.3 Measured phases, metrics, and `report.py`

Each measured pass executes and times, in order:

1. `er ingest` — base corpus
2. `er standardize` — dbt staging + intermediate
3. `er train` — Splink EM, TF materialisation
4. `er match --mode full` + clustering + `er reconcile` — cold start
5. `er assemble` — all entities
6. **incremental cycle** — `er ingest` (batch) → `er standardize --changed-only` → `er match --mode incremental` → `er reconcile` → `er assemble --touched-only`

Per-phase metrics:

| Metric | Definition |
|---|---|
| `wall_ms` | monotonic clock around the stage; also written to `run_stages` |
| `records_per_sec` | `rows_out / (wall_ms / 1000)` for the phase — `rows_out / wall_ms` would be records per *millisecond*, off by a factor of 1000 |
| `candidate_pair_count` | `COUNT(*)` over the DISTINCT canonicalised pair set derived from `int_blocking_keys` |
| `pairs_above_auto_merge` | `match_scores` rows at `match_probability >= auto_merge` for the run |
| `memory_peak_bytes` | max over the S10.3 sampler (below) |
| `snapshot_count` | `snapshot_end - snapshot_start` summed over the phase's `run_stages` rows |

Run-level metrics: **incremental ratio** = phase 6 wall time ÷ (phases 1+2+4+5) wall time — the headline number for G3 economics, expected to fall as the corpus grows — and the quality block from S10.5.

**Memory sampling.** A background thread samples every 250 ms and records the maximum of three sources; `memory_peak_bytes` is the max across all three:

1. `SELECT sum(memory_usage_bytes) FROM duckdb_memory()` on a `.cursor()` duplicate of the run connection — buffer-manager total only, and a *current* reading with no peak counter, which is why it is sampled rather than read once at phase end;
2. process RSS (`psutil.Process().memory_info().rss`);
3. `/sys/fs/cgroup/memory.peak` — **authoritative for pod sizing**, because it is the only source that includes DuckDB's out-of-buffer allocations, Python heap, and the dbt subprocess.

All three are reported separately in `latest.json`; the cgroup peak is the number S10.4 feeds to k8s sizing. Because DuckDB reads neither cgroup CPU nor cgroup memory limits, `ER_DUCKDB_THREADS` and `ER_DUCKDB_MEMORY_LIMIT` MUST be applied with `SET threads` / `SET memory_limit` on every connection opened by the harness, the CLI, and the dbt profile; without them DuckDB plans against host resources and the numbers are not comparable across machines.

**`benchmarks/report.py`** is the single entrypoint. `benchmarks/run_benchmark.py` executes one measured pass and returns raw phase records; `report.py` drives it, aggregates, compares, and writes.

| Flag | Meaning |
|---|---|
| `--run --scale <s>` | generate the corpus, execute `--repeat` passes, write `--out` (default `/app/artifacts/bench/latest.json`) plus a sibling `report.md` |
| `--repeat N` | default `3`; reports the **median** per phase per metric plus the coefficient of variation (`stdev/mean`) of `wall_ms` across passes |
| `--out PATH` | destination for the run JSON |
| `--compare RUN [--baselines-dir DIR] [--scale s]` | compare a run JSON against `DIR/<scale>.json`; prints a per-phase table with ratios |
| `--fail-threshold F` | default `1.25`; a phase whose median `wall_ms` exceeds `F ×` the baseline median yields `REGRESSION` |
| `--write-baseline` | the documented bootstrap: copy a run JSON to `DIR/<scale>.json`; refuses a `NON_COMPARABLE` run |
| `--validate-baselines --baselines-dir DIR --workflow PATH` | asserts the baseline file set equals the workflow's `scale` choice options, and that each dispatchable scale's runner and exported envelope in the workflow match its `scales.yaml` row (S10.2); used by the static CI job |

Verdicts and exit codes:

| Verdict | Condition | Exit |
|---|---|---|
| `OK` | every phase within `--fail-threshold` of baseline | 0 |
| `NO_BASELINE` | `DIR/<scale>.json` absent — the first run at a scale is not a failure | 0 |
| `REGRESSION` | at least one phase median above the threshold | 1 |
| `NON_COMPARABLE` | S10.4 comparability rule violated | 3 |

Bad arguments or an unreadable run JSON exit `2`. The verdict string is written into the run JSON and printed as the last line of stdout.

`benchmarks/report.py` is **not** an `er` command: the four codes above are local to this tool and are not the S4.0 taxonomy. In particular its `3` means `NON_COMPARABLE`, not "precondition failure", and no `er` command ever returns a benchmark verdict.

Outputs: `artifacts/bench/latest.json` (machine-readable: fingerprint, per-phase medians and CVs, all three memory series' peaks, quality block, verdict) and `artifacts/bench/report.md` (human-readable table).

<a id="s10-4"></a>
### 10.4 Comparability and interpretation

Every run JSON embeds an environment fingerprint: image digest, git SHA, runner label, cgroup `cpu.max` (quota and period, as read), cgroup `memory.max` and `memory.peak`, in-container `nproc`, `ER_DUCKDB_THREADS`, `ER_DUCKDB_MEMORY_LIMIT`, DuckDB / Splink / dbt-core / dbt-duckdb / ducklake-extension versions, `config_hash`, `generator.seed`, `model_version`, `tf_snapshot_id`, and the scale.

**NON-COMPARABLE rule (normative).** Numbers produced on different shapes of machine are not comparable, so a run is marked `NON_COMPARABLE` — and MUST NOT be committed as a baseline, and MUST NOT be compared against one — when any of:

- the effective CPU **quota** does not equal the scale's `cpu_limit` (S10.2). The quota is read from `/sys/fs/cgroup/cpu.max` as `quota ÷ period` (`"600000 100000"` → `6`), or from `cpu.cfs_quota_us ÷ cpu.cfs_period_us` on cgroup v1;
- cgroup `memory.max` does not equal the scale's `mem_limit` (S10.2);
- `ER_DUCKDB_THREADS` does not equal the scale's `cpu_limit`, or `ER_DUCKDB_MEMORY_LIMIT` does not equal its `duckdb_memory_limit` — DuckDB reads neither cgroup, so a correct cgroup with a wrong `SET` is a differently-shaped machine as far as the measurement is concerned;
- the run's scale differs from the baseline's scale;
- the coefficient of variation of `wall_ms` for any phase exceeds `0.15` across the `--repeat` passes.

**Why the CPU check reads `cpu.max` and not `nproc`.** `deploy.resources.limits.cpus` (S7.1) sets a CPU **quota**, not a cpuset: it caps how much CPU time the container may consume per period but does not hide cores, so in-container `nproc` reports the **host** core count regardless of the limit. A rule written against `nproc` would therefore mark *every* quota-limited run non-comparable — permanently, and most visibly on the larger runner where `nproc` is furthest from the limit, which is exactly where the `100k` baseline has to come from. `nproc` stays in the fingerprint as a diagnostic (it identifies the host class) and has no gate authority.

A `NON_COMPARABLE` run is still uploaded and still reported; it simply carries no gate authority. This is what keeps a single noisy sample on a shared runner from either firing a false regression or being frozen into a baseline.

The benchmark answers three questions: the maximum sustainable batch cadence per scale, the memory sizing per scale (cgroup peak → future k8s pod requests/limits), and which stage to optimise first.

<a id="s10-5"></a>
### 10.5 Quality metrics in the benchmark

Quality is **reported, never gated**. The three metrics — blocking recall, edge-level and cluster-level precision / recall / F1 — are defined normatively in S8.5, together with the `predicted` / `truth` / `universe` triple each is computed over; S10.5 does not redefine them. The benchmark emits all three into `artifacts/bench/latest.json` by calling the same `er.eval.pairwise_metrics(predicted, truth, universe)` implementation the tests call, so a benchmark number and a test number are never produced by two code paths.

A change that improves throughput while dropping blocking recall is therefore visible in the same report, but it fails no job: quality regressions are gated by S8's tests on committed fixtures, where the expected values are exact.

---

<a id="s11"></a>
## 11. Phase 2 Stub — Embedding Coherence (interface only in v1)

`src/er/embeddings/coherence.py` defines the seam so phase 2 is a config flip rather than a pipeline change:

```python
@dataclass(frozen=True)
class ClusterCoherence:
    entity_id: str
    dispersion: float              # 0.0 = perfectly coherent; higher = more dispersed
    outlier_record_keys: list[str] # members driving the dispersion, may be empty

class CoherenceScorer(Protocol):
    def score_clusters(self, entity_ids: list[str]) -> list[ClusterCoherence]:
        """Return one ClusterCoherence per input entity_id, in input order."""

class NoopScorer:
    """v1 default. Returns dispersion=0.0 and no outliers for every entity."""
    def score_clusters(self, entity_ids: list[str]) -> list[ClusterCoherence]:
        return [ClusterCoherence(e, 0.0, []) for e in entity_ids]
```

- The scorer is selected by `coherence.scorer` in S6 (`noop` in v1) and constructed once per run.
- **`er assemble` calls `score_clusters` exactly once per run**, immediately after `assemble.py` writes `er_touched_entities` and before the marts run, with the entity ids in `er_touched_entities` where `disposition='rebuild'`. It is never called per entity and never inside the label-propagation loop. It is `assemble` and not `reconcile` because `er_touched_entities` is written by `assemble.py` (S4.6) and runs *after* `reconcile` — at reconcile time the table holds no row for this `run_id`, so a scorer called there would always receive an empty list. Membership is final once reconcile has committed, so scoring at the start of assemble sees exactly the settled partition reconcile produced.
- A finding above the scorer's threshold lands as a `review_queue` row with `subject_type='entity'`, `entity_id` set, `rec_a_key` and `rec_b_key` NULL, `reason='coherence'`, `status='open'`, and the dispersion plus outlier record keys in `waterfall`. It is subject to the same upsert rule as gray-band rows: refresh `last_seen_run_id`, skip already-resolved subjects.
- Coherence findings never alter clustering in v1. `NoopScorer` produces zero rows, so the code path is exercised on every run without changing any output.
- Phase 2 replaces `NoopScorer` with a sentence-transformers implementation writing embeddings to a DuckLake column and using DuckDB VSS. No table, event type, or CLI verb changes.

<a id="s12"></a>
## 12. Milestones

| Milestone | Contents | Relations first written | Exit criteria |
|---|---|---|---|
| **M0 — Decision lock** | S12.1 resolved and written into S5/S6; S2.1 pinned-version table populated with literal versions and image digests | none | every row of S12.1 marked LOCKED with a one-line resolution; no code merged before this |
| **M1 — Substrate** | Compose fixed, Dockerfile with extensions baked at build time, `ddl.py`, `er init`, `er doctor`, `runs`/`run_stages` capture, the advisory lock, `ids.py`, the config schema and `config_hash`, `tests/conftest.py` namespacing, `tests/helpers/compare.py`, CI rebuilt, **the dbt scaffolding this milestone's own exit gate needs** — `dbt/dbt_project.yml`, `dbt/packages.yml` (so `dbt deps` resolves `dbt_utils`, which `unique_combination_of_columns` comes from), `dbt/profiles/profiles.yml` carrying both targets of S4.0b/S9.1 (`lake` and `mem`, every `env_var()` defaulted), and a `dbt/models/schema.yml` declaring the `ddl.py`-owned relations as **sources** with their S5.0 logical-key tests carrying the `keys` tag, because T-KEY-1a's gate is `dbt test --select tag:keys` and a selector that matches nothing exits 0 and proves nothing; no dbt **model** ships in M1, since the first dbt-owned relation does not exist until M2 — **and the `er run-all` chain with every downstream stage present as a nothing-to-do stub** — each stub writes its `run_stages` row, does no work and exits `10`, which is what makes the exit criterion below runnable before any of those stages has an implementation | `runs`, `run_stages` (all fourteen `ddl.py`-owned relations are *created* here and empty) | `er init && er doctor && er run-all --mode incremental --skip-ingest` on an empty lake: `er init` exits 0 and is idempotent on a second invocation; `er doctor` exits 0 with every S2.1 row it owns plus the six T-DOCTOR-1 runtime assertions passing; `er run-all` exits **0** — every stage returns `10` and, per S4.0, a chain of `10`s is a successful no-op run — and writes **exactly one `runs` row** with `status='succeeded'` and **four `run_stages` rows** (`standardize, match, reconcile, assemble`) each carrying a snapshot range. `--skip-ingest` is required: without `--source`/`--path` the invocation is rejected with exit `2` before any stage runs. Because ingest is skipped, **no `ingest_batches` row is written** and none is asserted. **Zero** relations matching `__splink__%` exist in `lake`; a **second concurrent `er run-all` exits 3**; T-DOCTOR-1, **T-KEY-1a** and T-CONC-1 green; the integration job green end to end. Stage commands that only print to stdout do not satisfy this. **T-KEY-1a and not T-KEY-1b**: the dbt-owned arm needs `int_std_records`, which does not exist until M2, and a milestone may only gate on relations it creates |
| **M2 — Ingest & standardize** | source adapters, `content_hash`, append-only landing, tombstone derivation, `stg_*`, `int_std_records`, `int_blocking_keys`, the blocking generator, the `base_10` fixture with `expected/base/std_hashes.csv` (its `expected/base/membership.csv` and `expected/base/golden.csv` are authored and committed here, but are first *asserted* in M3 and M4 respectively, since the relations they describe do not exist yet), the `deletion_scenario` fixture's three phases, **the synthetic generator** | `raw_records`, `ingest_batches`, `stg_crm`, `stg_billing`, `stg_webforms`, `int_std_records`, `int_blocking_keys` | T-KEY-1b, T-STD-1, T-BLK-1, **T-IDEM-1a**, **T-DEL-1a** green; `base_10` truth counts machine-checked at 23/10/18. The full-chain arms T-IDEM-1 and T-DEL-1 belong to M4 and M3: they assert on `entity_events`, `entity_membership`, `match_scores` and `golden_*`, none of which this milestone creates |
| **M3 — Match, assert, reconcile** | Splink settings builder, `er train`, TF freeze, the two-pass incremental path, assertions and CONTRADICTION-1, the partition-level never-cut, clustering, the overlap-matrix reconciler, `er assert`/`er review`, **the committed fixture model `fixtures/static/model_test_v1.json`**, the `incremental_batch` fixture's `base/`, `batch/` and `parity_pairs.csv` (T-INC-3 scores through the two-pass path, so it needs a scenario with a `batch/` phase), and the `supersession_scenario` fixture's `base/`, `batch/` and `expected/{base,batch}/` (its expectations name `match_scores`, `entity_membership` and `entity_events`, which are created here and nowhere earlier) | `model_registry`, `tf_lookup`, `match_scores`, `assertions`, `cut_edges`, `entities`, `entity_membership`, `entity_events`, `review_queue` | T-MATCH-SYM, T-MATCH-1a, T-MATCH-1b, T-INC-3, T-TRAIN-1, T-MODEL-1, T-TF-1, T-PERM-1, T-PERM-2, T-PERM-3, T-ASSERT-1, T-ASSERT-2, T-REVIEW-1, T-REVIEW-2, **T-DEL-1**, **T-SUPER-1** green; T-INV-1 armed as an autouse finalizer and green on every scenario. Every one of those reads only relations M1–M3 create; the golden reap that T-PERM-1 used to assert moved to T-INC-2 in M4 for the same reason |
| **M4 — Golden & incremental proof** | survivorship macros with the terminal tiebreak, lineage, `golden_display`, touched-only assembly, the retire/reap path, `er correct` (the correction pass), `--resume` | `er_touched_entities`, `golden_records`, `golden_lineage`, `golden_display` | T-GOLD-1, T-SNAP-1, T-INC-1, T-INC-1b, T-INC-2, T-CORR-1, T-CFG-1, **T-IDEM-1** (the full-chain arm, which asserts on `golden_records` and `golden_lineage` and so could not be gated earlier) green; full PR CI path green in under 10 minutes, enforced by the sum of the job timeouts |
| **M5 — Benchmark** | `benchmarks/run_benchmark.py`, `report.py` (`--run`, `--compare`, `--write-baseline`, `--repeat`, `--validate-baselines`), `benchmarks/scales.py`, the memory sampler, `benchmark.yaml`, the committed baselines, **and the `Baseline/dispatch parity` step added to `ci.yaml`'s static job** — all in this milestone, because the step references artefacts no earlier milestone produces | none (benchmark writes into a disposable namespace) | **Precondition (environmental, MUST be confirmed before the milestone starts).** The `100k` row of S10.2 dispatches onto `ubuntu-latest-8-cores`, and that larger-runner label is a repository/organisation setting an autonomous implementer cannot provision: with the label unavailable the `100k` job either queues indefinitely or lands on a 2-vCPU runner where the exported envelope (`cpu_limit=6`, `mem_limit=24g`) cannot be honoured, every run is `NON_COMPARABLE` under S10.4, and `--write-baseline` refuses it — so the third baseline is unobtainable by any amount of code. Confirm the label is enabled for this repository before starting M5; if it is not, take the S13 fallback rather than committing a `100k` baseline measured outside its S10.2 row. Then: **smoke, 10k and 100k** baselines committed via `--write-baseline` — all three, because S9.2's dispatch options are `[smoke, 10k, 100k]` and the static job's `--validate-baselines` step asserts that set equals the committed baseline set, so a missing `smoke.json` fails every PR. All three are *producible*: a `100k` dispatch lands on the runner and inside the envelope its S10.2 row declares, so it satisfies the S10.4 comparability rule and `--write-baseline` accepts it. Per-phase CV under 15%; incremental ratio and `blocking_recall` present in `artifacts/bench/latest.json`; the `--fail-threshold 1.25` comparison demonstrably executes in-image |
| **M6 — Phase 2** | embedding coherence implementation replacing `NoopScorer` | none | cluster-level recall on the 100k scale improves without cluster-level precision regressing; coherence findings land as `review_queue` rows with `subject_type='entity'` |

**Gating rule (normative).** A milestone's exit criteria may name only tests that read relations existing by the end of *that* milestone. S12 is a gate — "M1 cannot exit until…", "no code merged before this" — so a criterion that depends on a later milestone's relation does not merely mislead, it stalls an agent working ticket by ticket on a test it cannot make pass. Where a guarantee spans milestones, the test is **split into arms** (T-KEY-1a / T-KEY-1b, T-IDEM-1a / T-IDEM-1, T-DEL-1a / T-DEL-1) rather than deferred whole or asserted early; each arm is a real test with its own node id, and the arms together assert exactly what the single test asserted before. `scripts/lint_board.py` fails if any S8.3 id is unassigned to a milestone, so an arm cannot be created and then forgotten.

**Why the generator is in M2 and the fixture model is in M3.** The generator moves forward from M5 because `base_10`'s traps are hand-authored but the deletion, supersession and correction scenarios need seeded corpora larger than a human should hand-write, and its seed lives in the M1 config schema — there is no dependency left to wait for. The committed fixture model moves back into M3 and is *not* an M2 artifact because scenario tests **never train**: EM over `base_10`'s 23 records is degenerate (the `u` estimate is drawn from at most 253 pairs and the `m` estimate from at most 18), so the model it would produce is noise. `fixtures/static/model_test_v1.json` is trained once against the M2 generator's 10k corpus, committed, and loaded by every scenario test; `er train` itself is exercised only by T-TRAIN-1 and T-MODEL-1.

<a id="s12-1"></a>
### 12.1 Decision lock

Each row below is encoded into a table schema or into the identity of stored rows, and G2 (entity permanence) makes post-hoc re-keying expensive: reversing any of them after data exists requires a full re-key and re-mint, which by definition destroys the permanence the platform sells. **M1 cannot exit until every row is marked LOCKED.** No table may be created before that.

| # | Decision | Resolution | Status |
|---|---|---|---|
| D1 | Tenancy | Namespace-only tenancy, as defined normatively in S1 | **LOCKED** |
| D2 | Incremental scoring mechanism | Two Splink passes, unioned: `find_matches_to_new_records` for new-vs-corpus and a batch-only `link_type='dedupe_only'` Linker for new-vs-new — the mechanism, and the rule that `int_blocking_keys` is never an input to scoring, are normative in S4.3.4 | **LOCKED** |
| D3 | `entity_membership` representation | Current state: exactly one row per `(source_system, source_record_id)`, maintained by `MERGE INTO`; all history in `entity_events`; `merged_into` resolves external ids only, never current membership (S4.5.3) | **LOCKED** |
| D4 | Term-frequency policy | Frozen at training time, persisted as `tf_lookup` keyed by `tf_snapshot_id`, registered via `register_term_frequency_lookup` before every scoring call; `compute_tf_table` is never called outside `er train`; `er train` mints a `tf_snapshot_id` when it materializes `tf_lookup`, and **outside `er train` the only path that mints one is `er correct`**, via `er match --mode full --new-tf-snapshot` (S4.3.3, S4.0) | **LOCKED** |
| D5 | `never_match` semantics | Partition-level rather than edge-level: the cut algorithm, and the exclusion of `cut_edges` from every subsequent clustering run, are normative in S4.4.2 | **LOCKED** |
| D6 | Record identity | `record_key` as defined normatively in S5.0 — the `':'` ban, the three relations that materialize it (`int_std_records`, `int_blocking_keys`, `entity_membership`), and its use as Splink's `unique_id_column_name` | **LOCKED** |
| D7 | `raw_records` semantics | Append-only version history; logical key `(source_system, source_record_id, content_hash)`; `int_std_records` carries `content_hash` and selects the greatest `ingested_at` per record (S4.1, S4.2) | **LOCKED** |
| D8 | Deletion | In v1 scope: `is_deleted`/`deleted_at` on `raw_records`, `er ingest --full-refresh-keys` with an empty-delivery guard and a sentinel tombstone `content_hash`, the retraction path in reconcile, and resurrection on re-appearance (S4.1.1, S4.5.5). `member_removed` and `status='retired'` are therefore reachable and stay in the enums | **LOCKED** |
| D9 | Pair ordering | Canonical `rec_a_key < rec_b_key` on all four pair relations, as defined normatively in S5.0 | **LOCKED** |
| D10 | Id generation | All ids are ULIDs minted in Python (`ids.py`); DuckLake has no sequences. `reconcile()` takes an `IdFactory`; the mint order that makes the cluster→`entity_id` map reproducible is normative in S4.5.4 | **LOCKED** |
| D11 | `golden_records` column list | The literal typed column list in S5, whose survivable subset is `GOLDEN_SURVIVABLE_COLUMNS` (S5.0); S6.1 V2 enforces its set-equality with the `survivorship:` key set | **LOCKED** |
| D12 | Run metadata tables | `runs`, `run_stages`, `ingest_batches`, `er_touched_entities` exist from M1 and are the referents for every `run_id`; each stage records the snapshot **range** it produced, per the range-not-count rule normative in the S4 preamble | **LOCKED** |
| D13 | Clustering threshold | The clustering cut IS `thresholds.auto_merge`, passed explicitly as `cluster_pairwise_predictions_at_threshold(threshold_match_probability=auto_merge)`; threshold units and the half-open gray band are normative in S4.3 | **LOCKED** |
| D14 | Relation ownership | Exactly two owners: `ddl.py` (Python `CREATE TABLE IF NOT EXISTS`) and dbt (`contract: {enforced: true}`), per the ownership rule normative in S5.0 | **LOCKED** |
| D15 | Pinned versions (S2.1) | The S2.1 table carries literal versions for python, `splink==4.0.16`, duckdb, dbt-core, dbt-duckdb, the ducklake/postgres/httpfs extensions, and image **digests** for the catalog and object store; `er doctor` asserts every one | **LOCKED** |

<a id="s13"></a>
## 13. Risks & Mitigations

| Risk | Mitigation |
|---|---|
| Splink 4 API churn | `splink==4.0.16` pinned in S2.1 and asserted by `er doctor`; every Splink call is wrapped behind `src/er/matching/`, so an upgrade touches one package |
| **Splink 5 migration.** Splink 5 removes `find_matches_to_new_records` — the exact primitive D2's new-vs-corpus pass depends on — and also removes implicit caching, `use_cache`, `materialise_blocked_pairs` and salting; `score_pairs` is cartesian and is not a substitute | Blast radius is one module, `src/er/matching/incremental.py`, because nothing outside it calls Splink inference. Migration replaces the two-pass union with `predict_between` + `predict_within`; T-INC-3 (bit-equal scores across paths) and T-BLK-1 (blocking parity) are the acceptance gate for the swap. Do not migrate until both are green on the current pin |
| **Incremental candidate generation cannot pair two pre-existing records.** Two records already in the corpus never become a candidate pair in an incremental run, no matter what changes around them | The periodic correction pass — `er correct` (S4.0) — rebuilds candidates over the full corpus under a new `tf_snapshot_id` and re-scores, at `correction_pass.cadence`; T-CORR-1 builds a link only the full pass can find and asserts the pass finds it. This is a **candidate-generation** limit, not a clustering limit |
| **Corpus-dependent TF shifts move pre-existing pairs across `auto_merge`.** TF values are a property of the corpus, not of the pair | TF is frozen at training time and registered from `tf_lookup` (D4), so within one `tf_snapshot_id` INV-SCORE holds exactly. Drift accumulates only across `tf_snapshot_id` boundaries, is introduced only by the correction pass, and is bounded and measured by T-TF-1 rather than asserted away. T-INC-1b exercises the divergence and its repair |
| Non-deterministic reconciliation breaks T-INC-1 | Every tiebreak is a total order (fragment ranking, cut choice, path choice, survivorship chains all terminating in `record_key ASC`); minting order is an explicit `ORDER BY` on minimum member `record_key`; `IdFactory` is injectable so the reconciler is unit-testable as a pure function |
| DuckDB / extension version skew | dbt-duckdb executes in-process against the installed `duckdb` wheel, so one engine version exists by construction; the pins that matter are extension binaries matched to that build and a DuckDB at or above dbt-duckdb's DuckLake floor. Extensions are baked at Docker build time with `autoinstall_known_extensions=false` at runtime; `er doctor` (T-DOCTOR-1) asserts every S2.1 pin and runs first in the integration job |
| **The object-store image is EOL.** The pinned object-store image receives no further security fixes | Pinned by digest so builds stay reproducible, with an EOL comment in `compose.yaml`; it is a test/dev substrate only and is never part of a deployment. Re-evaluate the substitute at each S2.1 review; the only coupling is the S3 API surface `httpfs` uses, so a replacement is a digest change plus credentials |
| **Concurrent mutating runs.** Two writers against one `tenant` would interleave snapshot ranges, duplicate events, and race `MERGE INTO` on `entity_membership` | The single-writer advisory lock defined in S4.0b: acquired before the first stage, released in a `finally`, and a second run exits 3 without writing a `runs` row. T-CONC-1 asserts the refusal; M1 cannot exit without it. Concurrency is an explicit non-guarantee, not an untested hope |
| **Partial-stage failure.** A stage that fails after committing some snapshots leaves the lake in a valid but incomplete state | Every stage writes its `run_stages` row with `status` and its snapshot range before and after execution, so the failure point is recorded; `er run-all --resume <run_id>` restarts from the first non-`succeeded` stage. Stages that must be atomic (reconcile, the never-cut fixpoint, CONTRADICTION-1) fail **before** committing: no snapshot, no events, non-zero exit. Time travel to `run_stages.snapshot_start` is the rollback unit |
| Label propagation or the never-cut loop fails to converge | Both are bounded by `clustering.max_iterations`; on non-convergence the stage fails with the unconverged component size logged, no snapshot committed and no events emitted — never a silent partial partition |
| Splink intermediates polluting the lake | The primary DuckDB database is `:memory:` with DuckLake attached as `lake` and `output_schema='splink_scratch'`; T-INV-1 and `er doctor` both assert zero `__splink__%` relations in `lake` after every run |
| CI integration flakiness from service startup | Healthcheck-gated `depends_on` for the catalog and completion-gated one-shot initialisers for the object store and the lake (S7.1 — no probe is declared that the image cannot satisfy), `run --rm` rather than `up --abort-on-container-exit`, `down -v --remove-orphans` before and after every run, retries only at startup and never inside a test |
| **The `100k` scale needs a larger GitHub runner that no code change can provision.** `ubuntu-latest-8-cores` (S10.2, S9.2) is a paid, per-repository/organisation runner setting; without it the `100k` dispatch cannot execute inside its declared envelope, so M5's third baseline cannot be produced | It is a stated **M5 precondition** (S12), confirmed before the milestone starts rather than discovered inside it. If the label is unavailable: (a) attach self-hosted hardware carrying that label and the S10.2 `100k` envelope — the S10.4 fingerprint records what was actually measured and the comparability rule polices it, so a correctly sized self-hosted run is a valid baseline; or (b) demote the scale: flip the `100k` row's *Baseline committed* and *Dispatchable* cells in S10.2 to `no` exactly as `1m` already is, drop `100k` from `benchmark.yaml`'s dispatch `options`, and do not commit `benchmarks/baselines/100k.json`. The `--validate-baselines` equality (S9.1) keeps the option list and the baseline set in step, so the PR path stays green at `[smoke, 10k]` and `100k` returns as a one-line change the day the runner appears. Never commit a `100k` baseline measured outside its S10.2 row: S10.4 marks it `NON_COMPARABLE` and `--write-baseline` refuses it, and a baseline taken on the wrong machine silently redefines every later comparison |
| Benchmark numbers not comparable across machines | A per-scale resource envelope (runner, `cpu_limit`, `mem_limit`, `duckdb_memory_limit`) owned by S10.2 and applied by Compose, `ER_DUCKDB_THREADS` / `ER_DUCKDB_MEMORY_LIMIT` applied via `SET` on every connection, `--repeat 3` reporting medians with the coefficient of variation, and an environment fingerprint (runner class, image digest, cgroup `cpu.max` and `memory.max`, `nproc`, DuckDB/Splink versions, seed, scale) embedded in every report. The NON-COMPARABLE rule — the exact conditions, and the bar a run must clear before it may be committed as a baseline — is normative in S10.4 |
