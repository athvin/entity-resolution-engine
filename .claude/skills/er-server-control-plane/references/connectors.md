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

The product is configurable deduplication through APIs. Customers must be able
to configure a dataset, submit records and retrieve useful resolution results
without a Salesforce account. Salesforce is the first commercial integration;
file/S3 and SFTP use the same dataset, configuration and delivery contracts.
OpenAPI is the intended customer-facing description of those contracts, with
its current coverage tracked by the api-contract skill. A connector's external
mutations remain separate from computing or exporting resolution results.

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

Ingress and webhook seams exist in the Rust control plane (`rust/crates/er-server`).
Their existence does not establish a complete public integration contract:
documented configuration, transfer/replay semantics, results and an executable
external-client workflow are product work alongside the Salesforce capabilities.

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

Salesforce is the first commercial integration. Its initial delivery proves the
provider-specific extraction and action boundary against the shared API product.

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
- **Objects: Lead and Contact first** — both map onto the person schema
  (Lead's `Company` carried as metadata). Account model code exists; verify
  the full configuration, resolution and result path before promising Account
  or arbitrary custom-object support. Model breadth is tracked on the live
  board. The extract itself takes any queryable sObject name;
  `IsConverted = false` is applied to Lead by default (spec 1.26).
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

**As built (#993, #1296, #1305).** `POST …/merge-previews/generate` queues a
`merge_preview` job: `materialize_previews` (pod, read-only, one pinned
snapshot) then `publish_previews` (control, one transaction).

- **Checkpoint:** `(runs.snapshot_end, runs.id)` of the last run whose
  previews are durable, advanced by compare-and-set *after* publishing. Only
  succeeded runs with a `snapshot_end` are read, at most 20 per job; a
  re-executed run gets a new `snapshot_end`, so it is read again and the
  fingerprint de-duplicates its previews.
- **Affected entities:** `created`, `merged` and `member_added` events form
  groups (two new records forming a group emit `created`); `split`,
  `member_removed` and `retired` supersede an entity's stale planned previews.
- **Eligibility, on evidence only:** a member joins a group when its latest
  raw row is not deleted, that row's run has `source_delivery_provenance` for
  the member's own source, the source is not delivered by two connections, and
  an active `writeback_policies` row covers the (connection, object). Uploads
  have no provenance and never join.
- **Election:** `er_golden::scoped_masters_sql` — the policy vocabulary of the
  merge-plan export, restricted to the eligible members per (entity, group).
- **v1 limits:** person datasets only; related-record counts are
  `unavailable` (they need a provider read); a golden NULL proposes no change.

### 4.3.1 Delegated authorization (as built, #994)

Writeback acts as a **reviewing Salesforce user who consented**, never as the
extraction's client-credentials identity. `src/sf_oauth.rs` is the OAuth 2.0
web-server flow with PKCE; the routes are under
`/v1/orgs/{org}/connections/{connection_id}/writeback-authorization`:

| Route | Role | Does |
|---|---|---|
| `POST` | Admin | Starts a flow: a single-use `state` (stored only as SHA-256) and a PKCE verifier, valid 10 minutes, returning the Salesforce consent URL. `allow_automatic` (default false) records whether queued or policy-driven writeback may use the grant unattended. |
| `POST …/complete` | Admin | Consumes the state — it must be this org's, this connection's, unexpired, unused and **started by the same principal**; every mismatch is one 409 that does not say which. Exchanges the code with the verifier, then binds the identity service's org and user (both must match the `id` URL, and the instance must be the connection's). Any previous grant is revoked `replaced`. |
| `GET` | Viewer | The latest grant, without tokens. |
| `DELETE` | Admin | Revokes here at once (`requested`) and asks Salesforce to revoke the token, best effort. |

The worker side is `sf_oauth::access_token(client, org, connection, unattended,
egress)`: it refuses unattended use unless the grant allows it, refreshes, and on
Salesforce's `invalid_grant` marks the grant revoked `consent_withdrawn` before
returning, so nothing retries a dead grant. A revoked or absent grant is
`NotAuthorized` — the client credentials are never a fallback.

Secrets: the refresh token and the PKCE verifier are each returned by exactly one
statement (`salesforce_delegated_grant/read/for_use`,
`salesforce_oauth_state/update/consume`), and no response, audit detail or log
line carries one. Errors carry Salesforce's `error` code, never a body. Every
call re-checks the instance URL with the outbound URL guard and never follows a
redirect.

**Provider requirements.** The tenant's connected app must enable OAuth with the
`api` and `refresh_token` scopes, require PKCE, and list each allowed redirect as
a Callback URL. The consenting user needs the object permissions writeback uses
(edit and merge on the object). The same connected app's consumer key and secret
serve both extraction (client credentials) and this flow.

**Deploy configuration.** `ER_SF_OAUTH_REDIRECT_URIS` (chart value
`erserver.salesforceOAuthRedirectUris`) is the comma-separated allowlist; the
start route refuses any other `redirect_uri` with 422, and answers 503 while the
list is empty. The page at the redirect reads `code` and `state` from its query
and posts them to `…/complete` with the same credential that started the flow.

### 4.3.2 Journaled apply (as built, #995)

`POST /v1/orgs/{org}/datasets/{ds}/merge-previews/apply` (Admin; 409 when
nothing is approved) queues a `merge_apply` job: ONE control task,
`apply_merges` (`src/merge_apply.rs`), acting through the connection's delegated
grant (§4.3.1). Control tasks run one at a time, so two apply jobs never
overlap. Per job: at most 25 groups and 100 provider mutations.

| Step | Mechanism |
|---|---|
| `APPROVED → APPLYING` | `merge_apply/create/begin`: a conditional insert under a unique index on the preview — at most once, journaled in the same transaction |
| exact proposal | restored through `MergeProposal::build`; its fingerprint must equal the approved one |
| re-read | `queryAll` of every member's `SystemModstamp`, `IsDeleted`, `MasterRecordId` before any write |
| stale | a moved modstamp → `SKIPPED_STALE`, and an incremental `extract` of the object is queued so the next generation re-scores it |
| external merge | a victim deleted with `MasterRecordId` = master is evidence; all victims so → `APPLIED_EXTERNALLY`. Missing, or merged elsewhere → `FAILED` |
| preimage | written once (`merge_apply.preimage`) before the first mutation |
| operations | `merge_apply_operations`: written IN FLIGHT before each call (master PATCH, then SOAP `merge()` in batches of 2) and settled after |
| crash / lost response | an in-flight operation is resolved by the next run's re-read — proven landed (`verified`), proven not landed (re-attempted), or `FAILED` as unresolvable; never replayed blind |
| rate limits | 429/503 retried 3× with backoff, honouring `Retry-After` up to 30s; otherwise the group stays `APPLYING` and the job stops |
| budget | `REQUEST_LIMIT_EXCEEDED` stops the job; the begun group resumes next run |
| isolation | a refused operation fails its group only; the batch continues |
| cancellation | checked between groups |

A mutation that may have reached Salesforce (timeout, reset, 5xx other than 503)
is never retried in-call. Every transition is on the preview's history
(`GET …/merge-previews/{id}`).

### 4.3.3 Restore (as built, #996)

`POST /v1/orgs/{org}/datasets/{ds}/merge-previews/{preview_id}/restore` (Admin)
records one undo per apply (a repeat request returns it and queues nothing) and
queues a `merge_restore` job (`restore_merges`, `src/merge_restore.rs`); `GET`
on the same path (Viewer) shows its steps and conflicts. 409 unless the apply
merged something. Composition:

| Step | Mechanism |
|---|---|
| resolution | a never-match assertion per (master, victim), staged exactly as the engine's unmerge does, so resolution does not re-propose it |
| victims | SOAP `undelete()` from the Recycle Bin; when the bin no longer has the record, it is recreated from the apply's preimage (system fields dropped) and the step records old id → new id; a victim already live is left alone |
| master | each field the apply wrote is reverted to its preimage value only where Salesforce still holds what the apply wrote; a value already back is left; anything else is a conflict, never overwritten |
| progress | `merge_restore_steps`, in flight before each call; a lost undelete is resolved by the next run's re-read; a recreate with an unknown outcome is a conflict, never retried (it could duplicate the record) |
| outcome | `restored`, `partial` (with conflicts) or `failed`, journaled on the preview |

**Records the merge reparented to the master — opportunities, cases, activities,
attachments — are not moved back.** The view says so (`related_records_reverted:
false`). Salesforce Files archival and audit-driven retry remain #530.

### 4.3.4 Review: manual, batch and policy-driven (as built, #997)

The tenant surface is `/{org}/{ds}/writeback`: previews by state, each with its
master, victims, master field diffs, score, related-count availability, the
apply outcome, the undo (with its conflicts, and the statement that reparented
records are not moved back) and the full history.

| Mode | Route | Rule |
|---|---|---|
| single | `POST …/merge-previews/{id}/approve` · `/reject` | names the fingerprint the reviewer saw; the proposal must still validate under a live policy revision and connection |
| batch | `POST …/merge-previews/decide` (Steward, ≤100) | each item runs through exactly the single-item checks and reports its own status |
| policy | `PUT/GET …/writeback/auto-approval` (Admin / Viewer) | append-only rule per (connection, object): `min_score`, `max_members`, `enabled`; the actor is `policy:<revision>` |

A preview's **score** is weakest-link evidence from the active `match_scores`
edges at its snapshot: for each victim, its strongest edge to another member of
the same group, then the minimum. A victim with no edge (held only
transitively, or never scored) makes the score NULL, and **a NULL score is
never auto-approved**. A rule is evaluated only as a preview is published, in
the same transaction; changing or disabling it never rewrites a decision
already made. Every mode ends in the same apply (§4.3.2), with the same
authorization, staleness and journal.

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
| **API product** | Public configuration, durable JSON/file delivery, jobs and usable results, with an executable customer workflow and compatibility checks. File/S3 is part of this foundation. |
| **Salesforce commercial integration** | Extraction, mapping and polling through the shared workflow; external writeback separately requires preview → approve → apply, authorization, journal, staleness checks and recovery. |
| **SFTP transport expansion** | Scheduled pulls from customer-operated servers into the shared delivery contract; outbound result delivery is separate work. A hosted SFTP endpoint requires its own product decision. |
| **Further expansion** | Additional connectors, lower-latency capture, provider-native UI and resolution-model generalization, prioritized from customer evidence. |

These are product boundaries, not current delivery status or a frozen schedule.
Use the live project board for stack rank and dependencies. AppExchange work
belongs to Salesforce distribution and does not gate the generic API product.

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
