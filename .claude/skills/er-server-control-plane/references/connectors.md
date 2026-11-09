# Connectors: the agnostic ingress/egress layer

*How client systems get data into the resolution engine and merge decisions back
out — without the engine ever learning about any specific system. This is the
design for the connector seam named in backend-design.md §10 (`backend-design.md`)
and the plan for the first concrete connector, Salesforce.*

---

## Contents

- [1. Principle: producers and appliers at the edges of an unchanged engine](#1-principle-producers-and-appliers-at-the-edges-of-an-unchanged-engine)
- [2. The connector contract](#2-the-connector-contract)
- [3. The seams that already exist](#3-the-seams-that-already-exist)
- [4. The Salesforce connector (v1)](#4-the-salesforce-connector-v1)
- [5. Baseline connectors](#5-baseline-connectors)
- [6. Roadmap](#6-roadmap)
- [References](#references)

## 1. Principle: producers and appliers at the edges of an unchanged engine

The engine (architecture.md) is source-agnostic by construction:
it ingests tabular deliveries from a drop directory, change-detects them by content
hash, and emits golden records, lineage and `entity_events`. A connector lives
entirely at the two edges of that pipeline and the engine core is never modified to
accommodate it:

- **Ingress (a producer):** extract from a client system, land Parquet/CSV
  deliveries in the tenant's drop dir, and contribute one `sources:` config entry
  per object. The engine's existing ingest adapters, content-hash change detection
  and tombstones do the rest.
- **Egress (an applier):** consume engine outputs (`entity_events`, golden records,
  merge-plan) and apply them to the client system through a small, explicit verb
  set, governed by policy.

> **The engine decides truth; the connector decides action.** Merges happen in the
> lake as a consequence of the data; writeback treats those lake merges as
> *proposals for external action, governed by writeback policy*. This split is what
> keeps the core CRM-agnostic and the blast radius of any one connector contained.

"Salesforce-first" (the market) does not mean "Salesforce-coupled" (the
architecture). Salesforce is the first implementation of a contract that Marketo,
generic file/S3, and future systems implement the same way.

## 2. The connector contract

A connector is defined against the control-plane **connection** resource
(`/v1/orgs/{org}/connections`, backend-design.md §6 (`backend-design.md`)). A
connection is `{type, credential ref, object→source mapping}`, and the contract the
control plane promises a connector is exactly four things — nothing else:

1. Materialize deliveries into the tenant drop dir.
2. Contribute a `sources:` config entry (the mapping UI compiles directly into one).
3. Submit jobs with idempotency keys.
4. Consume `/events` webhooks.

A full connector implements six capabilities:

| # | Capability | What it does |
|---|---|---|
| 1 | **Auth** | Provider flow (OAuth for Salesforce); tokens in the secret store; health surfaced on the connection resource. |
| 2 | **Schema discovery** | Objects (standard + custom), fields, approximate counts; feeds field mapping and field analysis. |
| 3 | **Initial full sync** | Bulk extract per object → delivery files; one engine source per object, with `record_id_column` and `updated_at_column` set from the provider's identity/modified fields. |
| 4 | **Incremental capture** | Poll from a persisted watermark: changes → deliveries, deletions → tombstone files (the engine's retraction path handles the rest). CDC/webhooks are a per-connector upgrade. |
| 5 | **Writeback** | A small verb set — `merge_group(master, victims, master_field_updates)`, `upsert(records, external_id)`, later `convert` — driven only by the apply service. |
| 6 | **Limits & health** | API-budget tracking, backoff, poll cadence derived from provider limits. |

Capabilities 1–4 are ingress; 5 is egress; 6 spans both. A connector may ship
ingress-only first (insights mode) and add writeback later — the contract is the
same.

## 3. The seams that already exist

Two halves of the seam are built and running in the control plane
(`server/`); the Salesforce-specific capabilities (§4) are the work that remains.

### 3.1 Ingress — the drop-dir push seam

`POST /v1/orgs/{org}/datasets/{ds}/imports` (`upload_import` in `er-server`) is the
generic ingress endpoint every connector targets:

- The `source` is validated against the org's **published** config before any byte
  lands on disk — a draft-only source `422`s with "publish a config version
  containing `sources.<name>` first", rather than failing later as the run's exit-2.
- Upload, JSON push, S3 polling and Salesforce extraction publish closed staging
  files through `er-server::delivery::publish`. Each accepted input lives at
  `{drop_root}/deliveries/{delivery_id}.csv`, accompanied by a versioned JSON
  manifest. Files and directory entries are flushed before enqueue. Publication
  never replaces an existing identity; a conflicting retry is refused.
- The queued job carries that manifest in `params.delivery`, alongside `source`
  and `delivery_id`. It pins the dataset, byte length/digest and source mapping
  digest (including column order). The runner reads only that input and refuses
  missing bytes, altered bytes, changed mappings or a mismatched dataset.
  Accepting B cannot change the bytes read by queued A.
- A size ceiling (`MAX_IMPORT_BYTES`) guards the push path; bulk history is synced
  into the drop dir directly rather than through the HTTP upload.

A connector's ingress is therefore "authenticate, extract, map to a `sources:`
entry, push files (or write them to the drop dir), enqueue." No engine change.

#### Replay, old jobs and retention

Accepted files are retained after success and failure; this change introduces no
cleanup policy. Do not remove them while a queued job, retry or operator replay
can still need them. The Salesforce/S3 cursor advances only after publication
and durable enqueue. An enqueue failure leaves the manifest and CSV intact;
retrying publication under the same identity is safe. To recover an orphaned
upload, first check whether its consumer exists, then re-upload the retained CSV
under the original source mapping. The original manifest records the mapping
digest for this check. Salesforce and S3 can retry without advancing the cursor.

Ingest retries use the existing §9.1 pre-image queries: delete only this run's
raw rows and receipts in one lake transaction, then replay its pinned delivery
set. This keeps a lost completion report from creating duplicate rows/receipts.
The separate per-delivery append/tombstone transaction work is tracked in #957;
immutable inputs and pre-imaging do not replace that atomicity requirement.

An old delivery job carrying `source`/`delivery_id` without `params.delivery` is
refused: its original bytes cannot be inferred from a mutable legacy source file.
Re-deliver its original input or re-extract from the last known safe watermark;
do not retry it against the newest file and assume it was the same delivery.
A nonempty old Salesforce extract report also refuses cursor advancement.

An operator run-all **without a delivery identity** retains the explicit legacy
`{source}.csv` directory convention; it does not discover accepted manifests or
choose the newest accepted input. A publish-triggered rebuild uses `skip_ingest`
to operate on the already-landed corpus. The CLI's explicit `--drop` convention
is unchanged. Rolling back to an older binary while pinned jobs remain queued
would lose this guarantee; drain them or stop dispatch before rollback.

### 3.2 Egress — signed webhooks

`er-server`'s webhook handlers deliver control-plane events (today
`job.completed`) to subscribed URLs. Delivery is best-effort (a few retries, never
blocks the queue) and each payload is signed with the subscription secret so
receivers can authenticate the sender. The signature rides in the
`X-ER-Signature` header as `sha256=<hex>` (GitHub-style): the receiver
recomputes HMAC-SHA256 over the **raw request body** with its subscription
secret and compares against the hex after the `sha256=` prefix — a receiver
that hex-compares the whole header value fails on the prefix, and one that
re-serializes the JSON before hashing fails on the bytes. A retry delivers the
*same* bytes the original attempt did, because the body is built and stored
once (`webhook.rs`'s `emit`) and the signature is computed over that stored
body — so a redelivery re-validates with no special-casing. This is how a
connector learns "a run finished, new merges are available to apply" without
polling.

### 3.3 Writeback — designed, not yet built

The apply service (§4.3) is specified here and in backend-design.md §10 but is not
yet implemented — it is the central delta from today's read-only product and the
reason the Salesforce connector is the first real build on this seam.

The first implementation boundary is `er-server/src/writeback.rs` (#992): a
pure, immutable proposal builder. It validates member scope and preimages,
computes allowlisted field diffs, and fingerprints canonical content. It accepts
an already elected master; `er-golden::master` remains the election owner, and
the caller must restrict election to one connection/object group before passing
it here. A cross-source export master is not necessarily eligible for writeback.
Missing golden attributes mean no update; explicit null can propose a clear.
The fingerprint identifies content, not authorization or current provider state.
Persisted preview/approval (#993), delegated user authorization (#994), journaled
apply (#995), undo (#996), review/auto-approval (#997), and upsert (#998) remain
separate work. This helper alone enables no external writes.

## 4. The Salesforce connector (v1)

Salesforce is the first concrete connector because it is the first market
(research-cloudingo.md). v1 proves the whole contract end
to end.

> **Salesforce as a login provider is a separate thing.** Human SSO via Salesforce
> OIDC (frontend-design §2.4) authenticates *users*; the Salesforce *connector*
> moves *data*. They share a vendor, not a code path.

### 4.1 Ingress — BUILT (#512), with one recorded deviation

The extraction half is shipped: the `salesforce_connection` entity and its
`/v1/orgs/{org}/connections` CRUD (admin; the secret is never listed, echoed
or audited), the `extract` job kind (schedulable), the Bulk API 2.0 client
(`er-server/src/salesforce.rs`, proven against a mock in
`tests/salesforce_mock.rs`), and the `sf_extract_cursor` watermark. The pod
stages the result as an ordinary drop-root CSV delivery in the source's
declared column order (`extract_params::source_csv_columns`, shared with
`push_records`); the control half enqueues `run_all_incremental` and advances
the cursor, publish-then-advance like the S3 poller.

- **Auth — deviates from the original sketch, deliberately.** This section
  used to specify the web-server flow; what shipped is **OAuth 2.0
  client-credentials** (`auth_kind: client_credentials`, the only kind v1
  accepts). An extract job is headless: a web-server flow means a redirect
  URI, a refresh token and a user present, none of which a scheduled pod has.
  One connection = one Salesforce org; sandbox orgs work the same way (their
  My Domain is the `instance_url`). There is still **no secret store**: the
  client secret is a plaintext column under the `webhook.secret` discipline
  (no `history: true`, one credential-returning statement that only the
  work-order resolver calls), and the credential reaches the pod on the work
  order, resolved per attempt with the instance URL re-checked against the
  outbound-URL guard.
- **Objects: Lead and Contact first** — both map cleanly onto the person schema
  (Lead's `Company` carried as metadata). Account is second, blocked on the
  organization entity type (backend-design.md §13). The extract itself takes
  any queryable sObject name; `IsConverted = false` is applied to Lead by
  default (spec 1.26).
- **Initial sync:** Bulk API 2.0, extracting **only mapped fields** (this minimizes
  our data footprint and is part of the security answer). Each object becomes one
  engine source (`sf_contact`, `sf_lead`) with `record_id_column = Id`,
  `updated_at_column = SystemModstamp`. The mapping UI's output compiles directly
  into a `sources:` config block entry.
- **Change capture: polling, not CDC, for v1** — a schedule on the `extract`
  kind running `SystemModstamp > watermark`. **Deletions do not propagate
  yet, on any mode**: `getDeleted` is not built, and the executor's ingest
  always lands base deliveries (`stage_ingest` passes `full_refresh: false`
  unconditionally), so even a `mode: full` extract upserts rather than
  tombstones. Both halves — the `getDeleted` poll and a full-refresh landing
  mode — are one follow-up ticket. CDC is the later upgrade for a
  near-real-time tier.

### 4.2 The convergence loop

Because ingress is file-based and deletion-aware, writeback needs no special
reconciliation code: **an applied merge is seen by the next poll as victim
deletions → tombstones → engine retraction.** The loop closes itself — a direct
consequence of the append-only + tombstone design.

### 4.3 The apply service (writeback)

The engine merges entities *in the lake*; Salesforce merges records *in Salesforce*.
The apply service is the bridge:

- **MergePlan generation:** after each reconcile+assemble, diff `entity_events`
  merge events since the last apply watermark. For each merged entity spanning
  multiple records of one Salesforce object: elect a master (a configurable
  **master-election policy** — e.g. the record closest to golden values, tie-break
  oldest), and compute `master_field_updates` = golden values that differ from the
  master's current values, respecting a per-field writeback allowlist.
- **Two-step apply per group:** (1) update the master's fields to golden values,
  (2) merge victims into it. Salesforce `merge()` takes at most one master + two
  victims per call, so larger groups apply as sequential merges into the same
  master. Each merge natively reparents attachments, notes and opportunities.
- **Preview / dry-run:** every plan is a materialized, reviewable resource (master,
  victims, field diffs, related-record counts). Auto-merge policy = auto-approve
  plans matching tenant policy by object and score band.
- **Journal:** per-group states `PLANNED → APPROVED → APPLYING → APPLIED | FAILED |
  SKIPPED_STALE | APPLIED_EXTERNALLY`, with pre-merge victim field snapshots (these
  power restore). Failed groups never block the batch. If victims are already gone,
  someone merged manually → `APPLIED_EXTERNALLY` (idempotent).
- **Staleness guard:** if a member's `SystemModstamp` moved since our sync snapshot,
  skip and requeue (re-sync, re-score) by default.

### 4.4 Explicit v1 boundaries

- Writeback via `merge()` (Lead/Contact/Account) and `upsert`.
- **No Apex, no managed package.** Tenants keep Salesforce native Duplicate Rules
  for entry-time blocking; we own bulk/fuzzy/cross-object dedupe.
- A synchronous `/check` endpoint (score one candidate against the frozen model + TF
  snapshot, callable from a Flow) is a later differentiation-phase feature, not v1.

## 5. Baseline connectors

Two connectors fall out of the generic seam with little or no provider-specific work
and should exist alongside Salesforce:

- **File upload** — the import wizard (backend-design.md §11): upload → profile →
  column-map → preview → commit. This is the drop-dir push seam (§3.1) with a UI.
- **S3 / object-store drop** — point a connection at a bucket prefix; deliveries are
  synced into the drop dir on a schedule. No provider API at all.

These also serve as the reference implementation of the contract for any future
connector.

## 6. Roadmap

| Phase | Scope |
|---|---|
| **Insights (ingress only)** | Salesforce OAuth + Bulk sync + polling; Lead/Contact; merge-plan **export** as the action bridge. Zero write risk. |
| **Writeback** | The apply service (§4.3): preview → approve → apply, journal, staleness guard; manual/mass/auto tiers; undo/restore. |
| **Continuous** | 1–5 min micro-batch polling marketed as near-real-time (latency stated honestly). |
| **Breadth** | Marketo (same contract; SF↔Marketo cross-system dedupe falls out of the engine's multi-source model); generic file/S3 as first-class; custom objects once the entity-type generalization lands (backend-design.md §13). |

## References

- backend-design.md §6 (`backend-design.md`) — the `connections` resource and the
  four-part connector contract.
- backend-design.md §10 (`backend-design.md`) — connector abstraction and the apply
  service (the normative source for this doc).
- backend-design.md §13 (`backend-design.md`) — the object-model generalization that
  gates Account/custom objects.
- `er-server` `upload_import` — the `POST .../imports` drop-dir ingress seam.
- `er-server` `create_webhook` / `webhook_deliveries` — the signed-webhook egress seam.
- research-cloudingo.md — why Salesforce is first.
