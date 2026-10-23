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
before every screen, so an unauthenticated route cannot be added without also
making the login step conditional. It is also desktop-only, on the stated
grounds that the desktop DOM is a superset of the mobile one — true while the
responsive pattern is the grid-collapses-to-cards one described in
`dense-surfaces.md`, and false the moment a screen renders different markup
below `lg`.

<!-- BEGIN GENERATED: screens-coverage -->
### Screens swept by the axe gate

- `/`
- `/[org]/assistant`
- `/[org]/campaigns`
- `/[org]/dashboard`
- `/[org]/duplicates`
- `/[org]/insights`
- `/[org]/records`
- `/[org]/reviews`
- `/[org]/runs`
- `/[org]/settings/config`
- `/[org]/settings/members`
- `/[org]/settings/models`
- `/[org]/settings/webhooks`
- `/[org]/sources`

### Routes NOT in the axe gate

Each of these renders without any accessibility check. See the prose above
for the three categories that legitimately sit outside the sweep.

- `/[org]/records/[entityId]`
- `/[org]/runs/[jobId]`
- `/[org]/runs/schedules`
- `/[org]/settings`
- `/[org]/settings/audit`
- `/[org]/settings/profile`
- `/[org]/sources/new`
- `/admin`
- `/admin/audit`
- `/admin/queue`
- `/admin/tenants`
- `/admin/tenants/[org]`
- `/admin/tenants/new`
- `/app`
- `/invite/[token]`
- `/login`
- `/reset`
- `/reset/[token]`
<!-- END GENERATED: screens-coverage -->
