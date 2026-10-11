# Connectors: the agnostic ingress/egress layer

*How client systems get data into the resolution engine and merge decisions back
out — without the engine ever learning about any specific system. This is the
design for the connector seam named in [backend-design.md §10](backend-design.md)
and the plan for the first concrete connector, Salesforce.*

---

## 1. Principle: producers and appliers at the edges of an unchanged engine

The engine ([architecture.md](architecture.md)) is source-agnostic by construction:
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
(`/v1/orgs/{org}/connections`, [backend-design.md §6](backend-design.md)). A
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

`POST /v1/orgs/{org}/imports?source=<name>` (`server/src/erserver/api.py`) is the
generic ingress endpoint every connector targets:

- The `source` is validated against the org's **published** config before any byte
  lands on disk — a draft-only source `422`s with "publish a config version
  containing `sources.<name>` first", rather than failing later as the run's exit-2.
- The delivery is written to `{drop_root}/{source}/{delivery_id}.{ext}` and an
  incremental run is enqueued. The runner's `adapter_for` reads the same source
  config the import validated against.
- A size ceiling (`MAX_IMPORT_BYTES`) guards the push path; bulk history is synced
  into the drop dir directly rather than through the HTTP upload.

A connector's ingress is therefore "authenticate, extract, map to a `sources:`
entry, push files (or write them to the drop dir), enqueue." No engine change.

### 3.2 Egress — signed webhooks

`server/src/erserver/webhooks.py` delivers control-plane events (today
`job.completed`) to subscribed URLs. Delivery is best-effort (a few retries, never
blocks the queue) and each payload is signed with the subscription secret
(HMAC-SHA256 in `X-ERServer-Signature`) so receivers can authenticate the sender.
This is how a connector learns "a run finished, new merges are available to apply"
without polling.

### 3.3 Writeback — designed, not yet built

The apply service (§4.3) is specified here and in backend-design.md §10 but is not
yet implemented — it is the central delta from today's read-only product and the
reason the Salesforce connector is the first real build on this seam.

## 4. The Salesforce connector (v1)

Salesforce is the first concrete connector because it is the first market
([research-cloudingo.md](research-cloudingo.md)). v1 proves the whole contract end
to end.

> **Salesforce as a login provider is a separate thing.** Human SSO via Salesforce
> OIDC (frontend-design §2.4) authenticates *users*; the Salesforce *connector*
> moves *data*. They share a vendor, not a code path.

### 4.1 Ingress

- **Auth:** OAuth 2.0 connected app (web-server flow); one connection = one
  Salesforce org; sandbox connections supported. Tokens live in the secret store.
- **Objects: Lead and Contact first** — both map cleanly onto the person schema
  (Lead's `Company` carried as metadata). Account is second, blocked on the
  organization entity type (backend-design.md §13).
- **Initial sync:** Bulk API 2.0, extracting **only mapped fields** (this minimizes
  our data footprint and is part of the security answer). Each object becomes one
  engine source (`sf_contact`, `sf_lead`) with `record_id_column = Id`,
  `updated_at_column = SystemModstamp`. The mapping UI's output compiles directly
  into a `sources:` config block entry.
- **Change capture: polling, not CDC, for v1** — `SystemModstamp > watermark` plus
  `getDeleted`, every 1–5 minutes. Deletions become tombstone files; the engine's
  retraction path does the rest. Polling matches the micro-batch cadence the ~40s
  incremental cycle supports and burns few API calls. CDC is the later upgrade for a
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

- [backend-design.md §6](backend-design.md) — the `connections` resource and the
  four-part connector contract.
- [backend-design.md §10](backend-design.md) — connector abstraction and the apply
  service (the normative source for this doc).
- [backend-design.md §13](backend-design.md) — the object-model generalization that
  gates Account/custom objects.
- `server/src/erserver/api.py` — the `POST /imports` drop-dir ingress seam.
- `server/src/erserver/webhooks.py` — the signed-webhook egress seam.
- [research-cloudingo.md](research-cloudingo.md) — why Salesforce is first.
