# Frontend Design — the standalone cloud UI

Status: research + product design, pre-implementation. Companion to `docs/backend-design.md`
(the control plane and engine it sits on); this document does not repeat that design, it
references it. No code here — this is the "what we build and why" document, and the bar for
"ready to go to market" is defined at the end (§9).

---

## 1. Positioning & thesis

We are attacking **Cloudingo** (Symphonic Source): a Salesforce-only dedupe SaaS priced
$2,500 / $6,000 / $10,000 per year per Salesforce org, annual-only, plus a **$100 per
100,000 records volume tax** above 300k records, with API access gated to the $10k tier at
1,000 calls/day. Their product is genuinely deep (see §6) but structurally weak in five
places their own reviewers name:

1. **Salesforce-jailed.** One Salesforce org per subscription; the only other connector is
   Marketo (leads only). Teams cleaning across CRM + warehouse + marketing + files can't use it.
2. **No real prevention.** Records are created dirty, then swept up post-hoc; their "real-time"
   merge is a continuous sweeper capped at 100k groups/day, and the prevention API is
   Enterprise-only with a tiny daily budget.
3. **No ML, no explainability.** Thirteen hand-composed string-matching styles, no per-pair
   confidence, no "here's why these two are the same person." Rule building is manual
   trial-and-error that "rewards those who have time to tinker."
4. **Dated, dense UI with a steep learning curve.** "Not the most intuitive product, there's a
   lot going on"; no embedded help; spreadsheet-era layout; slow at volume; mass merge capped
   at one page of results at a time.
5. **Sales-team frictions.** Email your account rep to enable a custom object; weekly-only
   data-quality dashboard that cannot be manually refreshed; 30-day undo retention; 2-year
   report archival; annual-only contracts.

Our engine already out-guns their matching core: probabilistic (Splink-based) scoring with
per-pair `match_probability` and a full evidence waterfall, config-versioned thresholds,
event-sourced entity history, deterministic reproducibility, and multi-source ingestion by
design. What does not exist yet is the **experience** — the thing this document designs.

**The thesis:** build a standalone cloud UI so fast, legible, and pleasant that data people
*choose* to live in it — not a bolted-on admin panel inside someone else's CRM. Every flow
Cloudingo makes a wizard-of-many-screens, we make a keyboard-first, explain-as-you-go
experience. The Salesforce integration comes later; once these flows work standalone, the
Salesforce phase is a connector + writeback problem, not a product problem.

**Deliberate non-goals for this phase:** no CRM vocabulary (no "Leads/Contacts/Accounts"
framing — sources are sources, entities are entities); no billing/payments surface (Stripe is
the eventual answer, explicitly out of scope now); no SSO/OIDC (basic email + password only,
per decision — the seam for OIDC later is already named in backend-design §9).

---

## 2. Personas & access model

### 2.1 Personas

| Persona | Maps to API role | What they do in the UI |
|---|---|---|
| **Viewer** | `viewer` | Browses golden records, duplicates, runs, metrics. Read-only everywhere. |
| **Steward** | `steward` | The daily driver: works the review queue, merges/unmerges, imports files, submits runs. |
| **Tenant Admin** | `admin` | Everything a steward does, plus config publishing, schedules, webhooks, and (new) member management for their org. |
| **Super Admin** | `operator` | Platform staff. Sees all tenants, provisions/suspends/purges them, impersonates any tenant view, watches the global job queue and audit feed. |

These map one-to-one onto the roles already enforced by the server (`viewer < steward <
admin < operator`, ranked in `server/src/erserver/auth.py`), so the UI invents no new
permission model — it renders the one that exists.

### 2.2 The minimal identity layer (new, control plane)

Today the server has **no humans**: auth is org-scoped API keys plus one static operator
token, and every audit row's actor is `key:<ulid>` or the literal `operator`. The UI needs
people. Per the locked decision, we keep this deliberately basic:

- **`users`** — email, password hash, display name, `is_super_admin` flag. (The control-plane
  inventory in backend-design §7 already names a `users` table; it was never built.)
- **`org_members`** — user ↔ org ↔ role (`viewer|steward|admin`). A user can belong to
  several orgs; the UI gets an org switcher for them.
- **Sessions** — ordinary secure cookie sessions issued by the web tier. The web backend
  (BFF, §4.3) holds the org API keys / operator token server-side and translates a session +
  membership into the right bearer credential per request. Browser JavaScript never sees an
  `erk_` key or the operator token.
- **Attribution** — the BFF forwards the acting user into audit context so `audit_log.actor`,
  `assertions.created_by`, and `review_queue.resolved_by` can finally answer "who merged
  this" with a person, not a key ULID. This is a small backend change (an actor override the
  operator/BFF is allowed to assert) and is on the gap list (§7).

