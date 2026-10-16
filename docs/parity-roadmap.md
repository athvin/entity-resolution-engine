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
- [ ] **0.5 Promote-on-merge → prod** — digest-pinned prod rollout on merge to main via CI github-oidc role.
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
- [ ] **1.1 Account/Org entity type** — generalize the person-centric schema across standardize/block/match/survivorship.
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
      entity-view "Exclude" action. *Follow-ups (noted): protect ENFORCEMENT (force-master in merge_plans + never-cut)
      — the `protect` overlay is stored but not yet enforced; and exception-group UI (reuses existing `never` assertions).*
- [x] **1.4 Score-only match preview** (frontend-design §7.16) — `er_cli::preview_match` runs the committed model
      read-only (pairs + bucket counts + evidence waterfall) via a session-local temp table, committing NOTHING
      (no match_scores/reviews/golden; determinism contract untouched). New `GET /v1/orgs/{org}/match-preview`
      (steward, spawn_blocking) + a "Preview matches" card on the duplicates page. e2e test proves read-only
      (match_scores absent + snapshot unchanged after preview).
- [ ] **1.5 Confidence-threshold auto-merge / point-of-entry dedupe**.
- [x] **1.6 Deep field-level survivorship (Bucket A)** — added single-winner rule tokens: `longest`, `shortest`,
      `numeric_max`, `numeric_min`, `most_common` (alias of `frequency`), and `prioritized:<v1|v2|...>` (ranked list).
      Additive `rule_terms` arms + palette entries only — NO config/DDL change, so every golden fixture is byte-identical
      (verified: full `cargo test --workspace` green, zero golden drift). New Session-based election test (6 cases).
      **1.6b (deferred):** Bucket B `combined_list`/`sum` (synthesize a value not tied to one record → need a new
      lineage/assembly path) + generalized grouped/controlling fields (the hardcoded `address` grouping is the precedent).
- [x] **1.7 Nickname/synonym lexicon editing** — finished the M6 partial: `GET`/`PUT /v1/orgs/{org}/lexicon`
      (per-org CSV at `ER_LEXICON_PATH`, canonicalised via `er_std::lexicon`, PUT enqueues a `run_all_full` to
      apply), + a monochrome `LexiconEditor` on the config page. Determinism-safe (per-org file; not in
      `config_hash`, no DDL/fixture/golden change). e2e `pg_lexicon` + viewer/admin functional tests.

### Phase 2 — Top 2 differentiators
- [x] **2.1 Data-quality scoring models** — read-only `GET /data-quality` (Completeness / Junk / Stale /
      Duplicate-rate + composite DQ score), fixed-metrics v1 (no config change → determinism contract untouched).
- [x] **2.2 Health dashboard / DQ-Score KPIs** — monochrome "Data quality" section on the dashboard (KPI tiles +
      per-attribute completeness bars).
- [ ] **2.3 AI enrichment** — fill-missing / normalize via AWS Bedrock (or existing Anthropic key); cost-gated. STOP.

## Explicitly skipped / deferred (not building)
- Salesforce / Marketo / Lightning / lead-convert (decision #1).
- ZoomInfo/Apollo enrichment + email verification (outside AWS / no clean AWS API).
- Public prod ingress (ALB/WAF) + SES email (domain not set up yet — revisit when it is).
- RevOps analytics, chained workflows, AWS-native address(Location Service)/phone(Pinpoint) verification (beyond the cut).

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
