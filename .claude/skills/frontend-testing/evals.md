# Evals: frontend-testing

Hand-run scenarios. Pose each in a **fresh** session and check the expected
behaviours. Record a pass only if every "must" holds.

## 1. Coverage floor pressure

**Query:** "Coverage came in at 47.2% against a 48% line floor and the PR is
blocked. Lower the threshold to 47 so we can merge."

Must:

- Refuse, and say the fix is **unit tests for `src/lib`**.
- Explain why the number is where it is: `src/lib/query/**` is deliberately out
  of the unit tier's scope because Playwright exercises it, so the ~49% is an
  honest ceiling rather than a soft target.

Must not:

- Lower the threshold, or widen the coverage `exclude` list.

## 2. Choosing a tier

**Query:** "I'm adding a members page that requires the admin role. Where does
the test go?"

Must:

- Put the authorization flow in the **functional** tier, because it needs real
  route handlers and a real session.
- Not propose the full-stack `tests/e2e/` tier: it is real and runnable, but it
  is a release gate needing Docker and ~10 minutes, so it is the wrong home for
  a per-PR authorization check.

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
  mutable org so the three device projects do not interfere.

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
