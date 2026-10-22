---
name: security
description: This repo's security ground truth — the attack surface (internet-facing Next.js BFF in front of the privileged Rust er-server control plane), the known vulnerability classes specific to entity resolution (tenant-isolation by lake-catalog separation, config-as-code raw SQL, webhook SSRF, assertion-forced merges), and the exact existing test harness that locks each one. Use when reviewing an endpoint or handler for SSRF/injection/tenant-isolation/auth/business-logic abuse, adding a security regression test, triaging a vulnerability finding, writing a proof-of-concept, or deciding where a security invariant belongs. For the autonomous campaign that fans out, verifies, and writes the tests, run /pentest instead. Not for adding a normal table/column/SQL statement — that is the entity-framework skill.
---

# Security

Two rules, and the whole suite is built on them:

**No finding is real until an adversarial verifier reproduces it in fresh context.**
**No finding is done until a regression test locks it in an existing harness.**

Strix's differentiator is "no speculative findings — every finding ships a working PoC." We keep that
and add the part that matters for *this* repo's goal (secure **and** properly tested): the PoC is not
the deliverable, the **test** is. A PoC proves a bug once, today, on one build. A regression test in
`cargo test` / `pnpm test` proves it can never silently come back.

## The shape of the target

Two tiers. Get this straight before touching anything.

| Tier | Where | Auth | Faces | Role |
|---|---|---|---|---|
| **BFF** (Next.js) | `frontend/src/` | cookie session `er_session` (argon2id, sha256-at-rest), holds per-org `erk_` keys AES-256-GCM-sealed | **the internet** | the real external attack surface (~61 routes) |
| **`er-server`** (Rust/axum) | `rust/crates/er-server/` | bearer: operator token or `erk_<id>_<secret>` | internal (BFF → it) | the privileged downstream |

Role lattice: `Viewer < Steward < Admin < Operator` (`lib.rs`, derived `Ord`). Auth is a
`Principal` extractor (`FromRequestParts`) so a handler that forgets it simply has no principal —
auth is structurally hard to skip. Authorization is the `guard(principal, org, minimum)` call;
wrong-org resolves to **404** (no org enumeration), under-privileged to 403.

**The crown jewel is the SQL governance** (see the `entity-framework` skill). Every persistent
statement is declared and parameter-bound; the only textual substitution in the whole generated crate is the
DuckLake snapshot pin (an `i64`, not injectable). This shrinks injection to a handful of
*declared-raw* config fields. Respect it: the fastest way to introduce a vuln here is to route
user input into SQL outside a declared statement.

## File map

| What | Where |
|---|---|
| Attack-surface inventory (route/sink → tier → guard → gap) | [references/attack-surface.md](references/attack-surface.md) |
| **Category → exact test harness → how to extend** (the core) | [references/harness-map.md](references/harness-map.md) |
| Tenant isolation (org_id rows + lake-catalog invariant + impersonation) | [references/tenant-isolation.md](references/tenant-isolation.md) |
| Injection (governed SQL, config-as-code SQL, CSV, path, `:action`) | [references/injection.md](references/injection.md) |
| SSRF / egress (webhook + LLM-tool + the IMDS limit) | [references/ssrf-egress.md](references/ssrf-egress.md) |
| AuthN / session (cookies/CSRF, brute-force, key hashing, error leak) | [references/authn-session.md](references/authn-session.md) |
| Business logic (assertions, thresholds, DoS, config panics) | [references/business-logic.md](references/business-logic.md) |
| Secrets / IaC / supply chain | [references/secrets-iac.md](references/secrets-iac.md) |
| The autonomous campaign | [../pentest/SKILL.md](../pentest/SKILL.md) |

## Reviewing one thing for security: the checklist

```
- [ ] 1. Which tier is it? BFF (internet-facing) or er-server (privileged)?
- [ ] 2. What auth/role does it require, and is guard()/requireOrgAccess actually called?
- [ ] 3. Does any caller-controlled value reach: SQL, an outbound URL, a filesystem path,
         a format!/template string, an LLM prompt, or a CSV/JSON export cell?
- [ ] 4. Find the category in references/harness-map.md → its existing test file.
- [ ] 5. Write the regression test; show it FAILS against current code (that is the PoC).
- [ ] 6. Propose the fix; show the test passes. Do not weaken the test to make it pass.
```

Step 5 is non-negotiable. A security test that passes before the fix is proving nothing.

