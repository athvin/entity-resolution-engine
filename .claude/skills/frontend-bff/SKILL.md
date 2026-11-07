---
name: frontend-bff
description: Explains the Next.js backend-for-frontend boundary — that the browser never reaches the Rust control plane, the server-only upstream client versus the browser's same-origin client, why authorization sits in the handler, not the proxy, and the five gates (route authz, the one upstream path, the semgrep no-NEXT_PUBLIC policy, `server-only`, and a direct DuckDB/S3/Postgres ban). Use when adding or reviewing a route handler under `src/app/api`, wiring a component to data, handling sessions or impersonation, adding an environment variable, or touching anything that crosses the browser/server line. For SSRF or tenant-isolation review see security; for the er-server side see er-server-control-plane; for tokens and styling see frontend-design-system; for tables and charts see frontend-data-ui; for the marketing surface see landing-page; for the generated API types see api-contract; for test tiers see frontend-testing; for the trace context and action header see observability.
---

# The BFF boundary

The frontend is a standalone Next.js app in `frontend/`, with its own gate. Its
defining property:

> **The browser never talks to the Rust control plane.** Every request goes to
> a Next.js route handler, which authorizes it and then calls upstream with a
> privileged credential.

That is why the boundary has five independent enforcement mechanisms rather
than a convention.

## Two clients, one set of types

| Module | Runs | Talks to | Credential |
|---|---|---|---|
| `src/lib/bff/upstream.ts` | **server only** | the Rust control plane | a parameter, not resolved there |
| `src/lib/api/client.ts` | browser | the BFF, **same-origin only** | the session cookie |

`upstreamFetch` is the one place the BFF builds a request to er-server. The
credential is a parameter because the three stories genuinely differ: the tenant
proxy selects a stored org key by role, the operator console uses the operator
token, and the widget's signed token *is* the credential. Error mapping stays
with the caller for the same reason — the relay returns `false`, the widget
returns a 404 that leaks nothing, the proxy raises a typed `BffFailure`.

The invariant is **only `upstream.ts` may name `ERSERVER_BASE_URL`** — the one
thing a new upstream path cannot avoid doing, which is what makes it gateable
(gate 5 below). It replaced five hand-built call sites that had already drifted,
so anything that must ride *every* upstream call — `traceparent`, the action
header — is now addable in one place rather than in five hopefully.

There is **no** `src/lib/api/erserver.ts`. It was the dead sixth path, deleted
with that refactor; the data-boundary rule still bans the import by name so it
cannot quietly come back.

The browser client is typed from the same generated `schema.d.ts` (see the
api-contract skill), so the types are shared while the transport is not. It
throws `BffRequestError`; there is also an Arrow fetch path for the columnar
read surface.

A browser module importing a server module is the mistake the whole boundary
exists to prevent. `server-only` is the mechanism that makes it a build error
rather than a review finding.

## Authorization happens in the handler, not the proxy

`src/proxy.ts` is Next 16's renamed middleware. It is a **cookie-presence gate
only** — it runs where there is no database access, so it *cannot* tell whether
a session may see a particular org.

So authorization is the route handler's job, in its own file:

| Route family | Must call |
|---|---|
| `src/app/api/orgs/**` | `requireOrgAccess(org, minimum)` |
| `src/app/api/orgs/[org]/datasets/[ds]/**` | `requireDatasetAccess(org, ds, minimum)` |
| `src/app/api/admin/**` | `requireSuperAdmin()` |
| `src/app/api/telemetry/**` | `requireIdentity()` |
| `src/app/api/impersonation/**` | `requireSuperAdmin()` to start, `requireIdentity()` to end |

**A handler that forgets is reachable by any authenticated user, and the BFF
attaches a privileged credential upstream on its behalf.** That is the whole
risk, and it is why the check below exists.

The dataset row is the one the gate cannot catch for you. `requireDatasetAccess`
is `requireOrgAccess` **plus** the dataset slug check, and the authz gate accepts
*either* name under `orgs/**` — so a dataset-scoped handler calling the plain
`requireOrgAccess` passes every gate while skipping the slug check, and
`datasetPath` is only safe to interpolate because both segments were checked when
the access value was minted.

The two either-guard rows are deliberate rather than lax. Ending an
impersonation needs only that you are the person inside it: demanding super
admin there would strand a session whose operator rights were revoked
mid-impersonation.

`PUBLIC_PATHS` in the proxy are the routes reachable without a session — login,
the invite and reset flows, the CSP report sink. Note the reasoning for the
invite/reset entries: the token in the URL *is* the credential, so redirecting
to `/login` would strand exactly the person the link was sent to. Do not add to
this list without the same kind of justification.

Security headers (including the CSP report-only policy) live in
`next.config.ts`'s `headers()`, deliberately **not** in the proxy.

## The five gates

**1. `scripts/check-bff-authz.mjs`** (`pnpm authz:check`, inside `pnpm check`).
A file-level textual gate: every `route.ts` in each route family above must
contain that family's guard, counting either name where the table lists two.

Two false-green shapes it is built against. It **exits 2 if it examines zero
route files** — rename detection; a gate that silently finds nothing to check is
worse than no gate, because it reports success. And an **unregistered top-level
directory under `src/app/api/` is a failure**, because the gate once scanned
only `orgs` and `admin` and reported success having never looked at a new
directory. Adding one is therefore a deliberate entry in `RULES`, or in
`UNAUTHENTICATED_DIRS` with a stated reason for serving no session at all. Its
exported functions are tested in both directions.

