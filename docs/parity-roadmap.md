# Competitor-parity roadmap (living tracker)

Target: reach feature parity with **Cloudingo** and **DataGroomr** (the two incumbent data-quality /
entity-resolution SaaS tools), plus a couple of extras, while staying **significantly more performant**
(our Rust standardize → dedupe → golden-record engine, bit-identical to Splink, in-process DuckDB, flat
memory) and **cost-conscious**. Delivered **dev-first, one PR per feature**, each tested and merged on
green CI, so git history records the *why* of every design decision.

This file is the durable status board — every PR updates it. Full analysis + reasoning live in the approved
plan (`~/.claude/plans/typed-painting-cat.md`) and `docs/research-cloudingo.md`.

## Scope decisions (locked 2026-10-15)
1. **Skip ALL Salesforce-coupled features** — no SF connector, Marketo, Lightning UI, lead→contact convert.
   We are a generic data-quality/ER product; our wedge is not being SF-locked.
2. **Stand up terraformable prod now** — `envs/prod` reusing the env-agnostic modules; promote-on-merge-to-main.
   Domain-blocked pieces (public ALB/WAF, SES) are deferred until the domain exists.
3. **Core parity first, then the top 2 differentiators** — data-quality scoring + health dashboard, and AI
   enrichment via AWS Bedrock. RevOps analytics, chained workflows, AWS-native verification = later backlog.
4. **Coverage: measure + non-regression floor, nightly** (cargo-llvm-cov + vitest coverage). Every feature PR
   ships behavioral tests; coverage may not regress; the PR hot path stays fast.

**Builder-key ⇄ prod:** the never-expiring `AdministratorAccess` key in `infra/terraform/envs/dev/builder.tf`
gets the §8.2 `DenyProd*` guards when prod lands (PR 0.4) so it cannot mutate `er-prod-*`; it is not deleted
(the build loop still needs non-expiring creds). Prod deploys use the CI github-oidc role, never the builder key.

