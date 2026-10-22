---
name: frontend-bff
description: Explains the Next.js backend-for-frontend boundary in this repo — that the browser never reaches the Rust control plane, the split between the server-only upstream client and the browser's same-origin client, where authorization actually happens and why the proxy cannot do it, the three gates that enforce the boundary (the authz script, the semgrep rules including the no-NEXT_PUBLIC policy, and `server-only`), and the monochrome design rule. Use when adding or reviewing a route handler under `src/app/api`, wiring a component to data, handling sessions or impersonation, adding an environment variable, or touching anything that crosses the browser/server line. For the generated API types see api-contract; for test tiers and gates see frontend-testing.
---

# The BFF boundary

The frontend is a standalone Next.js app in `frontend/`, with its own gate. Its
defining property:

> **The browser never talks to the Rust control plane.** Every request goes to
> a Next.js route handler, which authorizes it and then calls upstream with a
> privileged credential.

That is why the boundary has three independent enforcement mechanisms rather
than a convention.

## Two clients, one set of types

| Module | Runs | Talks to | Credential |
|---|---|---|---|
| `src/lib/api/erserver.ts` | **server only** | the Rust control plane | bearer, injected per request |
| `src/lib/api/client.ts` | browser | the BFF, **same-origin only** | the session cookie |

Both are typed from the same generated `schema.d.ts` (see the api-contract
skill), so the types are shared while the transport is not. The browser client
throws `BffRequestError`; there is also an Arrow fetch path for the columnar
read surface.

A browser module importing `erserver.ts` is the mistake the whole boundary
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
| `src/app/api/admin/**` | `requireSuperAdmin()` |

**A handler that forgets is reachable by any authenticated user, and the BFF
attaches a privileged credential upstream on its behalf.** That is the whole
risk, and it is why the check below exists.

`PUBLIC_PATHS` in the proxy are the routes reachable without a session — login,
the invite and reset flows, the CSP report sink. Note the reasoning for the
invite/reset entries: the token in the URL *is* the credential, so redirecting
to `/login` would strand exactly the person the link was sent to. Do not add to
this list without the same kind of justification.

Security headers (including the CSP report-only policy) live in
`next.config.ts`'s `headers()`, deliberately **not** in the proxy.

## The three gates

**1. `scripts/check-bff-authz.mjs`** (`pnpm authz:check`, inside `pnpm check`).
A file-level textual gate: every `route.ts` under `src/app/api/orgs` must
contain `requireOrgAccess`, every one under `src/app/api/admin` must contain
`requireSuperAdmin`.

It **exits 2 if it examines zero route files** — rename detection. A gate that
silently finds nothing to check is worse than no gate, because it reports
success. Its exported functions are tested in both directions.

**2. `.semgrep.yml`** — three rules, of which the policy one matters most:

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

## The UI is strictly black and white

Both themes are grayscale: no brand accent, no coloured chips or badges, no
palette. This is **ESLint-enforced**, not just documented: a
`no-restricted-syntax` rule rejects Tailwind palette utilities
(`bg-emerald-500`, `text-red-600`, …).

The rule matches the **string literal**, not the JSX attribute, deliberately —
so it also catches class strings built inside `cn()`, `cva()` variants, and
helpers. Signal state with weight, scale, spacing or motion instead of hue, and
use the semantic tokens in `globals.css` (all chroma-0 in both themes).

One exception exists: `--destructive` is still red.

## Layout

| Path | Holds |
|---|---|
| `src/app/(auth)/` | login, invite, reset |
| `src/app/(tenant)/[org]/` | the tenant product surface |
| `src/app/(admin)/admin/` | the operator console |
| `src/app/api/` | the BFF route handlers |
| `src/lib/api/` | the two clients plus the generated schema |
| `src/lib/{auth,bff,db,domain,query}/` | session, BFF helpers, drizzle, domain logic, TanStack Query hooks |
| `src/lib/entities/{bindings,readers}/` | **generated** by `er-entity gen` — never hand-edit |

TypeScript is strict, with `noUncheckedIndexedAccess`,
`exactOptionalPropertyTypes` and `verbatimModuleSyntax`. The `@/*` alias maps to
`./src/*`.

Handler conventions, session and impersonation mechanics, and the data-access
rules: [references/handlers-and-data.md](references/handlers-and-data.md).

## Common workflows

**Adding a tenant API route.** Create `src/app/api/orgs/[org]/…/route.ts`, call
`requireOrgAccess` **in that file**, then call upstream through `erserver.ts`.
The authz gate will fail the build if you forget, but write it deliberately
rather than relying on the gate to remind you.

**The browser needs a config value.** Do not add `NEXT_PUBLIC_*`. Serve it from
a route handler.

**A component needs data.** Use a TanStack Query hook from `src/lib/query/`
against the browser client. Note that `query/**` is deliberately outside the
unit-coverage scope — it is Playwright's territory.

**Adding a status indicator.** No colour. Use weight, scale, spacing or motion;
the ESLint rule will reject a palette utility even inside a `cva` variant.