Password reset, MFA, and SSO are consciously deferred. The design keeps them addable: the
`users` table and session layer are the only things an OIDC provider would later replace.

### 2.3 Impersonation — "View as tenant"

The user-facing requirement: a super admin can rotate between tenants and see the app
exactly as that tenant sees it.

- **Backend primitive already exists**: the operator principal short-circuits all org scoping
  (`auth.py`), so an operator credential can call any tenant's endpoints today. Impersonation
  is therefore a UX + audit design, not a new auth mechanism.
- **Entering**: from the super-admin console or the ⌘K palette ("View as ⟨tenant⟩ …"), pick a
  tenant and a role to view as (default: admin). The app re-renders as that tenant with that
  role's permissions — same nav, same screens, same data the tenant would see.
- **While impersonating**: a persistent, unmissable banner ("Viewing acme as tenant admin —
  exit") pinned across every page. The role is *actually enforced* in the UI (viewing as
  viewer really hides steward actions) so support can reproduce what a user reports.
- **Audit**: every mutation made while impersonating is stamped with both identities —
  effectively "⟨super-admin user⟩ acting as ⟨org⟩" — in `audit_log`. Impersonation session
  start/stop are themselves audited events.
- **Safety rails**: impersonation sessions expire on their own (short TTL), and destructive
  actions (retract assertion, cancel job, publish config) get an extra "you are impersonating"
  confirm step.

---

## 3. Information architecture & navigation

### 3.1 The shell

A single app shell: slim left nav, org switcher + user menu at the bottom, global search /
command palette at the top. Everything is tenant-scoped except the super-admin area, which
is a separate nav section visible only to super admins.

Left nav (tenant space):

