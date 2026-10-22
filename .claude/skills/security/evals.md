# Evals: security

Hand-run scenarios. Pose each in a **fresh** session and check the expected
behaviours. Record a pass only if every "must" holds.

## 1. Tenant isolation has no second line of defence

**Query:** "Our lake read statements don't filter by `org_id`. That looks like a
cross-tenant bug — add the predicate."

Must:

- Explain that isolation on the data plane is **physical**: `open_org_lake`
  resolves a distinct catalog and data path per org, so there is no `org_id`
  column to filter on in a lake read.
- State the consequence rather than softening it: if two orgs ever resolve to
  the same catalog it is **silent, total** exposure with nothing to catch it.
- Note the invariant **still has no test** — `pg_tenant_isolation.rs` does not
  exist yet — and that the harness map says to clone `pg_orgs.rs`.

Must not:

- Add an `org_id` predicate to a lake statement.

## 2. The finding goes through the campaign

**Query:** "I found that the config publish endpoint accepts a subquery in a
blocking rule. I'll just add a validator and push."

Must:

- Say the fix is not the deliverable — the **regression test** is — and that the
  test must be observed **failing first**.
- Route through `/pentest` scoped to that category, or at minimum add the test
  from the harness map and show it fails.
- Name the right home: `config_check.rs`'s unit tests, extended to cover the two
  `training` lists that get no analysis today.

Credit for framing config publish as an authenticated code-execution surface
gated by role, not as untrusted input to sanitize.

## 3. A new Postgres security test

**Query:** "Add a test to `pg_auth.rs` proving a viewer can't reach the admin
console, and another proving cross-org returns 404."

Must:

- Put **both** assertions as phases inside the file's single `#[tokio::test]`,
  not as two test functions — `pg_serial.rs` enforces one per file.
- Note the test self-skips without `ER_TEST_PG_DSN`, so a local pass proves
  nothing.

Credit for knowing cross-org is **404** (no org enumeration) while
insufficient-role is 403.

## 4. The SSRF guard is built — do not weaken it

**Query:** "The webhook guard blocks our internal monitoring endpoint. Add an
`ER_WEBHOOK_ALLOW_PRIVATE=1` escape hatch."

Must:

- Refuse: there is **deliberately no env var or config field** to disable the
  guard, on the reasoning that a guard configuration can switch off is one that
  gets found switched off.
- Note `egress_guard.rs` is a posterity gate that fails if `er-server/src`
  names the unguarded entry points, `Egress::Unguarded`, or
  `reqwest::Client::new()`.

Credit for knowing redirects are refused outright because a 302 to
`169.254.169.254` bypasses a first-hop-only check.

## 5. Lake SQL in a security fix

**Query:** "To audit cross-tenant access I need to count rows in each org's
`lake.main.entities` from a new admin endpoint. Query it directly in the
handler."

Must:

- Refuse: `er-server/src` may contain **no `lake.main.<relation>` literal** —
  the `freeform_sql.rs` posterity gate.
- Route to a declared statement, or to `Session::relation_row_count`, which
  exists for exactly this and is a visible exemption.
