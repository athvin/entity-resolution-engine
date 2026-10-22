# Evals: frontend-bff

Hand-run scenarios. Pose each in a **fresh** session and check the expected
behaviours. Record a pass only if every "must" holds.

## 1. Authorization in the handler

**Query:** "Add `GET /api/orgs/[org]/exports` that proxies the backend's export
endpoint. The proxy already checks the session cookie, so no auth needed in the
handler, right?"

Must:

- Correct the premise: `src/proxy.ts` is a **cookie-presence gate only** — it
  runs without database access and cannot tell whether the session may see this
  org.
- Call `requireOrgAccess(org, minimum)` **in the route file itself**.
- Name the consequence of forgetting: the route is reachable by any
  authenticated user, and the BFF attaches a privileged credential upstream.

Credit for mentioning `check-bff-authz.mjs` and that it exits 2 if it examines
zero route files.

## 2. A client-visible config value

**Query:** "The dashboard needs the API base URL at runtime. Add
`NEXT_PUBLIC_API_URL`."

Must:

- Refuse: `NEXT_PUBLIC_*` is **banned outright** by the `no-next-public-env`
  semgrep rule, because the prefix inlines the value into the client bundle.
- Offer the alternative: serve the value from a route handler.

Must not:

- Treat it as a reviewable exception or suggest an allowlist entry.

## 3. Importing the upstream client in a component

**Query:** "My server component needs golden records. Can I import
`src/lib/api/erserver.ts` directly, and can the client component that renders
it do the same?"

Must:

- Distinguish: a **server** component may; a **client** component may not.
- Say `erserver.ts` carries `server-only`, so a browser import is a build
  error, and the browser path is `src/lib/api/client.ts` (same-origin, to the
  BFF).

Credit for noting both are typed from the same generated `schema.d.ts`.

## 4. Impersonation scope

**Query:** "While a super admin is impersonating org `acme`, should they still
be able to read org `globex` since they're a super admin?"

Must:

- Say no: impersonation **replaces** their powers entirely, and every other org
  answers **404** (not 403), so the session cannot even enumerate tenants.
- Frame it as the premise of "view as tenant" — seeing exactly what the tenant
  sees.

## 5. A request that straddles the boundary

This scenario tests **routing**, not knowledge. The styling half belongs to
frontend-design-system and the rendering half to frontend-data-ui; a pass means
the BFF half is answered authoritatively here and the other halves are deferred
rather than guessed at.

**Query:** "Expose the per-run spill metrics from the backend and show them on
the run detail page as a green/amber/red health bar."

Must:

- Own the BFF half completely: a new `route.ts` under
  `src/app/api/orgs/[org]/…` calling `requireOrgAccess` **in that file**, then
  upstream through `erserver.ts`; the component reading it through a
  `src/lib/query/` hook against the browser client.
- Refuse the green/amber/red treatment, and hand the question of what replaces
  it to the design-system skill rather than inventing a scheme.
- Not propose a charting library for the bar — that is frontend-data-ui's rule,
  and the answer there is hand-rolled DOM/CSS.

Credit for noting that `src/lib/query/**` is deliberately outside the unit
coverage scope because it is Playwright's territory, and for mentioning that the
new screen needs registering in the axe gate if the run detail page is not
already there.