**2. `.semgrep.yml`**, of which the policy rule matters most:

| Rule | Forbids |
|---|---|
| `no-next-public-env` | **any** `NEXT_PUBLIC_*` variable, by design |
| `no-html-injection-sink` | raw HTML injection sinks |
| `no-dynamic-code-eval` | dynamic code evaluation |

`NEXT_PUBLIC_*` is banned outright, not audited. A `NEXT_PUBLIC_` variable is
inlined into the client bundle, so the prefix is an invitation to leak
configuration that looked harmless. If the browser needs a value, serve it from
a route handler.

Note semgrep is **not a required check** — its registry packs need network, and
a flaky required check is worse than none. It still runs; read it.

**3. `server-only`.** Server modules import the `server-only` package, which
makes a browser import a build error. Vitest aliases it to a stub so unit tests
can import those modules — see the frontend-testing skill.

**4. `eslint-rules/data-boundary.mjs`** (inside `pnpm lint`). The frontend
reaches data only through er-server, and this rule makes that a lint error
rather than a review finding:

| Where | Forbidden |
|---|---|
| anywhere in `src/` | DuckDB / DuckLake / MotherDuck, Parquet readers, the AWS SDK, any other Postgres client (`pg`, Neon, Vercel) |
| outside `src/lib/db/` | constructing the Postgres connection: `postgres`, PGlite, any `drizzle-orm/<driver>` subpath |
| a `"use client"` module | importing `drizzle-orm`, `@/lib/db`, `@/lib/bff/*` (except `errors` — so this covers `upstream.ts`), `@/lib/api/erserver` (the deleted path, still banned by name) or `@/lib/env`; calling `fetch`/`EventSource`/`WebSocket`/`sendBeacon` with an absolute URL |

Type-only imports pass, because they are erased. The lake rule has no
exception, even under `src/lib/db/`: a tenant's lake is isolated by catalog
separation that er-server enforces, and a DuckDB-WASM import in a component
would build, run and bypass it. If a screen needs lake data, er-server serves
it — the Arrow read surface exists for that. The rule is tested in both
directions by `tests/unit/data-boundary-rule.test.ts`.

A module both sides need, such as a request cap the UI displays and the handler
enforces, belongs in `src/lib/domain/`, not `src/lib/bff/`.

**5. `scripts/check-bff-upstream.mjs`** (inside `pnpm check`). Enforces the one
upstream path: only `src/lib/bff/upstream.ts` may name `ERSERVER_BASE_URL`,
which is the thing a new path cannot avoid doing — a far better tripwire than
looking for `fetch(`, of which the app has many legitimate uses. The allowlist
is keyed by path and each entry carries a stated reason.

Deliberately a script rather than a semgrep rule, and the reason is on record:
tried there as a `pattern-not`, semgrep rejected the pattern as invalid
TypeScript **while still reporting the rule as run with zero findings** — a
silent false green over the single most important property in the app.

## Layout

| Path | Holds |
|---|---|
| `src/app/(auth)/` | login, invite, reset |
| `src/app/(tenant)/[org]/` | the tenant product surface |
| `src/app/(admin)/admin/` | the operator console |
| `src/app/api/` | the BFF route handlers |
| `src/lib/api/` | the browser client plus the generated schema |
| `src/lib/{auth,bff,db,domain,query}/` | session, BFF helpers and `upstream.ts`, drizzle, domain logic, TanStack Query hooks |
| `src/lib/entities/{bindings,readers}/` | **generated** by `er-entity gen` — never hand-edit |

TypeScript is strict. The flag that changes how you write code here is
`exactOptionalPropertyTypes`: an explicit `undefined` is not assignable to a
plain optional property, so a forwarded optional field is declared
`?: T | undefined` rather than `?: T`. `noUncheckedIndexedAccess` and
`verbatimModuleSyntax` are on as well, and `@/*` maps to `./src/*`.

Handler conventions, session and impersonation mechanics, and the data-access
rules: [references/handlers-and-data.md](references/handlers-and-data.md).

## Common workflows

**Adding a tenant API route.** Create `src/app/api/orgs/[org]/…/route.ts`, wrap
the body in `respond(async () => …)` so failures become the BFF envelope, call
`requireOrgAccess` — or `requireDatasetAccess` under `datasets/[ds]` — **in that
file** before forwarding, then go upstream through `upstreamFetch`. The authz
gate will fail the build if you forget the guard entirely, but it cannot tell you
that you picked the org-level one in a dataset route, so write it deliberately
rather than relying on the gate to remind you.

**The browser needs a config value.** Do not add `NEXT_PUBLIC_*`. Serve it from
a route handler.

**A component needs data.** Use a TanStack Query hook from `src/lib/query/`
against the browser client. Note that `query/**` is deliberately outside the
unit-coverage scope — it is Playwright's territory.

**Adding a status indicator.** No colour — both themes are grayscale and ESLint
rejects a palette utility even inside a `cva` variant. This is the rule most
often broken while doing BFF work, when a handler's new state needs rendering
somewhere; the channel to use instead, and the one `--destructive` exception,
are the frontend-design-system skill's territory.
