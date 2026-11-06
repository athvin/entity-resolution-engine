# Evals: frontend-testing

Hand-run scenarios. Pose each in a **fresh** session and check the expected
behaviours. Record a pass only if every "must" holds.

Scenario 6 is the selection test: it names no skill, so it measures whether this
one is reached at all and whether an adjacent skill wins instead. Pose it
without priming.

## 1. Coverage floor pressure

**Query:** "Coverage came in a hair under the line floor and the PR is blocked.
Lower the threshold so we can merge."

Must:

- Refuse, and say the fix is **unit tests for `src/lib`**.
- Explain why the floor is where it is: `src/lib/query/**` is deliberately out
  of the unit tier's scope because Playwright exercises it, so the floor is an
  honest ceiling rather than a soft target.

Must not:

- Lower the threshold, or widen the coverage `exclude` list.
- Quote a single percentage as "the floor" — there are four metrics, and the
  generated reference carries them.

## 2. Choosing a tier

**Query:** "I'm adding a members page that requires the admin role. Where does
the test go?"

Must:

- Put the authorization flow in the **functional** tier, because it needs real
  route handlers and a real session.
- Not propose the full-stack `tests/e2e/` tier: it is real and runnable, but it
  is a release gate needing Docker, so it is the wrong home for a per-PR
  authorization check.

Credit for also suggesting a unit test for the authz helper's branches, and for
noting `login()` POSTs the real BFF route rather than stubbing auth.

## 3. The mystery pixel diff

**Query:** "CI is failing with screenshot diffs on five specs, but I only
changed a backend file. The last commit bumped `@playwright/test`."

Must:

- Identify the cause: the **container tag must match the dependency version**,
  because the baselines were rendered inside that image.
- Say the fix is to bump the workflow image tag to match (or revert), not to
  regenerate baselines to make the diff go away.

Credit for noting that visual baselines are Linux-only, so this never
reproduces locally on a Mac.

## 4. Mutating the wrong fixture org

**Query:** "My new test needs to create a source. I'll use `ORG` since it's
already set up."

Must:

- Say `ORG` is **read-only and pixel-compared** — mutating it breaks visual
  baselines for other specs.
- Direct to `triageOrg(projectName)` (or `wizardOrg`), which gives a per-project
  mutable org so the device projects do not interfere.

## 5. Trusting a new gate

**Query:** "I added a check that fails the build when a route handler is missing
authorization. It passes on main, so we're good."

Must:

- Say a gate must be observed **failing** before it is trusted, and propose
  deliberately breaking it once.
- Name at least one concrete false-green trap from this repo: a piped exit code
  masking a failure, semgrep reporting zero findings on a parse error, a mock
  diverging from the real router, or a required check that never reports.

Credit for mentioning the rename-detection pattern — exiting non-zero when the
gate examined zero files.

## 6. A required check this skill has to claim

**Query:** "The `postgres.js driver contract` check is red on my PR and I only
touched `src/lib/db`. Where does that test even live, and why was `make
frontend` green?"

Must:

- Reach **frontend-testing** without being asked for it by name, and not hand
  the question to ci-and-supply-chain, which owns job wiring rather than tiers.
- Name the driver tier: `frontend/tests/driver/`, run by `pnpm test:driver`
  against a real PostgreSQL.
- Explain the green local gate: `tests/driver/**` is outside
  `vitest.config.ts`'s `include`, so neither `pnpm check` nor `make frontend`
  runs it, and a clean local run cannot speak for that context.

Must not:

- Claim PGlite covers it, or suggest pointing it at `pglite://memory`. The tier
  exists because `postgres.js` against a real server is the thing under test,
  and PGlite never constructs that driver.
- Describe the frontend tiers as four.

## 7. A screen the axe gate cannot see

**Query:** "I'm adding a public pricing page. Anything to do for the a11y gate?"

Must:

- Say the screen must be added to `SCREENS` in `tests/functional/a11y.spec.ts`,
  because a screen not in that list is silently unchecked — the gate passes
  having examined nothing about it.
- Say a route reachable without a session sets `anonymous: true` on its entry,
  **not** a change to the sweep's login step, which is already conditional on
  that flag.

Credit for noting the sweep skips mobile projects on purpose, and that a screen
rendering different markup below `lg` breaks that premise and needs its own pass.
