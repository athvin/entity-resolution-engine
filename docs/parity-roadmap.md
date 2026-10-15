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
- [ ] **0.1 Frontend CI + this roadmap** — `.github/workflows/frontend.yml` (lint+types+unit, and the Playwright
      functional tier in the pinned image). *In progress.*
- [ ] **0.2 Coverage (nightly)** — cargo-llvm-cov + vitest `--coverage`, committed baseline + non-regression check.
- [ ] **0.3 `envs/prod` foundation** — network/EKS/Aurora-multi-AZ/lake/CloudTrail with `er-prod-*` names; ALB/WAF+SES stubbed (domain-blocked).
- [ ] **0.4 Builder-key prod guard** — §8.2 `DenyProd*` denies on `builder.tf`; commit it into the repo.
- [ ] **0.5 Promote-on-merge → prod** — digest-pinned prod rollout on merge to main via CI github-oidc role.
- [ ] **0.6 Password-reset page** — add `(auth)/reset`; closes the self-serve username/password workflow.

### Phase 1 — Stable-core parity (generic; no external deps)
- [ ] **1.1 Account/Org entity type** — generalize the person-centric schema across standardize/block/match/survivorship.
- [ ] **1.2 Mass update / mass delete** — filtered mass field-update + mass-delete with preview + audit.
- [ ] **1.3 Protected/excluded flag + survivorship exception rules**.
- [ ] **1.4 Score-only match preview** (frontend-design §7.16) — no-commit match run w/ probability + waterfall.
- [ ] **1.5 Confidence-threshold auto-merge / point-of-entry dedupe**.
- [ ] **1.6 Deep field-level survivorship** — grouped/controlling fields, combined-list, prioritized, sum, max-chars.
- [ ] **1.7 Nickname/synonym lexicon editing** — finish the M6 partial.

### Phase 2 — Top 2 differentiators
- [ ] **2.1 Data-quality scoring models** — Completeness / Junk / Stale / Duplicate-rate.
- [ ] **2.2 Health dashboard / DQ-Score KPIs**.
- [ ] **2.3 AI enrichment** — fill-missing / normalize via AWS Bedrock (or existing Anthropic key); cost-gated. STOP.

## Explicitly skipped / deferred (not building)
- Salesforce / Marketo / Lightning / lead-convert (decision #1).
- ZoomInfo/Apollo enrichment + email verification (outside AWS / no clean AWS API).
- Public prod ingress (ALB/WAF) + SES email (domain not set up yet — revisit when it is).
- RevOps analytics, chained workflows, AWS-native address(Location Service)/phone(Pinpoint) verification (beyond the cut).

## Change log
- 2026-10-15 — Program kicked off; plan approved; PR 0.1 opened (frontend CI gate + this tracker).
