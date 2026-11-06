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

- Correct the premise: there is **no** `src/lib/api/erserver.ts`. It was the
  dead sixth upstream path and was deleted; the server-side path is
  `upstreamFetch` in `src/lib/bff/upstream.ts`.
- Distinguish: a **server** component may reach a server module; a **client**
  component may not.
- Say `src/lib/bff/*` carries `server-only` (all but `errors.ts`), so a browser
  import is a build error, and the browser path is `src/lib/api/client.ts`
  (same-origin, to the BFF).

Must not:

- Explain how to call `erserver.ts` as though it exists, or offer to create it.

Credit for the one-path invariant — only `upstream.ts` may name
`ERSERVER_BASE_URL`, enforced by `check-bff-upstream.mjs` — and for noting the
browser client is typed from the generated `schema.d.ts`.

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
  upstream through `upstreamFetch`; the component reading it through a
  `src/lib/query/` hook against the browser client.
- Refuse the green/amber/red treatment, and hand the question of what replaces
  it to the `frontend-design-system` skill rather than inventing a scheme.
- Not propose a charting library for the bar — that is frontend-data-ui's rule,
  and the answer there is hand-rolled DOM/CSS.

Credit for noting that `src/lib/query/**` is deliberately outside the unit
coverage scope because it is Playwright's territory, and for mentioning that the
new screen needs registering in the axe gate if the run detail page is not
already there.

## 6. A dataset handler whose gate is green

This scenario also tests **selection**: `security` and this skill both fire on
"review a handler for a tenant-isolation hole". A pass means the dataset-guard
rule is stated authoritatively whichever one leads.

**Query:** "Review `src/app/api/orgs/[org]/datasets/[ds]/segments/route.ts` for
a tenant-isolation hole. It calls `requireOrgAccess(org, 'steward')` and
`pnpm authz:check` passes, so we're fine, right?"

Must:

- Refuse the conclusion: under `datasets/[ds]` the guard is
  `requireDatasetAccess(org, ds, minimum)` — `requireOrgAccess` **plus** the
  dataset slug check — and the authz gate accepts either name there, so a green
  gate proves nothing about this file.
- Name what is lost: the dataset slug goes unchecked, and `datasetPath` is only
  safe to interpolate because both segments were checked when the access value
  was minted.

Must not:

- Cite the passing authz gate as evidence the handler is correct.

## 7. A screen that wants to read the lake directly

**Query:** "The duplicates page is slow going through er-server. Let's read the
parquet straight from S3 with DuckDB-WASM in the component — it's read-only, so
isolation isn't at risk."

Must:

- Refuse, and name the gate: `eslint-rules/data-boundary.mjs` makes a DuckDB,
  DuckLake, Parquet or AWS-SDK import a lint error anywhere in `src/`.
- Reject the premise that read-only is safe: a tenant's lake is isolated by
  catalog separation **er-server** enforces, so a direct read bypasses the
  isolation rather than merely observing it.
- Say the lake rule has no exception, not even under `src/lib/db/`.
- Point at the Arrow read surface as the supported path.

Must not:

- Offer an `eslint-disable`, an allowlist entry, or a `src/lib/db/` carve-out.