## Already shipped before this program (baseline)
Explainable probabilistic Fellegi-Sunter matching (per-pair probability + evidence waterfall), incremental
link/cluster, survivorship rule chains + lineage + master election, entity-events history, drop-dir import with
change detection + new-source wizard, jobs queue + cron + dispatcher, HMAC webhooks, name/email/phone/address
standardization, dashboard/metrics/runs, roles + invites + impersonation + tenant lifecycle + audit, AI
assistant. Tenant username/password auth is fully built (argon2id). M6 (PR #56) was the first parity milestone.

## PR checklist

### Phase 0 — Delivery foundations (make "merge to master → it works" literal)
- [x] **0.1 Frontend CI + this roadmap** — `.github/workflows/frontend.yml` (lint+types+unit, and the Playwright
      functional tier in the pinned image). *Merged — PR #97.*
- [x] **0.2 Coverage (nightly)** — cargo-llvm-cov (rust.yml) + vitest `--coverage` (frontend.yml), **nightly +
      workflow_dispatch only** (off the PR/main-push hot path). Non-regression floor via `--fail-under-lines`
      (Rust, initial 45) and `coverage.thresholds` (frontend, lines 28/fn 15/stmt 29/br 35, measured just-above).
      Floors are a regression backstop and ratchet UP over time; the real gate is per-PR behavioral tests. *Merged — PR #98.*
- [x] **0.3 `envs/prod` foundation (+ 0.4 combined)** — `infra/terraform/envs/prod/` reuses modules/{network,eks,
      lake,efs} + prod Aurora (multi-AZ, warm floor, deletion-protected) with `er-prod-*` names, Environment=prod.
      `terraform plan`-validated: **61 to add, 0 change** — apply-ready. **Not applied** (cost-smart: no idle prod
      compute until there's a reason; one command applies it). CloudTrail deferred — the `core` org-trail already
      covers prod management events; only prod-lake S3 *data-event* logging remains (needs a core audit-bucket-policy
      change → its own PR). ALB/WAF + SES stay deferred (domain-blocked).
- [x] **0.4 Builder-key prod guard** — `builder.tf` now carries the §8.2 `DenyProd*` denies (omitting
      `DenyAccessKeyCreation`, which the dev loop needs for `lake.tf`); **applied + behaviorally verified** on the live
      key (prod mutation → AccessDenied, dev mutation → NotFound). Key is now dev-scoped/prod-guarded, not a prod-gate
      blocker. Committed into the repo.
- [x] **0.5 Infra gate (fmt + validate) on merge to main** — per the prod-apply decision (prod stays plan-only until a
      tenant), 0.5 is a **credential-free** `terraform fmt -check` + per-env `terraform validate` gate on every
      infra change (`.github/workflows/infra.yml`), proving the config stays valid + apply-ready without an IAM role.
      The heavier state-diff `terraform plan` gate (needs a read-only OIDC plan role + state access) → **0.5b**, to do
      when prod is actually applied. Also cleaned the pre-existing `envs/dev/rds.tf` fmt debt so the gate is green.
      *Blocked on the prod apply decision: prod EKS isn't applied (cost-deferred), so a real rollout has no target.
      Needs the deploy-mechanism pass + a go-ahead to apply prod (or a plan-gate-only interim). Revisit with user.*
- [x] **0.6 Password-reset page** — added `(auth)/reset` (request) + `(auth)/reset/[token]` (confirm), fixed the
      `PUBLIC_PATHS` gap (`/reset` was missing → pages would have redirected to /login), and added a "Forgot your
      password?" link on login. Functional tests (5) + regenerated login visual baselines. Completes the self-serve
      tenant username/password workflow.

### Prerequisite fixes (discovered during Phase 1 scoping)
- [x] **P1 Dispatcher staged-action drain** — the all-Rust rebuild never ported the Python `steward.drain_org`,
      so in production the dispatcher **never applied staged steward actions** (bulk-resolve, assertions, unmerge
      all staged intent that never took effect; e2e tests masked it by calling `apply_staged_action` directly).
      Added `dispatcher::drain_and_correct` + an `apply_corrections` job kind (the steward apply_now path now
      enqueues it): drain pending actions into the lake → `run_correct`. Idempotent under retry. Un-breaks M6
      steward and is the prerequisite for PR 1.2 (mass ops) + 1.3 (exceptions). Regression test drives the real
      path in the e2e tier.

### Phase 1 — Stable-core parity (generic; no external deps)
- [x] **1.1 Account/Org entity type [EPIC]** — hand-written parallel schema behind an `entity_type` discriminator
      (user-chosen over the `entity-framework-design.md` generator; the generator would later supersede — accepted).
      Decomposed:
  - [x] **1.1a foundation** — `EntityType` discriminator on `Config` (`#[serde(default, skip_serializing_if)]` →
        `config_hash` byte-identical for person configs), `ACCOUNT_CANONICAL_ATTRIBUTES` + `canonical_attributes()`
        selector, and parallel Account `Owner::Dbt` lake tables (`acct_stg_records`/`acct_int_std_records`/
        `acct_golden_records`/`acct_golden_lineage`/`acct_golden_display`). Determinism-gated: full `cargo test
        --workspace` green, ONLY `model_registry_golden.json` changed (config_golden + dbt_sources + all base_10
        person fixtures byte-identical).
  - [x] **1.1b standardizers** — Account `er-std` normalizers (`company_name`/`company_name_norm`/`company_variants`,
        `domain`/`domain_norm`; reuse `phone_e164` + `null_semantics(lowercase_trim)` for addr_*) with own golden
        oracles; additive `acct_stg_arm` + `build_acct_int_std_records`; `run_standardize` branches on `entity_type`.
        R1 fix: `metadata_excluded`/`metadata_columns` now iterate `spec.columns.values()` (entity-agnostic;
        person set-identical). Person fixtures byte-identical; only NEW account/company/domain files added.
  - [x] **1.1c matching + pipeline** — hand-authored `acct_model_v1.json` (no TF-adjustment → deterministic); `run_match`/
        `run_reconcile`/`run_golden` branch corpus + golden by `entity_type`; shared entity-agnostic tables reused
        (match_scores/entity_membership/entities — one lake = one entity_type); parallel `acct_golden_*_sql` builders
        (person builders untouched → byte-identical). New `acct_base` fixture + `acct_e2e` behavioural test (3 dup pairs
        cluster, Umbrella singleton, 4 entities, golden assembles). seed_lexicon handles the empty (account) lexicon.
  - [x] **1.1d frontend (browse + view)** — read API `golden_list`/`entity_detail` parameterized by a `GoldenView`
        (`read.rs`), selected per request via `org_lake_golden` + `entity_type_from_env` (default Person →
        person responses byte-identical); `acct_read.rs` e2e asserts the Account projection (company_name, no
        given_name, "acme" search, detail golden). Frontend seam filled: `goldenAttributes`/`recordAttributes`/
        `displayName`/`EntityType` in `attributes.ts` + `ACCOUNT_GOLDEN_ATTRIBUTES`; `useOrgEntityType` reads
        `entity_type` from the active config (default person, parse-error-safe); `GoldenRecord` widened to the
        person+account superset; records browser + entity detail render entity-aware columns/attributes. Person
        DOM is byte-identical (per-column `className` carried so the Phone cell keeps its no-`truncate` class) →
        `@visual` baselines unchanged. 8 vitest selector units + the account read e2e; all determinism fixtures
        (config_hash/content_hash/std_hash/golden vectors) byte-identical.
  - [x] **1.1e frontend follow-ups** — entity-aware **new-source wizard** (`csv-profile.ts` `ACCOUNT_CANONICAL_ATTRIBUTES`
        + `canonicalAttributes(entityType)` + account `suggestMapping` hints; the wizard's Map step + completeness gate
        read `useOrgEntityType`) and every secondary display surface migrated off the hardcoded person lists:
        duplicates list, merge dialog, enrich labels, field-compare grid, command palette (company icon + domain),
        campaigns preview — all via `displayName`/`goldenAttributes`/`recordAttributes`/`canonicalAttributes(entityType)`.
        The **duplicates** read-API path got its own `GoldenView` switch: `GoldenView.group_preview` (person name+email /
        account company+domain), `read::duplicates(session, &view, …)`, and `duplicates_list` resolves it via
        `org_lake_golden`. `DuplicateGroup` widened to the person+account superset. Person default throughout →
        person DOM byte-identical, `@visual` baselines unchanged. Coverage: new `canonicalAttributes`/account
        `suggestMapping` vitest units, a functional `account.spec.ts` (duplicates company/domain preview + wizard
        account canonical mapping) on an `account-dev` mock org (`entity_type: account`, company/domain golden +
        entity-aware mock `/duplicates`), and the `acct_read.rs` e2e extended to assert the account `read::duplicates`
        projection. All determinism fixtures byte-identical.
- [x] **1.2 Mass UPDATE** — durable golden field overrides across a filtered set. New `golden_overrides` lake
      table + `override_field`/`clear_field` steward actions (ride the drain) + a post-assembly UPDATE over
      golden_records that is a byte-exact **no-op when unused** (base fixtures untouched; only the model_registry +
      dbt_sources goldens change, for the new table). `POST /v1/orgs/{org}/mass-update` + a monochrome records-page
      panel (typed-count confirm). Mass **DELETE** is reframed as steward exclusion/suppression (→ 1.3): a literal
      delete doesn't fit the source-driven model (re-ingest resurrects the record).
- [x] **1.3 Record suppression (exclusion)** — new `record_overlays` lake table + `suppress_record`/`protect_record`/
      `retract_record_overlay` steward actions on the drain; `suppress` drops a record from reconcile clustering (all
      three inputs: nodes + match edges + always edges, suppress-wins-over-always), survives re-ingest (unlike a
      tombstone). This is the generic **"exclusion" that replaces literal mass-delete**. Determinism-safe: no-op when
      empty (only the schema-registry + dbt-sources goldens changed for the new table; base_10 output byte-identical),
      proven by `pg_record_overlays` e2e + the full golden suite. Endpoints `POST /records/{suppress,unsuppress}` +
      entity-view "Exclude" action. *Follow-up (noted): exception-group UI (reuses existing `never` assertions).*
- [x] **1.3b Protect ENFORCEMENT** (PR #116) — the `protect` overlay was inert storage in 1.3; now a member with an
      active `protect` overlay is elected the surviving **master** for its entity, overriding the survivorship policy, on
      the READ path. A `protected` CTE (active `record_overlays kind='protect'` ∩ affected members) `COALESCE`'d ahead of
      the policy election in `read::merge_plans`. Gated on the overlay table existing AND ≥1 active protect row, so when
      nothing is protected the SQL collapses to the exact prior `masters AS (…)` — **byte-exact**, no fixture touched
      (merge_plans has no committed golden; chose the read path over golden assembly precisely to stay off the determinism
      contract). Endpoints `POST /records/{protect,unprotect}` (suppress generalised to `record_overlay_action`) + a steward
      "Protect" entity-view action. Proven by `pg_protect_master` e2e (real merge_scenario, since base_10 emits only
      `created`) + Playwright. *Follow-up → 1.3c: never-cut guarantee + contradiction precheck (the write-path half).*
- [x] **1.3c Protect NEVER-CUT + contradiction surfacing** (the write-path half of 1.3b) — the reconcile never-cut
      fixpoint now treats any edge incident to a protected record as uncuttable, exactly like a `cut_protect`-probability
      edge: a `never` rule that could be satisfied only by severing a protected record escalates instead of cutting it.
      `er_cli::load_protected_keys` (sibling of `load_suppressed_keys`, `kind='protect'`, missing-table-tolerant) threads a
      `protected` set into `never_cut_fixpoint`/`choose_cut_edge`. The escalation (previously discarded) is **surfaced** as
      an open **`never_unsatisfiable`** review-queue item — the reason code already in the schema + the existing review UI,
      so no new table/enum/UI. Design choice: surface via the designed `review_queue` channel (not the reserved `cut_edges`
      relational-cut table, whose NOT-NULL `match_probability`/lifecycle are for a different future feature). The
      synchronous **409 precheck** was NOT added — `protect` is a *staged* overlay drained at reconcile, not a cheap
      synchronous request like `unmerge_entity`, so forcing a 409 there would be awkward; the contradiction instead
      surfaces at reconcile time via the review item (a more robust, source-agnostic catch). Determinism-safe: the review
      write is gated on a protect overlay existing AND an escalation occurring, so the base path never touches
      `review_queue` — base_10 + all golden e2e byte-identical, **no fixture changed**. Proven by two er-reconcile unit
      tests (protected-record escalates; off-path protection is a no-op; empty set reproduces the prior cut) + a gated
      `protected_record_is_never_cut_and_escalates_through_ducklake` DuckLake e2e. *(High-confidence escalations, possible
      pre-1.3c, stay silent until a future rebaseline — noted in code.)*
- [x] **1.4 Score-only match preview** (frontend-design §7.16) — `er_cli::preview_match` runs the committed model
      read-only (pairs + bucket counts + evidence waterfall) via a session-local temp table, committing NOTHING
      (no match_scores/reviews/golden; determinism contract untouched). New `GET /v1/orgs/{org}/match-preview`
      (steward, spawn_blocking) + a "Preview matches" card on the duplicates page. e2e test proves read-only
      (match_scores absent + snapshot unchanged after preview).
- [x] **1.5 Confidence-threshold auto-merge / point-of-entry dedupe** — already satisfied by the existing reconcile
      path: `run_reconcile` clusters exactly the active `match_scores` at or above `config.thresholds.auto_merge`
      (`er-cli/src/lib.rs`), and incremental link-new applies it at point-of-entry. No new code was needed; the gray band
      `[review_low, auto_merge)` already routes to the review queue. The threshold IS the auto-merge policy.
- [x] **1.5b Surface per-run auto-merge count** — the run ledger now shows, per run, how many pairs that run scored
      at or above the auto-merge threshold. Derived at **read time** in `read::runs_list` via a `match_scores` count
      grouped by `run_id` (a full/incremental run shares one `run_id` across match+reconcile, so it attributes to the
      right ledger row), pinned to the same snapshot; the threshold is loaded from the org's `ER_CONFIG_PATH`
      (`auto_merge_from_env`, `Option` — the count is omitted, not guessed, when no config is readable). **No persistence,
      no schema migration** (avoided a `runs` column — CREATE-IF-NOT-EXISTS DDL makes that a de-facto migration — and the
      unwritten `run_stages` counter path); tolerant of a lake that never matched. Determinism untouched (runtime
      reporting read from a snapshot, not in the golden set). New `auto_merged_pairs` field on each `/v1/orgs/{org}/runs`
      row + an "Auto-merged" column on the run-history table. Proven by a `pg_golden_read` assertion (base_10 full run
      auto-merged ≥1 scored pair) + a `runs.spec` functional assertion + refreshed `runs.png` visual baselines.
- [x] **1.6 Deep field-level survivorship (Bucket A)** — added single-winner rule tokens: `longest`, `shortest`,
      `numeric_max`, `numeric_min`, `most_common` (alias of `frequency`), and `prioritized:<v1|v2|...>` (ranked list).
      Additive `rule_terms` arms + palette entries only — NO config/DDL change, so every golden fixture is byte-identical
      (verified: full `cargo test --workspace` green, zero golden drift). New Session-based election test (6 cases).
      **1.6b (deferred):** Bucket B `combined_list`/`sum` (synthesize a value not tied to one record → need a new
      lineage/assembly path) + generalized grouped/controlling fields (the hardcoded `address` grouping is the precedent).
- [x] **1.6b Bucket B survivorship (`combined_list` / `sum`)** — value-SYNTHESIZING rules: `combined_list` unions each
      member's distinct non-empty value (sorted, `'; '`-joined); `sum` totals the numeric values (non-numeric → NULL,
      dropped). The challenge: a synthesized value has no owning record, but the golden path re-reads every value from one
      member's `record_key`. **Design (low-risk, zero schema change):** the survivorship *decision* still names a
      deterministic representative member (`min(record_key)`) + the rule for lineage, and **records assembly** computes the
      value itself via a correlated aggregate over all active members (`synthesized_value_expr`, shared with the decision).
      So **no new `golden_lineage` column, no schema/model_registry/dbt_sources change** — the determinism surface is
      untouched. A SOLE synthesizing rule only (bails if mixed with ranking rules or applied to the composite `address`).
      Config is free-string chains, so adding the tokens needs no config schema change and unchanged chains hash
      identically. base_10 uses neither token → every golden fixture **byte-identical** (verified: `er-match` base_10
      golden gate + full `lake_e2e` green, `git status` shows no fixture/golden/schema file touched). Tests: 4 er-golden
      decision unit tests (union/sum/`min` representative + the two bail guards) + a gated `combined_list_survivorship_…`
      DuckLake e2e asserting the synthesized golden email equals the members' sorted distinct emails.
- [x] **1.7 Nickname/synonym lexicon editing** — finished the M6 partial: `GET`/`PUT /v1/orgs/{org}/lexicon`
      (per-org CSV at `ER_LEXICON_PATH`, canonicalised via `er_std::lexicon`, PUT enqueues a `run_all_full` to
      apply), + a monochrome `LexiconEditor` on the config page. Determinism-safe (per-org file; not in
      `config_hash`, no DDL/fixture/golden change). e2e `pg_lexicon` + viewer/admin functional tests.

### Phase 2 — Top 2 differentiators
- [x] **2.1 Data-quality scoring models** — read-only `GET /data-quality` (Completeness / Junk / Stale /
      Duplicate-rate + composite DQ score), fixed-metrics v1 (no config change → determinism contract untouched).
- [x] **2.2 Health dashboard / DQ-Score KPIs** — monochrome "Data quality" section on the dashboard (KPI tiles +
      per-attribute completeness bars).
- [x] **2.3 AI enrichment** — a steward "Enrich with AI" panel on the entity view suggests fill-missing /
      normalize values for ONE golden record. The LLM call is confined to the BFF (`POST /v1/orgs/{org}/enrich`,
      steward), reusing the existing Anthropic plumbing (`ANTHROPIC_API_KEY`/`ANTHROPIC_BASE_URL`, new
      `ERWEB_ENRICH_MODEL` default `claude-opus-5`); the model key never reaches the browser. Suggestions are
      **read-only** — accepting one rides the existing PR 1.2 `override_field`/mass-update write, so it is durable +
      auditable and **no engine/DDL/golden/fixture changes** (determinism contract untouched; frontend-only PR).
      Prompt-injection-safe: record data is framed as untrusted DATA, output is pinned by a structured-output JSON
      schema, and the field allowlist + no-op filter are re-enforced server-side (a schema-ignoring model still can't
      widen the write surface). Cost-gated: steward-only, one entity at a time, low effort. **Bedrock is a base-URL +
      model-id swap** (`ANTHROPIC_BASE_URL` + `anthropic.claude-opus-5`) — noted as a follow-up, no code change here.
      Unit tests (11, `src/lib/enrich`) + enrichment Playwright spec (steward apply + no-op drop + viewer absence).

## DataGroomr full-parity program (started 2026-10-20)

`docs/research-datagroomr.md` (#202) ranks 88 platform-agnostic capabilities. A verified
gap pass (three code sweeps against all 88 rows) found ~50 PRs of work in 8 waves; this
section tracks them. Where our design is already better — non-destructive linking instead of
delete-the-loser, event-sourced unmerge instead of a 15-day Recycle Bin, frozen TF snapshots,
warehouse-native instead of Salesforce-locked — parity is recorded as achieved-by-design
rather than cloned. Full plan: `~/.claude/plans/keep-going-robust-floyd.md`.

Scope note: decision #1 (skip Salesforce-coupled features) still holds, and the AWS-only rule
now *replaces* rather than defers the third-party rows — Bedrock for AI, SES for mail, Location
Service for address, End User Messaging for phone, in-cluster DNS/TLS probes for email/website.

### Wave 0 — foundations + the bugs the gap pass found
- [x] **W0.8 Bedrock IAM — and the correction that it was never un-terraformable.**
      This program recorded Bedrock as "blocked on an account decision", which was half wrong.
      Probed 2026-10-21: the AWS provider (already pinned at 6.68.0, no bump needed) has
      `aws_bedrock_use_case_for_model_access` and `aws_bedrock_foundation_model_agreement` as
      first-class resources plus an `..._agreement_offers` data source, so model access is fully
      declarative. The mechanism was never the blocker; only the *content* is.
      Split accordingly, because only one half is a decision:
      **The permission half is BUILT AND APPLIED.** New `pod_identity.tf` + `bedrock.tf`:
      `er-dev-api-pod` role, an `invoke-claude` policy, and the Pod Identity association. Verified
      live — `aws iam get-role-policy` shows all three statements. Planned clean against real
      state (2 add, 0 change, 0 destroy) before applying. No cost, nothing outward-facing.
      Two design points worth keeping. The policy must allow the **foundation model in any
      region**, not just `us-east-2`: every current Claude model is reachable only through a
      cross-region inference profile (`us.*`/`global.*`), and Bedrock authorizes the underlying
      model in whichever region the profile routes to — pinning the region makes the call fail
      exactly when the profile does its job. And `ses.tf`'s own `er-dev-mail` role and association
      were **folded into one shared pod role**: the web+api Deployment has a single
      ServiceAccount (`er-api`), and Pod Identity binds one role per (cluster, namespace, service
      account), so mail and Bedrock have to be two policies on one role rather than two roles.
      **The model-access half is one input away**, gated on `var.bedrock_use_case_form` so
      everything else applies without it. That variable is the raw body of
      `PutUseCaseForModelAccess` — an opaque blob carrying a legal entity's name, website and
      intended use — and the agreement it unlocks is usage-priced (Claude Sonnet 4.6 in us-east-2
      listed $3/M input, $15/M output on 2026-10-21). Supplying it is the account owner's act, not
      something this repo can fabricate.
      **SES is blocked differently, and more honestly.** The account has **zero** Route53 hosted
      zones, zero registered domains and zero SES identities, so `var.ses_domain` has nothing to
      point at — this is an absent input, not a permissions gap. Note also that `ses.tf`'s runbook
      assumes Cloudflare DNS, which contradicts the program's own AWS-only rule; a Route53 zone
      would keep it inside AWS. For a dev instance the cheapest path needs no domain at all: SES
      sandbox can verify a single *email address* identity, which costs nothing and needs one
      click in the inbox.
- [x] **W0.1 worktree + this tracker section.** All program work happens in a dedicated
      worktree (`../ere-datagroomr-parity`) so it cannot disturb other local branches.
- [x] **W0.7 the webhook SSRF — the repo's own headline finding, closed.**
      `.claude/skills/security/references/ssrf-egress.md` has described this since the security
      suite was written (2026-10-19) and nothing had been built: `create_webhook` validated its
      URL as `!url.trim().is_empty()`, the dispatcher then POSTed job payloads to it with plain
      `reqwest`, and `networkpolicy.yaml` says egress is *deliberately* unrestricted. An admin in
      any tenant could aim the control plane at `http://169.254.169.254/latest/meta-data/...`, at
      `localhost`, or at any `*.svc.cluster.local` service. Found while scoping O3b (the webhook
      outbox): adding a delivery log with response codes would have turned a blind SSRF into a
      usable internal scanner, so the guard had to come first.
      New `er-server/src/urlguard.rs`: https-only, no embedded credentials, and — the part that
      matters — it **resolves the host and checks every resolved address**, not the name. Called
      at registration (so a bad URL is a 422 and never reaches the table) *and* before every
      delivery, because a name that resolved public at registration can resolve private at
      delivery; a registration-time-only guard does nothing about DNS rebinding. A host that does
      not resolve is deliberately *not* refused — registering an endpoint before standing it up is
      legitimate, and the delivery-time check is what makes that safe.
      **Redirects closed too.** `reqwest::Client::new()` follows up to ten hops, so
      `https://public.example/` 302-ing to IMDS defeated a first-hop check entirely. New
      `webhook::client()` disables redirect-following and adds a 10s timeout; every construction
      site uses it.
      **No off switch** — no env var, no config field, because a guard configuration can disable
      is a guard that gets found disabled. The one unguarded path exists so the delivery test can
      POST to `127.0.0.1`, and `tests/egress_guard.rs` fails if any file under `er-server/src`
      names it or builds a default client (comment lines excluded — the first version flagged its
      own explanation).
      Two bugs the enumerated tests caught while building it, both the kind a review would pass
      over: **`::1` was allowed**, because `to_ipv4` also maps the deprecated IPv4-*compatible*
      form and `::1` is in it, so unwrapping before judging turned THE IPv6 loopback into
      `0.0.0.1`; and **`https://[::1]/` was allowed**, because `host_str` keeps the brackets, the
      resolver failed on them, and "a lookup failure is not a rejection" waved it through.
      Tests, exactly where the security reference said they belonged: `urlguard.rs` units over
      every denied range one-by-one; ten refusal shapes folded into `pg_webhooks::webhooks_crud`
      (nothing stored, and an ordinary https endpoint still accepted); and a second phase of
      `pg_webhook_deliver` — refused, recorded legibly, and **the job still succeeds**, because a
      webhook pointed somewhere it may not go is the tenant's configuration problem and not a
      reason to fail their run.
      **Both are phases of existing tests, not new `#[tokio::test]`s, and that is a constraint
      worth knowing:** cargo runs a binary's tests on parallel threads, so two tests that each
      `DROP TABLE … CASCADE` the shared control-plane schema deadlock against each other — one's
      drop takes a lock the other's insert needs through the `orgs_history` trigger.
      `pg_scheduler_tick.rs` has said so since #211; this PR broke the rule in two files and CI
      caught it. The reason it got past local verification is sharper than the rule: **running the
      Postgres tier with `--test-threads=1` hides the whole class, and CI does not pass that
      flag.** Verify with the exact CI invocation
      (`cargo test -p dedupe-cli -p er-cli -p er-server -p er-entity-diff`), not a serialized one.
      **The honest limit, unchanged:** this is the application layer. Whether a pod can reach IMDS
      at all is a NetworkPolicy question answered by the manual `docs/infrastructure.md` §17
      probe, and none of these tests speak to it. The reference doc now says which legs are locked
      and which is not.
- [x] **W0.4 + W0.4a — the CSV upload path, and the gate that hid its absence.**
      **The bug:** `POST /v1/orgs/{org}/imports` had a complete BFF handler (multipart,
      256 MiB cap, 202/409/413/422 mapping) and a mock that answered it, while the router
      registered `get(imports_list)` only and er-server had **no multipart support at all**.
      Every tier was green; a real upload 405d. Uploading a CSV is how data enters the
      product, so this was the whole front door.
      **The fix:** a real `upload_import` handler — steward-gated, validates `source` against
      the **active published** config (naming the declared sources when it refuses), streams
      the part to `<drop_root>/.<source>.<delivery>.csv.part` and renames it into
      `<drop_root>/<source>.csv` so a dispatcher claiming the run mid-upload can never read a
      half-written file, then enqueues the `run_all_incremental` that picks it up. The cap is
      enforced twice (route body limit + running count) and lives on `AppState`
      (`with_max_import_bytes`) so the 413 path is testable without a 256 MiB request.
      **The gate:** `er-server/tests/mock_contract.rs` parses the router and the frontend mock
      and fails when the mock answers a route+method the router lacks — observed failing on
      exactly this one violation (all 44 other mock routes verified) before the fix landed.
      Tests: `pg_imports.rs` (happy path incl. byte-verbatim landing, no `.part` leftovers,
      re-upload is a second delivery, job params carry the source) + all six refusals
      (viewer 403, no `source` 422, undeclared source 422, no `file` part 422, oversize 413,
      suspended org 409).
- [x] **Found while testing: `source_priority` merge plans could never load their ranks.**
      `org_source_ranks` passed the org **name** to `pg::get_config_version`, which keys on the
      org **id**, so the lookup missed for every org that had a published config and the
      handler answered `404 no published config`. The whole policy was dead and untested.
      Fixed by routing both lookups through the already-fetched org row (new
      `org_active_config`, which takes `&OrgDetail` precisely so the id/name footgun cannot
      recur). Locked by `pg_source_ranks.rs`, verified by reintroducing the bug and watching
      it fail.
- [x] **W0.2 scheduler tick — schedules now actually fire.** `schedules` shipped as inert
      storage: the table, its CRUD and its cron validation all existed, but nothing ever read a
      row to enqueue anything and `last_enqueued_at` was never written. Every schedule in the
      product was decorative, and `config.correction_pass.cadence` with them — so the periodic
      correction pass never ran either.
      **The tick** rides the dispatcher loop (one loop means one thing to stop on SIGTERM),
      rate-limited to every 20s, and a tick failure is logged rather than taking the job loop
      down. Two deliberate properties: firings are keyed
      `sched:<schedule_id>:<yyyymmddhhmm>` and ride the queue's idempotency upsert, so **two
      dispatcher replicas produce one job** (`last_enqueued_at` is for operators to read, not
      what makes the firing unique); and there is **no catch-up** — a dispatcher down across a
      firing does not backfill it, because backfilling converts an outage into a burst of
      `run_all_full`s. Suspended orgs are excluded in SQL, so a suspended tenant accrues nothing.
      **A real cron parser** (`er-server/src/cron.rs`) replaces the charset-only `valid_cron`,
      which accepted `60 * * * *` and `*/0 * * * *` and stored schedules that could never fire.
      It implements cron(8)'s day rule (day-of-month OR day-of-week when both are restricted)
      and folds `7` to Sunday; the API now validates through the same parser the tick runs, so
      an unrunnable schedule cannot be created. 8 unit tests.
      **Attribution**: scheduled runs set `jobs.schedule_id` and `created_by = 'scheduler'` via
      a new `enqueue_scheduled` statement — the column existed for exactly this and nothing set
      it. `enqueue_scheduled` returns whether *this* call inserted (the minted id coming back),
      because the idempotent upsert returns a replayed row that is otherwise indistinguishable.
      **The cadence is wired**: publishing a config reconciles one config-owned schedule
      (`source = 'config:correction_pass'`) — created if absent, re-pointed if the cadence
      changed, never duplicated across republishes.
      Tests: `pg_scheduler_tick.rs` (fires on the matching minute, not on others; attribution;
      `last_enqueued_at` stamped; same-minute repeat is a no-op; next match is a new firing;
      a long gap does not backfill; disabled and suspended both stop it; `*/15` fires exactly
      4× an hour) + `pg_publish.rs` extended for the cadence schedule.
- [x] **W0.3 run modes stop lying, and the drift guard is connected.** Four disconnected
      things, one gap: `run_all_incremental` shared the `run_all_full` dispatcher arm;
      `job.params` (`skip_ingest`, set by six call sites) was never read; `write_run_start`
      hardcoded `mode = "full"` so the ledger could not tell the two apart; `runs.lexicon_hash`
      and `runs.rebuild_reason` were declared columns nothing ever wrote; and
      `er-versions::check_mode_preconditions` — the guard that decides whether an incremental
      run may extend the existing corpus — had **zero callers in the workspace**.
      **`skip_ingest` is honored**, which fixes a real failure: a publish-triggered rebuild
      re-read the drop directory and died with "no source deliveries found" whenever the
      deliveries had been cleaned up after landing.
      **The ledger is honest**: the executing mode, the lexicon hash the run scored under, and
      a rebuild reason when the run was promoted.
      **The guard runs** on every `run_all`, with escalation allowed: an incremental run whose
      config / model / std / survivorship / lexicon fingerprint drifted is *promoted* to full
      rather than refused (a refusal is just a failed job; a promotion is what the operator
      wants) and the ledger records why. Note this is groundwork — until E7 makes incremental
      genuinely cheaper, a promotion costs nothing. The guard has to exist first, or E7's
      cheap path would silently extend a corpus scored under different versions.
      Tests: a `lake_e2e` case asserting the ledger stores the fingerprint the guard computes,
      that an unchanged fingerprint proceeds as incremental, that a moved lexicon promotes to
      full and names the drifted field, and that the same drift refuses without escalation.
- [~] **W0.5 harness debt — partly done, the seed deliberately deferred.** The stale
      `make check-all` references are fixed (see W0.6). The full-stack tier stays dormant, but
      the reason is now understood rather than just noted.
      Recovering the deleted `frontend/dev/seed.py` was itself informative: step 4 of its
      documented happy path **uploaded the three sources over HTTP** to the Python server, which
      had that route. The Rust port dropped it, and the seed that exercised it was deleted at
      about the same time — so nothing was left to notice. That is the same hole W0.4 fixed,
      seen from the other side, and it is the clearest evidence for this program's false-green
      rule.
      What a revived seed needs, now that the upload endpoint exists: a workspace (lake catalog
      + data dir + drop dir + config file); `POST /v1/orgs` with the lake/model env; three API
      keys written to `.state/keys.json`; a published config version; the three committed
      `base_10` CSVs uploaded through `POST /imports`; a wait on the dispatcher; and
      `.state/expected.json` ground truth. Two things the old script did are no longer needed —
      the corpus generator (the `base_10` fixtures are committed) and `train` (point
      `ER_MODEL_PATH` at the committed `model_test_v1.json`, which is what every pg/lake test
      does). **One blocker remains**: the dispatcher has no `train` arm at all, so a seed that
      trains cannot work — `train` is in `SCHEDULABLE_KINDS` and falls straight through to
      "dispatcher does not handle job kind 'train'". That arm lands in Wave 3 (E10), which is
      the natural time to revive this tier.
      Deferred knowingly: the BFF→er-server seam that this tier would cover is now held by the
      W0.4a mock-contract gate plus the `pg_*` tests (real server, real Postgres), so the tier's
      unique value is smaller than it was before W0.4a existed.
- [x] **W0.6 `er-web` image push.** The ECR repo and `frontend/Dockerfile` both existed, and
      `images.yml`'s own header said er-web "is built by the frontend pipeline" — but
      `frontend.yml` had no image job at all, so nothing ever pushed an `er-web` tag and
      `make dev-up TAG=<sha>` (which needs both images) could not roll the BFF out. Added the
      `er-web` job beside `er-api`, with a `detect` job so each image builds only when its own
      tree moved (a workflow-level `paths:` filter cannot tell the two trees apart). Verified
      by building the image locally — it passes the Dockerfile's own
      `ERWEB_DATABASE_URL is required` migrate-script check — and by unit-testing the detect
      filter's cases, including the `frontend-notes/` prefix trap.
      Also cleaned the stale gate references this turned up: `make check-all` does not exist and
      was cited three times in `docs/plan-to-aws.md` and once in `docs/infrastructure.md`,
      alongside two Python contract tests deleted with the Python tree.

### Wave 1 — matching backbone
- [x] **E1 model registry + API (#34 model versioning, #21 field weights, #28 starter models).**
      A tenant's model was a **file path in its env** (`ER_MODEL_PATH`). There was no way to see
      which model an org scored against, no way to change it without editing the org's env, no
      history of what it used to be, and no way to read the m/u probabilities the engine had
      always carried — `grep m_probability` over the server and the frontend returned nothing.
      New `matching_model` entity: immutable versions, monotonic within a name, with
      `model_json` stored as text so it round-trips byte-exact (`model_hash` is taken over those
      bytes and determinism depends on them), provenance, and `state` draft→active→archived.
      `GET /models`, `POST /models/versions`, `GET /models/{name}/versions/{v}`,
      `POST .../activate`.
      Three decisions worth keeping:
      * **`model_version_label` is a column, and required on upload.** It is the identity
        stamped onto scored rows, and it must equal the `model_version` the frozen TF snapshot
        CSV is keyed by — the TF lookup joins `(model_version, tf_snapshot_id)` and a mismatch
        silently returns *no* term frequencies, changing scores rather than failing. Too
        dangerous to derive, so the caller states it. (Pleasant consequence: activating a new
        label is fingerprint drift, so W0.3's guard promotes the next incremental run to full.)
      * **Activation is one statement**, `CASE`-updating the target and the incumbent together.
        Two statements would leave a window with no active model; a partial unique index would
        be worse, since a single UPDATE moving both rows can transiently violate a
        non-deferrable unique index even when the end state is legal.
      * **Seeded from `ER_MODEL_PATH`, not from a copy.** Creating an org reads the exact file
        the dispatcher used to read and activates it, so a tenant created after the registry
        scores identically to one created before. Seeding is skipped when the org already has
        versions, because `POST /v1/orgs` is an idempotent upsert and versions auto-increment.
      The dispatcher resolves the active version in its async half (the blocking half cannot
      touch Postgres) and **falls back to the env file** when an org has no registry row — which
      is what keeps every pre-existing org and every fixture unchanged. The TF snapshot itself
      stays a file; the registry stores the settings document and the stamp, not the frozen
      frequencies.
      Weights (#21) are *derived* from the stored document rather than duplicated into columns,
      so they cannot drift from what the engine loads: per level, `m`, `u`, and the
      `log2(m/u)` the level contributes.
      Tests (`pg_model_registry.rs`): seeding + activation on org create; an upsert does not
      accumulate versions; the stored document is byte-identical to the seeded file; the weights
      view exposes m/u and log-odds; an unloadable document is refused at the API rather than
      inside a run; a missing label is refused; uploads land as drafts; activation archives the
      incumbent and exactly one version is ever active; re-activating is a no-op; a missing
      version 404s; an account model cannot be activated against a person org, and the refusal
      leaves the active pointer alone.
- [x] **E2 the config→model compiler (#4 rule matching, #21 field weights, #22 null policy,
      #24 derived keys).** The keystone. The `comparisons:` / `blocking:` DSL was authored,
      validated at publish, canonicalized into `config_hash` and rendered in the config studio —
      and **read by nothing that ran**. The level tokens (`exact`, `jaro_winkler:0.90`,
      `variant_match`, `username_exact`, `dob_same_year_month`) were free strings with no
      interpreter; the SQL the engine executed came from a model file on a disk the tenant cannot
      reach. New `er-compile` crate is that interpreter, plus `POST /v1/orgs/{org}/models:compile`
      which lands the result as a **draft** registry version (compiling is not activating).
      **Classic models are Fellegi-Sunter models.** A competitor ships two engines here — a
      weighted-sum "classic" matcher and a trained probabilistic one. We ship one: a declared
      model is an FS model whose m/u were set from weights instead of estimated, so a
      hand-weighted model still produces the same evidence waterfall, the same per-pair
      explanation and the same threshold semantics. No second code path to keep honest.
      **The scale is calibrated, not guessed.** Summing the trained model's own exact-level Bayes
      factors comes to ~60 bits, and a grid search over base_10 agrees: at 20 bits the compiled
      model finds 18 clusters where there are 10 (never enough evidence for pairs that disagree
      on a field or two); at 120 bits it finds 9 — it has begun *merging distinct people*. 55–60
      reproduces the committed membership exactly across every ratio tried, so the default sits
      inside the stable region. Louder is not safer.
      Two bugs worth recording, both caught by tests rather than review: the first m/u mapping
      fixed `u` and derived `m` upward, which hit the probability ceiling so an exact level and a
      fuzzy one came out *equally* weighted; and the first version emitted the config's blocking
      **key** where the engine wants a join **predicate**, so it blocked on `(email)` as a
      boolean. Blocking keys are now rewritten to `l.<expr> = r.<expr>` by a qualifier that skips
      string literals, function names, SQL words and already-qualified references.
      Determinism: `weight` and `null_policy` are `skip_serializing_if`, so every existing
      tenant config canonicalizes and hashes byte-identically — verified, no fixture touched.
      Tests: 11 unit (token→SQL against the committed model's strings, the weight→bits mapping
      and its monotonicity, probability bounds at the extremes, blocking-predicate rewriting
      incl. literals/aliases/nested calls, null-policy placement, TF on the exact level only,
      weight totalling); 7 integration (the compiled structure equals the committed model's
      level-for-level, the document loads in the engine, declared blocking reaches the model in
      order, parameters are pinned and labelled declared, equal weights pull equally, 3× weight
      is 3× the evidence, partial weights refused); and the one that matters — a `lake_e2e` run
      resolving base_10 with the compiled model and asserting it yields the **same 10 entities**
      as the trained model.
- [x] **E3 hard constraints and relationship exclusions (#18, #32).** One mechanism for all
      three shapes, because all three are the same kind of statement: *not a weight*. A weight
      says "this field matters more" and can always be outvoted by enough weak agreement — which
      is how a matcher merges two different people who share a city and a first name. A
      constraint cannot be outvoted.
      New `constraints:` config block, compiled into a conjunct on **every** blocking rule, so a
      violating pair is never generated as a candidate — the cheapest place to enforce it and
      the only place where "before similarity counts" is literally true:
      * `required_match: [tax_id]` — equal **and non-blank** on both sides. The presence half
        matters: without it, two records that both lack a tax id would "agree" on it.
      * `veto: [ssn]` — tolerate missing, refuse conflicting. Different from required_match
        because plenty of records have no SSN, but two *different* non-null ones are proof these
        are different people. This is the catastrophic-false-merge guard.
      * `exclude_when_equal: [household_id]` — the relationship exclusion. Records sharing a
        household or parent account are related, not duplicates, and they are the classic false
        positive in person resolution.
      Tier B (same as `blocking`: a re-score, not a retrain). Hash-safe — the block is
      `skip_serializing_if` empty, so existing configs are byte-identical.
      **A limit found by testing, and now asserted rather than discovered later:** a
      candidate-level veto does *not* survive transitive closure. On base_10, C001 and C009 have
      different emails so they are never compared — but both are compared to W001, whose
      placeholder email normalizes to NULL and so triggers no veto, and connected components
      then puts all three in one entity. The lake test asserts both halves: the vetoed pair is
      absent from `match_scores` (the real guarantee), *and* the three still co-cluster (the
      limit), with a message telling the next person that a passing stronger assertion means
      entity-level enforcement has landed.
- [ ] **E3b entity-level constraint enforcement** — feed config-declared vetoes into reconcile
      as synthetic `never` rules so the existing `never_cut` fixpoint separates them
      partition-level, with its escalation-to-review behaviour when every path is protected.
      The machinery is already built for steward assertions; this is wiring plus a
      within-cluster conflict scan.
- [x] **E4 type-aware comparators (#25).** Four new level tokens, no schema change — the
      standardizer already parses addresses into components and stores `birth_date` as a real
      `DATE`, so these had everything they needed:
      * `date_within_days:<n>` — a real date difference. A birth date entered a day out is a
        typo; as text those two values look nothing alike to an edit-distance comparator, and
        across a month boundary (03-31 vs 04-01) string similarity is actively misleading.
      * `date_transposed` — the US/EU convention swap, where "04/03/1985" is 4 March or 3 April
        depending on the form. **Neither existing comparator can see it**: string similarity is
        high but the dates are a month apart, and a day-window wide enough to catch it would
        also catch genuinely different people. The test asserts that a 2-day window does *not*
        match a transposition, which is the justification for the level existing.
        Note it is only *representable* when both numbers are ≤ 12 — `1985-14-03` is not a date
        and was never stored — and that is exactly when the ambiguity arises.
      * `numeric_within:<tol>` / `numeric_within_pct:<pct>` — absolute and relative closeness,
        `try_cast`-guarded so non-numeric text is rejected rather than erroring, and with a
        positive-denominator guard so two zeros cannot divide by zero.
      * `address_components` — compare the parsed number/street/postal jointly. "1420 Judah St
        Apt 5" and "1420 Judah Street #5" agree component-wise while looking only moderately
        similar as text.
      Tested by **running the generated SQL in DuckDB**, not by asserting the emitted string: a
      comparator that is valid SQL but means the wrong thing — comparing dates as text, dividing
      by zero — would pass a string assertion and be wrong in production.
      Deferred with a reason: **geo distance** needs `addr_lat`/`addr_lon`, which arrive with the
      Location Service geocoder in Wave 5 (V4). Adding the columns now would mean a fixture
      rebaseline for a comparator with nothing to read.

### Waves 2–8 (queued)
2. **Scale + prevention**
   - [x] **E6a blocking estimates (#3).** `GET /v1/orgs/{org}/blocking-estimate` — what a run
         would cost *before* it runs: pairs per rule, the marginal pairs each rule adds that no
         earlier rule found, the cartesian total it avoids, and the largest blocks. Viewer-level,
         because estimating a cost writes nothing.
         This is the direct answer to the competitor's hard **three-hour analysis cap**, whose
         documented workaround is "split your dataset with filters or raise the confidence
         floor" — two guesses made blind. Blocking decides a run's entire cost, so show the
         number instead of capping the clock.
         The estimator itself was already in `dedupe-core/blocking_analysis.rs`, reachable only
         from `dedupe-cli`; this lifts it onto the API, reading the rules from the **model** the
         run will execute rather than the config, since asking the config would estimate a run
         nobody is going to perform.
         The test asserts the claim that makes an estimate worth having: the candidate count
         **equals the number of pairs the run actually scores** at threshold 0 over the same
         corpus. An estimate that does not match the work is worse than none.
         It also found something: **base_10's own model carries a redundant blocking rule** —
         `birth_date || given_name` contributes zero marginal pairs over the first three. That is
         exactly the actionable output this feature exists to produce, and it is invisible
         without the breakdown. The test pins it, so a corpus change that makes the rule useful
         gets noticed.
   - [x] **E8 online resolve API (#37).** `POST /v1/orgs/{org}/resolve` — post an **unsaved**
         record, learn which entity it belongs to and how confident that is, before storing it.
         This is how the engine embeds in someone else's system instead of owning their data,
         and it is table stakes for offering resolution as a service.
         Viewer-level and read-only, the same reasoning as the pre-merge report: asking who a
         record is writes nothing. Resolving and ingesting stay deliberately separate, so a
         lookup can never mutate the corpus it is asking about.
         The payload goes through **the same standardization the corpus went through** — the
         declared source column mapping, the same normalizers, the same nickname lexicon — via
         `build_int_std_records` over a temp table, because a score is only meaningful between
         comparably prepared values. Candidates come from `predict_between`, which was built,
         golden-tested and reachable only from `dedupe-cli`. The response carries the
         per-comparison evidence waterfall, so a caller can explain the decision it just made,
         and the lake snapshot it read, so it can tell whether the corpus was fresh.
         The test asserts the two properties that decide whether it is usable: it **agrees with
         the batch path** (an obvious duplicate returns `auto_merge` and points at the entity the
         full run built, with every candidate inside that entity reporting the *same* entity id),
         and it **commits nothing** (partition unchanged, snapshot unchanged, no probe row in
         `raw_records`). Plus: a stranger is rejected rather than weakly matched, candidates come
         back best-first, and an undeclared source is refused naming the ones that exist.
         Two things fixed while building it: `seed_lexicon` creates its table outright, so the
         resolve path re-seeds (a session that already ran a pipeline would otherwise collide);
         and `entity_membership` has no active flag, so the entity lookup takes the newest row
         per record by `(assigned_at, run_id)` — a plain join would report an entity the record
         has since left.
   - [x] **E9a push records as JSON (#41's write half).** `POST /v1/orgs/{org}/records:ingest`.
         Before this, data could only arrive as a **file** — a pipeline had no way to hand
         records over programmatically, which is the integration a data service is asked for
         first.
         It takes the *same* path a file does: rendered into the source's declared CSV shape,
         staged under a temp name, renamed into the drop root, run enqueued. Landing records into
         the lake directly from the API would mean the API competing for the per-tenant writer
         lock the dispatcher holds, plus a second ingest path to keep honest against the first.
         The two paths now share `delivery_preconditions` and `finish_delivery`, so a delivery
         accepted one way cannot be refused the other.
         The CSV header comes from the source's **declared** mapping, not from the records' own
         keys: a record that omits a column lands as an empty cell rather than shifting every
         later column, and an **undeclared key is reported rather than dropped** — silently
         ignoring it would leave a caller believing it sent data the pipeline never saw.
         Tests: role boundary; the declared-column header; an omitted column lands empty; a value
         containing a comma, a quote and a newline is CSV-escaped (the failure that would
         silently corrupt every later column); undeclared column named in the refusal; empty,
         non-object and unknown-source bodies refused. The existing upload test still passes
         against the shared helpers, which is what makes the refactor safe.
         Capped at 10k records per push, because this body is buffered and parsed in memory while
         the upload path streams — a bigger load belongs in a file.
   - [x] **E7 true incremental (#36/#40) — and three bugs it exposed.** `run_all_incremental`
         re-scored the whole corpus: `dedupe-core::incremental::{predict_between,predict_within}`
         were built and golden-tested, and **no production caller existed** outside the
         single-record `resolve_record` probe. `PipelineInputs.mode` only labelled the ledger row.
         **What ships:** the two-pass path DesignDoc §4.3.4 specifies —
         `predict_between(unchanged, delta)` then `predict_within(delta)` — unioned into the
         existing `__match_src` staging and the same declared MERGE, so the write path is
         unchanged. Pairs between two unchanged records are *not* re-scored: with the model and
         the TF snapshot both frozen, re-scoring them could only reproduce the stored number.
         That is what makes this **equal to** a full pass rather than an approximation, which is
         DesignDoc §4.5.6's INV-EQ. Pass 2 is load-bearing on its own — without it two new
         records that match each other but nothing in the corpus are never compared, so "this
         batch contains a new duplicate pair" silently resolves to two entities.
         **The delta is the `scored_records` ledger** (new lake entity), not the ingest batch id:
         a run that lands records and then dies before scoring them would leave them looking
         "already ingested" to any batch-based definition and they would never be scored at all.
         Diffing the corpus against *what was actually scored* has no such hole, and it makes a
         changed record (same key, new `content_hash`) and a departed one (tombstoned, so
         standardize drops it) fall out of the same comparison. The ledger is written only at the
         END of a successful pass, and when it is absent or was written under another model
         identity the pass says so in `MatchDelta::full_reason` and scores everything.
         **Reconcile and golden stay whole-corpus, deliberately.** Clustering is a function of
         the edge set and connected components over the above-threshold graph is linear in edges;
         the cost incremental exists to avoid is the *quadratic* pair generation in match.
         Re-clustering from the full edge set is also what makes the partition provably equal to
         a full run's. `clustering::incremental_cluster` therefore stays unwired, with reason.
         **Bug 1 — `is_active` was a lie.** Written `true`, never written `false` by anything in
         the workspace. Both passes now deactivate what they are about to supersede *before*
         re-scoring (`match_score/update/invalidate_stale`), so a pair that falls below
         `review_low`, or whose record left the corpus, stops reading as current. A full pass
         supersedes everything and the MERGE brings back whatever still clears the threshold.
         **Bug 2 — deletion did not delete.** `entity_membership` is upsert-only, so a tombstoned
         record kept its row and still read as a member of an entity. Pruned against the
         standardized corpus (`entity_membership/delete/absent_from_corpus`) — not against the
         clustering node set, because that also excludes *suppressed* records, which are still in
         the corpus and must keep their membership. Found by writing the test, not by reading.
         **Bug 3 — `run_all` could not run twice in one session.** `seed_lexicon` used a bare
         `CREATE TABLE`, so the second call died on "table nickname_variants already exists" —
         and a base run followed by an incremental batch *is* two runs in one session. Now
         re-seedable.
         Tests (all four failure modes observed failing first): `an_incremental_batch_lands_where_a_full_rerun_would`
         resolves the **`incremental_batch` fixture** — committed for T-INC-3 and left with no
         consumer when the Python tests were removed — through two universes and asserts
         bit-exact agreement on which pairs are scored, on their probabilities, on the partition,
         *and* that the shared answer equals the fixture's ground truth, so "both paths agree"
         cannot be satisfied by both being wrong. Sabotaging pass 1 loses exactly the ten
         new-vs-corpus pairs including the declared bridge; sabotaging pass 2 loses exactly
         `(billing:B009, webforms:W010)` — the pair `attribution.csv` marks `pass2`.
         `an_incremental_pass_scores_only_the_batch_and_still_finds_new_vs_new_pairs` asserts the
         pass is actually cheap (6 records of 29) and reads the roles from `attribution.csv`
         rather than from the implementation, because equivalence alone is satisfiable by a pass
         that quietly re-scores everything — the most likely way for this to rot.
         `a_deleted_record_stops_being_resurrected_by_its_own_stale_scores` covers bugs 1 and 2.
         `er run-all` gains `--mode` and `--allow-escalate`, behind the same drift guard the
         control plane applies; DesignDoc specified both and neither existed.
         Still unbuilt: tombstone *event* attribution in `entity_events` (already a documented
         gap in `run_reconcile`'s own note), and retiring `entities` rows that lose every member.
   - [x] **E6b segment scoping (#54) — splitting the work without splitting the data.**
         The incumbent caps a run at 3 hours and tells you to split your dataset. Splitting the
         *corpus* is the obvious reading and it is destructive: every cross-segment duplicate
         becomes invisible, and the records outside the slice lose the entities they are already
         in. E7's machinery allows the better answer — split the **work**: a `segments` config
         block maps a name to a predicate, and a scoped pass scores `segment × whole corpus`
         plus `segment × segment`, reusing the stored scores for pairs where neither endpoint is
         in the slice. Frozen model + frozen TF make those stored numbers exactly what a
         re-score would produce, so **a scoped run leaves `match_scores` where a full run would**
         and running each segment in turn covers the corpus with no pair unexamined. Same
         INV-EQ argument as E7, same proof obligation.
         The scope is widened to include anything new or changed wherever it lives. Deliberate:
         the alternative is to score a slice and then write a ledger claiming the records
         outside it were scored, which the next pass would believe.
         **A run names a segment; it never carries SQL.** The predicate lives in the published
         config, where it is parsed and column-checked beside the blocking rules — publish is
         the only gate it ever passes, because nothing downstream re-parses it. A request
         carrying SQL would hand every steward an injection point into the engine's own
         queries. Validated twice: `create_job` 422s an unknown name *at submit*, listing the
         declared ones (a typo must not become a queued job that fails minutes later), and the
         dispatcher re-resolves against the config it is about to run under, because a schedule
         stores its params and the config can be republished without it.
         `GET /v1/orgs/{org}/segments` makes the names discoverable, with each predicate — an
         operator choosing how to split a long run needs to see what each slice covers, and it
         is their own config. `er run-all --segment` for the CLI.
         **Also fixed here, surfaced by segments:** E7's ledger sync recorded the *whole corpus*
         as scored. Harmless for an ordinary incremental pass (the rest already had matching
         rows) but a lie for a scoped one, and the next pass would act on it. The upsert now
         covers only what the pass scored; retirement still compares against the whole corpus,
         so the two now read deliberately different staging tables.
         Tests: `resolving_each_segment_in_turn_lands_where_one_full_run_would` segments
         base_10 on `source_system` — chosen because **17 of its 19 pairs are cross-source**, so
         the naive corpus-filtering implementation loses every one of them (observed: sabotaging
         it drops exactly those 17) while two same-source pairs keep the within-slice pass
         covered. Run with no new data on purpose, so three passes over an unchanged corpus must
         round-trip `match_scores` bit-identically. Plus `pg_segments.rs` (name-not-SQL in the
         job params, 422 on unknown/empty/wrong-case names naming what exists, discovery,
         viewer 403, and a bad predicate refused at publish with nothing going live) and
         er-config tests that an unused `segments` block leaves `config_hash` untouched while a
         declared one moves it.
   - [x] **E5a — activating a model version now actually takes effect.** Found while scoping
         ensembles, which forced the question "which models' scores are allowed to cluster?"
         and revealed that nothing had ever answered it. Scores are keyed by
         `(pair, model_version, tf_snapshot_id)`, so a run under a new model writes **new**
         rows and left the old model's rows `is_active` — and neither edge read filters by
         model (`auto_merge_edges_live`, which reconcile clusters from, and `active_page`,
         which the duplicates surface pages, both take every active row). Measured on base_10:
         19 pairs became **38 active scores**, two models voting at once, silently and for good.
         The sharp consequence is not the row count: **a model built to be stricter could not
         change anything**, because the looser model it replaced went on merging the records —
         which negates the point of the registry (#214), where activation is the explicit,
         audited act. A run now retires every active score whose model identity is not in its
         live set (`match_score/update/retire_other_models`). Retiring, not deleting: the
         previous model's verdicts stay in the table, inactive and stamped with the run that
         retired them, so a review can still cite them. Rolling back to an earlier model means
         a re-score, which the drift guard already forces on any model change.
         The live set is a *list* in the signature because an ensemble is a list — that is the
         seam E5b plugs into, not speculation.
         Test: `activating_a_new_model_retires_the_one_it_replaces` activates a deliberately
         stricter model (a much harder prior) and asserts the thing an operator cares about —
         exactly one model identity active, the old scores retired-not-dropped and stamped, and
         the partition **splitting** into more entities than the loose model produced.
         Sabotaging the retirement reproduces the original bug exactly (`v0001` 19 active beside
         `v0002-strict` 17).
   - [ ] E5b ensembles + cascades (#31 — N models per run, `then_run` cascade vs `and_run`
         union, per-model threshold; E5a's live-set seam is the entry point),
         E9b import-time screening + update rules (#38/#39), E9c S3 auto-ingestion (#47).
3. **Learning loop**
   - [x] **E10a the `train` job, which the dispatcher had no arm for.** `train` has been in
         `SCHEDULABLE_KINDS` since the queue shipped — so the schedule API accepted it, and every
         tier-C config publish (one whose `comparisons` changed) enqueued one — while
         `dispatcher.rs` had **no `"train"` arm**. It fell through to `other => Err("dispatcher
         does not handle job kind …")` and failed every single time.
         The failed job was the smaller half. A tier-C publish enqueues `train` AND a
         `run_all_full` rebuild, so the rebuild went ahead and re-resolved the whole corpus
         **against the model the publish was meant to replace** — a config change that looked
         applied and was not.
         `train_and_register` sits in the dispatcher's async half for the same reason
         `apply_corrections` does: its OUTPUT is a registry row, which needs the Postgres client
         the blocking half cannot hold. The sync training itself goes to `spawn_blocking`.
         Two deliberate properties. **The new version is a DRAFT, never activated** — activation
         is the explicit, audited act the registry exists for (#214), and training is not consent
         to re-cluster a tenant's data. **The TF snapshot is APPENDED, not overwritten** —
         `build_frozen_tf_tables` filters the CSV by `(model_version, tf_snapshot_id)`, so
         snapshots coexist and each model reads only its own; replacing the file would retune the
         *active* model's frozen term frequencies underneath it, which is precisely the guarantee
         that makes an incremental or scoped run equal to a full one (E7/E6b).
         The template is the **active** version's settings, so training refines what is live
         rather than whatever is on disk; `ER_MODEL_PATH` remains the fallback for an org created
         before the registry.
         Tests: `pg_train.rs` runs real training over a standardized base_10 lake and asserts the
         draft is registered and traceable to its job, the active version is untouched byte for
         byte, the learned m/u probabilities are present and in range (a trained model identical
         to its template means training did nothing), and the TF file is appended — existing rows
         unchanged and first, new snapshot id present, exactly one header.
         Plus **`dispatch_coverage.rs`**, the gate for the general bug: every kind in
         `SCHEDULABLE_KINDS` (and `apply_corrections`) must be routed, checked textually because
         the arms are string literals and the compiler has no opinion about a missing one. It
         self-checks that its own scan can see a `match`-arm kind, a pre-branch kind, and not an
         invented one — then observed failing with `["train"]` when the arm is removed.
   - [ ] E10b pair labels + uncertainty-sampled labelling queue + label-driven training
         (#19/#20), E10c per-version evaluation (precision/recall/ROC — `accuracy.rs` is built
         and has no API), related-record features (#26), household/link entities (#33).
4. **Operating at scale**
   - [x] **O2a job retry + failure classification (#58's retry half).** `attempt` and
         `max_attempts` shipped and **nothing ever retried** — a failed job stayed failed until
         a human hit resume.
         Worse, the UI was already built for a world where this worked. `job-detail.tsx` renders
         an error panel keyed on `error_class` and tells the operator "transient failures retry
         automatically"; `dashboard-content.tsx` counts `precondition` failures and names the
         last failure's class; the runs and admin-queue tables each have a column for it;
         `lib/query/hooks.ts` declares the field. **`jobs` had no `error_class` column at all**,
         so none of it could ever display anything — a failed job showed no reason whatsoever,
         and the retry promise was false twice over. (Same family as the imports false-green,
         but a *field* rather than a route, so the W0.4a gate could not catch it.)
         Added `error_class` + `error_detail`, and a `fail_or_retry` statement that classifies
         and decides in **one** statement — a read-then-write would let two dispatchers each
         conclude they were the last attempt. Backoff is exponential and capped (5s→300s).
         **Only transient failures retry**, which is what makes the UI's promise literally true:
         retrying a precondition (no model yet) or a bad config burns attempts on something that
         cannot change without a human, and the dashboard already reframes preconditions as
         expected rather than alarming. Classification matches the strings this codebase
         actually emits, and anything unrecognised stays `internal` and is *not* retried —
         silently retrying an unknown failure turns one confusing log line into three.
         `claim` now counts the attempt as it hands it out, so a worker that dies without
         reporting has still used one; `mark` clears a stale error on success, since the UI
         renders `error_class` whenever it is present. One webhook per **outcome**, not per
         attempt — three notifications for one incident is how an integration learns to ignore
         them.
         Tests: the full classification taxonomy against real dispatcher messages; a transient
         failure requeues with a deadline and is **provably not claimable yet** (otherwise the
         "backoff" is a hot loop); attempts run out and the last one fails terminally; a
         precondition fails immediately without burning attempts; a success clears the error.
         Caught by the gates on the way: `er-entity-diff`'s live-schema fixture and the OpenAPI
         snapshot both flagged the new columns, and five `returns: entity` job statements had to
         select them — the generated row mapper reads every entity column regardless of the
         SELECT list.
   - [x] **O3a the webhook event catalog (#43's correctness half).** A subscription was an
         unvalidated free-form array, and `webhooks_for_delivery` merely tests whether the
         subscriber's list contains the fired type — so a tenant could subscribe to
         `entity.merged` (nothing emits it) or `job.suceeded` (a typo) and wait forever for a
         delivery that was never coming. Nothing told them, and nothing *could*: there was no
         list of what exists.
         How invisible that was: the repo's own `pg_webhooks.rs` subscribed to
         **`run.succeeded` and `review.opened`**, neither of which this system emits. The test
         suite encoded the hole.
         Added `webhook::EVENT_CATALOG`, validation at registration (422 naming the events that
         *do* exist), and `GET /v1/orgs/{org}/webhooks/events` for discovery — the same list
         registration checks, so the two cannot disagree.
         **The catalog had to be made true, not just written.** The first draft listed
         `job.running`; the dispatcher marks a job running and never emits for it, so that would
         have reproduced the exact bug. And `job.canceled` was listed before it was real —
         cancel was the one terminal state that delivered *nothing*, so a system waiting on an
         outcome waited forever. Cancel now emits, and the catalog is exactly the three terminal
         outcomes. A test reads the emit sites rather than trusting the list, and asserts
         `job.running` stays unsubscribable until something fires it.
         Still to come in O3b: the outbox + delivery log + backoff (today a delivery failure is
         collected and discarded), and entity-level events off the `entity_events` ledger, which
         wants the outbox first — thousands of un-retried POSTs per run would be worse than none.
   - [ ] O2b emailed run reports (#58's reporting half), O3b webhook outbox + entity events,
         O4 write-back handler (#44), O5 tags/approval (#51), O6 bulk depth (#9/#62/#52),
         O7 insights + time series (#45/#57/#85), O8 workflows + triggers (#55/#56).
5. **Cleanse + verify** — transform rules (#61), international standardization (#60),
   per-record DQ (#63/#73), the AWS-native verification suite (#64–#72).
6. **AI on Bedrock** — Bedrock wiring + per-org BYO LLM (#77), LLM adjudication (#27),
   NL rule authoring (#29), AI schema mapping + model setup (#48/#35), staged agent
   write-actions (#81).

   **Blocked on one account-level decision, verified 2026-10-20.** Probed live with the
   `er-builder` profile against `us-east-2`:
   * Claude models **are** present in the region — `list-foundation-models --by-provider
     anthropic` returns Fable 5, Opus 5, Sonnet 4.5/5, Haiku 4.5 and more, and the matching
     `us.anthropic.*` inference profiles are `ACTIVE`.
   * But `get-foundation-model-availability` reports
     `authorizationStatus: NOT_AUTHORIZED`, `agreementAvailability: NOT_AVAILABLE`,
     `entitlementAvailability: AVAILABLE`, `regionAvailability: AVAILABLE` — i.e. the region
     and the entitlement are fine and **the account has not accepted the model agreement**.
     `bedrock-runtime converse` fails with `ValidationException: Operation not allowed` for
     both the bare model id and the inference-profile id, and
     `get-use-case-for-model-access` answers "You have not filled out the request form".

   The fix is two calls — `aws bedrock put-use-case-for-model-access` (the Anthropic
   use-case form: company name, website, intended use) then
   `aws bedrock create-foundation-model-agreement --model-id … --offer-token …` using a token
   from `list-foundation-model-agreement-offers`. **Deliberately not run**: it accepts a
   priced legal agreement on the account and submits company details, which is the owner's
   decision and not something to fabricate. Everything else in Wave 6 can be built and tested
   against the existing `ANTHROPIC_BASE_URL` seam and the Playwright mock's scripted
   `/anthropic/v1/messages`, so this blocks live verification, not progress.
7. **Governance + platform** — config promotion (#76), MCP server + service identity
   (#79/#78), embeddable widget (#59), saved views (#87), metering (#88), field-value
   rollback UI (#12/#13).
8. **Review-UI depth** — group merge editor (#8), pre-load integrity check (#46), insights
   UI (#45/#57), verify-before-export (#86 de-Salesforced).

**The false-green rule** this program runs under: every new gate is observed *failing* before
it is trusted, and a mock-tier green is never evidence that a backend exists. Any feature
spanning BFF → er-server → engine needs at least one test that crosses the whole seam.

## Explicitly skipped / deferred (not building)
- Salesforce / Marketo / Lightning / lead-convert (decision #1).
- SOC 2 / HIPAA / GDPR attestations (#75) — procurement artifacts, not code. The buildable
  half (data residency; running inside our own VPC) we already have.
- Third-party enrichment orchestration (#82–#84) and partner verification APIs
  (ZoomInfo/Apollo/Melissa) — blocked by the AWS-only rule; the AWS-native equivalents are
  Waves 5–6.
- Credit *sales* mechanics (#88) — replaced by a usage-metering ledger.
- Public prod ingress (ALB/WAF) — domain-blocked, unrelated to parity.
- ~~SES email~~ — built (`POST /v1/email`, SES API over Pod Identity); the terraform apply is
  Wave 5's step zero.
- ~~AWS-native address/phone verification~~ — now in scope (Wave 5, V4).

## Change log
- 2026-10-15 — Program kicked off; plan approved; PR 0.1 opened (frontend CI gate + this tracker).
- 2026-10-15 — PR 0.1 merged (#97): frontend CI gate green (incl. pinned-image visual baselines).
- 2026-10-15 — PR 0.2 opened: nightly coverage (cargo-llvm-cov + vitest), floors as a non-regression backstop.
- 2026-10-15 — PR 0.2 merged (#98). PR 0.3+0.4 opened: envs/prod (plan-validated 61-to-add, not applied) + builder
  key §8.2 prod-guard (applied + verified live). Prod is apply-ready; builder key can no longer touch er-prod-*.
- 2026-10-15 — PR 0.3+0.4 merged (#99). Phase 0 nearly done (0.5 blocked on the prod-apply decision). PR 0.6 opened:
  password-reset pages + PUBLIC_PATHS fix + login link; completes the tenant username/password workflow.
- 2026-10-15 — PR 0.6 merged (#100). Phase 1 scoping found a prod bug: steward-action drain was never wired into the
  Rust dispatcher. Opened PR P1 (fix) to wire it before building mass-ops on that path.
- 2026-10-16 — PR 2.3 (AI enrichment) implemented on `feat/ai-enrichment`: BFF enrich route + steward entity-view
  panel, suggestions ride the existing `override_field` path (frontend-only; determinism contract untouched).
  Local gate green (`pnpm check`: eslint+prettier+tsc + 49 vitest incl. 11 new); enrichment Playwright spec green and
  no regression in the entity-detail suite (incl. the viewer `@visual` baseline — the panel is steward-gated).
- 2026-10-16 — Account epic 1.1a (hand-written approach chosen; prod stays plan-only): `entity_type` discriminator
  (hash-safe) + Account attribute set + parallel Account dbt tables. Full `cargo test --workspace` green; only
  `model_registry_golden.json` changed (config_golden + dbt_sources + base_10 person fixtures byte-identical).
- 2026-10-16 — Account epic 1.1d (frontend browse + view) closes the epic for the read path. Read API serves either
  entity via a request-selected `GoldenView` (default Person → person responses byte-identical); UI reads the org's
  `entity_type` from config and renders entity-aware records/detail with person DOM byte-identical (Phone cell keeps
  its no-`truncate` class → `@visual` unchanged). Gate: `pnpm check` (lint+prettier+tsc + 56 vitest incl. 8 new),
  er-server tests (new `acct_read` + person `pg_golden_read`), fmt/clippy/doc clean, all determinism fixtures
  byte-identical. Deferred to 1.1e: entity-aware new-source wizard + secondary display surfaces (person-safe today).
- 2026-10-16 — Account epic 1.1e **fully closes the epic (write/compose UI)**: the new-source wizard and every
  secondary display surface (duplicates list, merge dialog, enrich labels, field-compare grid, command palette,
  campaigns) now read `useOrgEntityType` and render the account vocabulary (company_name/domain) instead of the
  hardcoded person lists; the `/duplicates` read path gained its own `GoldenView.group_preview` entity-switch
  (`read::duplicates(…, &view, …)` via `org_lake_golden`). Person default throughout → person DOM byte-identical,
  `@visual` baselines unchanged. Gate: `pnpm check` (lint+prettier+tsc + 59 vitest incl. new account mapping units),
  functional `account.spec.ts` green on all 3 projects + person regressions (duplicates/wizard/palette/campaigns/
  records/reviews/enrich) green, fmt/clippy(`--workspace --all-targets -D warnings`)/doc clean, `acct_read` e2e
  extended for account `read::duplicates`. No determinism fixture/golden changed.
