---
name: frontend-testing
description: Explains this repo's frontend test tiers and quality gates — which tier a change belongs in (unit, component, Playwright functional, the Docker-backed full-stack tier that is a release gate, not a PR gate), the `postgres.js` driver tier needing a real Postgres, the per-PR coverage floor, the axe accessibility gate, PGlite as the test database, the fixture orgs and the container-tag-must-match-dependency rule for baselines, plus the known false-green traps. Use when adding or fixing a frontend test, when a Playwright or vitest run fails, when choosing where new coverage belongs, when visual baselines change, or when a frontend CI job is red. For the BFF boundary and its authz gate see frontend-bff; for tokens, styling and interface copy see frontend-design-system; for dense data surfaces see frontend-data-ui; for the public marketing surface see landing-page; for CI job wiring and required checks see ci-and-supply-chain.
---

# Frontend testing

[references/frontend-testing.md](references/frontend-testing.md) is the living
standard; this skill is the working
summary. The frontend gate **blocks merges**: several of its jobs are required
checks on `main`, and because branch protection is repository configuration
rather than a file in the tree, no count of them here could stay true — the
ci-and-supply-chain skill's `required-checks.md` carries each job's status-check
context. A frontend change is not done until its tier passes.

## The test tiers, and how to choose

Each tier is `frontend/tests/<tier>/`, the full-stack one being `tests/e2e/`.
Directories and commands are generated into
[references/tiers-and-thresholds.md](references/tiers-and-thresholds.md), which
is the source of truth if this table ever disagrees with it.

| Tier | Command | In CI |
|---|---|---|
| unit | `pnpm test` | yes, per-PR |
| component | `pnpm test` | yes, per-PR |
| functional | `pnpm test:e2e` | yes, per-PR |
| driver | `pnpm test:driver` | yes, per-PR — with its own Postgres service |
| full-stack | `make frontend-e2e` | **no — release gate** |

Choosing:

- **Pure logic** — a formatter, a guard, a reducer, a BFF helper's branches →
  unit.
- **A component's rendered behaviour** in isolation → component.
- **A flow through real route handlers**, a session, authorization, or anything
  that needs the app actually running → functional. This is where most
  meaningful frontend coverage lives.
- **Something `postgres.js` itself has to get right** — a binary round trip, a
  bytea or type-encoding question → driver. It is the only tier that reaches a
  real PostgreSQL, and `vitest.driver.config.ts` pins `environment: "node"`
  because under jsdom a Node `Buffer` fails postgres v4's
  `instanceof Uint8Array` check, so the driver sends bytea as text and the
  server rejects it — a failure that reads as driver incompatibility and is not.
- **The whole stack against a real backend** → the full-stack tier. It is real
  and runnable, but it is **not on the PR path** — it needs Docker.

The full-stack tier runs against one Postgres, a local DuckLake, `er-server`
and the built Next app. It was dormant from the all-Rust cutover until W0.5b
revived it, so treat any older note calling it unavailable as stale. Reach for
it when a change spans BFF → er-server → engine; its sequence is
`make frontend-dev`, `make frontend-seed`, `pnpm build`, `make frontend-e2e`.

## The coverage floor

Enforced **per app-changing PR** by `pnpm check` (`vitest run --coverage`), scoped to
`src/lib/**`. The thresholds and the Playwright pin are generated:
[references/tiers-and-thresholds.md](references/tiers-and-thresholds.md), which
is also the canonical copy of why the floor sits where it does.

In short: `src/lib/query/**` is deliberately outside the unit tier's remit
because Playwright exercises those hooks, so the floor is an honest ceiling and
not a soft target. Raising it means writing unit tests for `src/lib`, **not**
re-scoping the measurement to flatter it.

Marketing-only diffs use `scripts/ci/marketing-paths.sh` to select lint,
formatting, types, build, marketing browser specs and landing-page axe coverage.
They skip app unit/coverage, API drift and the Postgres driver tier. Shared app
code, shell, styles, dependencies or test configuration restore the full gate.
The nightly `coverage` job only publishes the HTML artifact; the app PR gate
enforces the floor. See ci-and-supply-chain for classification and cache rules.

## The accessibility gate

`tests/functional/a11y.spec.ts` runs `@axe-core/playwright` with
`.withTags(["wcag2a", "wcag2aa"])` across the product's screens and asserts
**zero `serious` or `critical` violations**. Lesser impacts are not gated.

It skips mobile projects deliberately — one axe pass per screen, because the
desktop DOM is a superset of the mobile one. That premise holds only while a
screen's responsive behaviour is the grid-collapses-to-cards pattern; a screen
that renders *different markup* below `lg` breaks it and needs a mobile pass.

Adding a screen means adding it to this spec's `SCREENS` list, with a `ready`
predicate that waits for real content. **A screen not in that list is silently
unchecked** — the gate passes having examined nothing about it. Which routes are
currently missing is no longer a thing to remember: it is derived, in the
frontend-data-ui skill's `screens.md`, and regenerated by `make skills-check`.

