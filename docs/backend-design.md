# Backend design: a multi-tenant SaaS on the ER engine

*A high-level design for a FastAPI backend with background job execution that turns the existing entity-resolution engine into the backend of a direct [Cloudingo](research-cloudingo.md) competitor. This is a design document — no code, no API payload schemas. Written 2026-09-25.*

> **Implementation status (2026-09-25).** Phases 1–3 of §15 are built and
> tested — see [server/README.md](../server/README.md). Engine side:
> `src/er/service.py` (typed orchestration facade: run-all, correct, train)
> and the per-tenant env overlay in `src/er/lake/env.py`
> (`lake_environment`). Server side (`server/`, package `erserver`): jobs
> queue with per-org serialization by unique index, exit-code-keyed retries,
> cancel/resume, startup reaper, and live per-stage heartbeats streamed off
> the engine's S5.2 stderr records; API-key auth with roles + operator token
> + audit log; config versions with tier classification and publish (which
> finally executes `correction_pass.cadence` as a schedule); cron schedules;
> the snapshot-pinned read path (golden records, duplicate groups, entity
> detail with lineage and events, runs, metrics); steward writes with
> lock-conflict staging and dispatcher draining; imports (the drop-dir
> connector seam); signed webhooks; merge-plan export. Verified by three
> suites, including a full E2E that drives a real lake (Postgres catalog +
> MinIO) entirely through the API and dispatcher. Deliberately not built
> (needs external systems or is §15 phase 4–5): the Salesforce connector and
> writeback, the k8s Job launcher, OIDC, STS per-tenant credentials, FTS
> search, Marketo, non-person entity types.

---

## 1. Context and product thesis

Cloudingo is a Salesforce dedupe SaaS ($2,500–$10,000/yr per org) whose feature bar is documented in [research-cloudingo.md](research-cloudingo.md): filter-based duplicate detection, a merge grid (manual/mass/auto merge), survivorship rules, scheduled + real-time automation, undo/restore, dedupe-aware import, search and field analysis, dashboards, and team permissions.

We already have the hard part — a proven, incremental entity-resolution engine (this repo). What we don't have is any serving layer: the engine is a batch CLI with, by explicit design, "no serving API or built-in scheduler" ([architecture.md](architecture.md)). This document designs that missing layer.

**Decisions taken:**

