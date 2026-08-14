# Entity Resolution Platform — Technical Specification

**Version:** 1.0 (Draft)
**Stack:** Python · Splink (DuckDB) · dbt-duckdb · DuckLake · Docker Compose · GitHub Actions
**Companion document:** `entity-resolution-design-doc.md` (architecture & guarantees)

---

## 1. Scope & Goals

Build the entity resolution and golden record pipeline described in the design doc as a testable, benchmarkable codebase:

- **G1 — Correctness under test.** Every pipeline stage has automated tests that run in CI on every PR, against realistic fixtures, inside Docker Compose.
- **G2 — Entity permanence proven, not assumed.** CI includes scenario tests that assert entity IDs survive merges, splits, and full re-resolution.
- **G3 — Incremental processing proven.** CI includes a test that processes a base corpus, then an incremental batch, and asserts (a) results match a from-scratch run and (b) the incremental run only touched the affected subgraph.
- **G4 — Benchmarkable.** A benchmark harness measures per-stage throughput at multiple corpus scales. Runnable locally and on demand via CI (`workflow_dispatch`), never on the PR path.

Out of scope for v1: the embedding coherence layer (phase 2, spec'd in §11 as an interface stub), LLM review triage, multi-tenant serving API, Kubernetes manifests (Compose is the test/dev substrate; k8s deployment is a separate spec).

---

## 2. Technology Choices

| Concern | Choice | Notes |
|---|---|---|
| Language | Python 3.12 | Splink is Python-native; Rust ports of hot paths are a later optimization once benchmarks identify them |
| Package/env manager | `uv` | Lockfile-based, fast in CI |
| Matching | Splink ≥ 4.x, DuckDB backend | Pin exact version; Splink 4 API differs substantially from 3.x |
| Transformations | dbt-core + dbt-duckdb | Standardization, blocking keys, golden assembly as dbt models |
| Storage format | DuckLake | Catalog = Postgres in Compose; object store = MinIO (S3-compatible) |
| Query engine | DuckDB (pinned, same version in dbt profile and Python) | Version skew between dbt-duckdb and the Python duckdb package is a real failure mode — pin both to the same DuckDB release |
| Orchestration (v1) | Python CLI (`typer`) invoking stages in order | No Airflow/Dagster in v1; the CLI is the contract, orchestrators wrap it later |
| Containerization | Docker Compose | One image for the pipeline, services for catalog + object store |
| CI/CD | GitHub Actions | Lint → unit → integration (Compose) → dbt tests; benchmark as manual workflow |
| Lint/type | `ruff` (lint+format), `mypy --strict` on `core/` | |
| Testing | `pytest`, `pytest-xdist`, dbt data tests | |
| Config | Pydantic-validated YAML | Per-tenant config: sources, survivorship, thresholds |

---

## 3. Repository Layout

```
entity-resolution/
├── pyproject.toml               # uv-managed; pinned deps
├── uv.lock
├── docker/
│   ├── Dockerfile               # single pipeline image (python + dbt + duckdb)
│   └── compose.yaml             # postgres (catalog), minio, pipeline, benchmark profiles
├── src/er/
│   ├── cli.py                   # `er` CLI: ingest, standardize, match, reconcile, assemble, run-all
│   ├── config/
│   │   ├── schema.py            # Pydantic models for tenant config
│   │   └── loader.py
│   ├── lake/
│   │   ├── ducklake.py          # attach/init DuckLake, snapshot helpers
│   │   └── ddl.py               # table DDL (source of truth for schemas in §5)
│   ├── ingest/
│   │   ├── landing.py           # raw_records upsert, content-hash change detection
│   │   └── sources.py           # source adapters (v1: CSV/Parquet drop folder)
│   ├── matching/
│   │   ├── model.py             # Splink settings builder from config
│   │   ├── train.py             # full-corpus training; writes versioned model JSON
│   │   ├── incremental.py       # candidate gen via blocking_keys + score batch
│   │   └── full.py              # full-corpus predict + cluster
│   ├── entities/
│   │   ├── reconcile.py         # cluster→entity mapping (design doc §6.2)
│   │   ├── ids.py               # ULID minting, redirect resolution
│   │   └── events.py            # entity_events append + replay
│   ├── golden/
│   │   └── assemble.py          # drives dbt golden models for touched entities
│   ├── review/
│   │   └── assertions.py        # apply always/never-match constraints pre-clustering
│   └── embeddings/              # phase 2: interface only in v1
│       └── coherence.py         # scoring interface + no-op impl
├── dbt/
│   ├── dbt_project.yml
│   ├── profiles/profiles.yml    # duckdb profile attaching DuckLake
│   ├── models/
│   │   ├── staging/             # stg_* : per-source standardization
│   │   ├── intermediate/        # int_blocking_keys, int_std_records
│   │   ├── marts/
│   │   │   ├── golden_records.sql
│   │   │   └── golden_lineage.sql
│   │   └── schema.yml           # dbt tests: not_null, unique, relationships
│   ├── macros/
│   │   ├── std/                 # lowercase_trim, email_norm, phone_e164, null_semantics
│   │   └── survivorship/        # rule macros: source_priority, recency, completeness, frequency, validated
│   └── seeds/
│       └── nickname_variants.csv
├── configs/
│   ├── default.yaml             # thresholds, survivorship, blocking rules
│   └── test.yaml                # config used by fixtures/CI
├── fixtures/
│   ├── generator/               # synthetic data generator (also powers benchmark)
│   │   ├── personas.py          # ground-truth person generation
│   │   ├── corruptions.py       # typos, nicknames, format drift, missingness
│   │   └── emit.py              # per-source record emission
│   ├── static/                  # small, hand-authored, committed fixture sets (§8.2)
│   │   ├── base_10/             # 10 persons / 23 records / 3 sources
│   │   ├── incremental_batch/  
│   │   ├── merge_scenario/
│   │   ├── split_scenario/
│   │   └── assertions_scenario/
│   └── expected/                # golden expected outputs per scenario
├── tests/
│   ├── unit/                    # pure-python: normalizers, reconciler, id logic
│   ├── integration/             # full pipeline against Compose services
│   └── conftest.py              # ephemeral DuckLake per test session
├── benchmarks/
│   ├── run_benchmark.py
│   ├── scales.yaml              # 10k / 100k / 1m definitions
│   └── report.py                # JSON + markdown report, run comparison
└── .github/workflows/
    ├── ci.yaml                  # PR path
    └── benchmark.yaml           # workflow_dispatch + optional nightly
```

---

## 4. Pipeline Stages — Component Specs

The CLI is the orchestration contract. Every stage is idempotent and commits exactly one DuckLake snapshot on success.

```
er ingest --source <name> --path <dir>        # → raw_records
er standardize [--changed-only]               # dbt run --select staging intermediate
er train                                      # full-corpus Splink training → model vN
er match --mode incremental|full              # → match_scores (+ candidate pairs)
er reconcile                                  # clusters → entity_membership, entity_events
er assemble [--touched-only]                  # dbt run --select marts (golden records)
er run-all --mode incremental                 # the standard batch: ingest→…→assemble
```

### 4.1 Ingest (`src/er/ingest/`)

- Reads CSV/Parquet from a drop directory per source (v1 adapter; interface allows DB adapters later).
- Computes `content_hash` = SHA-256 over standardization-relevant columns; records with unchanged hash for the same `(source_system, source_record_id)` are skipped and counted.
- Appends to `raw_records` with `ingest_batch_id` (ULID) and `ingested_at`.
- Emits a batch manifest (record counts: new / changed / unchanged) used by tests and the benchmark.

### 4.2 Standardization (dbt staging + intermediate)

- One `stg_<source>` model per source mapping source columns → canonical schema, applying macros: `lowercase_trim`, `email_norm` (validity flag, plus-addressing per config), `phone_e164`, `null_semantics`, `name_norm` (+ nickname variant column from seed), address componentization.
- `int_std_records` unions staged sources; carries `std_version` (from `dbt_project.yml` vars).
- `int_blocking_keys` materializes the blocking key index: `(key_type, key_value, source_system, source_record_id)`, incremental materialization keyed on batch.
- **`--changed-only`:** staging models are incremental on `ingest_batch_id`; only new/changed raw records flow.
- Address parsing note: libpostal is heavyweight to build. v1 uses a regex/usaddress-based parser behind an interface; the fixture generator only emits address patterns the v1 parser handles, and the parser interface allows a libpostal container later without model changes.

### 4.3 Matching (`src/er/matching/`)

- `model.py` builds Splink 4 settings from `configs/*.yaml`: comparisons per attribute (exact / jaro-winkler thresholds / phonetic / null level), TF adjustments on name+email, blocking rules mirrored from `int_blocking_keys` key types.
- `train.py`: full-corpus EM training (`estimate_probability_two_random_records_match`, `estimate_u`, `estimate_parameters_using_expectation_maximisation`), writes `models/model_v{N}.json` + metadata row in `model_registry` table. Training never runs in incremental mode.
- `incremental.py`:
  1. Candidate pairs = batch keys ⋈ `int_blocking_keys` (existing) ∪ batch-internal pairs.
  2. Load frozen model; score candidates via Splink two-frame `link` / `find_matches_to_new_records` pattern.
  3. Persist scored pairs ≥ `review_low` into `match_scores` with `model_version`, `run_id`.
- `full.py`: corpus-wide `predict()` + persist. Used by training runs and the periodic correction pass.
- Threshold bands (`auto_merge`, `review_low`) come from config; pairs in the gray band are written to `review_queue`, not clustered.

### 4.4 Assertions (`src/er/review/assertions.py`)

- Before clustering, edges are adjusted: `never_match` pairs removed regardless of score; `always_match` pairs added with score 1.0 and `evidence='assertion'`.
- Applied identically in incremental and full modes (this is what makes steward corrections survive re-runs).

### 4.5 Clustering + Reconciliation (`src/er/entities/`)

- **Incremental clustering:** connected components over the affected subgraph only — batch records, their scored partners, and those partners' current entity co-members (one membership join). Implemented as iterative label propagation in DuckDB SQL (bounded iterations; assert convergence).
- **Full clustering:** Splink's `cluster_pairwise_predictions_at_threshold`.
- **Reconciliation (`reconcile.py`)** implements design doc §6.2 exactly:
  - unassigned-only cluster → mint ULID;
  - single-entity cluster → extend membership;
  - multi-entity cluster → merge: survivor = most members (tiebreak oldest `created_at`, then lexical ULID for determinism); losers → `merged_into`, redirect row, merge event;
  - split → majority fragment keeps ID (same deterministic tiebreaks), minorities mint IDs + split events.
  - **Determinism requirement:** given identical inputs, reconciliation output is byte-identical. All tiebreaks are total orders. This is load-bearing for the incremental-vs-full equivalence test (§8.3, T-INC-1).
- `ids.py` exposes `resolve(entity_id) -> current_entity_id` following redirect chains (with cycle guard).

### 4.6 Golden Assembly (dbt marts + `src/er/golden/`)

- `golden_records.sql` applies survivorship per attribute via macros dispatched from config: rule order per attribute, e.g. email = [validated, source_priority, recency].
- `golden_lineage.sql` emits one row per (entity, attribute): contributing record, winning rule, `survivorship_version`.
- `assemble.py --touched-only` computes touched entity set from this run's membership/event deltas and passes it as a dbt var; the marts filter to those entities (incremental materialization, `delete+insert` by `entity_id`).
- Presentation transforms (proper-case display columns) live in a separate `golden_display` model so matching-layer data is never re-cased.

---

## 5. Data Model (DDL summary)

`src/er/lake/ddl.py` is the source of truth; dbt `schema.yml` mirrors constraints as tests. Key tables (all DuckLake):

```sql
raw_records(source_system, source_record_id, payload JSON, content_hash,
            ingest_batch_id, ingested_at,
            PRIMARY KEY (source_system, source_record_id, content_hash))

std_records(source_system, source_record_id, std_version,
            given_name, family_name, name_variants LIST,
            email, email_valid BOOL, phone_e164,
            addr_number, addr_street, addr_unit, addr_city, addr_region, addr_postal,
            birth_date, birth_date_precision, updated_at_source,
            ingest_batch_id)

blocking_keys(key_type, key_value, source_system, source_record_id)

match_scores(run_id, model_version, rec_a, rec_b, match_probability,
             evidence JSON, scored_at)          -- rec_* = (source_system, source_record_id)

entity_membership(source_system, source_record_id, entity_id, assigned_at, run_id)

entities(entity_id, status, merged_into, created_at, updated_at)
         -- status ∈ {active, merged, retired}

entity_events(event_id, entity_id, event_type, details JSON, run_id, occurred_at)
         -- event_type ∈ {created, member_added, member_removed, merged, split, retired}

assertions(rec_a, rec_b, kind, created_by, created_at)   -- kind ∈ {always, never}

review_queue(rec_a, rec_b, match_probability, waterfall JSON, status, run_id)

golden_records(entity_id, <canonical attributes…>, survivorship_version, assembled_at)
golden_lineage(entity_id, attribute, source_system, source_record_id, rule, assembled_at)

model_registry(model_version, trained_at, corpus_snapshot, params_path, metrics JSON)
```

---

## 6. Configuration (`configs/*.yaml`)

Pydantic-validated; invalid config fails fast at CLI start. Shape:

```yaml
tenant: test
thresholds:
  auto_merge: 0.95
  review_low: 0.60
standardization:
  email_strip_plus_addressing: false
sources:
  crm:      { adapter: csv, priority_rank: 1 }
  billing:  { adapter: csv, priority_rank: 2 }
  webforms: { adapter: csv, priority_rank: 3 }
blocking:
  - { key_type: email_exact,        expr: "email" }
  - { key_type: phone_exact,        expr: "phone_e164" }
  - { key_type: name_postal,        expr: "substr(family_name,1,4) || '|' || addr_postal" }
  - { key_type: dob_name,           expr: "birth_date || '|' || substr(given_name,1,3)" }
comparisons:
  given_name:  { levels: [exact, jaro_winkler:0.9, variant_match, null] , tf: true }
  family_name: { levels: [exact, jaro_winkler:0.9, null], tf: true }
  email:       { levels: [exact, username_exact, null], tf: true }
  phone_e164:  { levels: [exact, null] }
  birth_date:  { levels: [exact, year_month, null] }
  addr_postal: { levels: [exact, null] }
survivorship:
  email:       [validated, source_priority, recency]
  phone_e164:  [validated, source_priority, recency]
  given_name:  [source_priority, frequency, completeness]
  family_name: [source_priority, frequency, completeness]
  address:     [recency, source_priority]
  birth_date:  [frequency, source_priority]
```

---
## 7. Docker Compose Environment

`docker/compose.yaml` defines the test/dev substrate. Profiles: `test` (CI path) and `bench`.

```yaml
services:
  catalog:                # DuckLake catalog database
    image: postgres:16
    environment: { POSTGRES_DB: ducklake, POSTGRES_USER: er, POSTGRES_PASSWORD: er }
    healthcheck: { test: ["CMD-SHELL", "pg_isready -U er"], interval: 2s, retries: 30 }

  objectstore:            # S3-compatible storage for DuckLake data files
    image: minio/minio
    command: server /data --console-address ":9001"
    environment: { MINIO_ROOT_USER: er, MINIO_ROOT_PASSWORD: erpassword }
    healthcheck: { test: ["CMD", "mc", "ready", "local"], interval: 2s, retries: 30 }

  objectstore-init:       # create bucket
    image: minio/mc
    depends_on: { objectstore: { condition: service_healthy } }
    entrypoint: ["/bin/sh","-c","mc alias set local http://objectstore:9000 er erpassword && mc mb -p local/lake"]

  pipeline:
    build: { context: .., dockerfile: docker/Dockerfile }
    profiles: [test]
    depends_on:
      catalog: { condition: service_healthy }
      objectstore-init: { condition: service_completed_successfully }
    environment:
      ER_CATALOG_DSN: postgresql://er:er@catalog/ducklake
      ER_S3_ENDPOINT: http://objectstore:9000
      ER_CONFIG: /app/configs/test.yaml
    command: ["pytest", "tests/integration", "-x", "-q"]

  benchmark:
    build: { context: .., dockerfile: docker/Dockerfile }
    profiles: [bench]
    depends_on:
      catalog: { condition: service_healthy }
      objectstore-init: { condition: service_completed_successfully }
    environment: { ER_CATALOG_DSN: postgresql://er:er@catalog/ducklake, ER_S3_ENDPOINT: http://objectstore:9000 }
    command: ["python", "benchmarks/run_benchmark.py", "--scale", "${BENCH_SCALE:-10k}"]
    deploy: { resources: { limits: { cpus: "4", memory: 8g } } }   # pin resources → comparable numbers
```

- Single `Dockerfile` (python 3.12-slim, `uv sync --frozen`, dbt + duckdb pinned). Multi-stage: builder installs, runtime copies venv. Image also serves as the future k8s job image.
- `dbt` runs inside the pipeline container against a profile that ATTACHes DuckLake using `ER_CATALOG_DSN` / `ER_S3_ENDPOINT`.
- Local dev: `docker compose --profile test up --abort-on-container-exit` = exactly what CI runs.

---

## 8. Testing Strategy

### 8.1 Layers

| Layer | Runs | Substrate | Scope |
|---|---|---|---|
| Static | every PR | GH runner | ruff, mypy --strict (core), dbt parse + `dbt compile` |
| Unit | every PR | GH runner (no services) | normalizer macros via duckdb-in-memory, reconciler, ID/redirect logic, config validation |
| Integration | every PR | Docker Compose | full `er run-all` against fixtures; scenario tests below |
| dbt data tests | every PR (inside integration) | Compose | uniqueness/not-null/relationships + custom singular tests |
| Benchmark | manual / nightly | Compose `bench` profile | throughput at scale; regression comparison |

### 8.2 Static fixtures (`fixtures/static/`)

Hand-authored, small, committed, with ground truth. Each scenario = input CSVs per source + `expected/` outputs (membership crosswalk + golden records as CSV).

- **`base_10`** — 10 ground-truth persons, 23 records across `crm`, `billing`, `webforms`. Includes: nickname pair (robert/bob), typo'd surname (jaro-winkler level), shared household (same address, different persons — must NOT merge), missing emails, format-drifted phones, a placeholder email (`test@test.com` → null).
- **`incremental_batch`** — 6 new records: 3 join existing entities, 2 form a new entity, 1 bridges two existing entities (forcing a merge during incremental).
- **`merge_scenario`** — batch that causes a 2-entity merge; expected: survivor ID per deterministic rules, redirect row, merge event.
- **`split_scenario`** — an assertion (`never_match`) that severs a previously-merged entity; expected: majority keeps ID, minority gets new ID, split event.
- **`assertions_scenario`** — `always_match` pair below threshold + `never_match` pair above threshold; expected memberships honor assertions in both incremental and full modes.

Ground-truth fixtures also carry `persona_id`, letting tests compute precision/recall of matching itself.

### 8.3 Scenario tests (integration, the ones that encode the guarantees)

- **T-STD-1** Standardization determinism: run `er standardize` twice; `std_records` byte-identical.
- **T-MATCH-1** On `base_10`, pairwise precision = 1.0 and recall ≥ 0.9 vs persona ground truth at `auto_merge`; household non-merge holds.
- **T-INC-1 (G3)** Full-run on base+incremental combined vs base-run-then-incremental-run: final `entity_membership` partitions are identical (entity IDs may differ between the two universes; compare as set-partitions), and golden record *values* identical.
- **T-INC-2 (G3)** During the incremental run, count of golden records rewritten == touched entities (assert untouched entities' `assembled_at` unchanged).
- **T-PERM-1 (G2)** `merge_scenario`: loser ID still resolves via `ids.resolve()` to survivor; `entity_events` contains exactly one merge event; survivor chosen per deterministic rule.
- **T-PERM-2 (G2)** `split_scenario`: original ID retained by majority fragment; new ID minted for minority; events recorded.
- **T-PERM-3 (G2)** Full re-resolution (`er match --mode full && er reconcile`) after incremental history: no entity ID changes for unchanged partitions (reconciliation owns IDs).
- **T-ASSERT-1** Assertions honored in both modes; a full re-run does not undo them.
- **T-GOLD-1** Survivorship: for each attribute in `base_10` expected outputs, winning value and `golden_lineage.rule` match expectations (covers source_priority, recency, validated, frequency paths).
- **T-SNAP-1** DuckLake time travel: query golden_records at pre-incremental snapshot; matches pre-incremental expected output.
- **T-IDEM-1** Re-running `er run-all` on an already-processed batch is a no-op (manifest reports 0 new; no new snapshots with data changes).

### 8.4 Unit test highlights

- Property-based tests (`hypothesis`) for normalizers: idempotence (`f(f(x)) == f(x)`), casing invariance, E.164 round-trips.
- Reconciler tested as a pure function on synthetic (cluster, membership) inputs covering all four §6.2 branches + tiebreaks + redirect chains + cycle guard.
- Config: invalid survivorship rule name, unknown source, overlapping thresholds → validation errors.

---

## 9. CI/CD (GitHub Actions)

### 9.1 `ci.yaml` — PR path

```yaml
name: ci
on:
  pull_request:
  push: { branches: [main] }
jobs:
  static:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: astral-sh/setup-uv@v4
      - run: uv sync --frozen
      - run: uv run ruff check . && uv run ruff format --check .
      - run: uv run mypy src/er
      - run: uv run dbt parse --project-dir dbt --profiles-dir dbt/profiles
  unit:
    runs-on: ubuntu-latest
    needs: static
    steps:
      - uses: actions/checkout@v4
      - uses: astral-sh/setup-uv@v4
      - run: uv sync --frozen
      - run: uv run pytest tests/unit -n auto -q
  integration:
    runs-on: ubuntu-latest
    needs: unit
    steps:
      - uses: actions/checkout@v4
      - uses: docker/setup-buildx-action@v3
      - name: Build pipeline image (cached)
        uses: docker/build-push-action@v6
        with: { context: ., file: docker/Dockerfile, load: true,
                tags: er-pipeline:ci,
                cache-from: type=gha, cache-to: type=gha,mode=max }
      - name: Run integration suite
        run: docker compose -f docker/compose.yaml --profile test up
             --abort-on-container-exit --exit-code-from pipeline
      - name: Collect artifacts (dbt logs, test junit, run manifests)
        if: always()
        uses: actions/upload-artifact@v4
        with: { name: integration-artifacts, path: artifacts/ }
```

- Integration job includes dbt data tests (`dbt build` runs inside the suite).
- Branch protection on `main`: all three jobs required.
- Image build cache (GHA cache) keeps the integration job fast; target < 10 min total PR path.

### 9.2 `benchmark.yaml` — manual/nightly

```yaml
name: benchmark
on:
  workflow_dispatch:
    inputs:
      scale: { type: choice, options: [10k, 100k, 1m], default: 10k }
  schedule:
    - cron: "0 6 * * 1"        # weekly, 10k scale, optional — can be disabled
jobs:
  bench:
    runs-on: ubuntu-latest      # document runner specs in the report; upgrade to larger runner for 1m
    steps:
      - uses: actions/checkout@v4
      - run: docker compose -f docker/compose.yaml --profile bench up
             --abort-on-container-exit --exit-code-from benchmark
        env: { BENCH_SCALE: ${{ inputs.scale || '10k' }} }
      - uses: actions/upload-artifact@v4
        with: { name: bench-${{ inputs.scale || '10k' }}-${{ github.sha }}, path: artifacts/bench/ }
      - name: Compare vs baseline
        run: uv run python benchmarks/report.py --compare artifacts/bench/latest.json benchmarks/baselines/${{ inputs.scale || '10k' }}.json --fail-threshold 1.25
```

- Never on the PR path (G4). `--fail-threshold 1.25` turns the scheduled run into a soft regression alarm: fail if any stage is >25% slower than the committed baseline.
- Baselines are committed JSON, updated deliberately via PR when a slowdown/speedup is accepted.

---

## 10. Benchmark Harness

### 10.1 Data generation (`fixtures/generator/`)

Deterministic (seeded) synthetic corpus generator, shared by fixtures and benchmarks:

- `personas.py` generates ground-truth persons (names from weighted frequency lists so TF adjustments have realistic skew, emails, phones, addresses, DOBs).
- `corruptions.py` applies per-source corruption profiles: typo rate, nickname substitution, missingness per field, format drift, stale addresses.
- `emit.py` emits per-source CSVs with configurable duplication factor (records per persona per source).
- Because ground truth (`persona_id`) is known, benchmarks report **match quality (precision/recall/F1) alongside speed** — a perf change that silently hurts recall must be visible.

### 10.2 Scales (`benchmarks/scales.yaml`)

| Scale | Personas | Records | Incremental batch |
|---|---|---|---|
| `10k` | 4,000 | 10,000 | 100 |
| `100k` | 40,000 | 100,000 | 1,000 |
| `1m` | 400,000 | 1,000,000 | 10,000 |

### 10.3 Measured phases & metrics

Per run, `run_benchmark.py` executes and times:

1. ingest (base corpus)
2. standardize (dbt)
3. train (Splink EM)
4. match full + cluster + reconcile (cold start)
5. assemble (all entities)
6. **incremental cycle**: ingest batch → standardize --changed-only → match incremental → reconcile → assemble --touched-only

Metrics per phase: wall time, records/sec, peak DuckDB memory (`duckdb_memory()`), candidate-pair count, and for phase 6 the **incremental ratio** = incremental cycle time / cold-start time (the headline number proving G3 economics; expect it to fall as corpus grows). Plus quality: precision/recall/F1 vs personas at `auto_merge`.

Output: `artifacts/bench/latest.json` + human-readable markdown table; `report.py --compare` diffs two runs.

### 10.4 Interpretation rules

- Numbers are comparable only within the pinned Compose resource limits (§7) on the same runner class; the report embeds runner specs, image digest, DuckDB/Splink versions, seed, and scale.
- The benchmark answers: max sustainable batch cadence, memory sizing per scale (feeds future k8s pod sizing), and which stage to optimize first (candidate Rust ports).

---

## 11. Phase 2 Stub — Embedding Coherence (interface only in v1)

`src/er/embeddings/coherence.py` defines the interface so the pipeline has the seam without the dependency:

```python
class CoherenceScorer(Protocol):
    def score_clusters(self, entity_ids: list[str]) -> list[ClusterCoherence]:
        """Return per-entity dispersion score + outlier member ids."""
```

- v1 ships `NoopScorer`. Phase 2 adds a sentence-transformers implementation writing embeddings to a DuckLake column and using DuckDB VSS; low-coherence entities feed `review_queue`.
- The reconcile stage already calls the scorer hook for touched entities, so phase 2 is a config flip, not a pipeline change.

---

## 12. Milestones

| Milestone | Contents | Exit criteria |
|---|---|---|
| M1 — Skeleton | repo, Dockerfile, Compose, DuckLake init, CLI stubs, CI static+unit green | `er run-all` no-ops end-to-end in Compose |
| M2 — Standardize | dbt staging/intermediate, macros, fixtures `base_10`, T-STD-1 | standardization tests green |
| M3 — Match + reconcile | Splink model, incremental & full match, reconciler, T-MATCH-1, T-PERM-*, T-ASSERT-1 | permanence scenario tests green |
| M4 — Golden | survivorship macros, lineage, T-GOLD-1, T-SNAP-1, T-INC-* | full PR CI path green < 10 min |
| M5 — Benchmark | generator scales, harness, baselines, `benchmark.yaml` | 10k + 100k baselines committed; incremental ratio reported |
| M6 (phase 2) | embedding coherence impl | design doc addendum first |

---

## 13. Risks & Mitigations

| Risk | Mitigation |
|---|---|
| Splink 4 API churn | pin exact version; wrap all Splink calls behind `matching/` so upgrades touch one module |
| DuckDB version skew (dbt-duckdb vs python duckdb vs DuckLake extension) | single pinned DuckDB version enforced in CI (`er doctor` command asserts versions match) |
| Incremental clustering misses transitive links | by design; periodic full pass (design doc §7.3) + T-INC-1 bounds the incremental/full gap on fixtures |
| Non-deterministic reconciliation breaks T-INC-1 | total-order tiebreaks everywhere; determinism unit tests |
| CI integration flakiness from service startup | healthcheck-gated `depends_on`; retries only at startup, never inside tests |
| Benchmark numbers not comparable across machines | pinned Compose resources + environment fingerprint embedded in every report |