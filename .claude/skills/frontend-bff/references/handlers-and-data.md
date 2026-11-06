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
| `requireDatasetAccess(org, ds, minimum)` | `src/lib/bff/proxy.ts` | `EffectiveDatasetAccess` — the above plus a slug-checked `dataset` |
| `requireSuperAdmin()` | `src/lib/bff/admin.ts` | the `Identity` |

`requireDatasetAccess` is `requireOrgAccess` followed by `requireDatasetSlug`,
and it is what makes `datasetPath(access, suffix)` safe to interpolate: both
segments were checked when the access value was minted, so the path cannot
traverse. Use it for every handler under `datasets/[ds]` — the authz gate accepts
the org-level helper there too, so this is the one guard choice no gate makes
for you.

`viaOperator` records that the caller is acting through the operator token — a
super admin with no membership in that org. Keep it in mind when writing audit
entries: "who did this" and "under what authority" are different questions.

Role comparison goes through `ROLE_RANK`; do not compare role strings directly.
**It ranks only the three membership roles** — `viewer < steward < admin` — and
`OrgRole` has no fourth member, so `operator` is not a minimum you can ask for.
The backend's enum does carry `Operator`, and that asymmetry is the point:
operator authority never arrives as a rank at the BFF, it arrives out of band
through `requireSuperAdmin()` and is recorded on the access value as
`viaOperator`.

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
import is a build error.

**If a module holds a credential, reads env, or touches the database, add
`import "server-only"` as the first line.** It is cheap and it converts a whole
class of leak into a compile error. That covers `src/lib/env.ts`, `upstream.ts`
and most of `src/lib/bff/`, `src/lib/db/`, `src/lib/auth/session.ts` and
`password.ts`, and every AI, adjudication and usage helper that reads a stored
key. The set grows with the app, so no enumeration of it here would stay true —
learn the rule instead.

What does not grow is the list of **deliberate exceptions**, and each is a
decision someone could undo by accident:

| Shared on purpose | Why |
|---|---|
| `src/lib/bff/errors.ts` | the error envelope the browser client shares |
| `src/lib/auth/redirect.ts` | the login page is a client component |
| `src/lib/db/schema.ts` | `drizzle-kit generate` loads it in plain Node, where `server-only` throws |

A fourth shape sits outside the set without being an exception at all: a module
that takes what it needs as a **parameter** instead of reading it.
`src/lib/bff/readiness.ts` is the worked example — the upstream base URL is
passed in rather than read, deliberately, "so the environment branch is reachable
in a test" (`env()` memoizes its first successful parse for the life of the
process), and `/readyz` is anonymous so no credential is involved. It holds
nothing, so the rule never fires on it and the missing import is correct.

So the tell is the rule's **trigger**, not the directory. A module that does hold
a credential, read env or touch the database, and carries neither the import nor
a reason, is missing it — the import is additive and costs nothing, so that
asymmetry is worth acting on. A module under `src/lib/bff/` that holds none of
those things is not evidence of anything.

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

Only `src/lib/db/` constructs that connection; the data-boundary lint rule
rejects a `postgres`, PGlite or `drizzle-orm/<driver>` import anywhere else.

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
         (a NEW top-level api/ directory also needs a RULES or
          UNAUTHENTICATED_DIRS entry, or the authz gate fails on it)
- [ ] 2. Wrap the body in respond(async () => …) so failures become the envelope
- [ ] 3. Inside that file, before forwarding: requireOrgAccess(org, minimum) —
         or requireDatasetAccess(org, ds, minimum) under datasets/[ds] —
         or requireSuperAdmin()
- [ ] 4. Go upstream via upstreamFetch from src/lib/bff/upstream.ts
- [ ] 5. Map failures to a BffFailure with a useful status and code
- [ ] 6. No NEXT_PUBLIC_* anywhere; add `import "server-only"` to new server modules
- [ ] 7. pnpm check — authz:check exits 2 if it examined no routes, and
         check-bff-upstream.mjs rejects a second path to ERSERVER_BASE_URL
- [ ] 8. Add a unit test for the handler's authz branches
```

Step 3 says *in that file*. The authz gate is textual and per-file precisely
because a helper that authorizes "somewhere upstream" is unverifiable by
inspection — and the consequence of a miss is an unauthenticated path to a
privileged upstream credential.

It also says *which* guard. Under `datasets/[ds]` the gate counts either name, so
picking the org-level helper there is the one authorization mistake in this
checklist that goes green.