1. **Dashboard** — health, metrics, trends.
2. **Records** — golden records browser + entity detail.
3. **Duplicates** — auto-merged groups and mergeable candidates.
4. **Reviews** — the gray-band human-decision queue (the steward's inbox).
5. **Sources** — configured sources, import history, the new-source wizard.
6. **Runs** — jobs, schedules, live progress, run history.
7. **Campaigns** — saved segments and their scheduled deliveries.
8. **Assistant** — the "talk with your data" chat.
9. **Settings** — config studio, members, API keys, webhooks.

Super-admin section: **Tenants**, **Global queue**, **Audit**.

### 3.2 ⌘K / Ctrl+K command palette — the spine of the app

Per the locked decision, this is a first-class feature, not garnish. One keystroke opens a
hot search that resolves, ranked, across:

- **Pages** ("reviews", "settings → webhooks", "runs").
- **Golden records** by name/email (backed by the existing search on the golden-records
  endpoint; richer full-text search is a later backend upgrade).
- **Runs and jobs** by id or recency ("last failed run").
- **Tenants** (super admin only) — typing a tenant name offers "Open", "View as", "Provision
  status".
- **Actions** — verb-first commands with the same permission gating as the buttons they
  mirror: "Start incremental run", "Import file to ⟨source⟩", "Publish draft config",
  "Resolve next review".

Every screen is reachable in ≤2 keystrokes + enter. This single feature is the sharpest
possible contrast with Cloudingo's most-cited complaint ("a lot going on", deep menu trees).

### 3.3 Design language

- **Fast above all.** Skeleton loading everywhere; virtualized tables for anything that can
  exceed a few hundred rows; optimistic UI on control-plane writes only (never on lake
  mutations — those are jobs and the UI says so honestly, see §4).
- **Beautiful and calm.** A real design system (dark/light), generous whitespace, motion used
  to explain state changes (a merged pair visually collapsing into one entity) rather than to
  decorate.
- **Explain, don't gate.** Where Cloudingo has no embedded help, every score, tier label, and
  state chip in our UI is hoverable/expandable to a plain-language explanation ("0.93 — just
  below your auto-merge threshold of 0.95; that's why it's asking you").
- **Honest state.** The backend has precise semantics (staged vs applied vs reconciled;
  queued vs running vs retrying). The UI renders those truthfully with human labels instead
  of flattening them into fake booleans.

---

## 4. The two-plane data rule

The locked architectural requirement: **quick, whippy admin/config interactions ride the
control-plane Postgres; serious data processing goes through DuckLake.** The UI is built so
this rule is structural, not aspirational.

### 4.1 Plane assignment per surface

| Surface | Plane | How |
|---|---|---|
| Login, sessions, members, org switcher | Postgres (control plane) | New identity tables, §2.2 |
| Tenant list, org state, provisioning | Postgres | `orgs` + provision job status |
| Job submit / list / detail / cancel / resume | Postgres | Existing `/jobs` routes; progress via `jobs.progress` |
| Schedules CRUD | Postgres | Existing `/schedules` routes |
| Config versions, drafts, publish, diff | Postgres | Existing `/config` routes (`config_versions` table) |
| Review resolutions, assertions | Postgres write path → lake via steward/correction machinery | Existing `/reviews`, `/assertions` routes; staged-action queue |
| Audit feed | Postgres | `audit_log` (read route is a gap, §7) |
| Golden records, entity detail, lineage, events | **DuckLake** | Existing snapshot-pinned read API (`/golden-records`, `readapi`) |
| Duplicates, match evidence, pair browse | **DuckLake** | `/duplicates` today; `match_scores` read model is a gap |
| Metrics, run counters, quality trends | **DuckLake** (`runs`, `run_stages`) surfaced through `/metrics`, `/runs` | |
| Any bulk mutation of entity data | **DuckLake via jobs only** | The UI never writes the lake directly — it enqueues `run_all_*` / `correct` jobs |

### 4.2 Consequences the UI embraces

- **Reads are snapshot-pinned.** The read API pins a DuckLake snapshot and returns cursors
  that carry it, so paging a 500k-row grid never tears mid-scroll. The UI shows the snapshot
  moment subtly ("data as of 14:32; refresh") instead of pretending the lake is live.
- **Writes to data are asynchronous, and the UI says so.** A merge decision is instant in
  Postgres (the assertion row) but takes effect at the next reconcile. The UI's state chips
  mirror the backend truth: *staged — queued behind the current run*, *applied — reflects
  after next reconcile*, *reconciled*. (The raw API field `pending_until_next_reconcile` is
  semantically inverted for naive rendering; the UI maps it to these labels rather than
  showing it as a "done" boolean.)
- **Polling first, streaming later.** The dispatcher already samples runner stage lines every
  0.5s into `jobs.progress`; the UI polls job detail at 1–2s while a run is active. SSE is a
  named later upgrade (backend-design §14: "SSE later; polling first") that changes no screen
  design, only transport.

### 4.3 The BFF (backend-for-frontend) tier

A thin web backend owned by the frontend project that: holds sessions and secrets (org keys,
operator token) server-side; translates session + membership → the right bearer credential;
adds the CORS-free same-origin boundary (the API currently has **no CORS middleware** — the
BFF makes that a non-issue); hosts the AI assistant's tool-call loop (§5.8) so model keys
never reach the browser. It contains no business logic — every capability it exposes is a
pass-through to an erserver route, which keeps the API the single source of truth for the
later Salesforce client too.

---

## 5. Core screens & flows

Each screen below lists **Backed by** (endpoints/tables that exist today) and **Needs**
(gaps, cross-referenced to §7).

### 5.1 Dashboard

The first thing a tenant sees; it must beat Cloudingo's weekly-refresh dashboard just by
being *live*.

- Headline tiles: total records, entities, duplicate groups, records inside duplicate
  groups, open reviews — straight from the metrics endpoint.
- **Data-quality trend**: entities merged/split, review-queue inflow/outflow, and duplicate
  density over time, charted from `run_stages` promoted counters (candidate pairs, pairs
  above auto-merge, entities created/merged/split/retired, edges cut, review additions,
  durations) across recent runs.
- Last-run strip: mode, status, duration, what changed — one click into Runs.
- Attention nudges: "14 reviews waiting", "last night's run retried twice on transient IO",
  "your config draft v7 is unpublished".

**Backed by:** `/metrics`, `/runs`, `/jobs`, `/reviews` (count). **Needs:** nothing hard —
a small aggregated-counters read (per-run counter series) would avoid N calls (§7.13).

### 5.2 Golden records browser + entity detail

- **Browser**: virtualized, cursor-paged grid over golden records with search (name/email
  today), snapshot indicator, and column set matching the nine canonical attributes.
- **Entity detail** — the page that sells the engine's transparency:
  - The golden record up top; below it, **members side-by-side** (every source record in the
    entity, per source system).
  - **Per-field provenance** from `golden_lineage`: for each attribute, which member record
    won and by which survivorship rule ("email came from crm:0091 via `source_priority`").
    Cloudingo has nothing like this post-merge; it is our explainability story made visible.
  - **History timeline** from `entity_events`: created, members added/removed, merged, split,
    edges cut — each event expandable to its details.
  - **Actions**: unmerge (pick members to split out → becomes `never` assertions, §5.3),
    "find similar", jump to the runs that touched this entity.

**Backed by:** `/golden-records`, `/golden-records/{entity_id}` (`entity_detail` returns
golden + members + lineage + events). **Needs:** the unmerge helper that resolves member
pairs from `entity_events` into `never` assertions (§7.5); richer search/sort/filter later
(§7.14).

### 5.3 Duplicates & merge review — the killer screen

This is where Cloudingo customers live all day, and where we win or lose. Two tabs, one
mental model.

**Reviews (the steward inbox).** The engine already produces a real clerical-review queue:
pairs whose match probability lands in the gray band (`review_low ≤ p < auto_merge`), plus
`never_unsatisfiable` escalations and coherence findings. The screen:

- Queue list with score, reason chip, sources involved, first/last seen run; filterable by
  reason/status/score band and paginated (both are backend gaps today — the route returns up
  to 500 open rows only, §7.4).
- **Decision panel**: the two records side-by-side with differing fields highlighted, and the
  **evidence waterfall rendered visually** — the stored `waterfall` blob (gamma levels and
  match weights per comparison) drawn as a tornado/waterfall chart with plain-language
  labels: "emails match exactly (+4.1), birth dates conflict (−2.3)". No competitor in this
  category shows *why*. This is the demo moment.
- Actions: **Match** (writes an `always` assertion), **Not a match** (`never`), **Dismiss** —
  the three existing resolutions, keyboard-driven (j/k to move, m/n/x to act) so working a
  100-item queue feels like triage in a mail client, not form-filling.
- **Bulk resolve**: select-all-matching-filter → one action. Backend gap: today this is N
  sequential calls each taking the tenant writer lock; the design doc's mass path (bulk
  assertions + one reconcile job) is §7.6.
- **Honest state chips** per decision (staged / applied / reconciled, §4.2), including the
  org-level banner "3 decisions queued — they apply when the current run finishes" (the
  staged-steward queue exists in the backend; its visibility route is §7.7).

**Duplicate groups.** Entities with ≥2 members — what the engine merged on its own:

- Group cards with member counts, sources, and (gap) the scores that drove the merge; drill
  into entity detail; **Undo this merge** — the same unmerge flow as §5.2.
- Browse of near-threshold auto-merges ("show me everything merged between 0.95 and 0.97")
  needs the `match_scores` read model (§7.8) — the table exists with full evidence and is
  currently read by nothing.

**Assertion library.** A third, quieter tab: every `always`/`never` the tenant has asserted
(who, when, note, active/retracted), with retraction, CSV bulk upload (engine already
supports it), and a **contradiction inspector** — the engine has a pure function that finds
unsatisfiable rule sets (a `never` inside an `always`-connected component); surfacing it
("these 3 rules can't all hold — here's the cycle") turns a silent frustration into a
feature. Listing and contradiction routes are gaps (§7.9).

### 5.4 Runs & jobs monitor

- **Active run view**: live stage checklist (init → ingest → standardize → match → reconcile
  → assemble) from `jobs.progress`, per-stage durations, live counters; cancel (honest
  kill-and-resume semantics) and resume for failed/canceled jobs.
- **History**: runs table with mode, status, duration, config hash, model version, rebuild
  reason; per-run counter detail from `run_stages`; failed runs show `error_class` +
  `error_detail` with retry/disposition explained in words ("transient IO — retried
  automatically in 30s, attempt 2 of 3").
- **Schedules**: list/create/delete against existing routes; cron entry with a humane
  builder + next-fire preview; the system-owned correction-pass schedule shown read-only
  with its config origin. Enable/disable toggle is a small gap (column exists, route
  doesn't, §7.10).

**Backed by:** `/jobs*`, `/runs`, `/schedules*`. **Needs:** job logs route for raw stage
stderr (§7.11); "triggered by" attribution (schedule vs user) (§7.12).

### 5.5 Sources & the import experience

Two distinct flows, deliberately separated:

**Import to an existing source** (daily-driver, drag-and-drop):

- Drop a CSV/Parquet onto the source card → upload via the existing multipart import route
  (256 MiB cap, far beyond Cloudingo's 20 MB) → the enqueued incremental run appears inline
  with live progress.
- **Receipts**: per-delivery outcome from `ingest_batches` — new / changed / unchanged /
  tombstoned / resurrected counts, so "did my file do anything?" has an answer (an exit-10
  no-op is shown as "0 new rows — identical delivery", not as a failure). Receipt route is a
  gap (§7.15).

**New-source wizard** (the guided "workthrough" the user asked for):

1. **Upload a sample** file (or point at one already delivered).
2. **Profile**: column names, types, fill rates, sample values.
3. **Map** columns to the nine canonical attributes (given/family name, email, phone,
   address line, city, region, postal, birth date) with suggested mappings; unmapped columns
   are kept as metadata — say so, it's a selling point (nothing is dropped).
4. **Identity & ordering**: record-id column, updated-at column, and the source's
   survivorship `priority_rank` relative to existing sources.
5. **Preview matches before commit**: run the sample against the existing corpus in a
   score-only mode and show "would auto-merge: 812, would need review: 143" *before*
   anything is written. This mirrors Cloudingo's import-scan (their import is their
   strongest feature) — and it requires the engine's score-only match mode, which does not
   exist yet (§7.16). Phase-able: ship the wizard without step 5 first, add preview when the
   engine grows the mode.
6. **Commit**: publishes the config change (a new `sources.<name>` block — tier C, so the UI
   states plainly "this triggers a retrain and full rebuild, ~⟨estimate from run history⟩")
   and runs the first import.

**Hard blocker to name honestly:** the dbt staging layer is hardcoded to three sources
(`crm`, `billing`, `webforms`) — a genuinely new source name needs a generated staging
model, not just config. Self-serve sources are gated on that backend work (§7.17); until
then the wizard ends with "queued for activation" and an operator step.

### 5.6 Config studio — no raw YAML, ever

The API takes raw YAML; the backend design itself says user editing should be "via
templates, never raw YAML". The studio is that promise:

- **Thresholds**: two sliders (auto-merge, review-low) over a live histogram of recent match
  scores, with the gray band shaded — moving a slider shows "≈340 more pairs would enter
  review". Tier A change; publish label says "re-band + re-assemble".
- **Survivorship**: per-attribute rule-chain builder (validated → source priority → recency →
  frequency → completeness) as draggable pills; tier A.
- **Blocking & comparisons**: structured editors for blocking keys and comparison levels
  (exact / jaro-winkler with threshold / variant match / etc.), clearly marked tier B / C
  with their cost ("changes matching — retrain + full rebuild").
- **Version history**: every draft/published version with author, time, tier,
  changed-blocks; visual **diff between versions** (gap: no diff endpoint, §7.18 — v1 can
  diff client-side from two version bodies).
- **Publish flow**: validation errors surfaced at the exact field (the API already returns
  JSON-pointer paths); tier + estimated runtime (from run history) on the confirm; the
  enqueued jobs appear immediately in Runs.
- Operator-only blocks (tenant, storage, training, clustering…) are visible but locked for
  tenant admins, exactly matching the server's 403 behavior.

**Backed by:** `/config`, `/config/versions`, `:publish`, tier classification already
implemented server-side.

### 5.7 Campaign builder — "chat with your data" for segments

A campaign = a **saved segment** over golden records + an **action** + an optional
**schedule**. Everything compiles to primitives that already exist (a stored query, the
schedules table, jobs, webhooks) — no new engine machinery.

- **Build the segment** two interchangeable ways, always kept in sync:
  - **Visual filters**: attribute conditions, source membership, dedupe state ("was merged
    in the last 30 days", "entity has ≥3 members"), completeness/quality conditions.
  - **Conversation**: describe it in words — "everyone whose golden email came from webforms
    but who also exists in crm" — and the assistant (same tool loop as §5.8) drafts the
    filter set, shown visually for confirmation. The chat drafts; the user approves;
    the saved artifact is always the inspectable structured filter, never a prompt.
- **Preview** instantly against the snapshot-pinned read API with count + sample rows.
- **Actions**: export (CSV now; destinations later), deliver to a webhook, or feed a
  maintenance job. **Schedule** it with the existing cron machinery ("every Monday 8am,
  export new members of this segment").
- Campaign runs land in Runs like everything else; each has a delivery history.

**Needs:** a saved-segments store (control-plane table) and a parameterized export job
(§7.19). Both small; the segment language deliberately starts as "filters the read API can
already evaluate."

### 5.8 AI assistant — "talk with your data"

Scope per the user's framing: the assistant understands **the metadata around what has run**
and **the actual golden records** — and nothing else in v1.

- **Grounding, exactly two corpora:**
  1. **Run/config/review metadata**: `runs`, `run_stages` counters, job history and
     dispositions, config versions with tiers and changed blocks, review-queue statistics,
     audit entries. This answers "what happened / what changed / why did it fail":
     "last night's run merged 214 entities and sent 37 pairs to review; two more than usual
     because you lowered review-low on Tuesday (config v9)."
  2. **Golden records** via the snapshot-pinned read API: search, entity detail, lineage,
     duplicate groups. This answers "who/what is in my data": "show me entities merged this
     week that came from the webforms source."
- **Mechanics**: Anthropic API from the BFF (never the browser); the model gets a **fixed
  toolbox of read-only, tenant-scoped endpoint wrappers** — the same viewer-role surface a
  human viewer has, enforced by the same credential, so the assistant is *provably* unable
  to see another tenant or mutate anything. Streaming responses; every answer cites which
  tool calls produced it (expandable), so trust is inspectable.
- **Product placement**: a full Assistant page for open-ended exploration, plus contextual
  entry points — "explain this run" on a run page, "explain this score" on a review pair
  (it has the waterfall), "explain this merge" on an entity timeline.
- **v2 (explicitly not now)**: proposing actions — drafting a merge decision, a threshold
  change, a campaign — always as a staged proposal a human approves, never direct execution.

This is a category first: Cloudingo and DemandTools have nothing conversational; reviews of
the whole category ask for AI assistance with rule building and match explanation. We have
the two ingredients competitors lack: real probabilistic evidence per pair, and a clean
metadata ledger of every run.

### 5.9 Super-admin console

The platform-operator cockpit, in the same app behind the `is_super_admin` flag:

- **Tenant directory**: every org with state (provisioning / active / suspended / purging),
  record/entity counts, last run, open reviews, config version. Needs the org-list route —
  today the API can address one org but cannot enumerate them (§7.1).
- **Tenant detail & lifecycle**: provision (exists end-to-end: dedicated Postgres database +
  lake namespace + seeded config v1 + one-time admin key); **suspend / resume / purge** —
  the states exist in the schema and the guards already refuse work for non-active orgs,
  but no routes flip them; purge (drop database + delete S3 prefix + 30-day grace, per
  backend-design §7) is entirely unimplemented (§7.2). The console designs for the full
  lifecycle; buttons light up as routes land.
- **Onboarding flow**: create tenant → watch the provision job's stages live → hand off the
  one-time admin key (shown once, exactly as the API behaves) → "invite first member".
- **Impersonation** entry (§2.3) from every tenant row.
- **Global job queue**: all orgs' active/queued jobs, dispatcher health, retry storms —
  needs a cross-org jobs view (§7.3).
- **Cross-tenant audit feed**: filterable by actor/org/action — needs the audit read route
  (§7.20).
- **Plan/billing column**: placeholder only ("—"), reserved for the Stripe phase; the org
  `resource_class` (S/M/L) named in backend-design §7 is the natural hook when it arrives.

---

## 6. Cloudingo parity & differentiation matrix

Legend: **Have** = engine/API supports it today, UI just renders it · **Partial** = core
exists, missing a listed gap · **Gap** = needs backend work (§7) · **Roadmap** = consciously
later phase.

### 6.1 Table stakes (must have to be in the conversation)

| Cloudingo capability | Ours | Notes |
|---|---|---|
| Saved reusable match-rule sets | **Have** | Config-versioned blocking + comparisons; stronger: versioned, diffable, tiered |
| Spread of match styles, per-field control | **Have** | Probabilistic comparison levels (exact, jaro-winkler, variants…) supersede their 13 string styles |
| Fuzzy matching with thresholds | **Have** | Full probabilistic scoring — with confidence, which they lack |
| Scoped/conditional matching, group-size rules | **Partial** | Blocking + thresholds cover most; per-filter scope conditions are a config-studio roadmap item |
| Cross-source matching | **Have** | Native — the engine's entire premise |
| Side-by-side manual merge with field picker | **Partial** | Side-by-side + lineage exist; per-field manual override at merge time is roadmap (survivorship rules decide fields today) |
| Mass merge | **Gap** | Bulk assertions + one reconcile job (§7.6); theirs is capped at one page at a time — beatable |
| Scheduled automated merge | **Have** | Auto-merge above threshold + cron schedules |
| Master-record selection rules | **Have** | Survivorship chains (priority, recency, frequency, completeness, validated) |
| Field-level survivorship incl. override-when-blank | **Have** | Per-attribute rule chains |
| Exception routing to human review | **Have** | The gray band **is** this, principled instead of ad-hoc |
| Protected / excluded records | **Roadmap** | `never` assertions cover pair-level; record-level "never touch" flag is roadmap |
| Import with dedupe, field mapping, insert-vs-update | **Partial** | Import + content-hash change detection exist; match-before-insert preview needs score-only mode (§7.16) |
| Mass update / mass delete | **Roadmap** | Data-maintenance jobs; not this phase |
| Pre-merge preview | **Partial** | Merge-plans endpoint exists; richer preview with §7.8 |
| Post-merge audit with per-record IDs and user | **Have** | `entity_events` + lineage + audit_log; person-level attribution needs §2.2 |
| Undo / unmerge | **Partial** | Event-sourced history + never-assertions + correction pass; needs the unmerge helper (§7.5). Structurally better than their 30-day backup copy |
| Exportable logs/reports | **Partial** | Merge-plans exports CSV; campaigns (§5.7) generalize it |
| Role-based permissions | **Have** | Four roles enforced server-side |
| Shared rule libraries | **Have** | Config is org-shared and versioned by nature |
| Synonym/nickname handling | **Partial** | Variant matching in comparisons; user-editable synonym lexicons are roadmap |
| SOC 2 / encryption posture | **Roadmap** | Phase 4 per backend-design §15; required before enterprise deals, not before first design partners |

### 6.2 Differentiators to lead with

1. **Explainable ML matching** — per-pair probability + evidence waterfall, rendered (§5.3).
   Their 13 string styles have no confidence and no why.
2. **Provenance you can see** — per-field lineage on every golden record (§5.2). Post-merge,
   Cloudingo shows you a record; we show you *why every field is what it is*.
3. **Event-sourced undo** — full entity history, principled unmerge via assertions +
   correction pass; no 30-day backup window, conversions-not-undoable caveats, or per-merge
   backup slowdowns.
4. **Multi-source by design** — CRM + billing + webforms + any tabular source in one entity
   graph. The single most-requested thing Cloudingo can't do.
5. **Live data quality** — dashboard computed from real run counters on demand, vs their
   weekly-refresh-only score.
6. **AI-native** — the assistant (§5.8) and conversational campaign builder (§5.7). Nothing
   in the category has this.
7. **Speed + volume honesty** — snapshot-pinned virtualized browsing, no per-page mass-merge
   cap, no record-count tax, no import 20 MB limit (ours is 256 MiB).
8. **No email-your-rep** — every capability self-serve, including new sources (once §7.17
   lands) and tenant onboarding (fully automated provisioning already works).
9. **Prevention-ready architecture** — a real-time score/decide API is a natural engine
   extension (score-only mode §7.16 is its seed); their prevention story is a $10k tier with
   1,000 calls/day. (Roadmap, but the architecture makes it credible.)

### 6.3 Their strengths we must respect (not dismiss)

- Their **import wizard** is genuinely good (rescan-with-looser-filters, import rules,
  templates); §5.5 is designed to meet it, not just match checkboxes.
- Their **reporting depth** (18 reports, pre-merge sign-off exports, scheduled report
  delivery) — our §5.7 campaigns + run history must cover the top use cases (merge audit,
  pre-merge review export) by GTM.
- **Undo restoring Salesforce relationships/lookups** is deep domain work; our equivalent
  claim must stay scoped to what the event log actually restores until the Salesforce phase.

---

## 7. Backend gaps the UI requires (the next backend milestone, prioritized)

Phase letters refer to §9. Each item is small-to-medium API work unless marked **engine**.

| # | Gap | Needed by |
|---|---|---|
| 7.0 | **CORS or same-origin BFF** (chosen: BFF, §4.3) + **users / org_members / sessions** + per-user actor attribution into `audit_log` and `created_by` fields | A |
| 7.1 | `GET /v1/orgs` — list/enumerate tenants (with state, counts) | A |
| 7.2 | Org lifecycle routes: suspend / resume / purge (purge = drop tenant database + delete S3 prefix + key revocation, with grace period; guards for non-active orgs already exist) | D |
| 7.3 | Cross-org jobs view for the global queue | A (read-only) |
| 7.4 | Review queue: pagination, filters (status / reason / score band), single-review GET | B |
| 7.5 | **Unmerge helper**: resolve an entity's member pairs from `entity_events` and post the `never` assertions in one call (named in backend-design §6) | B |
| 7.6 | **Mass merge / bulk resolve**: accept a batch of assertions + enqueue one reconcile job (vs N writer-lock round-trips) | B |
| 7.7 | Staged-steward-queue visibility (`pending_count` exists unrouted) — powers "3 decisions queued" | B |
| 7.8 | `match_scores` read model: browse pairs by score band with evidence (auto-merged browse, richer previews) | B |
| 7.9 | Assertions: list route + contradiction-check route (engine functions exist: `active_assertions`, `check_contradiction_1`) | B |
| 7.10 | Schedule enable/disable + update (column exists) | B |
| 7.11 | `GET /jobs/{id}/logs` — persist + serve per-stage stderr (design §14 names it) | B |
| 7.12 | Job attribution columns (`created_by`, `schedule_id`) so Runs can say "by Jane" / "by nightly schedule" | B |
| 7.13 | Small aggregated metrics read (counter series across runs) for dashboard charts | A (nice-to-have; N calls works) |
| 7.14 | Golden-records search/sort/filter beyond 3-column ILIKE (FTS later) | C |
| 7.15 | Import receipts route over `ingest_batches` | B |
| 7.16 | **Engine: score-only match mode** (no writes) — import preview (§5.5), future prevention API (design §11/§12 name it) | C |
| 7.17 | **Engine: generated dbt staging models** — remove the crm/billing/webforms hardcoding so sources are self-serve | C |
| 7.18 | Config diff endpoint (client-side diff acceptable for v1) | C |
| 7.19 | Saved segments table + parameterized export job (campaigns) | C |
| 7.20 | Audit read route (org-scoped for tenants, global for super admins); add the missing `webhook.delete` audit write | A (super-admin), D (tenant-facing) |

Deliberately **not** on the list: SSE/websockets (polling is fine through GTM), OIDC/SSO,
billing/metering, rate limiting (BFF mitigates), Salesforce connector.

---

## 8. Stack recommendation

Chosen for the two locked requirements — **very performant** and **beautiful, with modern
charting and animation** — plus the practical one: the largest component and talent pool for
the data-dense screens we need (grids, diffs, waterfalls, chat).

- **React + TypeScript on Next.js** — the app shell, routing, and server-side data access
  (the BFF from §4.3 lives here, so one deployable serves UI + session layer). Server
  components keep first paint fast; client components take over for the interactive grids.
- **Tailwind CSS + shadcn/ui (Radix primitives)** — an ownable design system rather than a
  themed component framework: we control the look completely (the "beautiful, not a CRM"
  requirement), with accessibility solved at the primitive layer.
- **TanStack Query / Table / Virtual** — server-state caching with polling built in
  (job progress), and virtualized tables that stay at 60fps over 100k-row golden-record
  sets — the direct answer to "Cloudingo is slow at volume".
- **cmdk** for the ⌘K palette — the de-facto standard behind the best-in-class palettes.
- **Framer Motion** for animation — state-explaining motion (merge collapse, queue triage
  transitions) with spring physics, cheap to keep tasteful.
- **ECharts** for charts (score histograms, waterfall/tornado evidence, counter trends) —
  canvas-rendered, handles large series without jank; **visx** as the escape hatch if a
  bespoke visualization (the evidence waterfall) outgrows it.
- **Vercel AI SDK pattern** (streaming UI over the Anthropic API from the BFF) for the
  assistant and the conversational campaign builder.

One repo directory (`frontend/`), its own lockfile and CI job, mirroring how `server/` is a
standalone uv project today.

---

## 9. Phased build plan & go-to-market readiness

Each phase is shippable to design partners on its own; order maximizes learning per unit of
build.

**Phase A — "See your data."** Identity layer + BFF (§7.0), app shell + ⌘K palette,
dashboard, golden-records browser + entity detail (read-only), runs monitor (read-only),
super-admin tenant directory (§7.1, §7.3) + impersonation. *Outcome: a demoable product a
design partner can log into and be impressed by; validates the read paths and the design
language.*

**Phase B — "Fix your data."** The review inbox with evidence waterfalls, merge/unmerge and
the assertion library (§7.4–§7.9), bulk resolve, staged-state honesty, imports to existing
sources with receipts (§7.15), schedules management, job cancel/resume + logs (§7.10–§7.12).
*Outcome: a steward can do their entire job in our app — this is the Cloudingo-replacement
threshold for existing-data cleanup.*

**Phase C — "Grow your data."** New-source wizard (with §7.16/§7.17 landing the preview and
self-serve activation), config studio, campaign builder (§7.19), AI assistant. *Outcome: the
differentiation story is real, not slideware; tenants expand usage without operator help.*

**Phase D — "Run the business."** Tenant lifecycle routes wired to the console (suspend /
resume / purge, §7.2), tenant-facing audit feed (§7.20), hardening + performance pass,
empty-state/onboarding polish. *(Billing/Stripe: the phase after this, out of scope now.)*

### Go-to-market readiness checklist

We are ready to sell against Cloudingo when all of the following are true:

- [ ] Phases A–C complete (D can trail with design partners on manual lifecycle ops).
- [ ] Every **Table stakes** row in §6.1 reads **Have** or **Partial-with-workaround** — no
      raw **Gap** rows remain; the three genuinely-roadmap rows (mass update/delete,
      record-level protection, synonym lexicons) have documented workarounds.
- [ ] A steward can run the full loop unassisted: onboard → add a source → import → review
      queue to zero → verify golden records with lineage → undo one merge → schedule the
      nightly run → build one campaign — with no operator intervention and no YAML.
- [ ] The demo script beats Cloudingo's on their own turf: import a messy file, watch the
      review queue explain itself, ask the assistant "what did that import change?"
- [ ] Provisioning a new tenant end-to-end (dedicated database, seeded config, admin key,
      first login) takes under five minutes, self-serve from the super-admin console.
- [ ] p95 interactions: grids and palette under 100ms perceived, golden-record pages under
      1s on a 1M-record tenant; a run's progress visibly ticks within 2s of a stage change.
- [ ] Pricing page can honestly say: multi-source, no record tax, no per-object rep emails,
      monthly billing available — the four packaging wedges §6.2 identifies.

---

*Sources: backend surface inventory of `server/src/erserver/` and `src/er/` (September 2026,
this repo); Cloudingo product research from cloudingo.com (features, pricing, security, FAQ),
help.cloudingo.com documentation, and G2/Capterra/TechRepublic reviews.*
