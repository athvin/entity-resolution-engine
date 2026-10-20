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
- [x] **W0.1 worktree + this tracker section.** All program work happens in a dedicated
      worktree (`../ere-datagroomr-parity`) so it cannot disturb other local branches.
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
- [ ] **W0.2 scheduler tick** — `schedules` has CRUD and cron validation but **nothing ticks**:
      no row is ever enqueued and `last_enqueued_at` is never written, so every schedule in the
      product is inert. `config.correction_pass.cadence` is likewise stored and unused.
- [ ] **W0.3 `run_all_incremental` is not incremental** — it shares the `run_all_full`
      dispatcher arm, and `job.params` (`skip_ingest`, set by 6 call sites) is never read.
      Behaviour lands in E7; W0.3 stops the lie and wires `er-versions::check_mode_preconditions`
      (a crate with zero consumers today).
- [ ] **W0.5 harness debt** — `make frontend-seed` is documented in the Makefile as "NOT
      restored" (its `frontend/dev/seed.py` was deleted), which is why the full-stack Playwright
      tier is dormant; `make check-all` is still cited in `docs/infrastructure.md` +
      `docs/plan-to-aws.md` and does not exist; `fixtures/static/FORMAT.md` names deleted
      Python scripts.
- [ ] **W0.6 `er-web` image push** — the ECR repo and `frontend/Dockerfile` exist but no CI job
      pushes the image, so the BFF cannot be rolled out to dev EKS for live acceptance.

### Waves 1–8 (queued)
1. **Matching backbone** — model registry + API (#34/#21/#28), the config→model compiler that
   makes `comparisons:`/`blocking:` actually execute (#4/#21/#22/#23/#24), hard constraints and
   relationship exclusions (#18/#32), type-aware comparators (#25).
2. **Scale + prevention** — ensembles/cascades (#31), blocking estimates + segment scoping
   (#3/#54), true incremental (#36/#40), online resolve API (#37), push-records + import-time
   dedupe (#38/#39/#41), S3 auto-ingestion (#47).
3. **Learning loop** — labels/train/evaluate (#19/#20 + the evaluation-tooling gap),
   related-record features (#26), household/link entities (#33).
4. **Operating at scale** — job retry + run reports (#58), webhook v2 with an outbox (#43),
   write-back handler (#44), tags/approval (#51), bulk depth (#9/#62/#52), insights and
   time series (#45/#57/#85), workflows + triggers (#55/#56).
5. **Cleanse + verify** — transform rules (#61), international standardization (#60),
   per-record DQ (#63/#73), the AWS-native verification suite (#64–#72).
6. **AI on Bedrock** — Bedrock wiring + per-org BYO LLM (#77), LLM adjudication (#27),
   NL rule authoring (#29), AI schema mapping + model setup (#48/#35), staged agent
   write-actions (#81).
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
