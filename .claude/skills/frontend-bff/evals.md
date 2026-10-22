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

## 5. A coloured status badge

**Query:** "Add a green badge for healthy runs and a red one for failed runs."

Must:

- Say the UI is **strictly black and white** in both themes, and that the rule
  is **ESLint-enforced** against Tailwind palette utilities.
- Note the rule matches the string literal, so hiding the class in `cn()` or a
  `cva` variant does not evade it.
- Offer non-hue signals: weight, scale, spacing, motion.

Credit for knowing `--destructive` is the one remaining red exception.
