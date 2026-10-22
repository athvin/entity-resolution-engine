# Handlers, sessions, and data access

## Contents

- The authorization helpers
- Impersonation replaces powers, it does not add them
- Why identity is resolved before the org slug
- Failure is data
- The `server-only` set
- Drizzle and the control database
- Email goes through one relay
- Adding a handler: the checklist

## The authorization helpers

| Helper | Lives in | Returns |
|---|---|---|
| `requireOrgAccess(org, minimum)` | `src/lib/bff/proxy.ts` | `EffectiveOrgAccess` — identity, org, effective role, and `viaOperator` |
| `requireSuperAdmin()` | `src/lib/bff/admin.ts` | the `Identity` |

`viaOperator` records that the caller is acting through the operator token — a
super admin with no membership in that org. Keep it in mind when writing audit
entries: "who did this" and "under what authority" are different questions.

Role comparison goes through `ROLE_RANK`, matching the backend's
`viewer < steward < admin < operator` lattice. Do not compare role strings
directly.

## Impersonation replaces powers, it does not add them

When a super admin is impersonating ("view as tenant"), the impersonated role
**replaces** their own powers entirely:

- The impersonated role is enforced, not the super-admin role.
- **Every other org answers 404.** Not 403 — 404, so an impersonating session
  cannot even enumerate other tenants.
- The org's real stored key is used upstream.

The intent is literal: "view as tenant" means seeing exactly what that tenant
sees, with no residual elevation. A change that lets an impersonating session
peek outside its org breaks the feature's entire premise, so treat the 404
branch as load-bearing.

## Why identity is resolved before the org slug

`requireOrgAccess` calls `requireIdentity()` **before** validating the org
slug, with a comment explaining why: otherwise a malformed org name would
answer 404 and hide the fact that the session had expired.

That ordering is a usability-and-debuggability decision — a stale session
should say 401, not pretend the org does not exist. Preserve it if you refactor
the function.

Note the two status codes, which mirror the backend:

| Situation | Status |
|---|---|
| no / expired session | 401 |
| org not visible to this identity | **404** |
| org visible, role insufficient | 403 |

## Failure is data

The BFF treats expected failures as values rather than exceptions where a
caller can still do something useful. `BffFailure` carries a status, a code and
a message; `BffRequestError` is the browser-side counterpart.

The email relay is the clearest example: every caller already returns something
usable without email — an invite link the admin can copy, for instance — so a
mail failure must not take the request down with it.

When adding a handler, ask what the user should see if the upstream call fails,
and return that rather than letting a 500 escape.

## The `server-only` set

Modules importing `server-only` cannot be pulled into a client bundle; the
import is a build error. The set includes:

- `src/lib/env.ts` — the environment reader
- `src/lib/api/erserver.ts` — the upstream client with the bearer
- all of `src/lib/bff/*`
- `src/lib/auth/{session,password,redirect}.ts`
- `src/lib/db/*`
- `src/lib/assistant/tools.ts`

**If you write a module that holds a credential, reads env, or touches the
database, add `import "server-only"` as the first line.** It is cheap and it
converts a whole class of leak into a compile error.

Vitest aliases `server-only` to a stub (`tests/server-only-stub.ts`) so unit
tests can import these modules outside a React Server context. That alias is
test-only; it does not weaken the production guarantee.

## Drizzle and the control database

The BFF has its own Postgres schema (`erweb`) accessed through drizzle, separate
from the Rust control plane's `public` schema. Migrations are owned here:

```sh
pnpm db:generate   # drizzle-kit generate
pnpm db:migrate    # node scripts/db-migrate.mjs
```

In deployment the migrate script runs as a Helm pre-install/pre-upgrade hook
from the web image — see the infra-platform skill.

The database layer branches on the URL: a `pglite:` URL dynamically imports
PGlite and its drizzle adapter instead of connecting to Postgres, which is what
lets the functional tier and some unit tests run with no database process. See
the frontend-testing skill.

Do **not** reach into the Rust control plane's tables from drizzle. The two
schemas are separate on purpose; cross-reading them couples two deploy
lifecycles.

## Email goes through one relay

`src/lib/bff/relay.ts` is the BFF's only path to email, and it calls the
backend's operator relay rather than any mail server.

The platform keeps exactly one SMTP stack, one outbox and one retry policy. So
nothing in the frontend talks to a mail provider, and nothing should start —
adding a second path means two retry policies and two places a message can be
lost.

## Adding a handler: the checklist

```
- [ ] 1. File under src/app/api/orgs/[org]/… or src/app/api/admin/…
- [ ] 2. First statement: requireOrgAccess(org, minimum) or requireSuperAdmin()
- [ ] 3. Call upstream via src/lib/api/erserver.ts (typed from schema.d.ts)
- [ ] 4. Map failures to a BffFailure with a useful status and code
- [ ] 5. No NEXT_PUBLIC_* anywhere; add `import "server-only"` to new server modules
- [ ] 6. pnpm authz:check (or pnpm check) — it exits 2 if it examined no routes
- [ ] 7. Add a unit test for the handler's authz branches
```

Step 2 says *in that file*. The authz gate is textual and per-file precisely
because a helper that authorizes "somewhere upstream" is unverifiable by
inspection — and the consequence of a miss is an unauthenticated path to a
privileged upstream credential.