## Common workflows

- **Reviewing a diff / a PR** → use the built-in `/security-review` (diff-scoped). This skill is the
  project-specific knowledge layer *behind* that review — cite the attack-surface and harness-map.
- **General correctness review** → `/code-review`.
- **Hunting the whole surface (or one category) with PoCs + regression tests** → `/pentest`
  (hermetic by default; `/pentest live` for the fenced live pass).
- **Adding one security regression test by hand** → open [references/harness-map.md](references/harness-map.md),
  find the category, copy the named harness's pattern.
- **"Is live exploitation safe here?"** → only against a local throwaway stack, and only via
  `/pentest live`, which hard-refuses cloud/prod targets. See [../pentest/references/modes.md](../pentest/references/modes.md).

## The rules that bite (repo-specific security invariants)

- **Tenant isolation on the data plane is physical, not row-level.** Lake read statements carry **no
  `org_id` predicate** — isolation rests entirely on `open_org_lake` resolving a distinct
  `ER_LAKE_CATALOG`/`ER_LAKE_DATA_PATH` per org. If two orgs ever share a catalog, it is *silent,
  total* cross-tenant exposure with no second line of defence. This invariant must have a test.
- **Three config fields are raw SQL by design**: `blocking[].expr`, `training.deterministic_rules`,
  `training.em_blocking_rules`. Publish-time analysis (`config_check.rs`) checks column *references*
  in `blocking[].expr` only — the two `training` lists get none. Treat config publish as an
  authenticated code-execution surface gated by role, not as safe input.
- **Outbound URLs from config/requests are SSRF** unless host-filtered — and the guard now exists.
  `webhook::client()` refuses redirects outright (`Policy::none()`) because a first-hop-only check is
  bypassed by a 302 to `169.254.169.254`, and carries a delivery timeout so a stalling listener cannot
  hold a dispatcher thread. There is **no env var and no config field to disable it**, deliberately: a
  guard configuration can switch off is a guard that gets found switched off. The cost is a
  bypass-shaped seam (`emit_with_egress` / `Egress`) for the delivery test, and
  `tests/egress_guard.rs` is the posterity gate that pays it — it fails if anything under
  `er-server/src` names the unguarded entry points, `Egress::Unguarded`, or
  `reqwest::Client::new()`. Kubernetes egress is still unrestricted by design, so the app-layer
  guard remains the only layer we can test locally.
- **`er-server/src` must contain no `lake.main.<x>` literal and no `query_strings` caller.** These are
  posterity gates (`freeform_sql.rs`). If a security fix needs lake SQL, it must be a declared
  statement — never assembled inline.
- **Every request/config DTO should carry `#[serde(deny_unknown_fields)]`** (mass-assignment
  defence). This is a positive invariant worth a posterity gate.
- **Internal errors must not reach clients.** `format!("db: {e}")`-style bodies leak schema/driver
  detail; return a generic `detail` and log the rest.

## Finding → how to prove it → the test that locks it

A condensed index; the full table with exact files and line hints is
[references/harness-map.md](references/harness-map.md).

| Category | Prove it by | Test home |
|---|---|---|
| Cross-tenant lake read | two orgs made to share a catalog → one reads the other | NEW `er-server/tests/pg_tenant_isolation.rs` |
| Webhook SSRF → IMDS (**guarded**) | register `http://169.254.169.254/...`; delivery refuses it | units in `webhook.rs`, `tests/pg_webhooks.rs`, + the `tests/egress_guard.rs` posterity gate |
| Config-as-code SQL | publish a config whose rule calls a subquery/function | `config_check.rs` unit tests |
| CSV formula injection | a record name `=cmd()` exported uninjected | unit in `read.rs` (`merge_plans_csv`) |
| Assertion forced-merge | `POST /assertions` with nonexistent keys succeeds | `er-server/tests/pg_steward.rs` |
| Login brute-force | N bad logins, no lockout | vitest + `frontend/tests/functional/auth.spec.ts` |
| Prompt injection / PII→LLM | tenant data steers a tool call to another org | `frontend/tests/unit/assistant-tools.test.ts` + functional spec |

## When a review turns up something real

Don't fix it inline and move on. Run (or hand off to) `/pentest` scoped to that one category so the
finding goes through verify → lock → report like every other — that is how it stays fixed. If you are
already mid-fix, at minimum add the regression test from the harness-map and show it fails first.
