# Screens and the accessibility gate

## Contents

- Why this file is generated
- What legitimately sits outside the sweep
- Screens swept by the axe gate
- Routes NOT in the axe gate

(A table of contents is only required past 100 lines, and the generated part of
this file is shorter than that today. It is here because the generated part
grows with every route added, and crossing the threshold would otherwise fail
the gate for a reason unrelated to whatever change added the route.)

## Why this file is generated

Generated from source by `scripts/skills-check.sh regen` — the route list is
read out of `frontend/src/app/**/page.tsx` and the gated list out of the
`SCREENS` array in `frontend/tests/functional/a11y.spec.ts`, so the
**unchecked** list below is derived, not maintained. Do not hand-edit below the
marker.

This is the same move `diagnostics.md` makes for documented-but-unimplemented
E0xx codes, and for the same reason: `frontend-testing` has long warned in prose
that "adding a screen means adding it to this spec's list; a new screen is
otherwise silently unchecked", and a prose warning is exactly the thing that
gets skipped. Deriving the gap means nobody has to remember.

## What legitimately sits outside the sweep

**A route appearing under "not in the axe gate" is not automatically a defect.**
Three categories legitimately sit outside it:

- **Dynamic-segment routes** (`[entityId]`, `[jobId]`, `[token]`) need a fixture
  id to visit, which the gate's fixture org may not provide. Worth adding when a
  stable fixture exists.
- **Routes behind a flow** (a wizard step, a post-invite screen) need the flow
  driven first, so they belong in a functional spec that already does that
  rather than in the flat sweep.
- **Redirect-only routes** render nothing to audit.

Everything else on the list is a screen a user can reach whose accessibility
nothing checks. Add it: an entry needs a `path`, a `name`, and a `ready`
predicate that waits for real content, which means the screen needs a stable
`data-testid`.

Note the gate's shape when adding one. The sweep logs in as a fixture user
before each screen **unless the entry sets `anonymous: true`**, which is how the
landing page is swept — so an unauthenticated route needs that flag and nothing
else. The harness change this used to require has already been made; the login
step is conditional today. It is also desktop-only, on the stated grounds that
the desktop DOM is a superset of the mobile one — true while the responsive
pattern is the grid-collapses-to-cards one described in `dense-surfaces.md`, and
false the moment a screen renders different markup below `lg`.

<!-- BEGIN GENERATED: screens-coverage -->
### Screens swept by the axe gate

- `/`
- `/[org]/[ds]/assistant`
- `/[org]/[ds]/campaigns`
- `/[org]/[ds]/dashboard`
- `/[org]/[ds]/duplicates`
- `/[org]/[ds]/insights`
- `/[org]/[ds]/labeling`
- `/[org]/[ds]/profile`
- `/[org]/[ds]/records`
- `/[org]/[ds]/reviews`
- `/[org]/[ds]/runs`
- `/[org]/[ds]/runs/schedules`
- `/[org]/[ds]/settings/config`
- `/[org]/[ds]/settings/models`
- `/[org]/[ds]/sources`
- `/[org]/[ds]/sources/new`
- `/[org]/settings/ai`
- `/[org]/settings/audit`
- `/[org]/settings/members`
- `/[org]/settings/profile`
- `/[org]/settings/webhooks`
- `/admin/audit`
- `/admin/queue`
- `/admin/tenants`
- `/admin/tenants/[org]`
- `/admin/tenants/new`
- `/datasets-dev/datasets`
- `/datasets-dev/datasets/new`
- `/docs`
- `/login`
- `/reset`

### Routes NOT in the axe gate

Each of these renders without any accessibility check. See the prose above
for the three categories that legitimately sit outside the sweep.

- `/[org]`
- `/[org]/[ds]/records/[entityId]`
- `/[org]/[ds]/runs/[jobId]`
- `/[org]/[ds]/settings`
- `/[org]/datasets`
- `/[org]/datasets/new`
- `/[org]/settings`
- `/admin`
- `/app`
- `/invite/[token]`
- `/reset/[token]`
- `/widget/[token]`
<!-- END GENERATED: screens-coverage -->