The sweep logs in as a fixture user before each screen unless that entry sets
`anonymous: true`, which is what a route reachable without a session needs — the
landing page is the entry that uses it. Setting the flag is the whole job; the
login step is already conditional on it.

## PGlite is the test database

`src/lib/db/index.ts` branches on the URL: a `pglite:` URL dynamically imports
PGlite plus the drizzle PGlite adapter and migrator, instead of connecting to
Postgres.

So the functional tier runs with `ERWEB_DATABASE_URL=pglite://memory` and needs
no database process, and unit tests that touch the database set the same at
module top. The driver tier is the deliberate exception and the only frontend
job carrying a Postgres service: PGlite cannot answer a question about
`postgres.js`, because it never constructs that driver.

The important property: it is the **real drizzle schema and real migrations**,
not a mock. A query that works against PGlite works against Postgres, and a
migration error surfaces in tests.

## Playwright fixtures

Fixture users, orgs and the login helper live in `tests/functional/helpers.ts`:

- Users spanning the role lattice, with one shared password constant.
- `ORG` is the **read-only, pixel-compared** org — do not mutate it from a
  test, or you will break the visual baselines for every other spec.
- `triageOrg(projectName)`, `wizardOrg(projectName)`, `datasetsOrg(projectName)`
  and their siblings return a **per-project** mutable org, so the device
  projects cannot interfere with each other. Use one for anything that writes.
  The projects share ONE mock server, so a journey that creates, suspends or
  publishes against a shared org races the others — this is a rule, not a
  precaution: it is the defect that made the datasets manage journey fail only
  in the full tier and pass in isolation.
- `DS` is the dataset every journey works inside. Each fixture org carries one
  named `default`, mirroring the backend's convergence sweep; the
  `datasets-*` orgs carry a second so the switcher, the manage table and the
  org-home dispatcher have a real choice to make. A fixture org added to the
  mock must also be added to `src/lib/db/test-fixtures.ts`, or the BFF session
  is not a member and every request 404s.
- `login()` POSTs the **real** BFF login route via `page.request`, sharing the
  cookie jar — no auth stubbing, so the session path is covered by every test
  that logs in.

The backend is a mock er-server (`tests/mock-erserver/server.mjs`), which also
serves a scripted model endpoint for the assistant surface. A test needing new
backend data extends it, and the new shape has to be checked against the real
OpenAPI document — see the mock-divergence trap below.

Every spec runs once per device project (`playwright.config.ts`: one desktop,
two mobile). Visual baselines are **Linux-only** (`ignoreSnapshots` when the
platform is not Linux), so snapshot assertions simply do not run on a Mac.

## The rule that causes mystery pixel diffs

**The Playwright container tag must equal the `@playwright/test` dependency
version.** The committed baselines were rendered inside that image.

A dependency bump without an image bump (or vice versa) produces pixel diffs
that look exactly like UI regressions and are not. Both values are shown in the
generated reference so a divergence is visible as a diff there.

Regenerate baselines with the repo's script rather than by hand, so they are
rendered in the same image CI uses.

## Known false-green traps

These have all bitten this repo, and they are why a gate is not trusted until it
has been observed **failing** — break a new one on purpose before relying on it.

- **A piped exit code, which bites by hand rather than in CI.** A pipeline
  reports its last command's status, so `pnpm test:e2e | tail` reads green over
  a failing suite — put the gate last in a compound invocation. No gate step in
  `frontend.yml` pipes, and that is the only reason it is safe there: its `run:`
  steps take GitHub's default `bash -e {0}`, which carries **no** `pipefail`
  (only an explicit `shell: bash` or `set -o pipefail` adds it, as `detect`'s
  pass-key step does). A step that must pipe has to opt in.
- **Semgrep's zero findings on a parse error.** A ruleset that fails to parse
  reports no findings, which is indistinguishable from "clean".
- **A mock that diverges from the real router.** A mock backend answering a
  shape the real service does not is a green test over a broken integration.
- **A locally-green `@visual` test.** Baselines are Linux-only, so
  `toHaveScreenshot` is a NO-OP on a Mac: the test passes having asserted
  nothing, and a new baseline is never written. Verify visual work in the
  pinned container — `scripts/frontend-dev/update-baselines.sh` to regenerate, or the same
  image running `pnpm test:e2e --grep @visual` WITHOUT `--update-snapshots`
  to compare the way CI does.
- **A green `make frontend`.** The local gate runs `pnpm check` and the
  Playwright functional tier; it does not run the driver tier, which is a
  required check. `tests/driver/**` sits outside `vitest.config.ts`'s `include`
  precisely so the PR gate cannot depend on a database being up — so a clean
  local run cannot speak for that context.
- **A required check that never reports.** A workflow-level `paths:` filter
  leaves a required check pending forever and blocks every PR — which is why
  `frontend.yml` has no `paths:` filter and uses a `detect` job instead. See the
  ci-and-supply-chain skill.
