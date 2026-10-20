# Tenant isolation

The highest-value category in a multi-tenant entity-resolution product: a single isolation failure
exposes one customer's golden records (names, emails, PII) to another.

## Contents
- The two mechanisms (and why one has no backstop)
- The invariant that has no test today
- Role lattice and guard coverage
- Impersonation and super-admin
- How to prove / how to lock

## The two mechanisms

Isolation works differently on the two planes — know which you're reasoning about.

1. **Control plane (Postgres) — row-level by `org_id`.** Every by-id statement filters on it, e.g.
   `entities/job/read/by_id.sql`: `WHERE org_id = $org_id AND id = $job_id`. Handlers resolve
   `org_or_404` → `row.id` first. This looked consistently correct in review; `revoke_key` is
   org-scoped and returns `false` for another org's key.
2. **Data plane (DuckLake) — physical, NOT row-level.** Lake read statements have **no `org_id`
   predicate** (e.g. `entities/review/read/by_id.sql` is just `WHERE id = $review_id`). The *only*
   thing keeping org A out of org B's data is `open_org_lake(env)` (`er-server/src/read.rs`) resolving
   a distinct `ER_LAKE_CATALOG` / `ER_LAKE_DATA_PATH` per org, and each org's lake is attached
   read-only.

**Consequence:** if two org rows ever share a catalog/data-path, cross-tenant exposure is silent and
total — there is no row predicate to catch it. Writing `env` is operator-only
(`create_org`, `update_resources` which allows only two DuckDB keys; `put_lexicon` can set only a
derived `ER_LEXICON_PATH`), so the blast radius is "operator misconfig," but the cost is catastrophic
and nothing asserts the invariant.

## The invariant that has no test today

> No two rows in `orgs.env` may share the same `ER_LAKE_CATALOG` or `ER_LAKE_DATA_PATH`.

This is finding #1 in the harness-map. It belongs in a **NEW** env-gated pg integration test
`er-server/tests/pg_tenant_isolation.rs` (clone `pg_orgs.rs`): create ≥2 orgs, read back `orgs.env`,
assert catalog/data-path uniqueness across all org rows. Also assert the fix path — that
`update_resources` refuses to set a catalog already owned by another org.

## Role lattice and guard coverage

`Viewer < Steward < Admin < Operator`. `guard(p, org, minimum)`: operator bypasses; wrong-org → 404;
in-scope-but-under-privileged → 403. Every handler guards (some via delegation —
`record_overlay_action`, `transition_org`). The durable protection is **finding #13**: a static gate
asserting every route in `build_routes` reaches a guard. The behavioural proof is **finding #2**:
extend `pg_auth.rs` (which already proves cross-org *write* → 404) with a wrong-org key GET against a
read lane (`/golden-records`, `/metrics`) → 404.

## Impersonation and super-admin

- **Super admins get implicit `role: "admin"` on every org** on the BFF (`requireOrgAccess`,
  `viaOperator: true`). That is by design but is a trust concentration — test that a *non*-super
  session can never obtain `viaOperator`.
- **Impersonation** (`app/api/impersonation/route.ts`) has a 60-min TTL and *replaces* super-admin
  powers while active. Covered by `frontend/tests/functional/impersonation.spec.ts` — extend it to
  assert the role downgrade is enforced server-side (not just UI) and that both entering and leaving
  impersonation are audited.
- The BFF sends `x-acting-user` but `er-server` never reads it (confirmed). Lock that with a check
  that it stays unread upstream, so it can't silently become a spoofable identity header.

## How to prove / how to lock

| Claim | PoC (hermetic unless noted) | Test |
|---|---|---|
| Two orgs can share a catalog | create two orgs with the same `ER_LAKE_CATALOG`; the uniqueness read returns a dup | NEW `pg_tenant_isolation.rs` |
| Cross-org read leaks | org-A key GETs `/v1/orgs/{B}/golden-records` → expect 404, assert it | extend `pg_auth.rs` |
| A route forgot its guard | static walk of `build_routes` finds a handler with no `guard`/`operator_only` | NEW static gate (#13) |
| Impersonation downgrade bypassable | (live) act-as a lower role, attempt an admin action | `impersonation.spec.ts` |