- **Multi-tenant SaaS** (Cloudingo's model: licensed per org).
- **CRM-agnostic core, Salesforce connector first** — the engine is source-agnostic; we keep that as the long-term moat while competing head-on where Cloudingo lives.
- **Insights-first, writeback fast-follow** — v1 ships a read-only "audit your org" product (find duplicates, show evidence, export merge plans); writing merges back into a customer's production CRM is the highest-blast-radius feature and follows immediately after, not eventually.

**Where we structurally exceed Cloudingo** (the pitch, not aspiration — all measured or built):

- Probabilistic ML matching (Splink): 96.4% cluster precision / 99.2% recall at 100k, vs hand-built deterministic filters.
- Cross-source resolution as the native model: Salesforce Leads + Contacts + Marketo + CSVs resolve into one entity; Cloudingo dedupes within one object.
- Full audit trail: per-attribute `golden_lineage`, `entity_events` (merge/split/retire), DuckLake time travel — point-in-time answers Cloudingo cannot give.
- Durable steward knowledge: always/never assertions are retractable and reapplied on every reconcile, vs one-shot merge decisions.
- Proven incremental scale: 10M records; +100k on a 10M lake in ~2m50s. Cloudingo surcharges above 300k records.
- A credible self-hosted option later, which answers the "external app, data leaves the org" objection better than Cloudingo can.

## 2. What the engine gives us vs what's missing

**Assets (exist today, need only orchestration or an API veneer):**

| Asset | Where |
|---|---|
| Pipeline stages as clean library functions (connection + `Config` + `StageRun` in, typed result out): ingest, standardize, train, match full/incremental, reconcile, assemble, correction chain | `src/er/ingest/landing.py`, `src/er/std/stage.py`, `src/er/matching/{full,incremental,train}.py`, `src/er/entities/reconcile_stage.py`, `src/er/golden/assemble.py` |
| Merge-grid primitives: gray-band review queue (`open_reviews`/`resolve_review`) and always/never pair assertions (`add_assertion`/`retract_assertion`) — the merge/not-duplicate/undo mechanism | `src/er/review/queue.py`, `src/er/review/assertions.py` |
| Job-ready run ledger: `runs`, `run_stages` (typed counters, snapshot ranges), `ingest_batches` receipts, stage-level resume, config-drift guard, idempotent re-execution per stage, exit-code taxonomy (0/1/2/3/10) | `src/er/obs/runctx.py`, `src/er/resume.py`, `src/er/versions.py`, `src/er/errors.py` |
| Incremental correctness machinery: content-hash change detection, tombstones, frozen model + TF snapshots, affected-set reconcile, correction pass to repair drift | DesignDoc S4.3–S4.5, `src/er/matching/correction.py` |
| Tenancy seam: `tenant` is a first-class config block keying a per-tenant Postgres advisory writer lock; isolation unit is the (metadata schema, data path) pair | `src/er/lake/catalog.py`, `configs/default.yaml` |

**Missing (what this design adds):** an HTTP API, a background job system, a scheduler (config declares `correction_pass.cadence` but nothing executes it), a Python read layer (golden records are raw SQL over dbt marts), connectors, and any writeback path. Also one honest engine gap: the canonical schema is person-centric (§13).

**The two constraints that shape everything:**

1. **One writer per tenant**, enforced by a session-scoped Postgres advisory lock; a second writer exits 3. Stages are synchronous and minutes-to-hours long (1M full ≈ 93s; 10M full ≈ 1h46m).
2. **Memory envelope up to ~56 GiB** at large scale, with process-global `ER_*` env configuring the lake connection.

## 3. Feature-parity matrix

Classification: **ENGINE** (exists, needs an API veneer) · **EXT** (engine extension) · **NEW** (net-new product work) · **CONN** (lives in the connector).

| Cloudingo capability | Our delivering feature | Class | Honest gap |
|---|---|---|---|
| Filters (prebuilt + custom, per object) | Match-config service: templates compiling to `blocking`/`comparisons`/`thresholds` config blocks, with a publish workflow (§6) | ENGINE + NEW | Matching is stronger (probabilistic) but there's no edit API, and edits trip drift guards. One model per tenant per entity type, not independent per-object rule sets. |
| Merge grid (manual/mass/auto) | Duplicate-groups API (entity-centric read model) + assertions + apply service | ENGINE + NEW | Engine review queue is pair-centric gray-band only; groups ≥0.95 auto-merge silently in the lake. The grid needs a new group read model. |
| Preserves attachments/notes/opportunities | Salesforce `merge()` natively reparents children | CONN | Free once writeback exists; we never touch child records. |
| Survivorship rules | `survivorship` config block (ordered rule chain per attribute) + new master-election policy | ENGINE + NEW | Engine picks field *values*, not a master *record*; Salesforce merge needs a master ID (§10). |
| Scheduling | `schedules` table + dispatcher cron (§5) | NEW (thin) | Engine is incremental-ready; only the scheduler is new. |
| Real-time | Continuous tier: 1–5 min polling micro-batches | NEW + CONN | **Minutes, not seconds.** Stated plainly; entry-time `/check` endpoint is a later phase. |
| Undo & restore | Never-assertion → reconcile split (engine) + writeback journal snapshots + SF Recycle Bin restore | ENGINE + NEW | Child-record reparenting done by SF merge is not automatically reversed; the product says so. |
| Import wizard | Upload → profile → map → preview → commit as ingest + incremental run | NEW + EXT | Preview needs a **score-only match mode** (score a file against the corpus without mutating entities) — an engine extension. |
| Find data | Search API over standardized columns + `golden_display` | NEW (thin) | Standardization (email/phone/name normalization) is the moat here. |
| Field analysis | Profiling job per source: fill rate, distinct counts, top values | NEW (thin) | Simple aggregates over staged data. |
| Reporting/dashboard | KPI endpoints over `runs`/`run_stages`/`entity_events`/`review_queue` counters | NEW (thin) | All raw material exists. |
| API integrations / Marketo | The engine's native multi-source shape; Marketo = second connector | ENGINE + CONN | We start cross-source; Cloudingo bolts it on. |
| Team access | Roles enforced at the API layer (§9) | NEW | — |
| "Data never stored nor cached" | **Cannot be claimed.** We persist a per-tenant lake by design | — | Positioning inversion, not a feature gap: persistence *buys* lineage, time travel, audit, and undo; self-hosted later. Risk R5 (§16). |

## 4. System architecture

**Three deployables, two container images, all sharing the `er` package as a library.**

| Deployable | Image | Replicas | Memory | Role |
|---|---|---|---|---|
| `er-api` (FastAPI/uvicorn) | `er-api` (slim, no dbt) | N stateless, autoscaled | 2–4 GiB | Control-plane CRUD, read path over DuckLake, steward writes, auth. **Never executes pipeline stages.** |
| `er-dispatcher` | `er-api` | 1, leader-elected | <1 GiB | Queue polling, per-tenant serialization, cron, k8s Job launch/watch, zombie detection. Never touches DuckDB. |
| `er-runner` | `er-pipeline` (existing, with dbt) | one k8s Job pod per pipeline run | tier-sized 8–64 GiB | Executes exactly one run via the `er/service.py` facade (§12), heartbeats progress, exits. |

**Why a k8s Job per run, not long-lived workers:**

- The engine's tenant seam is process-scoped: `src/er/lake/env.py` is the single reader of the `ER_*` env set. One pod per (tenant, run) makes env injection the isolation mechanism instead of a hazard.
- The advisory lock is session-scoped (`src/er/lake/catalog.py` holds a dedicated Postgres connection for the run's lifetime). A pod that dies releases the lock automatically — the crash-safety property the engine already documents. Long-lived workers multiplexing tenants would have to reimplement that care.
- Memory bin-packing: pods sized per tenant tier are the only sane answer to a 56 GiB envelope; idle 56 GiB long-lived workers are dead money, and an OOM kill's blast radius is one tenant's one resumable run.
- The `er-pipeline` image is already described in `docker/Dockerfile` as "later the k8s job image" — this is the intended landing.

The API stays responsive trivially because it never runs stages: it imports `er` only for config validation, the read-API module, and sub-second steward primitives. **Non-k8s fallback** for single-VM/self-hosted deployments: the dispatcher forks the runner as a subprocess with the same env-injection contract.

## 5. Job orchestration

**Principle: the control plane complements, never duplicates, the in-lake ledger.** `runs`/`run_stages` remain ground truth for execution (counters, snapshot ranges, resume state). A control-plane `jobs` table adds only what the ledger cannot know: existence before a run starts, and cross-tenant coordination.

`jobs` (control-plane Postgres): `job_id` (ULID), `tenant_id`, `kind` (`run_all_full` | `run_all_incremental` | `correct` | `train` | `lake_maintain` | `provision` | `steward_apply`), `params`, `state` (`queued → dispatching → running → succeeded | failed | canceled | superseded`), `priority`, `resource_class`, `idempotency_key`, `run_id` (FK into the tenant lake's `runs`, set at dispatch), `attempt`/`max_attempts`, `not_before`, `exit_code`, `error_class`, `outcome` (incl. `no_op`), `progress` (heartbeat cache), `schedule_id`, `created_by`.

**Queue technology: Postgres `SELECT … FOR UPDATE SKIP LOCKED`. No broker.** Per-tenant serialization is mandatory anyway and is a transactional constraint — a partial unique index on `tenant_id WHERE state IN ('dispatching','running')` makes double-dispatch impossible by construction. The queue and the engine's advisory lock live in the same Postgres, so they can never disagree about who is writing. Volume is hundreds of jobs/day — three orders of magnitude below where Redis/Celery earns its operational cost — and Celery's at-least-once redelivery is actively wrong for an engine that deliberately never auto-retries.

**Retry policy, keyed exactly to the engine's exit-code taxonomy** (`src/er/errors.py`):

| Runner exit / event | error_class | Policy |
|---|---|---|
| 0 | — | succeeded |
| 10 (nothing-to-do) | — | succeeded, `outcome=no_op`; coalesces identical queued jobs |
| 2 (config) | config | fail permanently; surface the config pointer |
| 1 + `transient_io` | transient_io | auto-retry ≤3 with backoff, re-dispatched as `--resume RUN_ID` (existing stage-level resume; same run_id, attempt++) |
| 1, other | stage failure | fail; human-triggered `:resume` |
| 3 + `lock_conflict` | lock_conflict | requeue with backoff, no attempt consumed (dispatcher serializes, so this means a zombie or out-of-band CLI run) |
| 3, other (schema drift, pending correction) | precondition | fail permanently + operator alert |
| Pod OOM / node lost | infra | retry with `--resume` if `runs` shows progress; escalate resource class one tier on second OOM |

**Dedup and coalescing:** clients send `Idempotency-Key` on job submission (replay returns the original job). A newly enqueued job identical to a still-queued one (`tenant, kind, params_hash`) is marked `superseded` pointing at it.

**Scheduling:** a `schedules` table (tenant, kind, cron, params, enabled) ticked by the dispatcher leader. On config publish, the `correction_pass.cadence` cron string the tenant config *already declares* is synced into a schedule row — the control plane finally executes what the config has always promised. System schedules: weekly `er lake maintain` per tenant, catalog backups.

**Cancellation:** `:cancel` → dispatcher deletes the k8s Job (SIGTERM) → the runner's cooperative cancel hook (§12) stops at the next stage boundary → the job is resumable. Honest limitation: mid-stage cancel is kill-and-resume-at-stage-boundary; stages are synchronous and dbt runs as a subprocess.

## 6. API resource model

All tenant-scoped routes under `/v1/orgs/{org}/…`.

| Resource | Verbs | Notes |
|---|---|---|
| `/orgs` | operator CRUD | tenant lifecycle (§7) |
| `/connections` | CRUD + `:test` | the connector seam. A connection = {type, credential ref, object→source mapping}. Contract with connectors: they (a) materialize deliveries into the tenant drop dir, (b) contribute a `sources` config entry, (c) submit jobs with idempotency keys, (d) consume `/events` webhooks. Nothing else is promised. |
| `/config`, `/config/versions` | GET active; POST draft; `:publish`; diff | versioned in the control plane (yaml, `config_hash`, author); publish states its consequence (see below) |
| `/jobs` | POST (Idempotency-Key required); GET list/detail; `/logs`; `:cancel`; `:resume` | detail embeds a `run_stages` projection from the heartbeat cache — the hot polling path never opens DuckDB |
| `/duplicates` | GET list/detail | duplicate groups = multi-member entities + gray-band candidates; detail is the merge-grid data: members, side-by-side fields from lineage, score evidence |
| `/reviews` | GET open; `:resolve` | wraps `open_reviews`/`resolve_review`; returns `pending_until_next_reconcile: true` |
| `/assertions` | POST always/never; retract | the merge / not-duplicate / undo primitive; "undo this merge" resolves members from `entity_events` and posts never-assertions; optional `apply=now` enqueues a reconcile job |
| `/golden-records` | GET search/list/{id}; `/lineage`; `/history`; `?as_of=` | snapshot-pinned pagination (§8); `/records/{source}/{id}` resolves a source record to its entity |
| `/imports` | POST (presigned upload → delivery → job); GET receipt | receipts map to `ingest_batches` |
| `/events`, `/webhooks` | GET feed; subscription CRUD | sourced from `entity_events` + job transitions; delivery with retries |
| `/metrics` | GET | dashboard KPIs, cached per snapshot |

**Config exposure — the 14 blocks split by consequence:**

- **User-editable (admin role, via templates, never raw YAML):** `thresholds`, `survivorship`, `standardization`, `blocking` (curated rule templates), `comparisons` (templated levels), `correction_pass` cadence, `sources` (indirectly, via connections).
- **Operator-only:** `training`, `storage`, `versions`, `clustering`, `coherence`, `generator`, `tenant` — these change model semantics, cost envelope, or physical layout.

**Publish workflow with cost tiers.** Any edit changes `config_hash`, which the incremental drift guard refuses — so publishing is a first-class workflow, not a file save. The diff is classified:

- **Tier A** (minutes): `thresholds`, `survivorship` order → re-assemble / re-band only.
- **Tier B** (match rebuild, no retrain): `blocking` keys.
- **Tier C** (retrain + full rebuild, scheduled with a cost estimate from `runs` history): `comparisons`, `standardization`, source column mappings.

Publish shows the tier and estimated runtime, then enqueues the required run with escalation allowed — turning the drift guard from a footgun into the audit spine linking config versions to the runs ledger.

## 7. Multi-tenant isolation

**Topology: shared Postgres cluster with one dedicated catalog *database* per tenant (`CREATE DATABASE er_t_{org}`, carrying `ER_LAKE_METADATA_SCHEMA=t_{org}` inside it), shared S3 bucket with a per-tenant prefix (`ER_LAKE_DATA_PATH=s3://er-lake/t_{org}/`).** The engine's own definition of a tenant is the (metadata schema, data path) pair; the SaaS wraps that unit in a database of its own per tenant — hard isolation was chosen over schema-per-tenant so that no tenant's tables ever share a database with another's, and each tenant gets database-granular backup/restore and a one-statement purge (`DROP DATABASE … WITH (FORCE)`). The org's `ER_CATALOG_DSN` is derived from `ERSERVER_TENANT_DSN_TEMPLATE` and rides in its env overlay like any other `ER_*` value; the metadata schema stays per-tenant even inside the dedicated database, so a stale or mis-derived DSN surfaces as an empty namespace rather than silently attaching another tenant's catalog. The engine's advisory writer lock is taken on the tenant's own database, which keeps the single-writer guarantee and makes cross-tenant lock-key collisions impossible. Dedicated bucket + STS-scoped credentials become an enterprise-tier option. Per-tenant Postgres *instances* still don't pay for themselves at $2.5k–10k/yr — this is many databases in one cluster, not many clusters.

- **Control-plane DB** (separate database, same cluster): orgs, users, api_keys, config_versions, jobs, schedules, webhooks, idempotency keys, audit_log, staged_steward_actions.
- **Secrets:** connector and per-tenant storage credentials live in a secrets manager, referenced by ID, injected into runner pods as env at dispatch. MVP uses one service credential with engine-enforced prefix isolation; a hardening phase adds STS-scoped per-tenant credentials as defense in depth.
- **Noisy neighbors / the 56 GiB problem:** orgs carry a `resource_class` — S (≤100k records, 8 GiB), M (≤1M, 24 GiB), L (≤10M, 64 GiB, dedicated node pool). The class sets pod requests/limits **and** `ER_DUCKDB_MEMORY_LIMIT`/`ER_DUCKDB_THREADS`, with per-class concurrency caps so L jobs can't starve the cluster.
- **Lifecycle** (implemented through `active`; `POST /v1/orgs` without a `config_path` runs the whole provision step): provision (org row in state `provisioning` → seed config template as version 1 → issue one-time admin key → `provision` job creates the dedicated database and runs `er init` → dispatcher flips the org `active`) → active → suspended (schedules off, writes rejected, reads allowed) → purge (optional export, drop database, delete prefix, delete secrets; 30-day grace). Jobs other than `provision` are refused (409) while the org is not `active`.

## 8. Read path

**Serve reads directly from read-only DuckDB attaches in the API pods. No Postgres serving copy in v1.** DuckLake already provides MVCC — a read-only attach sees the latest committed snapshot and is never blocked by the single writer. A sync-to-Postgres mart would add a freshness pipeline and a second schema to drift, solving a problem the storage layer already solved.

- A new `er/readapi/` module (§12) attaches per tenant using explicit settings (not process env), with a small per-pod LRU of warm attaches; DuckDB `memory_limit` kept low (~2 GiB, 2 threads) — API pods are not compute nodes.
- **Snapshot-pinned keyset pagination:** page 1 captures the current snapshot; the cursor embeds `(snapshot_id, last_key)`; later pages query `AT (VERSION => snapshot_id)`. A run committing between pages cannot shear results. The snapshot ID also keys the response cache, so invalidation is exact and free.
- Filter/sort columns are validated against the relation registry in `src/er/lake/model.py` — no string-built SQL from user input.
- `?as_of=` history endpoints reuse the same time-travel mechanism; `entity_events` supplies the merge/split narrative.
- **Explicit coupling:** `er lake maintain` snapshot expiry must retain snapshots longer than the maximum cursor/cache lifetime (cursors expire ≤1h; retention ≥24h).
- **Pre-designed escape hatch, not built:** if fuzzy "find data" search outgrows queries over the normalized columns, sync `golden_display` to Postgres FTS per tenant. That is the only case that earns a serving copy.

## 9. AuthN/AuthZ

- **Humans:** OIDC (hosted provider) → short-lived JWT; org membership and role resolved from the control plane per request, not trusted from token claims alone.
- **Machines:** org-scoped API keys (hashed at rest), role-scoped — what connectors and CI use.
- **Roles:** `admin` (config publish, connections, members), `steward` (reviews, assertions, job submit), `viewer` (read-only), plus platform-internal `operator` (operator-only config blocks, train/correct/resume, maintenance). Maps directly to Cloudingo's "team access."
- **Enforcement:** one middleware resolves principal → org → role and injects a `TenantContext` (org, role, lake-settings handle) as a FastAPI dependency; every engine call flows through it, making cross-tenant access structurally impossible rather than per-handler discipline. Isolation is schema-level, so no row-level security is needed.
- **Audit:** a control-plane audit_log for every authenticated mutation pairs with the lake-side `entity_events` for a complete "who merged what, when" story.

## 10. Connector abstraction and Salesforce v1

**Principle: connectors are producers and appliers at the edges of an unchanged engine.** Extraction lands Parquet files in the tenant's drop dir — the engine's existing ingest adapters, content-hash change detection, and tombstones do the rest. Writeback consumes engine outputs. The engine never learns about Salesforce.

A connector implements six capabilities:

1. **Auth** — provider flow (OAuth for SF); tokens in the secret store; health surfaced on the connection resource.
2. **Schema discovery** — objects (standard + custom), fields, approximate counts; feeds field mapping and field analysis.
3. **Initial full sync** — bulk extract per object → Parquet drops; one engine source per object (`sf_contact`, `sf_lead`) with `record_id_column = Id`, `updated_at_column = SystemModstamp`. The mapping UI's output **compiles directly into a `sources:` config block entry** — the existing per-source mapping mechanism is exactly the right seam.
4. **Incremental capture** — poll from a persisted watermark: changes → drops, deletions → tombstone files (the engine's retraction path handles the rest). CDC/webhooks are a declared per-connector upgrade.
5. **Writeback** — a small verb set: `merge_group(master, victims, master_field_updates)`, `upsert(records, external_id)`, later `convert`. Driven only by the apply service, never by the engine.
6. **Limits & health** — API budget tracking, backoff, poll cadence derived from provider limits.

### The apply service (writeback) — the critical delta from today's read-only pipeline

The engine merges entities *in the lake*; Cloudingo merges records *in Salesforce*. The bridge treats lake merges as **proposals for external action, governed by policy** — the engine decides truth, writeback policy decides action.

- **MergePlan generation:** after each reconcile+assemble, diff `entity_events` merge events since the last apply watermark. For each merged entity spanning multiple records of one SF object: elect a master (one configurable **master-election policy** — e.g., record closest to golden values, tie-break oldest), and compute `master_field_updates` = golden values differing from the master's current values, respecting a per-field writeback allowlist.
- **Two-step apply per group:** (1) update the master's fields to golden values, (2) merge victims into it. Salesforce `merge()` takes at most 1 master + 2 victims per call, so larger groups apply as sequential merges into the same master. Each SF merge natively reparents attachments, notes, and opportunities — which is precisely how Cloudingo delivers that bullet.
- **Preview/dry-run:** every plan is a materialized, reviewable resource (master, victims, field diffs, related-record counts). Auto-merge policy = auto-approve plans matching tenant policy by object and score band.
- **Journal:** per-group states `PLANNED → APPROVED → APPLYING → APPLIED | FAILED | SKIPPED_STALE | APPLIED_EXTERNALLY`, with pre-merge victim field snapshots (powers restore). Failed groups never block the batch. Idempotency: victims already gone means someone merged manually → `APPLIED_EXTERNALLY`.
- **Staleness guard:** if a member's `SystemModstamp` moved since our sync snapshot, skip and requeue (re-sync, re-score) by default.
- **The convergence loop closes itself:** an applied merge is seen by the next poll as victim deletions → tombstones → engine retraction. No special reconciliation code — a direct consequence of the append-only + tombstone design.

### Salesforce connector v1 scope

- OAuth 2.0 connected app (web-server flow); one connection = one org; sandbox connections supported.
- **Objects: Lead and Contact first** (both map cleanly onto the person schema; Lead's `Company` carried as metadata); Account second, blocked on the organization entity type (§13).
- Initial sync via Bulk API 2.0, only mapped fields extracted (minimizes our data footprint — part of the security answer).
- **Change capture: polling, not CDC, for v1** — `SystemModstamp > watermark` + `getDeleted`, every 1–5 minutes. Polling matches the micro-batch cadence the ~40s incremental cycle supports, needs no streaming infrastructure, and burns few API calls. CDC is the later upgrade for the near-real-time tier.
- Writeback via `merge()` (supports exactly Lead/Contact/Account) and upsert.
- **No Apex, no managed package in v1.** Tenants keep Salesforce native Duplicate Rules for entry-time blocking; we own bulk/fuzzy/cross-object. A later synchronous `/check` endpoint (score one candidate against the frozen model + TF snapshot, callable from a Flow) is a differentiation-phase feature.
- Marketo: deferred; same abstraction, and SF↔Marketo cross-system dedupe then falls out of the engine's multi-source model.

## 11. Product features on the engine

- **Match-config service ("Filters" + "Rules"):** named templates ("Same email", "Fuzzy name + postal") compile to config-block edits, flowing through the tiered publish workflow (§6). Prebuilt templates are mostly Tier A/B.
- **Duplicate-groups API (the merge grid's backend):** entity-centric read model — groups with members, per-field side-by-side values from lineage, score band, suggested golden values; actions merge (always-assertion), not-duplicate (never-assertion), skip. The pair-centric gray-band queue remains the "needs human judgment" subset within it. Mass merge = bulk assertions + one reconcile job; approval flows = named proposal sets gating the assertion batch (and, later, apply).
- **Import wizard:** upload → profile → column-map (compiles a transient `sources` entry) → **preview via score-only match mode** (engine extension: score a candidate file against the corpus without mutating entities) → commit as ingest + incremental run → optional upsert writeback. The preview report is itself a sellable artifact ("your file contains 1,214 records you already have").
- **Find data / field analysis / dashboards:** thin query APIs (§3); dashboards from small aggregate views over `runs`/`run_stages`/`entity_events` refreshed post-run, so "data-quality progress over time" is a chart, not a claim.
- **Undo/restore:** one "unmerge" action composing never-assertion(s) → reconcile split (engine, exists) → SF restore via Recycle Bin undelete + journal-snapshot fallback + master field revert (new). The product states plainly what is not reverted (child reparenting).
- **Automation tiers (tenant-facing):** *Scheduled* (daily/weekly run-all), *Continuous* (1–5 min micro-batch polling, marketed as near-real-time with the latency stated), *Auto-apply* (writeback without approval, per object and score band).

## 12. Engine changes workstream (deliberately small)

1. **`er/service.py` facade** extracted from `src/er/cli.py`'s orchestration sequence (`GlobalOptions.resolve` → writer lock → schema preflight → `RunContext` → stages → exit mapping, ~150–250 lines): `execute_chain(settings, config, chain, run_id, resume_id=None, hooks=None) → RunOutcome` with typed error classes instead of exit codes. The CLI becomes a thin caller; the runner calls this. The single biggest unblocking change.
2. **`LakeSettings` parameterization** of `src/er/lake/env.py` — an explicit settings object accepted by `connect()` / `invocation_session()` / `tenant_lock()`, env as fallback. env.py is documented as the one reader of `ER_*`, so this is a one-seam change — and without it the API cannot safely serve two tenants from one process.
3. **Progress + cancellation hooks:** `RunContext` accepts a stage-transition callback and a `should_cancel()` checked between stages; the runner wires heartbeats and SIGTERM to them.
4. **`er/readapi/` module:** registry-typed queries, keyset pagination, snapshot pinning, `AT (VERSION …)`.
5. **Steward write facade:** thin lock+connect wrappers over `open_reviews` / `resolve_review` / `add_assertion` / `retract_assertion` taking `LakeSettings` (primitives exist).
6. **Runner result line:** one machine-readable terminal JSONL event (outcome, error_class, run_id) for the dispatcher; per-stage JSONL already exists.

Plus one product-driven extension: **score-only match mode** for import preview (§11).

**Explicit non-goals:** no async engine, no long-lived DuckDB service, no dbt-subprocess replacement, no storage rework.

## 13. Object-model generalization — the biggest engine-side gap

The canonical schema is person-centric end to end (given/family name, email, phone, birth date, address; the standardization models, comparison library, and survivorship attributes all assume it). Arbitrary-object support is a real engine workstream, not configuration. Staged honestly:

1. **Person only — Lead + Contact dedupe.** This is Cloudingo's core volume use case, and cross-object Lead↔Contact resolution (both as sources of one person entity) is a differentiator we get for free — Cloudingo treats it as a separate convert workflow.
2. **A second hardcoded entity profile — "organization" (Accounts):** new attribute set (name, domain, billing address, phone), new staging models and comparisons, org-appropriate survivorship — run as a **separate engine namespace per (tenant, entity_type)** so the engine's single-entity-type assumption is untouched. Bounded, well-understood work.
3. **Schema-configurable entity types (custom objects):** an attribute registry per type driving generated models and comparison config, with per-type training. The Enterprise-tier unlock ("unlimited custom objects"); gates nothing earlier.

## 14. Observability and ops

- **Progress:** the runner heartbeats stage transitions + counters into `jobs.progress` at stage boundaries plus a periodic liveness ping. API polling reads the cache; `run_stages` stays ground truth; missed heartbeats mark the job lost and apply retry policy (zombie detection for free). SSE later; polling first.
- **Logs:** the engine's per-stage stderr JSON lines, shipped from pods tagged `tenant/job/run/stage`; `/jobs/{id}/logs` proxies.
- **Metrics:** API RED, queue depth and dispatch latency, per-class concurrency, stage durations by tier, retry/no-op rates, per-tenant data-quality series feeding `/metrics`.
- **Recurring system jobs:** correction pass per tenant cadence, weekly `er lake maintain` (with the snapshot-retention/cursor coupling from §8), catalog backups.
- **Alerting:** permanent job failures, assertion contradictions, schema-preflight and drift-guard refusals — the latter surfaced in-product as "this change requires a full run — run now?".

## 15. Roadmap

| Phase | Scope | Why |
|---|---|---|
| **1 — Walking skeleton** | Control-plane DB (orgs, keys, config as uploaded YAML), `er/service.py`, `jobs` + dispatcher + k8s Job runner for run-all full/incremental, heartbeat status, one pilot tenant | Proves the riskiest seam end-to-end: job-per-pod running the unmodified sync engine under the advisory lock inside a tier-sized memory envelope |
| **2 — Read + steward** | `LakeSettings`, read-API module, golden/duplicates/reviews/assertions endpoints, snapshot-pinned pagination, roles | Makes the product demoable against Cloudingo's merge grid — the buying moment |
| **3 — Automation + connector seam → MVP launch** | Schedules, retry/resume/idempotency, imports (preview report), events/webhooks, config versioning + publish tiers; **Salesforce connector in insights mode** (OAuth, Lead+Contact, bulk sync, polling); dashboards; merge-plan CSV export | The smallest sellable wedge: a zero-risk "audit your org" with visibly better match quality, priced under Cloudingo Standard, with the export as the action bridge |
| **4 — Parity: writeback** | Apply service (preview → approve → apply, journal, staleness guard), manual/mass/auto tiers, undo/restore, import commit + upsert, Accounts (organization entity type), field analysis, continuous micro-batch tier, per-tenant STS creds, SOC2 groundwork | Exit criterion: a Cloudingo Standard/Professional customer switches without losing a workflow (accepting minutes-not-seconds real-time) |
| **5 — Differentiation** | Cross-object Lead↔Contact resolution surfaced (with convert writeback), Marketo + generic file/S3 sources, custom objects, synchronous `/check` for entry-time dedupe, time-travel/audit reporting, per-tenant model tuning, self-hosted option | Attacks where Cloudingo structurally can't follow |

## 16. Hardest problems and risks

**Technical (each answered in the design):**

1. **Synchronous, memory-heavy stages vs a responsive multi-tenant API** → topology: the API never executes stages; job-per-pod with tier-sized memory; heartbeat cache so polling never opens DuckDB; OOM blast radius is one resumable run (§4, §5).
2. **Single-writer lock vs steward interactivity** — a 10M full run holds the tenant lock ~1h46m. → Steward writes try inline with a short lock timeout (common case sub-second); on conflict they land in `staged_steward_actions`, drained by a micro-job when the lock frees. Semantically honest: assertions only take effect at the next reconcile anyway, so the UI truthfully shows "queued, applies after the current job" (§9, §5).
3. **Process-global `ER_*` env vs one API process serving many tenants** → the `LakeSettings` change, confined to the single documented env seam; runner pods keep env injection unchanged (§12).
4. **Consistent reads and pagination while writers commit snapshots** → DuckLake MVCC + snapshot-pinned cursors + snapshot-keyed caching, with the `lake maintain` retention coupling stated (§8).
5. **User-editable config vs the config-hash drift guard** → versioned publish that classifies cost tier, states the consequence, and enqueues the run with escalation — the guard becomes the audit spine (§6).

**Product:**

- **R1 — Insights-first must clear budget.** Cloudingo buyers buy *merging*. Mitigation: merge-plan CSV export in the MVP; writeback is the first fast-follow, not a distant phase.
- **R2 — Survivorship vs Salesforce merge semantics.** SF merge keeps the master's values, so field-level survivorship needs the update-master-then-merge two-step — doubling failure modes and creating a brief inconsistent window. Needs an early spike against a real sandbox.
- **R3 — "Minutes" as the real-time story.** The incremental cycle plus serialized per-tenant writes caps us at micro-batch; buyers comparing tier sheets will see "real-time" on Cloudingo Professional and ask.
- **R4 — Lead+Contact-only MVP vs evaluations demanding Account dedupe on day one.** The organization entity type may need pulling forward into late Phase 3.
- **R5 — Persistence vs "never stored".** We persist CRM data (the lake, plus victim snapshots for restore) where Cloudingo says "never stored" and native rivals say "never leaves the org". The lineage/undo/time-travel framing must land with security reviewers, or self-hosted moves up the roadmap.

---

## Appendix: engine seams this design builds on

| Seam | Path | Used by |
|---|---|---|
| CLI orchestration sequence to extract as `er/service.py` | `src/er/cli.py` | runner (§12) |
| `ER_*` env reader to parameterize as `LakeSettings` | `src/er/lake/env.py` | API read path, runner (§12) |
| Session/connection + snapshot machinery | `src/er/lake/ducklake.py` | read path (§8) |
| Per-tenant advisory writer lock | `src/er/lake/catalog.py` | dispatcher, steward writes (§5, §16) |
| Relation registry (authoritative schema map) | `src/er/lake/model.py` | read-API validation (§8) |
| Review queue + always/never assertions | `src/er/review/queue.py`, `src/er/review/assertions.py` | merge grid, undo (§11) |
| Per-source column mapping (`sources` block) | `configs/default.yaml`, `src/er/config/schema.py` | connector mapping compilation (§10) |
| Run ledger, resume, drift guard, exit codes | `src/er/obs/runctx.py`, `src/er/resume.py`, `src/er/versions.py`, `src/er/errors.py` | job orchestration (§5) |
| Cloudingo feature bar | `docs/research-cloudingo.md` | parity matrix (§3) |
