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
      Both landed: 1.6b below, and **1.6c** (generalized groups) with the DataGroomr wave.
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
- [x] **W0.1 this tracker section.** The dedicated worktree this originally called for
      (`../ere-datagroomr-parity`) no longer exists and was not used: every program PR has
      been a branch off `main` in the primary checkout. Recorded because the instruction to
      work in it outlived the worktree, and a reader following it would create a second one.
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
- [x] **W0.4b the half of that gate it could not see — a UI shape may not invent fields.**
      `mock_contract.rs` compares route and method, and that is all it compares. The class it
      misses recurred **seven** times in this program, always as *the frontend states a contract
      the backend never fulfilled*, and always green in all three mock-backed tiers — because the
      mock encodes the frontend's belief about the API rather than the server's behaviour. The
      sharpest instance: `JobRow` declared `progress: { stages?: ... }` **required**, three
      components read `job.progress.stages` unguarded, the mock fabricated the object at 11
      sites, and er-server sends no such field and has no producer for one (see W0.5c).
      `frontend/tests/unit/wire-contract.test.ts` closes the field half. The authority is
      `src/lib/entities/bindings/*` — generated by `er-entity-gen` from `entities/*/entity.yaml`
      and drift-gated by `er-entity verify`, so it cannot disagree with what the server
      serializes. The check is **compile-time**: `NoUndeclaredFields<Ui, Entity, Composed>`
      resolves to `true` when a UI shape's keys are a subset of its entity's and to an error
      object *naming the offending keys* otherwise, so `tsc --noEmit` fails inside the already
      required `lint + types + unit` context — no new job, no new runtime. Observed failing:
      dropping `progress` from the allowlist yields `TS1360: Type 'boolean' does not satisfy the
      expected type '{ UNDECLARED_WIRE_FIELDS: "progress"; }'`.
      Two design points worth keeping. **BFF-composed fields are declared, not assumed**: the
      third type argument lists them with a comment naming where each comes from — `AdminOrgRow`
      legitimately carries `display_name`/`registered` (the erweb tenant registry, a different
      database) and `has_credentials` (the credential vault), which er-server correctly knows
      nothing about. And the gate **proves its own non-vacuity**: a `@ts-expect-error` case
      asserts an invented field does *not* type-check, so if the type ever collapses to a
      tautology the directive becomes unused and `tsc` reports TS2578. That assertion earned its
      keep immediately — it caught its own first draft, where the directive sat at the top of a
      multi-line comment and therefore suppressed a comment line instead of the statement.
      **What is left, and why.** Envelope and ordering mismatches: W0.5b's `forward<unknown[]>`
      over `{jobs: [...]}` and W0.5d's newest-first mock over an oldest-first endpoint are
      per-path facts, and `frontend/src/lib/api/openapi.json` documents **zero** paths —
      `er-server/src/openapi.rs` registers `components.schemas` only, deliberately. Those 12
      schemas are these same entity structs, which is exactly why the field half is buildable
      today and the envelope half needs `#[utoipa::path]` on the handlers first. The mock's two
      remaining fabrications (`outcome`, `exit_code` on jobs — absent from er-server, read by no
      component) are deliberately left as the fixture that half will be built against.
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
- [x] **W0.5b the full-stack tier, revived — and what it found in its first hour.**
      W0.5 recorded one blocker ("the dispatcher has no `train` arm"), which E10a cleared. That
      was the visible half. The whole substrate had gone with the Python tree: `up.sh` booted
      `uvicorn erserver.api` + `python -m erserver.dispatcher`; `env.sh` exported **eight**
      `ERSERVER_*` variables nothing in the Rust workspace reads (the control plane takes exactly
      three — `ER_PG_DSN`, `ER_OPERATOR_TOKEN`, `ER_SERVER_ADDR`); and `compose.yaml` built its
      MinIO and catalog-Postgres services from `docker/Dockerfile`, which no longer exists. `run.sh`
      called a `make frontend-seed` target that had been deleted. None of it could start.
      **The stack is now one Postgres and a local DuckLake** (catalog file + data dir, exactly what
      every `lake_e2e` case uses), with `ER_DISPATCH_IN_PROCESS=1` so one process serves the API and
      drains the queue — which makes the "half-up stack" state `run.sh` used to detect unreachable.
      MinIO and the catalog Postgres are gone; neither local gate exercised S3 or a Postgres catalog
      before either (`make check-e2e`'s lake tests are local too), so no existing coverage is lost,
      and the real AWS environment still covers both. Stated rather than hidden.
      **The seeding order is forced, not chosen**, and this is the part worth remembering. A first
      publish is tier C, so it enqueues `train` + `run_all_full` — and that rebuild is
      publish-triggered, so W0.3 sets `skip_ingest` on it: it never reads the drop directory, and on
      a new org both jobs fail for want of a corpus. The other direction is just as closed:
      `POST /imports` is the route that ingests, and `delivery_preconditions` refuses it until a
      config is already published. So something has to seed the lake before the first publish, and
      only the `er` CLI can. `frontend/dev/seed.mjs` does that, then publishes, and both
      publish-triggered jobs succeed.
      **The bug the tier found, which is the argument for having it.** `GET /api/orgs/{org}/jobs`
      did `forward<unknown[]>(...)` and passed the body through — a type ASSERTION about an HTTP
      body TypeScript cannot check. er-server returns `{"jobs": [...]}`; the UI hook types it
      `JobRow[]`, the Playwright mock returned a bare array, and the specs call `.some()` on it.
      Three consumers agreed on "array" and only the real backend disagreed. The damage was not a
      blank table: `dashboard-content.tsx` does `(jobs.data ?? []).find(...)`, and `?? []` does not
      apply to a non-null object, so **the whole dashboard crashed on render**. Fixed on both sides,
      because unwrapping alone would have broken the functional tier — the mock has to be faithful
      or it manufactures green. The W0.4a gate compares routes, **not response shapes**; a
      shape-level check would have caught this and is still unbuilt.
      Verified: 10 entities / 23 records live, matching base_10's committed truth;
      `duplicate_groups: 9` derived from the fixture independently equals the engine's count; the
      seed is idempotent. **8 of 15 cases pass, and the tier is left red on purpose** — the
      remaining failures are real findings, each documented in `docs/frontend-testing.md` with its
      diagnosis: a suspended org still accepts jobs (**202, not 409** — `create_job` has no
      suspension guard while publish and imports both do), every human action is attributed to
      `key:<uuid>` rather than the person (`Principal.actor` comes only from the API key and the BFF
      forwards no identity), tenant self-onboarding 503s (auto-provisioning was never ported), one
      cold-start timing flake, and one case not yet diagnosed. Tuning the specs to hide those would
      defeat the purpose of reviving the tier.
- [x] **W0.5c the tier's last red cases — a silent stall, a required field nothing sends.** The
      wizard case that W0.5b left "not yet diagnosed" had two causes stacked, and the first hid
      the second. The panel sat on "Create draft" until the 120 s `waitForURL` expired having
      **sent no request and raised no error**: `activate()` opened with
      `if (!activeConfig.data) return;` — a silent return inside an async pipeline whose UI state
      was already `running` — and neither action button was gated on that read, so a click landing
      before `GET /config` resolved was a no-op that looked identical to work in progress. The
      mock answers in ~1 ms and always wins that race; BFF → er-server → a fresh per-request
      Postgres connection does not. Both buttons now gate on the base config (Playwright's click
      waits for enabled, so the race is unreachable rather than merely legible) and all three
      silent returns set an error.
      **What that unblocked is the sharper finding.** With the stall gone the spec reached the run
      page and the page *threw*: `data.progress.stages` with no optional chaining, where
      er-server sends no `progress` key at all. `JobRow` declared it **required**, three call
      sites read it unguarded — run detail, the runs list's `lastStage`, the dashboard's
      active-job line — and the Playwright mock fabricated it at 11 sites, so all three were green
      in every tier and broken against every real backend. Per-stage progress has no producer in
      the product: the `run` ledger is one row per run with start/end, and the stage-level
      tracking this was shaped for went with the Python tree. Making the field optional let `tsc`
      enumerate the readers; the mock's fabrication is stripped, and `runs.spec.ts`'s assertion on
      the stage name `match` — which only the mock could ever satisfy — now asserts the waiting
      state the real server produces. The stages card already had that empty state.
      Two spec fixes, both honest rather than cosmetic: the unmerge case asserted exactly
      `memberCount - 1` members after peeling one record, which encodes a property the engine does
      not promise (re-clustering runs from the full edge set, so peeling the only hop holding
      another member in takes that member too — a 5-member group legitimately lands at 3); it now
      asserts strictly fewer, which is what the comment above it already claimed. And
      `ER_DEV_PG_PORT` makes the stack's Postgres port overridable, because 5433 collides with the
      engine's own lake-E2E instructions and `up.sh` died on "port is already allocated" before
      anything else ran.
      Verified: the wizard case passes against the real stack, and `make frontend` is green at 362
      passed. The one red case left over was a real product bug rather than spec drift, and it is
      W0.5d below.
      **A trap worth naming, found by this PR failing CI after `make frontend` passed.**
      `playwright.config.ts` sets `ignoreSnapshots: process.platform !== "linux"`, so every
      `@visual` assertion is *ignored* off Linux. A green local frontend gate is therefore **never**
      evidence about a baseline — only `make frontend-baselines` (the pinned Linux image) or CI is.
      Removing the mock's fabricated `progress` emptied the runs list's last-stage column and
      invalidated exactly one baseline; local said 362 passed, CI said 1 failed. Added to
      `docs/frontend-testing.md` alongside the semgrep-parse-error and `| tail` traps.
- [x] **W0.5d the receipts panel answered with the oldest deliveries.** `ingest_batch/read/page.sql`
      is a FORWARD keyset cursor — `ORDER BY id`, `id > $after` — which is right for walking a
      history and wrong for a latest-activity panel. **Three** consumers read it as though it were
      newest-first: the "did my file do anything?" receipts card via `?limit=20`; the assistant's
      `get_import_receipts` tool, whose own declaration promises "newest first" and which therefore
      handed the model the oldest 20 receipts; and the Playwright mock, which served the fixtures in
      their stored descending order regardless — so the panel looked correct in every tier while the
      real endpoint handed it the other end. Past 20 ingest batches a newly delivered source could
      never appear on the page that exists to say whether the delivery did anything. Found by
      `grow-your-data.spec.ts:147` once repeated tier runs pushed acme-dev to exactly 20 batches.
      Adds `ingest_batch/read/recent` and `GET /v1/orgs/{org}/imports/recent`, mirroring
      `read::runs_list` — "the most recent runs, newest first, capped at 20", the idiom this read
      should have had from the start. A separate endpoint rather than a flag, because a descending
      page under `after` semantics has no coherent meaning; `/v1/orgs/{org}/imports` stays the
      cursor it is and a test pins that it still starts at the oldest. The BFF route keeps its own
      `{items}` contract and **constructs** the envelope instead of asserting one about the upstream
      body — the W0.5b lesson. The mock now mirrors each endpoint's real ordering, which is the
      second half of the shape-gate argument: a mock must copy **ordering**, not just field names.
      Verified by sabotage: flipping the new statement to `ORDER BY id` fails the lake test on the
      newest-first assertion before it passes.
      **What this says about D9's arbiter.** `frontend/src/lib/api/openapi.json` documents **zero
      paths** — deliberately, per `er-server/src/openapi.rs`'s own header — so the plan's assumption
      that it could arbitrate response shapes is wrong as it stands. But its 12 `components.schemas`
      *are* the generated entity structs, and they are usable today: `components.schemas.Job` lists
      exactly the 21 fields er-server sends, with `progress`, `outcome` and `exit_code` **absent**
      and `created_by` present. So D9 splits — field-level checks are buildable now against those
      schemas, while envelope-level checks (W0.5b's `{jobs: […]}`, this entry's ordering) need
      `#[utoipa::path]` on the handlers first.
- [~] **W0.5 harness debt — superseded by W0.5b above.** The stale
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
- [x] **E3b entity-level constraint enforcement — the limit E3 recorded, closed.** A declared
      constraint now holds over the PARTITION, not just over the pair, so the stronger assertion
      E3's own test asked its successor to make is the one that ships.
      **The rule is not re-implemented, it is re-scoped.** The predicate evaluated inside a
      cluster is the very string `er_compile::constraint_conjuncts` already compiles into the
      blocking rules, wrapped in the `coalesce(…, false)` a WHERE clause applies to it. So there
      is one SQL expression and two scopes for it — the pairs blocking generates, and the pairs
      that end up co-clustered — and the candidate-level rule cannot drift from the
      entity-level one. Re-deriving the comparison in Rust would have meant two implementations
      of three-valued NULL logic, and the bug below is what that costs. `er-compile` is
      therefore a real dependency of `er-cli` now, not just a dev one; the dependency edge *is*
      the single-implementation guarantee.
      Violations enter the existing `never_cut` fixpoint as synthetic `never` assertions (never
      persisted as `assertions` rows — a republish that drops a veto must stop cutting, and a
      stored row would outlive the rule that justified it). Scope is pairs inside a
      multi-record component, computed from the edge set as it stands *after* suppression and
      *after* `always`: the fixpoint ignores a `never` whose endpoints are not co-clustered
      anyway, so a corpus-wide cartesian would reach the same answer and pay O(n²) for it.
      **What happens when a config rule contradicts a human is the interesting half.** An
      `always` edge enters at probability 1.0 and `cut_protect_probability` is 1.0, while
      `choose_cut_edge` needs `value < cut_protect` — so a steward's merge is uncuttable and the
      pair *escalates* instead. A config rule cannot silently reverse an explicit human
      decision, and it is not silently discarded either: it becomes an open
      `never_unsatisfiable` review. That required widening the escalation gate, which until now
      fired only when a protect overlay existed — so a constraint-induced contradiction would
      have been computed and then thrown away. A config with neither constraints nor
      protections still never touches `review_queue`. Note the `contradictions` endpoint is
      deliberately unchanged: it reads *persisted* assertions and answers "which steward
      statements are mutually unsatisfiable", so a config-versus-steward conflict surfaces as
      the review item above rather than there. Two kinds of conflict, two surfaces.
      **`required_match` needs no scan and the code says why.** Every scored edge in a
      component agreed on the column and required it non-null on both sides, and equality is
      transitive, so a component reached through scored edges cannot violate it. It is still
      included in the conjunct set, because an `always` edge can join records that disagree —
      and that is exactly the case that should escalate.
      **Two bugs in the same block, both found while scoping this and both fixed here.**
      `exclude_when_equal` compiled to `NOT (l.c = r.c AND l.c IS NOT NULL)`, which is
      **asymmetric in l and r**: with a right-hand NULL, `l.c = r.c` is NULL, the conjunct is
      `NOT (NULL AND TRUE)` = NULL, and a NULL conjunct in a WHERE clause *drops the pair*. A
      record with a household could never be compared against one without — the exact opposite
      of the rule's own stated intent that "a NULL key excludes nothing", and deterministic
      rather than half-working by luck, because blocking emits pairs in canonical key order.
      Why no test caught it: the existing constraint truth tables mirror the SQL in Rust, whose
      `&&`/`!` are two-valued, and `exclude_when_equal` was the one constraint with **no** truth
      table at all — its only test asserted the emitted *string*. The new test runs the real
      conjunct in DuckDB over all five NULL/equal/differ combinations for all three constraint
      kinds, which is the only way to see a three-valued fault; observed failing on exactly the
      right-NULL case with every other case passing.
      And **constraint columns were never validated at publish**: `config_reference_problems`
      already checked `blocking`, `comparisons`, `transforms` and `segments`, E3 added the block
      and never joined the list, and `MatchConstraints::columns()` — whose doc says it exists
      "for validating them against the schema" — had no caller in the workspace. A misspelled
      constraint column produced SQL naming a column that does not exist, so the tenant's run
      failed minutes later instead of their publish being refused. Now a 422, tested against
      all three lists with the three columns the design prose itself uses as examples
      (`ssn`, `tax_id`, `household_id`) — none of which is a standardized column.
      Tests: the flipped `lake_e2e` assertion (C001/C009 land in different entities while all
      three records keep a membership, so a cut is distinguishable from a disappearance), the
      steward-contradiction escalation above, the DuckDB conjunct truth tables, and the publish
      refusals. Sabotaged by making the derivation return nothing: both lake cases fail, and the
      first prints the original bug verbatim — C001, C009 and W001 in one entity.
      Known limit, stated rather than discovered: the in-scope records are staged into a session
      temp in 1 000-row batches, so a corpus whose clusters hold millions of records pays a
      proportional staging cost before the scan. Bounded by cluster membership, not by corpus
      size, and nil for a corpus with no declared constraints.
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
   - [x] **E6c the estimate and the segment catalog reach the UI.** E6a and E6b both shipped
         endpoints with **no caller at all**: an operator still could not see what a run would
         cost before starting it, which is the entire point of E6a, nor restrict a run to a
         declared slice, which is the point of E6b.
         The cost panel rides the Runs page's run-trigger surface and is fetched **on demand** —
         the estimate opens the org's lake and counts pairs, so a page that ran it automatically
         would make a cheap page expensive to open. A spec asserts exactly that by failing the
         request if anything fetches it before the button is pressed. It shows the pairs the rules
         generate against the cartesian total, the share avoided (to four decimals above 99.9%,
         because "99.9%" and "99.999%" are three orders of magnitude apart in real cost), each
         rule's MARGINAL contribution — the number that says whether a rule is pulling its weight
         — and the blocks large enough to dominate a run on their own. 409 is rendered as the
         designed state it is: an org with no model has nothing to estimate.
         The segment picker adds `segment: <name>` to the run's params. The job carries the NAME
         only; the predicate is resolved server-side against the org's own published config, so
         no SQL ever travels in a request — the property the dispatcher already enforced and the
         UI must not undermine. Offered for scoring runs but **not** for a correction pass, which
         re-clusters from the existing edge set and would be restricted by nothing.
         **The naming collision the plan warned about is real and was avoided:**
         `src/app/api/orgs/[org]/segments/` is the CAMPAIGNS saved-segment store, backed by the
         erweb database and unrelated to the config `segments:` block. This uses
         `/api/orgs/[org]/config/segments`, which is what it actually is. Two different things
         called "segment" is a naming accident in the product; putting them on one route would
         have made it a bug.
         Two fixture lessons, both found by failing first: the segment-scoping test needs its OWN
         org (`segment-dev`) because starting a run takes the org's single writer slot, so sharing
         with the Runs page's Run-now test leaves whichever ran second looking at a correctly
         disabled button — the same reason `palette-dev` exists. And `make frontend-baselines` was
         run and produced an EMPTY diff, which is the point: both additions are steward-gated and
         the runs baseline is a viewer's view, so the verification is what makes "no baseline
         moved" a fact rather than an assumption.
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
         Both leftovers landed in **D2** below: tombstone *event* attribution, and retiring
         `entities` rows that lose every member. Scoping them found the two were one bug.
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
   - [x] **E5b-1 the registry made per-name, which is what an ensemble needs to exist.**
         `matching_model/entity.yaml` has always said "one tenant can keep several models", and
         the registry made that impossible: `read/active` was `WHERE org_id AND state='active'
         ORDER BY version DESC LIMIT 1` and `update/activate` archived **every** other active row
         for the org, so activating a second *named* model silently retired the first. An
         ensemble is a list of named models active at once, so it could not be represented at
         all — this is the schema change E5b was always going to need first, and it is worth its
         own slice because it is the only part that touches DDL.
         Activation is now scoped to the name, taken from a subquery on `$model_id` rather than a
         parameter — the caller cannot pass a name that disagrees with the row, and the statement
         stays a single atomic UPDATE (the property E1 chose it for: no window with no active
         model, no transaction around the generated query functions). The invariant is also
         **structural**: `matching_models_one_active_per_name` is a partial unique index on
         `(org_id, name) WHERE state = 'active'`, so a second active row for one name is refused
         by Postgres even if some future writer forgets the archive half.
         `read/active_all` + `pg::active_models` serve the set; `read/active` stays for the
         single-model path, where it still answers the same row. `ORDER BY name` there is read
         determinism only — a cascade's step order comes from the published config's ordered
         `models:` list, never from the registry, so renaming a model cannot silently reorder a
         cascade.
         Tests, in `pg_model_registry.rs`'s existing single test as new phases: two names active
         simultaneously; activating a new version of one name archives only that name and leaves
         the other untouched; and a direct `INSERT` of a second active row for one name is
         refused *by that index* (asserted on `DbError::constraint`, not a flattened message —
         `err.to_string()` is only "db error"). Observed failing both ways: reverting the
         activate scoping leaves only the newly activated name active, which is the original bug.
         E1's "exactly one version is ever active" assertion did not weaken — it lists one name's
         versions, so it was always per-name in effect; only its wording was wrong.
   - [x] **E5b-2 ensembles that score, in full mode.** A config `models:` list of registry
         names; the dispatcher reads EVERY active version (per-name since E5b-1) and the
         published config selects which of them score, in config order — so retraining a member
         is an activate, not a publish. Each member scores the whole corpus under its own
         `(model_version, tf_snapshot_id)` key, so a pair carries one row per member and the
         existing model-blind edge read unions them: two models vote and neither erases the
         other. Publishing a `models:` change is tier **B** — a re-score, not a retrain, the
         reasoning `constraints` already carries — and a member naming no ACTIVE registry version
         is a **422 at publish** rather than a failed job with the tenant's rebuild queued behind
         it.
         `run_all` now delegates to `run_all_ensemble` with a one-member slice instead of keeping
         a parallel path, which is what makes the equivalence claim unfalsifiable rather than
         argued — there is only one path. `ensemble_identity` is single-sourced for the same
         reason: a one-member run records its **bare** `model_version`, byte-identical to a
         pre-ensemble run, so no existing tenant's next incremental run sees a fingerprint drift.
         Several members record `name=version+…`, composite rather than hashed because that
         string is a user-facing diagnostic — the guard's refusal reads `model_version last=… now=…`
         and an operator needs to see WHICH member moved.
         **Found while building it, and the reason the fingerprint had to change at all:**
         `RunFingerprint.model_version` is a single `Option<String>`. Recording one member's
         version there would leave a change to any OTHER member invisible to the drift guard — so
         an incremental run would never be promoted and the changed member would never score the
         existing corpus, silently. Same family as E10a's tier-C publish that rebuilt against the
         model it was meant to replace.
         **A second find, in this repo's own test fixture.** `build_frozen_tf_tables` is the
         snapshot CSV filtered to `(model_version, tf_snapshot_id)`, and an identity with no rows
         yields EMPTY tables — so a pass scores on with every TF adjustment dropped, exactly the
         silent-score-change `matching_model/entity.yaml` warns about for the same join. Latent
         for one model, likely for an ensemble member whose rows were never appended.
         `run_match_ensemble` now refuses it, naming the member — and the guard immediately caught
         **E5a's own retirement test**, whose `v0002-strict` model has never had TF rows in the
         committed snapshot. Its strictness came partly from the missing adjustments rather than
         only from the lowered prior it says it is testing; the test now re-keys the rows onto its
         own identity (test-local, no committed-fixture rebaseline) so the one declared variable
         is the only one that moved. Its conclusion held either way, which is why nothing noticed.
         Scope held deliberately to full mode, and said out loud in the run log rather than
         tripped over: `run_match_partial` is single-model by construction — it requires the
         scored-records ledger to hold exactly ONE identity before it reuses anything, and its
         delta is "records with no ledger row for this `(record_key, content_hash)`" with **no
         model filter**, so a record scored by member A but not B counts as scored and B would
         never see it. Per-member deltas are E5b-2b. A full pass is meanwhile always correct, just
         more expensive.
         Two fields were written and then REMOVED rather than shipped inert, each to arrive with
         the code that reads it: a per-member `threshold` (it needs the resolved
         `model_version -> threshold` mapping wherever clustering happens, and that mapping lives
         in the registry while `run_correct` re-clusters from the lake alone — persisting the
         resolved ensemble on the run row is the fix, and it is DDL) and `mode`
         (`and_run`/`then_run`). A declared field nothing honours is the dead-config shape this
         program has already found once.
         Tests: a one-member ensemble scores **bit-identically** to the single-model path, clusters
         identically, and records the bare identity; two members both score, both identities stay
         ACTIVE, and the lowered-prior member clears strictly fewer pairs. Sabotage: handing
         `retire_other_models` one member's identity at a time — the obvious way to write the loop
         — reproduces E5a's bug exactly, leaving only `v0002-strict` active with the loose member
         silently retired. Plus the structural config rules (distinct, non-empty, exact names) and
         the publish 422 / tier-B classification over real Postgres.
   - [ ] E5b-2b incremental ensembles (per-member deltas in `run_match_partial`: the ledger must
         match the ensemble's identity SET rather than hold exactly one, and the delta must be
         computed per member or a record scored by one member is never scored by another) and the
         `then_run` cascade + per-member thresholds, which want the resolved ensemble persisted on
         the run row.
   - [x] **E5b-3 the models page — and `active` stops naming one model.** The registry had no
         surface at all since E1 built it: a tenant could not see which model its data was scored
         against, what it used to be, or the m/u probabilities the engine always carried
         (`grep m_probability` over the frontend returned nothing).
         The API half was wrong the moment E5b-1 landed. `models_index`'s `active` was
         `rows.iter().find(state == "active")` — a single row, well defined only while activation
         was unique per ORG. Per-NAME, `find` returns whichever active row the list ordered first,
         so the field named an arbitrary ensemble member. Now a list, one per name, ordered by
         name for a stable render; the RUN order still comes from the config's `models:` list.
         The page: what is scoring now (one line per active name), every version with state /
         provenance / stamp, the derived per-level m, u and `log2(m/u)`, and the two ways to mint
         a draft — compile from the published config (no body: the rules already written become a
         model) or upload a settings document. Both land as drafts; activate is admin-only and
         per-version, and the spec pins that activating one name leaves the others active, which
         is the whole reason an ensemble can exist. Grayscale throughout — state is font weight
         and a check glyph, never hue.
         **The recurring defect, caught on new code.** I wrote the weights interface from memory
         and it was wrong three ways: `match_weight` for the server's `log2_bayes_factor`, an
         invented `prior`, a missing `id`. Reading `model_weights` fixed it before any UI existed,
         and W0.4b's wire-contract gate now carries both model pairs so `ModelVersionRow` cannot
         drift from the generated `MatchingModel`. `weights` is declared *composed*: parsed out of
         `model_json`, so the entity rightly has no such column.
         Fixtures, each learned the hard way: one org per device project
         (`models-dev`/`-ios`/`-android`) because activating archives what it replaces and all
         three projects share one mock server — the first run failed with `default v1` reading
         `archived`; `TRIAGE_ORGS` grants admin and steward only, so the read-only cases use the
         STEWARD, which is the tighter boundary anyway; and the screen joins `a11y.spec.ts`'s
         SCREENS in the same change that creates it, because one absent from that array is
         silently unchecked.
   - [x] **E9b-1 import-time screening: the receipt says what the delivery MET.** "Did my file do
         anything?" had an answer about landing (new / changed / unchanged / removed) and none
         about meaning. A delivery now reports how its records landed against the corpus: how many
         the engine already knew, how many it wants a decision on, and how many are genuinely new.
         **The design decision, and it is the whole slice.** The plan put screening on the staged
         rows at the API; `finish_delivery` is the single landing path for both the CSV upload and
         the JSON push, which makes it the obvious hook. It is the wrong one — that function runs
         in the API request path, so screening a large CSV there would block the request and
         compete with the dispatcher for the per-tenant writer. Screening instead happens between
         MATCH and RECONCILE, where it is not new work at all: an incremental pass has just
         compared these very records against the corpus, so the verdicts are a **read** over
         `match_scores`, and they are the same verdicts the run itself acted on. It is also the
         first moment they exist — the receipt is written during ingest, where they cannot.
         Filtered on `ingest_batch_id`, which the standardized corpus stamps with the batch that
         landed the CURRENT content version, so an unchanged re-delivery is excluded by
         construction. That is right: it was screened when it first landed.
         The counts are **nullable**, and the panel renders an em dash rather than a zero where
         they are unset. "Screened and nothing matched" (0/0/N) and "never screened" (all NULL,
         which is what a correction pass or a `skip_ingest` rebuild leaves) are different facts,
         and a zero would assert the first while meaning the second. `screened_at` exists so the
         distinction is one column rather than three.
         Three things the framework taught rather than the plan. **E009 exists**: a declared read
         that spans two relations is supported through `reads:`, which makes the impact surface
         explicit — I had inferred from the absence of any such read that it was forbidden, and
         the analyser corrected me. A lake **timestamp parameter** must be declared `varchar` with
         an explicit `::timestamp` cast, because a `timestamp` param generates `DateTime<Utc>` and
         duckdb's `ToSql` does not implement it. And a broken generated artifact **blocks the
         generator that would fix it** (`cargo run -p er-entity-cli` has to build the workspace),
         so the escape is `git checkout` the generated crate, then regen.
         **The schema has two sources and they can drift.** `create_ddl_table` builds from
         `er_schema::model`, a HAND-MAINTAINED S5 registry validated against a golden dump — not
         from the entity-generated DDL. Adding columns to `entities/` was not enough and
         `er-entity-diff` did not notice; the lake test failing on "Referenced column
         matched_count not found" is what caught it. Both now agree byte-for-byte, verified by
         diffing `create_table_sql` against the generated DDL. The golden rebaseline is 21 lines
         because it was patched at `indent=1` — its native formatting, confirmed by a round-trip
         check first; `json.dumps` at the default indent reformatted all 4,200 lines.
         Tests: the counts reconcile with the receipt's own landing breakdown
         (`matched + review + unmatched == new + changed + resurrected`) for every delivery across
         two phases, and a corpus that resolves 23 records to 10 entities must find matches rather
         than calling everything new. Observed failing: disabling the screening call fails on
         "a delivery that landed records must be screened". The functional tier pins the em dash
         for an unscreened delivery, with that fixture left NULL on purpose.
         Also brings the receipt under the wire-contract gate (W0.4b): `ingest_batch` had
         `languages: [rust]` and no `wire:`, so no TypeScript binding existed and the panel's
         hand-written `ReceiptRow` had nothing to be checked against — even though the receipt has
         been on the JSON wire since the read API shipped. Declaring the wire it was already on is
         what brought it under the gate, and `ReceiptRow` turned out to be a clean subset.
   - [~] **E9b-2 update rules (#39) — ANALYSED, and mostly already built. A decision is needed
         before any code.** The capability is "what to DO with a matched row": update-empty-only /
         update-all / nothing, bounded by a confidence range. Scoping it against this engine
         (2026-10-22) found that three of those four parts already have mechanisms, because this
         architecture **derives** golden records from members rather than mutating stored records —
         the incumbent's model is "the import edits rows in your CRM", which is not what happens
         here.
         * **update-all** is the default. `run_golden` reassembles each entity's golden from ALL
           members every run, and `recency` in an attribute's chain makes the newest value win.
         * **bounded by a confidence range** is structural, not a knob. A record only becomes a
           member if its edge cleared `auto_merge`; a gray-band match goes to review instead. So
           membership already *is* the confidence bound, and a second one would either duplicate
           it or contradict it.
         * **update-empty-only** splits by intent. For "prefer better data" the existing chain
           already covers it — `completeness`, `validated`, `longest`, `numeric_max` and
           `prioritized:` are all expressions of it. For "don't let an import overwrite what I
           curated", the mechanism is `golden_override`: a steward's latest-wins per
           (entity, attribute) value applied AFTER assembly, which survives every later run by
           construction.
         * **nothing** has no coherent equivalent, and that is a statement about the design rather
           than a gap. A member's values are what the golden is assembled from, so "join the
           entity but contribute nothing" describes a member that is not a member.
         **The one real gap, with its cost stated.** There is no survivorship rule expressing
         INCUMBENCY — "prefer the value that won last run, take a new one only where there was
         none". Every rule is a function of the current member set; none looks at history, and the
         chain terminates in `record_key ASC`, which is deterministic but arbitrary with respect
         to arrival order. Adding it means feeding the PRIOR golden into survivorship, which (a)
         breaks the stage's "identical SQL over identical inputs → identical rows" contract unless
         the prior golden becomes a declared, versioned input, and (b) reintroduces exactly the
         mutable-record model this architecture rejected — a value would persist because it was
         once chosen, not because the data says so.
         **So the decision is whether parity here means the behaviour or the knob.** The
         behaviours are reachable today through survivorship + `golden_override`; the knob is not,
         and buying it costs the determinism contract. Recorded rather than built, because
         building an update-rules subsystem beside survivorship would be a second mechanism for
         one question — the shape this program has twice removed (`ModelSpec.threshold`,
         `CombineMode`). A doc that maps the incumbent's three modes onto the existing config is
         probably the whole deliverable.
   - [ ] E9c S3 auto-ingestion (#47).
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
   - [x] **E10b labels, and the queue that decides what to ask about (#19).** The engine has
         always been able to learn from labels — `m_from_labels`,
         `truth_space_from_labels_column`, `prediction_errors_from_labels_table` all live in
         `dedupe-core::accuracy` — and there was **no way to produce any**: that whole module had
         no caller outside `dedupe-cli` and its own golden tests. The product shipped a trainable
         engine with no training signal and no accuracy number.
         New `pair_label` entity (Postgres), plus `POST`/`GET /v1/orgs/{org}/labels` and
         `GET /v1/orgs/{org}/labeling/next`.
         **The queue is uncertainty-sampled**, nearest the decision threshold first, and that is
         the entire point: pairs the model already scores at 0.99 or 0.01 only confirm what it
         knows, while the ones either side of the threshold are where its m/u estimates are
         actually wrong. A score-ordered queue spends a human's attention on the easy cases.
         Scores live in the lake, so it is a declared lake read
         (`match_score/read/uncertain_page`); already-labelled pairs are filtered in Rust,
         because labels are control-plane and there is no join across the two.
         Three details worth keeping. **A label is upserted on the pair**, not appended — two
         contradictory rows would make the training set ambiguous with no principled way to
         choose — and `created_at` survives a correction. **The pair is canonicalised** (lower key
         first) on the way in, because `match_scores` stores it that way and a label recorded the
         other way round would describe a pair nothing ever scored. **`verdict` is binary on
         purpose**: a labeller who is unsure should skip, because a guessed label is
         indistinguishable afterwards from a confident one.
         **Coverage is reported split by verdict and origin**, not just totalled: a set that is
         95% `no_match` teaches a model almost nothing about matches and a single total hides it
         completely. `origin` keeps explicit labelling, review feedback and LLM suggestions apart
         so training can weight or exclude a source, and so an accuracy number can say whose
         labels it was measured against.
         Two SQL constraints met on the way, both worth knowing: a **control-plane read must
         declare `returns: entity`** (explicit column lists are lake-only in this phase), so
         coverage is counted in Rust from one deterministic unbounded read — which is what
         training needs anyway; and the entity analyser's planner **has no `abs`**, so the
         distance-from-threshold ordering is written as a `CASE` (DuckDB has `abs`, but a
         statement that cannot be planned cannot be declared).
         Tests: `pg_labels.rs` (canonicalisation with the keys deliberately reversed; correction
         replaces rather than stacks and keeps `created_at`; three distinct refusals — unknown
         verdict, self-pair, empty key; viewer 403; and the queue failing on the *lake* rather
         than earlier when none is attached) and
         `lake_e2e::the_labeling_queue_offers_the_least_certain_pairs_first`, which asserts the
         ordering against real base_10 scores, asserts it **differs from score order** so the
         test can tell the two apart, and asserts determinism across identical requests.
         Replacing the ordering with `match_probability DESC` is observed failing.
   - [x] **E10b-2 a steward's verdict becomes a training label.** `pair_label.origin` has
         declared `reviewer` as an accepted value since E10b and **nothing ever wrote one** — and
         the labels API *counted* that bucket, so the coverage endpoint reported a category which
         could not fill. A person looking at a pair and deciding is the strongest signal a model
         can get, and it was being thrown away.
         Written in the DRAIN, as the entry above predicted and for the reason it gave: the
         staging endpoint has neither the pair (the action carries only `review_id` and the
         verdict) nor the lake, while `drain_and_correct` holds the Postgres client and, through
         `apply_and_correct`, the lake session. Only APPLIED actions produce labels — a resolution
         staged and never drained is not a verdict the engine acted on, and labelling it would
         train on an intent the corpus never saw.
         Three decisions worth keeping. A **dismissal yields no label**: "not worth my time" is
         not a verdict about whether the pair is the same person, and training on it would teach
         the model that uncertain pairs are negatives. The pair is **canonicalised lower-key
         first**, matching how E10b stores labels — a label on the other ordering describes a pair
         nothing ever scored. And the upsert is on `(org_id, rec_a_key, rec_b_key)` with
         `created_at` left out of the SET, so re-resolving a pair **corrects** its label instead
         of stacking a second one while still saying when the pair was first judged.
         The label names **who** decided. `pg::pending_actions` now carries the staged action's
         `created_by` instead of dropping it, which is the same reason D7 forwards an end-user
         identity at all — without it every reviewer label would read as the role.
         Two framework facts fell out. `assertion/create/from_review` **cannot** return the pair
         it resolved: the framework refuses `RETURNING` on a lake write ("a lake write must
         declare `returns: none`"), which is why there is a new `review/read/pair_live` rather
         than a widened write. And it is `pin: live` because the steward-apply transaction may
         have written the row itself, so no committed snapshot contains it — `by_id` is pinned for
         the read API and is the wrong statement here.
         Best-effort and after the marks: the resolution is already in the lake and the corpus is
         about to be corrected, so failing the drain because a label could not be upserted would
         undo real work to record an observation about it.
         Test in `pg_drain_dispatcher.rs`, which already resolves a review through the real drain:
         exactly one `reviewer` label, `match` verdict, canonical order, `labeled_by` equal to the
         staging principal, and the sibling `add_assertion` action yields **no** label (a steward
         asserting a relationship directly is not adjudicating a pair the model proposed).
         Observed failing: suppressing the derivation fails on "the match resolution leaves exactly
         one reviewer label, got []".
   - [x] **E10c-1 per-version evaluation: what the model thought at the time.**
         `GET /v1/orgs/{org}/models/{name}/versions/{version}/evaluation` plus the panel on the
         models page. The engine could always compute accuracy — `dedupe-core::accuracy` has
         carried `truth_space_from_labels_table` and `prediction_errors_from_labels_table` from
         the start — and until E10b there was no label to measure against. This is the first
         surface that answers "can I trust this version" from the tenant's own verdicts rather
         than from a training number.
         **The plan's central design problem for this PR did not exist.** It said every accuracy
         function wants a DuckDB `TableHandle` while `pair_labels` is Postgres-only, so labels
         must be materialised into a lake temp table first. They do not need to be:
         `pair_labels.score_at_label` already stores the probability shown to the labeller, and
         its own column description says why it exists — "so a later accuracy pass can tell what
         the model thought at the time rather than what it thinks now". E10b stored it for
         exactly this. The evaluation is therefore pure Postgres with **no lake access, no
         re-scoring, and no entity change at all**, and it is more faithful than re-scoring
         would be, because it measures the score the model actually produced.
         **The join is on `model_version_label`, not the integer `version`.** The scoring pass
         stamps `match_scores.model_version` with the label (`v0001`), the labelling queue hands
         it to the client per item (`uncertain_page` returns it per row), and the client echoes
         it back. Filtering on the integer matches nothing, which would have produced an honest
         but useless "0 labels" for every version — so a unit test pins that trap by name.
         **Two framework facts found by probing rather than reading the plan.** A control-plane
         read must declare `returns: entity`; aggregate column lists are lake-only in this
         phase, so the confusion matrix cannot be a `GROUP BY` in SQL. That is not a limitation
         to work around: `pair_label/read/all`'s own description already records the pattern —
         count in Rust from entity rows, deliberately unbounded because labels are
         human-authored and a LIMIT "would silently train on a prefix". So this PR adds **zero
         declared statements**; two were written and deleted once the existing read turned out
         to be the sanctioned one.
         **Null is not zero, and the distinction is the point.** A null precision means the
         model made no positive predictions on this label set; 0.0 means it made them and got
         every one wrong. A steward acts oppositely on each, so every ratio is null-on-no-
         denominator (O7a's rule) and the panel renders the dash with a title saying which.
         Metrics ship **split by origin** (E10b's rule: a number measured on an LLM's own
         suggestions is not the same claim as one measured on a human's), with an absent origin
         omitted rather than present-and-null, and beside `counts` naming every exclusion — a
         figure over 12 labels has to be distinguishable from one over 1,200.
         **Evaluating version B against version A's labels is declined, not approximated.** It
         needs re-scoring those pairs with B, which wants the lake and the model bytes; that
         belongs to a training job, not a read endpoint, and silently mixing A's scores into B's
         metrics would be worse than refusing. Stated in the handler rather than left implicit.
         A consequence worth knowing, pinned by the pg phase: **a correction drops a label out of
         its version's evaluation**, because the upsert assigns `model_version =
         EXCLUDED.model_version` and a correction sends none. That is right — the old score was
         shown against the verdict that was replaced.
         The curve is hand-rolled DOM/CSS (no chart library, per `frontend-data-ui`), computed
         exactly at each step rather than from a histogram since every label is already in
         memory. Observed failing: swapping `fp`/`fn` fails 3 of 8 unit tests — and notably NOT
         the perfect-separation one, because `fp = fn = 0` there makes the swap invisible, which
         is a reminder that the happy-path test is the weakest; and widening the version filter
         to accept every label fails the pg phase at 4 labels instead of 2.
         Two gates caught my own wrong claims mid-build: the envelope ledger showed this route
         is `scalar`, not the `record` my BFF comment asserted, and the mock-fabrication gate
         flagged the five computed response fields until they were accounted for in `EXPLAINED`
         (the evaluation echoes `matching_model`'s identity fields, so best-match attribution
         reads it as that entity — the attribution limit that file documents, seen from inside).
   - [x] **E10c-2 the labelling surface — the input every other label feature was missing.**
         E10b built `pair_labels` and the uncertainty-sampled queue and shipped **no UI at
         all**: `grep -r labeling frontend/src` returned nothing but a generated binding, and
         there were zero BFF routes. So a tenant could not record a verdict, which meant
         `m_from_labels` had nothing to train on and the per-version evaluation shipped in
         E10c-1 would have read "no label can be scored against this version" forever, for
         every tenant. Built ahead of label-driven training for that reason — measuring and
         training both need a label to exist first.
         The screen asks about one pair at a time with `FieldCompareGrid` (direct reuse, as the
         plan anticipated for F1 — queue items carry only keys, a probability and `evidence`,
         so without the record values the screen would ask a human to judge two opaque
         strings), the score rendered beside the threshold that explains the ask, and a
         keyboard flow (`m`/`n`/`s`). **There is no "unsure" verdict** — skip writes nothing,
         matching the entity's own reasoning that a guessed label is indistinguishable
         afterwards from a confident one.
         **Four bugs, all found by the tests rather than by review.** (a) The component
         advanced an index on a successful label while the server removes that pair from the
         queue — the list shifted down and the cursor moved up, so one pair was never asked
         about. Replaced with a skipped-set and "first pair not skipped", which is correct
         under both operations. (b) `canLabel` was `effectiveRole(...) !== "viewer"`, and
         `effectiveRole` returns **null while the session loads** — so `null !== "viewer"` is
         true and a viewer was offered the label buttons until it settled. Permission is an
         allowlist now. (c) The BFF POST passed `body: JSON.stringify(body)`, but `forward`
         serializes `body` itself, so er-server received a JSON string *literal* and could not
         deserialize `LabelIn`. (d) `forward`'s `role` defaults to `viewer` **and selects the
         stored key**, so the steward-only write was forwarding a viewer credential — a 403
         that would have read as a permissions bug in the product.
         **A fifth was in my own fixture, and it was the instructive one.** The mock's queue
         pairs were `crm:… / billing:…`, but `match_scores` stores a pair **lower-key-first**
         and the label upsert canonicalises — so the already-labelled exclusion silently never
         matched and the queue kept re-offering a pair the real server would have dropped. A
         fixture that violates an invariant the real data holds makes the mock agree with a
         broken client. Fixed by canonicalising the fixture, with the reason written beside it.
         Three pure functions were extracted and unit-tested rather than left inline, chosen
         because **none of them fails loudly when wrong**: the next-pair derivation, the
         permission predicate, and `labelPayloadFor`, which carries `model_version` and
         `score_at_label` from the queue item. That last is the contract with E10c-1 — drop
         either field and the label still trains a model but can never be measured, with
         nothing anywhere to explain the empty panel. Validation moved to
         `src/lib/bff/labels.ts` (the house pattern, beside `csv.ts` and `envelope.ts`) where
         the range check on `score_at_label` is testable: a value outside [0,1] is not a
         probability and would quietly poison every precision figure.
         Coverage fell below the per-PR floor when the component landed, and was restored by
         testing that extracted logic rather than by lowering the threshold — 48.26% lines /
         33.19% functions against floors of 48 / 33.
         The screen is registered in `a11y.spec.ts`'s `SCREENS` in the same change that adds
         it, so the axe sweep covers its keyboard affordances; the generated
         `frontend-data-ui/references/screens.md` picks it up. **Nine visual baselines moved**,
         all `desktop-chromium` — a nav entry is shared chrome, and those are exactly the
         tenant screens rendering the rail. Mobile projects hide it and did not move; login,
         admin and marketing have different chrome and did not either. Reviewing that the set
         is precisely "tenant screens on desktop" is what makes the regeneration safe.
   - [x] **E10c-3 the tenant's verdicts estimate `m`, and `m` only.**
         `dedupe-core::training::m_from_labels` has existed since the start with no caller
         outside `dedupe-cli` and its own golden tests. It needs a labels table; until E10b
         there was no label, and until E10c-2 no way to record one. This is the last link:
         `train_and_register` now reads the tenant's match verdicts and uses them for `m`.
         **This is where the plan's lake-materialisation problem is genuinely real** — unlike
         E10c-1, where `score_at_label` made it moot. `estimate_m_from_pairwise_labels` takes a
         DuckDB `TableHandle` and `pair_labels` is Postgres, so the dispatcher reads the
         verdicts where the client is and `run_train` stages them into a session temp table
         (`__train_labels`, following `__constraint_scope`'s batched-insert precedent — one
         `VALUES (…), (…)` would be a SQL string megabytes wide on a real label set). A
         `TableHandle` is a plain struct with public fields, so no new API was needed.
         **MATCHES ONLY, and this is the sharp edge.** The estimator reads **no verdict
         column** — its own module doc says "every pair treated as a match" — so a `no_match`
         pair reaching it is counted as agreement at whatever levels it disagrees on. That
         does not fail: it skews every `m` toward disagreement and moves every score the model
         will ever produce, silently. The dispatcher filters, the field is named
         `match_labels`, and its doc comment carries the reason.
         **Additive by construction.** The label pass runs AFTER `linker.train(...)` and
         replaces `m` alone; `u` and the prior come from random sampling and EM and are
         untouched — a tenant's verdicts say which records match, never how often two *random*
         records agree. The field is required rather than defaulted so the compiler found all
         three call sites: the CLI passes `&[]` (no control plane to read verdicts from), and
         the existing e2e's `&[]` **is** the determinism obligation.
         A label read that fails falls back to EM-only rather than failing the train, because
         EM is a complete training method on its own and is what every run did before labels
         existed. Provenance records `m_from_labels: <count>` so a draft's origin is legible
         from the registry — and `pg_train` asserts a no-label train writes **0** rather than
         omitting the key, because a missing key and "trained without labels" read identically
         to a steward while only one of them means the path ran and found nothing.
         `labels_estimate_m_and_only_m` asserts determinism FIRST — two no-label runs must be
         identical — so the `m` difference that follows cannot be run-to-run noise, then that
         `u` and the prior are byte-equal. The module doc claims "only m is trained"; this
         asserts it rather than trusting it. Observed failing: disabling the label pass fails
         it by name. Training still lands a **draft**, unchanged from E10a — training is not
         consent to re-cluster a tenant's data.
   - [ ] E10c-4 related-record features (#26), household/link entities (#33). **Deeper than
         the one-liner suggests, and the reason is worth knowing before starting:** the plan
         says `features.related:` is "compiled into the concat stage by `er-compile`", but
         er-compile produces SETTINGS — the concat relation is built by
         `dedupe_core::predict::materialize_concat_multi`, and at least four golden suites
         (`golden_batch1/3/4`, `golden_accuracy`) read through it. So this changes
         execution-oracle-governed engine SQL and carries a determinism obligation, not just a
         config block. Worth its own session with that in view.
   - [x] **D4 tracker hygiene — the count was wrong in three different ways.**
         Before: 7 open items, of which **3 were duplicates** and ~14 more were invisible.
         After: 18 open, 86 done, and every one countable.
         The three defects were not the same defect. `E9c` appeared twice as literally
         identical adjacent lines. The two `V0` bullets were NOT duplicates — the earlier one
         still listed `V1` as open, which shipped in #238, so the stale copy went and the
         current one stayed. And Waves 6–8 were prose paragraphs with no checkboxes at all,
         so fifteen PRs could not be counted by anything; they now carry one box each.
         Also corrected: W0.1 claimed "all program work happens in a dedicated worktree
         (`../ere-datagroomr-parity`)". That worktree does not exist and was never used —
         every program PR has been a branch off `main` in the primary checkout. An instruction
         that outlived its subject is worse than no instruction: a reader following it would
         have created a second one.
         Left deliberately: `F3` is marked absorbed rather than open, because the insights
         page and dashboard trends shipped with the attribution work (#292/#295/#297), and
         `O10` now records that it wants `#[utoipa::path]` first — the response-envelope
         question it was once blocked on is already answered by D13's derived ledger.
4. **Operating at scale**
   - [x] **O3b webhook delivery becomes an outbox (#43's reliability half).** Delivery was
         `let _ = webhook::emit_job_event(...)` at three call sites: the POST happened **inline**
         and its outcome was **discarded** — the per-target results were computed and thrown
         away. So a subscriber unreachable for one second missed the event permanently, and
         neither the tenant nor an operator could find out. Nothing retried, nothing was
         recorded, and `DeliveryResult` had no consumer.
         Now the event is written to a new `webhook_deliveries` table **before any network
         call**, and a bounded drain on the dispatcher loop delivers it with exponential
         backoff (5s→300s, the same curve as jobs, six attempts ≈ ten minutes). The split is
         what makes a transient failure recoverable at all: the event is already durable, so a
         503 becomes a backoff instead of a loss.
         **Classification, not blanket retry.** A 2xx is terminal-delivered; 408/429/5xx and
         transport errors retry; **any other 4xx fails immediately**, because a subscriber
         rejecting the request itself will reject it identically five more times and spending
         the attempts is pure waste; and a destination the outbound-URL guard refuses is never
         retried, because it will still be refused in five minutes.
         Two details that matter more than they look. The **payload is stored, not rebuilt** —
         the signature is computed over those bytes, so a rebuilt body would fail the
         subscriber's verification on exactly the deliveries that needed retrying. And the
         **url and secret are read at delivery time**, so rotating a secret or correcting a URL
         takes effect on the next attempt rather than being frozen into the queued row.
         `GET /v1/orgs/{org}/webhooks/deliveries` is the log, filterable by subscription and
         state — the question the old path could not answer.
         Tests: three phases of `pg_webhook_deliver` (recording does not send; the guard refuses
         the loopback subscriber terminally; a 503 retries twice then delivers while a 400 fails
         after one attempt, with the retried bytes asserted byte-identical). Sabotaging the
         classification to retry everything is observed failing.
         **Two test-isolation constraints learned the hard way**, both invisible under
         `--test-threads=1`: `claim` is global across orgs **by design** (one queue, one
         drainer, oldest first), so concurrent test binaries *steal each other's deliveries* —
         which is why these are phases of one test, as `pg_scheduler_tick` already was for the
         same reason about `schedule_tick`. And `DROP TABLE webhooks CASCADE` drops the
         dependent FK **constraint**, not the referencing table, so `webhook_deliveries` kept
         orphan rows from earlier runs and the global oldest-first claim picked one of those up
         instead of the test's own event. Every webhook test now clears the outbox explicitly.
         Still unbuilt: the frontend surface for webhooks (there is none), and the non-`job.*`
         event types (`entity.*`, `review.*`, `import.*`, `config.*`) — the catalog (#223)
         deliberately lists only what something emits.
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
   - [x] **O3c-1 the control-plane events: `config.published`, `import.received`,
         `review.resolved`.** O3a's catalog was deliberately only the three terminal job outcomes,
         so a subscriber could learn that a job finished and nothing about WHY it started. These
         three are the causes, and each fires from a place that already holds the Postgres client,
         so none of them needs the cross-store bridge the entity events do.
         The interesting decision is `review.resolved`, and it is the same one E10b-2 just made
         for labels: it fires from the **drain**, not from the resolve endpoint. A resolution is
         staged and applied later, so an event at staging time would tell a subscriber about an
         intent the engine has not enforced — they would fetch the corpus and find it disagreeing
         with the event they just received. It fires for dismissals too: a dismissal is a decision
         worth delivering even though it yields no training label.
         `import.received` fires from `finish_delivery`, which is where E9a put the shared
         preconditions, so both delivery paths (the CSV upload and the JSON push) get it from one
         site. It is deliberately NOT "the import finished" — that is `job.succeeded`, keyed by
         the `job_id` this payload carries so the two can be correlated.
         `config.published` carries the change TIER and the blocks that moved, which is the point:
         a subscriber can tell a threshold tweak (tier A, re-reads existing scores) from a
         comparison change (tier C, retrains and re-resolves the corpus) without diffing YAML.
         All three are queued through O3b's outbox, never POSTed inline — a tenant's unreachable
         endpoint must not be able to slow down or fail their own publish — and all three are
         best-effort after the work committed, for the reason that recurs: failing an operation to
         report on it destroys work to describe it.
         **O3a's honesty gate was extended rather than bypassed.** It checked `job.*` events by
         finding the state string in the emit sources and explicitly `continue`d on anything else
         ("a non-job event will need its own evidence when one is added"). A non-job event now
         brings that evidence: its literal type string must appear at an emit site. Observed
         failing — cataloguing an `entity.merged` nothing emits fails with "the catalog lists
         \"entity.merged\" but no emit site names it".
   - [x] **O3c-2 entity-level events off the `entity_events` ledger, as per-kind per-run
         aggregates.** This is the item O3a named when it refused to catalogue entity events:
         "thousands of un-retried POSTs per run would be worse than none." O3b's outbox fixed the
         *retry* half and left the volume exactly where it was — a 10M-row corpus still writes
         millions of ledger rows — so the fan-out decision was still open, and it is the whole
         design of this PR.
         **The answer is at most one event per KIND per run**, carrying the count and a
         `detail_path`, with the rows served by a new
         `GET /v1/orgs/{org}/runs/{run_id}/entity-events`. That is the standard webhook shape of
         notify rather than deliver, and it is only an acceptable trade because it is lossless:
         the aggregate expands back through the detail endpoint, which is asserted rather than
         claimed (the sum of the delivered counts equals the rows the endpoint serves).
         Six kinds are catalogued — `created`, `member_added`, `member_removed`, `merged`,
         `split`, `retired`. **`edge_cut` is deliberately absent**: it is in the ledger's own kind
         list (`er_schema::model::EVENT_TYPES`) but nothing writes it, and E3b left the `cut_edges`
         relation without a producer too, so it falls under exactly the rule O3a wrote for
         `job.running`. `entity_event_name` returns `None` for it and **logs the count** rather
         than dropping it silently, which is the "capped fan-out that LOGS what it dropped" half
         of the original requirement — if it ever gains a writer that surfaces as an
         operator-visible line instead of as events quietly never arriving.
         Two consequences of that rule are worth recording. The event names are spelled as
         **literals** in a `match`, not `format!("entity.{kind}")`, because O3c-1's honesty gate
         reads the emit sites for the literal string: a computed name would correctly fail it, as
         a name nothing *says* is a name nothing can be shown to emit. And `entity.merged` was
         the gate's own worked example of "an event nothing emits"; it has a producer now, so the
         refusal case moved to `entity.edge_cut` — the better example anyway, since it looks
         exactly as plausible as the five beside it and only the catalog's rule tells them apart.
         **The store crossing.** The ledger is in the lake and the outbox in Postgres, so
         `run_job` (which holds the lake session) now returns the per-kind counts and
         `dispatch_once` (which holds the Postgres client) emits them — the same bridge E9b's
         screening and E10b-2's labels used. The counts ride *beside* the outcome rather than
         inside it, because every other arm of that `if` yields `Result<(), _>`. Both halves are
         best-effort after the run committed: failing a run because a COUNT could not be read
         would destroy work in order to describe it.
         A new declared live read (`counts_by_kind_live`) does the aggregation in SQL and a
         snapshot-pinned sibling (`by_run`) serves the detail page, keyset-cursored on `seq`
         rather than `id` — `id` is a content hash, so an `id` cursor would page in an order no
         reader asked for.
         **Three gates observed failing.** Deleting one literal from the emit site fails O3c-1's
         catalog gate by name. Changing the emit to one POST per ledger row fails the per-kind
         uniqueness assertion. And dropping the kind filter from the detail endpoint fails the
         losslessness assertion — but only after the test was strengthened: a first full run over
         base_10 writes nothing but `created`, so the original single-run test passed the
         filter sabotage. The fix is a second run with two rows appended to the crm delivery, one
         a near-duplicate that joins an existing entity and one a stranger that makes a new one,
         so the partition moves in two directions at once and a filter that is ignored stops
         being indistinguishable from one that works.
   - [x] **O3c-3 the webhook UI — the first surface for any of it.** O3a built the catalog, O3b
         the outbox and the delivery log, O3c-1 the control-plane events and O3c-2 the entity
         aggregates, and `grep webhook frontend/src` returned nothing but generated entity
         bindings. So a tenant could not discover which events exist, could not subscribe without
         an API call, and — the one that actually costs them — could not see that their endpoint
         had been returning 500s for a day. Those three gaps are the page's three cards.
         **The failure reason is in the ROW, not behind a click.** The first draft put it in the
         expandable payload panel and the spec caught it: a reason one expand away is a reason
         nobody reads, and "nobody could find out that an endpoint was broken" is the whole gap
         the outbox closed. The row carries the response code, the reason, and attempts out of the
         ceiling, so "given up on" is distinguishable from "still trying" at a glance.
         Grayscale per §3.3, so the three delivery states are told apart by SHAPE — a check, a
         slash, a dashed ring — each with its word beside it, because a glyph alone is a legend
         the operator has to learn. An unrecognised state renders as pending rather than throwing:
         the safe reading of an unknown outcome is that it has not arrived.
         **`listFrom` turns the recurring envelope bug into a tested function.** er-server has no
         single list envelope — `{jobs}`, `{deliveries}`, `{events}`, `{items}`, and one bare array
         on `/webhooks` — and the BFF's browser contract is a bare array everywhere. W0.5b shipped
         that mismatch as `forward<unknown[]>` over `{jobs: […]}`, a type assertion about an HTTP
         body TypeScript cannot check, so the runs page rendered an empty table against the real
         backend while every mock-backed tier was green. The cast is now one helper with a unit
         test, and the jobs route moved onto it too.
         **A layout bug the new tab exposed.** The settings nav was a non-wrapping flex row; a
         sixth tab overflowed a 360px viewport and pushed the config studio's Save button under
         the lexicon list, so the click landed on a nickname. Found by `grow.spec.ts` on
         mobile-chrome, and the cause was confirmed by removing the tab and watching the test pass.
         `flex-wrap` fixes it; the two mobile config-studio baselines shifted and were regenerated
         (desktop fits six, so it did not) — the same "shared chrome invalidates baselines for
         screens the diff never mentions" rule E6c and E9b both paid.
         Two new wire-contract pairs, both observed failing: inventing `last_delivered_at` on
         `WebhookRow` fails `tsc` with `UNDECLARED_WIRE_FIELDS: "last_delivered_at"`. The gate
         cannot catch an *omission*, only an invention, which is why the reasoning for leaving
         `secret` off the row type is written beside the pair rather than relied on —
         `webhooks_index` never projects it, so a type that carried it would state a contract the
         server refuses on purpose.
         Coverage: the page's hooks are Playwright's, so the four floors were cleared by moving
         real logic out of the component into tested modules rather than by lowering a threshold —
         the delivery filters (server parameter names, and an empty filter omitted because `state=`
         is a 422 rather than "any"), the removed-subscription fallback, the subscribe guard
         (asserted satisfiable, which is the half the source-wizard stall got wrong), and the
         query-key nesting `useDeleteWebhook` relies on for its invalidation to reach the filtered
         caches at all.
   - [x] **D2 the E7 leftovers — and deletion was allocating, not deleting.** Scoped by probing
         the lake rather than by reading the note, which is why what it found is not what the
         note described: a deleted record was being minted a **brand-new entity**.
         `reconcile_plan`'s orphan rule gives every prior record the clustering placed nowhere a
         fresh singleton entity, with a `created` event. That is right for a SUPPRESSED record —
         still in the corpus, excluded from clustering, and entitled to keep an identity — and
         wrong for a TOMBSTONED one. The pure core cannot tell them apart: it sees only the old
         and new partitions, and "absent from the new partition" is equally true of both. So
         every deletion allocated an entity, the membership prune then emptied it, and the lake
         kept an **`active` entity with zero members** that the ledger had announced as created.
         Observed, not reasoned: the probe printed one orphan entity per deleted record.
         The fix is in `er-cli`, which is the only place that knows the corpus — the pure core
         and its Python-golden tests are untouched except for three additions that change no
         behaviour (a `DEPARTED` cause constant, a public `PlannedEvent::new`, and
         `sort_planned_events`, extracted so the S4.5.4 D1 ordering has exactly one
         implementation now that a caller appends events). `p_old` is filtered to the corpus, so
         the orphan rule stops firing for departures; the departures are collected rather than
         dropped and announced as `member_removed` with **`cause: departed`**.
         That cause is new because `recluster` was the only one there was, so a deletion reached
         a subscriber claiming the model had changed its mind about where a record belonged —
         the quietest kind of wrong, since both statements are about the same row leaving the
         same entity. The sabotage run shows the old behaviour emitting BOTH causes for one
         departure.
         Retirement is read from the lake AFTER the membership prune (new declared
         `resolved_entity/read/orphaned_live`, `pin: live` with `reads: [entity_membership]`),
         because the prune is the step that can empty an entity and the plan runs before it. It
         is also written after the plan's own transitions, so a retirement wins over an `active`
         the plan may have written for the same id.
         O3c-2's aggregate is what makes this urgent rather than tidy: `entity.created` was
         over-reporting by exactly the phantom count and `entity.retired` under-reporting by the
         orphan count, so the webhook stream described a corpus that never existed.
         Three gates, each observed failing. Putting departed records back in `p_old` reproduces
         the phantom `created` *and* the duplicate `member_removed`. Disabling the orphan sweep
         leaves the entity `active` with nobody in it. And `reconciling_an_unchanged_corpus_is_a_no_op`
         is the INV-EQ half: both new passes run on every reconcile, so the risk they carry is
         retiring or re-announcing something on a corpus that did not change — same partition,
         same lifecycle, zero events. No fixture or golden file moved.
   - [x] **D3 / 1.6c generalized grouped + controlling survivorship fields.** The untracked half of
         1.6b. `address` has always been exactly this mechanism — six `addr_*` columns, ONE
         decision — and it was two `const` arrays in `er-golden`, so no tenant could declare
         another. The consequence is sharper than "a missing knob": `given_name` and
         `family_name` are elected independently, so a golden record can take one from each
         source and describe **a person who never existed**. That is the motivating case and the
         end-to-end test (`Robert Chenoweth`, never `Robert Chen`).
         New `survivorship_groups:` block, keyed by the group's token — which is also the name
         its chain is declared under and the `attribute` its `golden_lineage` row carries, so a
         group costs no new relation. `fields` travel together; optional `controlling` names the
         field the chain RANKS on, and its absence means rank on the concatenation, which is
         exactly what `address` has always done (`most_common` over an address picks the most
         common address, not the most common street). Naming `address` overrides the built-in
         rather than duplicating it — two `attr_address` relations would be accepted by the
         config and refused by the SQL.
         **The determinism obligation is a test, not a hope.** The generated SQL is what every
         golden fixture pins, so `resolved_groups`/`resolved_scalars` are written such that an
         EMPTY block resolves to exactly the built-in address group in the position it has always
         occupied — asserted directly, including the scalar list's order. 163 test binaries green
         with zero fixture drift.
         **Six publish-time refusals, not silent preferences**, in `config_reference_problems`
         where the blocking and segment column checks already live: an unknown column, a
         one-field group (a scalar wearing a group's name), a group with no chain, a
         `controlling` field the group does not contain, a field in two groups, and a grouped
         field that KEPT its own chain — the last being the dead-config shape this program
         already deleted `ModelSpec.threshold` over. A group that half-applies is worse than no
         group: the golden record looks internally consistent and is not. Observed failing by
         disabling the validation loop.
         **One bug the change exposed.** The Account builders were taking their vocabulary from
         `config.entity_type`, and the first draft of the resolver did the same — so a config
         whose `entity_type` was defaulted emitted person columns into Account SQL. Caught by
         `er-sql-check`'s account mart test as `No field named r.given_name`, a planning failure
         a long way from its cause. The vocabulary is now the CALLER's: `acct_*` generates for
         Account by construction, and says so in its signature.
   - [x] **O7a resolution metrics over time (#45/#57's trend half).** Every dashboard number has
         always been computed LIVE from the lake, so the product could say what is true now and
         nothing about the direction of travel — and "is the duplicate rate going up" is the
         question an operator actually has. Nothing was retained to answer it.
         New `metric_snapshot` lake entity, written at the end of every `run_all`, plus
         `GET /v1/orgs/{org}/insights` returning the series newest-first with the derived rates.
         Four judgements worth recording. It counts **duplicate RECORDS as well as groups**:
         counting only groups calls a five-record cluster and a two-record cluster the same
         amount of duplication, which is how a worsening corpus looks flat. It keeps
         **`largest_group`**, because a sudden jump there is the signature of an over-merge and an
         average cluster size hides it completely. It is assembled in **one query per family**,
         since a snapshot built from several independent reads of a live lake can describe a
         corpus that never existed at any instant. And `duplicate_rate` is **null, not 0.0**, when
         there are no records — reporting zero would draw a reassuring flat line for an empty
         corpus.
         Keyed on `run_id` and upserted, so a correction pass or a retry reusing a run id
         corrects the snapshot instead of double-counting. Written **best-effort and last**: the
         run's results are already in the lake, so a failure to describe them must not fail the
         run that produced them. Rates are derived on read rather than stored, so changing a
         definition does not need a backfill.
         Test: `a_run_records_metrics_that_agree_with_the_corpus` checks every count against the
         lake it describes rather than against constants — so it asserts "the snapshot is true",
         not "the snapshot is what it was when I wrote this" — asserts `duplicate_records >
         duplicate_groups` on base_10 so the two definitions are distinguishable, and asserts a
         re-run of the same id corrects rather than appends.
   - [x] **O7b-2b the attribution surface.** `GET /insights/sources` shipped headless like every
         capability before it, so the card on the insights page is the same catch-up this program
         keeps paying. Bars from zero, not a pie: these quantities are COMPARED, not apportioned,
         which is the design standard's stated reason for ruling pies out. Each bar is a share of
         the worst source's duplicate count, so "is one feed dominating this" is answerable at a
         glance, and `contributed_groups` sits beside the count rather than inside the bar because
         it is a different fact.
         The spec asserts the per-source counts **sum** to the corpus-wide row, which is the same
         property the Rust test asserts of the server — a mock whose totals drifted would make a
         broken writer look correct, and that is this program's most-repeated defect. Observed
         failing by changing one fixture number (`Expected: 13, Received: 18`).
         **Reading the rendered baseline caught a bug in my own tile.** The duplicate-rate card
         showed `46.2%` above a delta of `+0.1` — the raw fraction, which reads as a tenth of a
         point when the rate had jumped nearly ten. A value and its change have to be in the same
         unit; it now says `+9.8 points`. The insights page also gains a visual baseline, which it
         lacked: this screen is CHARTS, and a bar that silently rendered at the wrong width would
         read as a different corpus rather than as a bug.
   - [x] **D13 the envelope half of D9 — nothing arbitrated the *shape* of a list response.**
         The last gap the other three gates each record as out of reach: `mock_contract.rs`
         compares route and method, `wire-contract.test.ts` and `mock-fabrication.test.ts`
         compare field names. None sees W0.5b, where the jobs route declared
         `forward<unknown[]>` over a body er-server sends as `{jobs: […]}` — a type *assertion*
         about an HTTP body TypeScript cannot check.
         **The plan said this needed `#[utoipa::path]` first. It did not, and that route would
         have been wrong.** A utoipa response body is a hand-written claim sitting beside the
         handler, free to drift from it — the same shape as the defect it would be built to
         prevent, and the exact mistake D12 had just finished removing from the mock. So the
         ledger is **derived**: `er-server/tests/response_envelopes.rs` reads every routed GET
         handler's response construction and generates
         `frontend/src/lib/api/envelopes.json`, byte-compared in `make check`. An envelope the
         classifier cannot read is a hard failure naming the handler, never a skip.
         **The measurement is the finding.** 43 routed GETs carry **six** shapes: 10 `bare`,
         10 `wrapped` (one key), 10 `record` (several keys), 10 `scalar`, 2 `page`, 1 `arrow`
         (`/golden-records` is Arrow IPC with the cursor in **headers** — not JSON at all).
         `/runs` is bare and `/jobs` wraps, one route apart, so a developer reading two
         neighbours learns the wrong rule either way. Unifying the API breaks every tenant
         reading it directly, so the gate writes the fact down per route rather than demanding
         one convention. The jobs route's comment claiming "er-server wraps every list
         response" was corrected — it is false for 10 of them.
         `tests/unit/bff-envelope-contract.test.ts` consumes the ledger and checks each BFF
         route three ways: the upstream path is a routed GET, a `forward<T[]>` never sits over
         a keyed body, and — the one that matters most — **the `listFrom` key is a key the
         response actually has.** `listFrom` returns `[]` for a missing key by design, so a
         wrong key is an empty table with no error anywhere; it is the single mistake the
         tolerant helper cannot surface at runtime.
         **Three honesty corrections during the build, each a wrong claim caught before it
         shipped.** (a) `Json(json!(x))` was labelled `array`, but the source cannot tell a
         `Vec` from one record — `list_orgs` and `get_job` use the identical form. Renamed
         `bare`, claiming only that there is no envelope to unwrap. (b) Reporting an object
         literal's *first* key called `get_org`'s inline record a wrapper named `id`; it now
         carries every top-level key, so one key reads as a wrapper and several as a record.
         (c) The error-arm stripper was line-based, so a handler with its match arms on one
         line lost its success arm too — and `list_orgs`/`get_job` write
         `Json(json!({"detail": …}))` inline rather than through `detail_err`, which left the
         error key reading as the success key.
         **Two false positives in the frontend half, both the same mistake:** the jobs route's
         comment *quotes* the historical `forward<unknown[]>` bug, so comments must be stripped
         before matching or the gate reports the defect that comment explains was fixed; and
         the BFF spells actions with a colon (`assertions:contradictions`, normalized by
         er-server's `rewrite_action_path`) with parameters named differently from the router,
         so both spellings need erasing or every detail route reads as unrouted.
         Observed failing five ways: a wrong `listFrom` key, a `forward<T[]>` over a wrapped
         body, a changed handler envelope (`webhooks_index` made to wrap — the ledger went
         stale AND the frontend named `orgs/[org]/webhooks/route.ts`, which is the loop
         closing end to end), and each of the two inputs emptying.
   - [x] **D12 the mock invented two job fields, and nothing looked at what the mock emits.**
         Found by sweeping the recurring class deliberately rather than by hitting it: the
         defect that has bitten this program most often is *the frontend states a contract the
         backend never fulfilled*, and the gates built for it both look at the frontend's
         **types** (`wire-contract.test.ts`) or the **route table**
         (`mock_contract.rs`). Neither reads what the mock actually sends, which is where the
         belief is authored.
         `exit_code` and `outcome` sat on the mock's job fixtures at 11 sites each — neither a
         column on any entity, neither produced anywhere in `rust/`, both Python-era leftovers
         (a subprocess exit status and an outcome string the Rust dispatcher never had). No
         component read them, so nothing was broken; they were a **loaded trap**, because the
         next UI to show `job.exit_code` would have gone green in all three mock-backed tiers
         and shipped a field that is always `undefined`. That is precisely how `progress`
         reached production and threw on three screens.
         The gate walks every object literal in the mock with the TypeScript AST and fails on a
         key the matched entity does not declare. **The authority choice was the whole
         difference:** reading `entities/*/entity.yaml` instead of the generated TypeScript
         bindings took it from 15 flagged fields to 2, because the lake-only entities
         (`assertion` among them) have no binding, so their rows were being misattributed to
         whichever entity shared the most fields. The 2 survivors were both real and are now
         recorded with their provenance — `run.auto_merged_pairs` is derived on read,
         `assertion.assertion_id` is a wire rename of the entity's `id`.
         **This reverses a recorded decision.** Both fabrications were previously left in place
         on purpose, as the fixture the envelope half of D9 would be built against. A sabotage
         run that re-adds `exit_code` and watches the gate name it by line number is a better
         fixture than a fabrication living in the shipped mock. The envelope half still needs
         `#[utoipa::path]` and still has no fixture; that is stated rather than papered over.
         Observed failing three ways before being trusted, per the house rule: the reintroduced
         field, the entity authority reading nothing, and the mock parse reading nothing — the
         last two because a gate whose input silently empties passes while examining nothing.
         **Four other classes were swept clean in the same pass**, which is worth recording so
         they are not re-derived: every control-plane foreign key is `NO ACTION` but no other
         hard delete has children (`api_keys` already soft-retires, and no org-delete path
         exists — `orgs` has 10 children); all six `forward<unknown[]>` routes genuinely return
         bare arrays; every forwarded query param is read (of four candidates, two were my
         check matching the POST handler on a shared path and two were a handler that
         delegates, as `audit_index` does to `audit_feed` — so a per-handler textual check
         needs to follow calls, or it reports the delegating case as dead); and all 78 routed
         handlers enforce authorization, `healthz` excepted and correctly.
   - [x] **D11 a webhook could not be deleted once it had delivered anything.** Found by clicking
         Remove on the real stack — the API answered `{"detail":"db: db error"}`.
         `webhook_deliveries.webhook_id` references `webhooks.id` with no `ON DELETE` action, so
         the hard `DELETE FROM public.webhooks` could not run once a subscription had emitted
         anything, which is every subscription that ever worked. The tenant got an opaque
         five-word error for a button the UI offers.
         **No test caught it because the existing one deleted a webhook that had never
         delivered** — the foreign key was simply never exercised. The regression test now emits
         to the subscription first and asserts THIS webhook is among the delivery targets, so the
         constraint is on the path rather than nearby.
         The fix is a soft delete, and the data model was already built for one: `by_id`,
         `for_delivery` and `list_active` all filter `AND active`, so deactivating is
         indistinguishable from deletion to every read, while the delivery log survives. That is
         the right semantics anyway — the log is history, and a tenant auditing what was sent to
         a subscriber they have since removed is the case it exists for. The statement moves from
         `delete/` to `update/deactivate`, because the framework checks the SQL verb against its
         folder.
         **One test had to be restructured, and it was hiding something.** `pg_history` drove its
         UPDATE image by hand under a comment reading "webhooks only create/delete" — true when
         written, and the reason the wrapper's own behaviour was never exercised there. It now
         gets its update image FROM `delete_webhook` and drives a raw DELETE for the delete
         image, which is only possible because that subscription never delivered — precisely the
         foreign key the soft delete exists for.
         Caught by CI rather than locally: `make check`'s Postgres tests self-skip without
         `ER_TEST_PG_DSN`, and after changing a statement I ran only `pg_webhooks` with the DSN
         rather than the whole tier. Running all of it is the step that was missing.
         **The mock was right and the server was wrong**, which is the inverse of this program's
         usual direction: `tests/mock-erserver` removes the subscription and keeps its
         deliveries, exactly the behaviour the server now has. The O3c-3 spec asserting
         "removing a subscription keeps its delivery history" was describing the intended
         contract the backend could not honour.
   - [x] **D10 `er` never reconciled its lake, so a lake older than a column stayed older.**
         Found by booting the full-stack dev stack and reading the seed output — the tier exists
         for exactly this and had not been run in a while:
         ```
         run seed-base: screening crm could not be recorded: Binder Error:
           Referenced update column matched_count not found in table!
           Did you mean: "changed_count"
         ```
         Three links. `create_ddl_table` is `CREATE TABLE IF NOT EXISTS` — it creates a missing
         table and never touches an existing one. The design's rule is that every lake job
         reconciles its lake first, and `er_entity_diff::reconcile_lake` does exactly that, but
         **only the dispatcher called it**; `er-cli` had no dependency on the reconciler at all.
         So E9b's four `ingest_batches` columns reached every dispatcher-driven org and no
         CLI-driven lake.
         And it was **invisible**: the screening write is best-effort, so the run printed one line
         and reported success while recording nothing. A best-effort path that swallows a
         PERMANENT error — a column that does not exist is not going to start existing — reports
         exactly like one that swallows a transient failure, which is how this survived.
         The fix is `er_cli::reconcile_lake_schema`, called once in `main` after the lake opens,
         because a CLI invocation IS a lake job. Additive changes apply in a transaction and are
         ledgered; a breaking difference refuses and leaves the lake untouched — the same contract
         the control plane holds itself to.
         Worth recording from writing the test: those four columns are **nullable**, and that is
         what makes them addable at all. A first draft of the fixture also omitted `path` and
         `full_refresh_keys` and got `ERR_SCHEMA_BREAKING: column path is absent` — the reconciler
         refusing to invent values for a NOT NULL column, which is the contract working rather
         than a problem.
         Two sabotages observed: reporting zero applied, and removing the reconcile entirely.
         **One behavioural consequence, and it is the intended one.** Reconciling creates every
         catalog relation, so a CLI lake now carries the whole schema from the first command
         rather than growing it lazily as `create_ddl_table` was called. That is exactly what a
         dispatcher-provisioned lake has always looked like — the CLI was the odd one out.
         `cli_e2e`'s "a data file per table" assertion held only because of the lazy creation; it
         now names the four relations base_10 never writes (`assertions`, `golden_overrides`,
         `record_overlays`, `record_qualities`) and fails if a populated one ever joins them,
         which is a stronger statement than the one it replaced.
         Caught by CI, not locally — `make check` **did not run `cargo doc`** despite CLAUDE.md
         claiming it "mirrors rust.yml", so a public doc linking to a private item passed here
         and failed there. `make doc` now runs rust.yml's exact rustdoc command and `check`
         depends on it; verified by restoring the broken link and watching it fail with the same
         message.
   - [x] **O7b-2 per-source duplicate attribution — and a gate with a hole in it.** The
         corpus-wide duplicate rate says a tenant has a problem; it does not say WHERE the
         problem comes from, and the answer is nearly always one feed. New
         `metric_snapshot_source` relation keyed `(run_id, source_system)`, written beside the
         corpus-wide snapshot and read by `GET /v1/orgs/{org}/insights/sources`.
         **Assembled in the same call, from the same `groups` CTE**, which is O7a's rule applied
         rather than restated: a snapshot built from several independent reads can describe a
         corpus that never existed. The test asserts it directly — the per-source record counts
         and duplicate counts must SUM to the corpus-wide figures, so the two cannot disagree.
         `contributed_groups` is kept as its own number beside `duplicate_records`, because a
         source can account for few duplicate records while TOUCHING many groups, which is the
         signature of a feed that adds one bad row to everything rather than one bad batch. Rows
         come back worst first: the question is "which feed is causing this", so the answer should
         be the first row. Rates are derived on read and null rather than 0.0 for a source with no
         records, the same judgement the corpus-wide series makes.
         **The gate that should have caught my mistake did not.** The new entity had no
         `er_schema::model::all_specs()` entry, so `create_ddl_table` could not build its table —
         and `write_metric_snapshot` is best-effort, so the failure arrived as a swallowed log
         line (`metric_snapshot_sources spec missing from S5 registry`) and the table simply never
         appeared. #283's registry-drift gate passed throughout: it compared only relations
         present in BOTH sources, skipping an absent one on the reasoning that "an entity may be
         declared before its registry entry exists". That reasoning was wrong — a DuckDB entity
         the registry has never heard of can never be created at all — and the skip is now an
         assertion naming the relation. Verified by removing the spec again and watching the gate
         fail with `["metric_snapshot_sources"]`.
   - [x] **O6-2 confidence bands, group-size floors — and two filters that were doing nothing.**
         The review queue was FIFO by review id with no way to say "the high-confidence tail
         first", which is the order that clears a 40,000-item backlog fastest. Duplicates paged by
         entity id with no size floor, and the clusters an over-merge hides in are the LARGE ones.
         Both are now declared statement parameters, so the filtering happens in the lake rather
         than after the page.
         **Two filters were already declared and ignored.** The BFF has listed `status` and
         `reason` as passthrough query parameters since the review inbox shipped, and
         `reviews_list` read neither — so the inbox's "Filter by reason" dropdown did nothing in
         production. Worse, the hook never sent `status` at all and the server had no status
         filter, so the queue listed RESOLVED reviews on every refetch. Both are the defect class
         this program keeps finding, and both were green in Playwright because the mock filters on
         exactly the parameters the server ignored — the mock encoding the frontend's belief
         rather than the server's behaviour, for the tenth time.
         **The band has to be honest about what it excludes.** A `never_unsatisfiable` escalation
         carries `CAST(NULL AS double)` for its probability — it is a structural contradiction,
         not a scored pair — so it is in no band, and it is the most urgent item in the queue.
         Three options were weighed: include it in every band (the filter then lies about itself),
         exclude it silently (the highest-priority work disappears behind a slider), or exclude it
         and SAY SO. The third: `unscored_open_count` is read whenever a band is active and the
         inbox renders "N escalations have no score and are outside every band". With no band the
         page is byte-identical to what it was before bands existed, nulls included.
         `min_members` is floored at 2 **in the statement**, not in Rust, because the statement's
         own description claims it: a one-member entity is not a duplicate group, so admitting one
         would change what the endpoint means. The analyser's planner has no `greatest`, so it is
         a CASE.
         **Two mock corrections, both the same class.** Its `never_unsatisfiable` row carried a
         probability where the engine writes NULL, which would have made the band appear to
         include it and the disclosure test vacuous. And its review probabilities were bunched in
         0.82-0.88, so every row sat in ONE band — the same trap the entity-event detail test fell
         into, where one kind cannot distinguish a working filter from an ignored one. Spread to
         0.62-0.92.
         Four sabotages observed, each independently: ignoring the band, ignoring the floor,
         suppressing the disclosure, and the Rust-side `status`/`reason` assertions that return
         the whole queue when the filter is dropped.
   - [x] **O6-1 bulk suppress and protect — and N correction passes for one decision.** The
         overlay endpoints took exactly one `record_key`, so the entity view's multi-select
         looped: one HTTP call per record, each with `apply_now`. Every one of those calls
         enqueues an `apply_corrections` job, and that job runs a correction pass over the
         **whole corpus** — so selecting ten members queued ten full passes to express one
         decision, and a steward clearing 500 junk records queued 500.
         It was slow rather than refused, which is why nothing noticed: `jobs_one_active_per_org`
         covers `dispatching|running|canceling` and NOT `queued`, so the jobs all queue happily
         and then run serially.
         `record_keys` now carries a batch, with `record_key` still accepted beside it — every
         existing caller keeps working, and a steward scripting against the API does not have to
         change to stay working. Keys are de-duplicated (the same key twice is one decision, not
         two audit rows) and order-preserving, so the staged actions read back in the order the
         steward chose. Still **one staged action per record**, because each is independently
         auditable and independently retractable; what the batch shares is the apply.
         Capped at 2,000 — the same ceiling the mass-update path uses — and over it is a 422
         naming both numbers rather than a truncation. A caller that believes it suppressed 3,000
         records and actually suppressed 2,000 has no way to find out which 1,000 are still live.
         A partial failure reports **what already staged**, because those actions are real and
         will drain; a bare "failed" would leave the caller retracting nothing and re-staging
         everything.
         `staged_response` gained a sibling rather than a wider signature: three other handlers
         stage exactly one action and their response shape is read by the frontend and pinned by
         tests.
         Both halves observed failing. Restoring the per-record apply makes the Postgres test
         report `left: 3, right: 1` on "three records must enqueue ONE correction pass"; restoring
         the frontend loop makes the Playwright test count two requests where it expects one. The
         UI reported "protected N records" either way, which is exactly why the test counts
         requests instead of reading the status line.
   - [x] **O7b-1 the insights surface — and a visual gate that was absorbing whole UI changes.**
         O7a retained the metric snapshots because every dashboard number had always been computed
         LIVE: the product could say what is true now and nothing about the direction of travel,
         and "is the duplicate rate going up" is the question an operator actually has. It then
         had **no caller at all** — no BFF route, no page. This is the caller: four tiles with
         sparklines, the signed change since the previous run, and the full series.
         Charts are hand-rolled DOM/CSS per the standing decision (pixel-deterministic for the
         baselines, legible to assistive tech). Two judgements the sparkline makes explicit:
         it scales **from zero**, because a series scaled to its own range makes a rate that moved
         from 4.0% to 4.1% look like a crisis; and a null point is a **gap, not a bar**, because
         O7a sends null rather than 0.0 for a rate it cannot compute and drawing a floor-height
         bar would assert the rate was zero. Both are asserted, the second by sabotage.
         `metric_snapshot` gained `wire: [json]` — it had been on the JSON wire since O7a shipped
         and never declared it, so no TypeScript binding existed and `InsightRow` had nothing to be
         checked against. Same omission E9b found in the delivery receipt, and the same fix. The
         two derived rates are declared as composed fields, which is what the gate is for.
         **The finding is bigger than the page.** Adding a nav item changed nothing in any
         baseline, which should have been impossible. Measured: a whole nav entry costs ~1,050
         pixels, and `maxDiffPixelRatio: 0.01` on a 1440x1170 full-page shot is a tolerance of
         about **16,800** — 16x headroom. Worse, `--update-snapshots` defaults to `changed`, which
         honours that tolerance, so a real change smaller than it is neither failed nor rewritten:
         the committed baseline silently stops depicting the app and goes on masking the next
         change too. Switching the regen to `=all` rewrote **20** stale baselines, including
         `marketing-hero-*` (which has no nav at all — pure accumulated drift) and the dashboard's
         job strip, which had still been showing the fabricated `progress` stage text D9 removed.
         The tolerance itself is left alone deliberately: tightening it is a gate-wide calibration
         that needs its own change, where the baselines it moves can be read as evidence rather
         than as noise inside a feature PR.
   - [x] **O4 the outbound write-back — the master record, not a notification.** The incumbent
         charges for merge write-back as an add-on. Here it is a catalogued event through the
         same signed, retried, logged outbox every other event uses, so there is no second
         delivery mechanism to operate: a tenant subscribes to `entity.write_back` and that
         subscription IS the per-org handler, with the secret, the delivery log and the UI all
         already built by O3b and O3c-3.
         **Deliberately NOT aggregated**, which is the one place it diverges from O3c-2 and the
         divergence is the design: an `entity.*` aggregate exists because a run's ledger volume
         scales with the corpus, while a write-back's volume is bounded by resolutions a HUMAN
         applied. And the receiving system needs the record — a count it would then have to fetch
         is not a write-back. Stated in the catalog entry itself so the contrast is readable
         where a subscriber chooses.
         **Driven off the derived LABELS, not the raw resolutions**, which gets dismissals right
         for free: a dismissal yields no label because it is not a verdict about the pair, and it
         changes no master either. The labels also already carry the canonical pair keys, which
         is exactly what the lookup needs. Both of a pair's keys are resolved and the entity ids
         deduplicated, so a `match` delivers one master and a `no_match` delivers both — a
         subscriber applying the change physically should not have to work out which.
         Fires only when the correction SUCCEEDED, one step further along than `review.resolved`:
         a write-back describes the corpus after the resolution took effect, so sending one for a
         failed correction would hand the tenant a master the engine does not believe in. A fresh
         session at the latest snapshot is opened after the blocking half commits, because that
         is the first point the corrected corpus is readable.
         Reuse rather than new machinery: one new declared read
         (`entity_membership/read/entity_of_key`) and the existing `read::entity_detail`, which is
         already entity-type aware — so person and Account both work with no parallel copy.
         Subscribers are checked BEFORE the lake is opened, since the read costs a session and a
         snapshot pin. Best-effort throughout, with every skip logged and its reason named,
         because a write-back that silently does not happen is indistinguishable from a tenant
         whose endpoint is fine.
         Two sabotages observed: suppressing the emit leaves the delivery list empty, and nulling
         the master leaves a payload the test rejects while still showing all four members — so
         the two halves of the assertion fail independently.
   - [x] **O2b emailed run reports — and the recipient question er-server cannot answer.**
         `POST /v1/email` has existed since #203 and `AppState.mail` was touched by exactly one
         caller: the `operator_only` send route. There was no dispatcher-side mailer at all, so
         nothing the engine did could ever reach a human by mail.
         **The plan's "which org members get a report" has no answer in this process**, and
         that is the finding rather than a gap to fill: membership lives in the *frontend's*
         database, so er-server has no user roster. Inventing a cross-store read for a
         notification would be a far worse trade than asking an operator for addresses. So
         recipients are an explicit list in the org's `env` under `ER_RUN_REPORT_TO` — the
         channel the dispatcher already reads nine `ER_*` keys from — and its **absence is the
         opt-out**: a tenant who never asked for mail does not get any because a run finished.
         The mailer is built by `dispatch_once` from env and passed into `dispatch_once_with`,
         mirroring exactly how `Profile::from_env()` is injected so tests can supply their own.
         The report rides the same settle as the `job.*` webhook and obeys the same rule the
         comment there already states — **one per OUTCOME, not per attempt**, because a job
         that fails twice then succeeds is one success, and three notifications for one
         incident is how an integration learns to ignore them. A send that fails is logged and
         dropped: mail is best-effort by design, and reporting an outcome is not part of
         producing it.
         **Reported kinds are a decision, not a default**: `run_all_full`,
         `run_all_incremental`, `correct`, `train` — the ones whose outcome a steward waits on.
         `lake_maintain` is platform housekeeping a tenant cannot act on and `provision` is
         already polled by the tenant-create UI with a progress indicator; mailing either
         teaches a recipient to filter the whole thread. Both still emit webhooks and still
         appear in the runs list.
         The whole decision is one pure function (`run_report_messages`) so every rule is
         testable without a mailer, a Postgres or a job — each branch is something someone will
         want to change later. Observed failing two ways: reporting every kind fails
         `housekeeping_kinds_are_not_reported`, and dropping the blank-address filter fails
         `absence_is_the_opt_out` (configured-but-blank must send nothing rather than mail an
         empty address). The success report carries the O3c entity counts and the failure
         report carries `error_detail`, which is already chain-flattened so it reads as a
         sentence rather than the outermost wrapper's Display.
         Fully verifiable without V0: `ER_MAIL_BACKEND=log` renders to the server log, so only
         real *delivery* waits on an SES identity. The two `dispatch_once_with` test call sites
         pass `Mailer::off()` deliberately — their orgs configure no recipients, and a mailer
         that cannot send turns a stray report into a logged error instead of a silent success.
   - [x] **O5a the tag vocabulary — and a soft delete decided before the first one was written.**
         Two entities (`tag`, `tagging`), eight declared statements, six handlers, five routes.
         A tenant can now name its own workflow and sort entities and review groups by it,
         which is the scope every bulk-action-by-tag will read.
         **Retiring a tag is a soft delete, and that is D11's lesson applied rather than
         rediscovered.** `taggings.tag_id` references `tags.id`, every foreign key in this
         schema is `NO ACTION`, so a hard `DELETE` would have failed for exactly the tags that
         had actually been used — which is precisely the webhook bug, where delete worked right
         up until the subscription had delivered something. Proven rather than asserted:
         swapping `retire.sql` to a `DELETE` makes the pg test 500 with "a tag with
         applications must still be retirable". **Applying a tag first is the only way a test
         can tell**, which is why the one that existed for webhooks missed it.
         The unique index on `(org_id, name) WHERE active` is partial for a reason the test
         pins: retiring frees the name for reuse while the retired row stays for history, the
         same shape `matching_models_one_active_per_name` uses.
         **Two framework rules learned by tripping them.** A cross-entity `returns` does not
         exist — `returns: entity` means *this* entity's shape, and typed column lists are
         lake-only for a control read — so a read belongs to the entity it RETURNS, not the one
         it joins through: `for_subject` returns tag rows and therefore lives on `tag`. And
         E009 then required the join be declared (`reads: [tagging]`) so the impact surface of
         a change to that relation is explicit rather than found by grep.
         **Duplicate detection is by SQLSTATE, not message text.** The first version matched
         the index name in the error string, which a Postgres upgrade is free to change;
         `pg.rs` already had the right idiom a few hundred lines up (`UNIQUE_VIOLATION` →
         `Ok(None)`). Detected at the schema rather than by reading first, because a
         read-then-write races two stewards naming the same tag.
         The role ladder: viewer reads, steward names **and** applies, admin retires. Steward
         creates because a steward who cannot name the work will put the distinction in a note
         field nothing can filter on; admin retires because it affects every subject the tag
         touched and every filter naming it. `subject_id` is deliberately **not** a foreign
         key — an entity lives in the lake and a review is rebuilt by every reconcile, so a
         database reference would point at a row this store does not own; the cost (a tagging
         whose subject has gone) is written into the column description rather than discovered.
         Applying is idempotent at the schema: the same tag twice is the same fact, so
         `DO NOTHING` keeps the original `created_by`, which is who first claimed the work.
         The envelope ledger from D13 caught the three new GET routes and forced a regenerate —
         that gate doing its job on the first unrelated change since it landed.
   - [x] **O5b the tag surface — and the 204 it found in O5a.**
         Chips on the entity view, reusable for reviews by passing a different `subject` pair,
         plus four BFF routes and the hooks. O5a is no longer inert.
         **Building it immediately found a real bug in O5a's own server code**, which is the
         argument for interleaving rather than shipping backends alone: apply, unapply and
         retire returned `204 No Content`, and `forward<T>` — the BFF's only forwarding
         helper — calls `response.json()` unconditionally. A 204 arrives as `SyntaxError:
         Unexpected end of JSON input`. The failure mode is the nasty kind: the upstream write
         SUCCEEDS, so the refetch shows the chip while the mutation's `onError` puts an error
         under it. The first functional test passed with a visible error on screen. No
         control-plane mutation in this server answers 204 — `delete_webhook` returns
         `Json(json!({"deleted": …}))` — so these three were a convention invented here that
         nothing on the client could consume. Fixed in O5a's PR rather than carried.
         **Grayscale is why the chip states are worth a component test.** A tag system is the
         most tempting place in the product to reach for colour, and with no palette the only
         thing separating applied from available is FILL versus BORDER. Invert that and every
         chip still renders and the screen still looks plausible, so nothing but an assertion
         notices. `TagChipsView` is split from the container precisely so that render logic
         sits in the component tier the repo defines for it ("a component whose output derives
         from data... chips").
         **Four diagnoses, three of them wrong first**, recorded because the sequence is the
         lesson: the failure snapshot is the END state, so a picture with no chip was the
         post-removal page rather than proof apply was broken; `mutateAsync` is a readability
         win and NOT a bug fix, since the nested-callback form worked (the comment claiming
         otherwise was corrected rather than left as a plausible story); the tests did race on
         a shared entity, but that was not this failure; and the real cause was that the picker
         stays open after a create — correct behaviour, since a steward tagging one thing
         usually adds several — so `tag-add` is absent and the test was wrong, not the UI.
         The viewer spec reads a SEEDED tag in the shared org, for the reason E10c-2 hit: the
         erweb fixtures grant a viewer membership only in `ORG`, so a viewer in a per-project
         org gets a 404 from the tenant layout. Seeded also means read-only, so it cannot race.
         Coverage fell below the floor twice as the component landed and was restored by
         testing real logic both times — never by lowering a threshold. The second pass
         extracted `src/lib/bff/tags.ts`, which also removed the `applySchema` duplicated
         across the apply and unapply routes. Three visual baselines moved: `entity-detail` on
         all three device projects, which is exactly right, since the chips render at every
         size and nothing shared changed.
   - [x] **O5c the approval gate — off by default, and disabled by retiring the tag.**
         A new schedulable kind `apply_approved`: for every review carrying the tenant's
         approval tag it stages a `resolve_review` and enqueues the existing
         `apply_corrections`. **The gate decides WHAT to merge and never how** — reusing the
         steward apply path means this adds no second way to merge a group, which a scheduled
         job reaching into the lake itself would have been.
         **Off by default, and that is the assertion that matters.** No `approval.tag` means
         nothing is ever staged; an approval workflow that is on until switched off would merge
         groups nobody looked at. The sabotage that ignores the config gate fails on exactly
         that phase. `approval` is `skip_serializing_if` empty, so `config_hash` is
         byte-identical for every tenant that never asked for it — the rule V1, E3 and E11a
         each followed, pinned here by its own test rather than left to the golden.
         **The config names a tag by NAME, not id**, for two reasons that turned out to be the
         same one: a config is a document a human edits and copies between environments (O9),
         where an id would not survive the copy — and resolving by name against the LIVE
         vocabulary means retiring the tag disables the workflow, which is what an operator
         would expect retiring it to do. Asserted: a review still carrying the tagging stages
         nothing once the tag is retired.
         **The tag comes off as each review is staged.** It means "approved, pending
         application", so leaving it would re-stage the same review every tick; the durable
         record is the staged action and its history, not the attachment. That is also what
         makes a second tick a no-op, and the sabotage that skips the untagging fails on it.
         Resolution is always `match`: approving a duplicate group means the records are the
         same, and reading a tag as "whatever the steward intended" would be a guess the data
         cannot support. A steward who means `no_match` resolves it directly. Capped at 200 a
         tick so a tenant approving a thousand groups sees them merged over several ticks
         rather than one job holding the org's single writer slot — the remainder keeps its
         tag, which is why removing it as it is staged matters.
         **Two guard rails caught me, both working as designed.** `dispatch_coverage` rejected
         my `APPROVED_MERGE_KIND` constant: it checks for a missing arm by scanning source for
         the kind's LITERAL, so a constant is invisible to it and reintroduces exactly the gap
         that let `train` sit in `SCHEDULABLE_KINDS` with no arm. Literals restored in both
         places, with the reason at the arm. And the job failed with `org env missing
         ER_LAKE_CATALOG` before reaching that arm, because every job reconciled its lake
         first — but `apply_approved` never opens one, so it now joins `provision` in skipping
         it. That is not merely expedient: it means a tenant whose lake is unreachable can
         still RECORD approvals, where the apply reconciles when it runs.
         O6 bulk depth (#9/#62/#52), O8 workflows + triggers (#55/#56). (O3b, O3c and O7a have
         shipped; O7b's insights depth is the remaining half of #45/#57/#85.)
5. **Cleanse + verify**
   - [x] **V3 per-record data-quality scoring (#63/#73).** 0-100 per record, with the three
         dimensions kept separately — because they fail independently and a single number says a
         record is bad without saying what to do about it: a record can be COMPLETE and wrong, 
         VALID and nearly empty, or both and five years stale.
         New optional `data_quality` config block (weights per attribute, per-dimension weights,
         freshness window) and a `record_quality` lake table written at the end of each run.
         Unset writes nothing, so the table's absence is a byte-exact no-op; `GET
         /v1/orgs/{org}/quality` answers `configured: false` rather than erroring, because
         scoring is opt-in and asking about it is a fair question either way.
         **The judgement that makes the score mean anything is what happens when a dimension does
         not APPLY.** Validity is measured over the validatable fields that are PRESENT: a record
         with no email is already penalised by completeness, and scoring its validity zero as well
         penalises one absence twice and makes "fill in the blanks" look like it improves
         validity. A record with none of them scores NULL and is averaged over the other
         dimensions rather than handed a zero it did not earn. Freshness is NULL when the source
         carries no timestamp, because unknown age is not old — otherwise a source that simply
         does not publish `updated_at` drags every one of its records to the bottom of the work
         queue. `email_valid`/`phone_valid` are reused, not recomputed: standardize already
         decided, and a second opinion that could disagree would be a bug with two plausible
         answers.
         The read reports BUCKETS and per-dimension means, not just a mean — a mean of 70 can be
         every record at 70 or half at 95 and half at 45, and those need different work — plus
         the worst records as a queue. Scores are upserted on `record_key` and pruned when a
         record leaves the corpus, so a tombstoned record does not keep a score for good (the
         same upsert-only trap E7 found in membership).
         Test: `record_quality_scores_each_dimension_only_where_it_applies` asserts the scores
         DISCRIMINATE (a constant would satisfy every range check), that base_10's
         email-less `crm:C008` loses completeness while keeping validity 1.0, that re-scoring the
         same corpus five months later LOWERS the total (so freshness is computed rather than
         stubbed), that re-scoring corrects rather than duplicates, and that every scored record
         lands in exactly one bucket.
   - [x] **V1 transform (cleanse) rules (#61).** Ordered per-attribute rules — `lower`, `upper`,
         `trim`, `replace`, `regex_replace`, `strip`, `substring`, `default`, `dictionary` —
         compiled into the staging projection and applied to the RAW value **before**
         standardization.
         **Before, not after, and that ordering is the feature.** The normalizers decide what an
         email or a phone number *is*; feeding them `"  BOB@EXAMPLE.COM "` spends their judgement
         on noise a cleanse rule removes for free. A rule that ran afterwards would be editing the
         engine's conclusion rather than its input.
         The test asserts that interaction rather than hiding it: a **case** rule on `given_name`
         is deliberately asserted to have **no visible effect**, because `name_norm` lowercases
         and has the last word — an operator reaching for "upper-case the names" will see nothing
         change, and the reason is the ordering. A `dictionary` rule (base_10's `Bob` → `Robert`)
         *does* survive, changing exactly one record and one column.
         Two small judgements. `strip` is a chain of literal replacements rather than a character
         class, so there is no escaping to get wrong and `-` or `]` behave like any other
         character. A `dictionary` passes **unmatched values through** — it is a correction list,
         not an allowlist, and nulling the rest would silently delete most of a column.
         `GET`-able preview via `er_cli::preview_transform`, which reads `raw_records` and returns
         **only the rows the rules change**: previewing against standardized values would answer
         the wrong question, and padding with unaffected rows makes a rule look inert.
         Validated at publish (new `regex-lite` dep, licence/advisory gates clean): an attribute
         that is not canonical, an empty rule list, an uncompilable regex, a non-positive
         substring, an empty `strip` or `dictionary`. A misspelled attribute key is otherwise
         **completely silent** — the rule simply never applies and the tenant concludes the
         feature is broken.
         Empty by default, so `config_hash` and every `std_hashes` golden are unchanged; the test
         re-standardizes with no rules and asserts byte-identical output. A config that *does*
         declare rules moves `config_hash`, which promotes the next incremental run to a full one
         through the existing drift guard — correct, because the corpus was standardized
         differently.
   - [ ] **V0 SES apply — the DNS half is now Terraform's; what remains is an apply.**
         The blocker was never "the account has no domain". §18.8 resolved that on 2026-10-10:
         **`dupezero.com`, registered at Cloudflare**, which pins the apex to Cloudflare's
         nameservers — so there is deliberately **no Route 53 zone**, and the same decision
         named the Terraform `cloudflare` provider as what writes into the zone ("SES
         Easy-DKIM CNAMEs and MAIL FROM records land in the Cloudflare zone the same way" as
         the ACM validation CNAME).
         `ses.tf` contradicted that: it listed "publish these records in Cloudflare" as a
         manual step, and the module's header said DNS was "deliberately NOT managed here" as
         though that meant not managed by Terraform at all. It meant not by THAT module's
         provider. The provider is now declared (`cloudflare ~> 5.0`, reading
         `CLOUDFLARE_API_TOKEN` from the environment so no token reaches state) and the three
         DKIM CNAMEs plus the MAIL FROM MX and SPF TXT are `cloudflare_dns_record` resources
         in `envs/dev/ses.tf`. The module stays single-provider on purpose — one that reached
         for `cloudflare_dns_record` would force every caller to configure that provider even
         when its zone lives elsewhere.
         **Not applied, and not validated by `terraform`**: this area is human-applied, and
         the agent session cannot run `init`/`validate`/`plan` (a hook enforces it), so the v5
         resource schema is written from the provider's documented shape and wants a `validate`
         before an apply. Two details worth a second look there: `cloudflare_dns_record` takes
         MX `priority` as its own field (which is why the module now exposes
         `mail_from_mx_host` separately from `mail_from_dns`'s combined string), and the DKIM
         records use `count = 3` rather than `for_each`, because the tokens are computed and
         `for_each` over them fails at plan time.
         What is genuinely left: export `CLOUDFLARE_API_TOKEN`, supply `ses_domain` and
         `cloudflare_zone_id`, apply — then file SES production access, which is a
         human-reviewed request and nobody's Terraform.
6. **AI on Bedrock**
   - [x] V5 Bedrock runtime path + per-org BYO LLM (#77). **The runtime path was already
         there** — `ANTHROPIC_BASE_URL` is applied at the only two call sites, so pointing the
         deployment at Bedrock is a base-URL + model-id swap and always was. What was missing
         was that all three knobs (on/off, model, key) were **process-wide environment
         variables**: a tenant could not switch the assistant off, could not use its own
         account, and — the one that actually mattered — could not find out why it was
         refusing, because both routes answered the same "ANTHROPIC_API_KEY is unset" whatever
         the real reason.
         So this is `erweb.org_ai_settings` plus a resolver, and four design calls:
         * **No row means INHERIT, not disabled.** `enabled` has no column default and a row
           is written only when an admin expresses a preference, so the table ships empty and
           every existing tenant keeps exactly the behaviour it had. A `false` default would
           have switched AI off for the whole fleet on deploy — a silent regression dressed as
           a migration.
         * **A tenant's key forces the PUBLIC Anthropic API**, never the deployment's
           `ANTHROPIC_BASE_URL`. On a Bedrock deployment that base URL is a Bedrock endpoint,
           and an Anthropic key sent there fails with a 401 that reads as "the tenant typed
           their key wrong". The endpoint has to follow the credential; this is the one rule
           here that is correctness rather than preference.
         * **The off switch outranks a stored key**, or switching AI off would also require
           deleting the key, and the off state would be unreachable for anyone on the
           deployment key.
         * **Saving VERIFIES**, with one `max_tokens: 1` call against the merged
           configuration — which is what removes the need for a model allowlist. A closed set
           is right for `subject_type` and `webhook.events` because an unconstrained value
           there is *silent*; a wrong model id is a loud 404, so an allowlist would only have
           to track every id across the public API and whatever Bedrock profiles a deployment
           uses, and would reject a model that works. `previewAiConfig` does the merge because
           only it can read the stored key: verifying the submitted fields alone would check a
           new model id against the deployment key while the tenant's own key is what the call
           will really use.
         The key is sealed in the existing AES-GCM vault under the pseudo-role `ai`, which
         cannot collide with `viewer|steward|admin`, so a credential row pasted across fails
         to decrypt rather than being used as a model key. It is **never returned** by either
         verb, not even truncated — an org has at most one, so `key_set` + `key_set_at`
         identifies it completely and a last-4 hint would be disclosure that buys nothing.
         The read is open to any member (the state is what tells a steward why the assistant
         is refusing) and the write is admin-only.
         Tests: 42 on the three-state encoding and the resolution precedence, 16 against real
         Postgres (PGlite) for the vault round-trip, the AAD binding and the partial-write
         merge. Two sabotages observed failing first — a tenant key keeping the deployment
         base URL, and an empty key box sending `null` — each caught by exactly the assertion
         written for it.
         **A test-environment trap found here, worth knowing before writing the next vault
         test:** the unit tier runs in jsdom, which installs its own `Uint8Array` global, so a
         Node `Buffer` fails `instanceof Uint8Array` and PGlite's bytea serializer rejects
         every sealed key with `Invalid input for bytea type`. It is purely an artifact — in
         the server runtime a Buffer *is* a Uint8Array, which is why the Playwright fixtures
         seal credentials into PGlite without trouble — but any test that writes through the
         vault needs `// @vitest-environment node`. A probe found this in one run after two
         wrong theories.
   - [ ] V6 LLM adjudication on gray-band reviews (#27). Auto-apply off by default.
   - [ ] V7 NL rule authoring + explain-rule (#29). Writes a DRAFT version, never publishes.
   - [ ] V8 AI schema mapping + AI model setup (#48/#35). Sends a profile summary, never rows.
   - [ ] V9 staged agent write-actions (#81), through the existing steward-action path.

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
7. **Governance + platform**
   - [ ] O9 config promotion: export/import/diff across orgs (#76).
   - [ ] O10 MCP server over the generated OpenAPI + org service identity (#79/#78). **Wants
         `#[utoipa::path]` on the handlers first** — `openapi.json` documents zero paths by a
         recorded decision, so there is nothing for an MCP server to generate tools from. The
         response-ENVELOPE question is already answered without it, by the derived ledger D13
         built (`frontend/src/lib/api/envelopes.json`).
   - [ ] O11 embeddable duplicate widget: a signed-token read-only micro-page (#59).
   - [ ] O12a saved views / folders (#87).
   - [ ] O12b usage metering ledger (#88).
   - [ ] O12c field-value rollback UI (#12/#13).
8. **Review-UI depth**
   - [ ] F1 group merge editor (#8) — today's inbox is a PAIR adjudicator; this adds the
         master-editing half. Reuses `field-compare-grid`.
   - [ ] F2 pre-load data integrity check (#46), riding E9b's staging.
   - [ ] F4 verify-before-export (#86, de-Salesforced).
   - [x] **F3 absorbed into O7b** — the insights page and the dashboard trend charts shipped
         with the attribution work (#292/#295/#297), which is why #45/#57 are not listed here.

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
