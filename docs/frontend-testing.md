# Frontend quality & testing (living standard)

What gates the frontend, what belongs in which test tier, and the rules that keep
the bar from drifting. The companion to [frontend-design.md](frontend-design.md),
which holds the product design — if this file and that one disagree about intent,
**the design doc wins and this one is stale.**

Written 2026-10-20, alongside the program in PRs #187–#201. Engineering-level
setup (how to run things, the dev substrate) stays in
[frontend/README.md](../frontend/README.md); this is the *why* and the
*definition of done*.

---

## Scope decisions (locked 2026-10-20)

1. **The frontend gate blocks merges.** `lint + types + unit`, `playwright
   functional tier` and `pnpm audit + licenses` are **required status checks** on
   `main`. Before this, branch protection required only `rust.yml`'s four
   contexts, so the frontend CI ran and was then ignored at merge time.
   (`rust.yml` now contributes five: `entities` was split out of
   `fmt + clippy + docs` on 2026-10-22, when that job was four serial
   ~10-minute steps and 42m27 on a cache miss. The new context was added to
   protection only after it had reported green on `main`.)
2. **No `paths:` filter on `frontend.yml`.** A workflow a path filter skips never
   reports, so a required check would sit pending forever on a backend-only PR.
   A `detect` job and an `if:` per job is the mechanism — mirroring what
   `rust.yml` already does for its `.claude/**` skip. *Verified:* a backend-only
   PR reports those three contexts as `conclusion=skipped` (completed, not
   pending), which branch protection accepts.

   **`detect` filters pushes to main too**, which is where this diverges from
   `rust.yml` — that workflow always runs on main because it *is* the gate
   protecting it. This one's main run only re-tests what the just-merged PR
   already tested (a squash commit and the PR head are near-identical trees), so
   re-running the Playwright container for a Rust-only merge cost ~5 minutes and
   proved nothing. It fails *safe*: an all-zeroes `event.before` (a branch's
   first push) or an unresolvable one (after a force-push) runs the gate rather
   than skipping it. Nightly and manual runs always run. All eight event paths
   are exercised against real commits before changing this job.
3. **Coverage is enforced per-PR, not nightly.** `pnpm check` runs
   `vitest run --coverage`; the floor lives in `vitest.config.ts`. This
   deliberately diverges from the Rust side (`rust.yml` runs llvm-cov nightly)
   because v8 instrumentation of this tier costs under a second — there is no
   latency argument for deferring it.
4. **The floor is a non-regression backstop, not the quality bar.** Ratchet it up
   as coverage grows; never down. The real gate is the behavioral suites.
5. **`semgrep` is advisory, not required.** Its registry packs are fetched over
   the network, and a flaky required check is worse than no required check. The
   local ruleset (`frontend/.semgrep.yml`) needs no network and is the half that
   cannot flake.
6. **Every gate must be observed failing before it is trusted.** See
   [Proving a gate works](#proving-a-gate-works) — this is the rule that caught a
   semgrep rule which reported "0 findings" while its pattern failed to parse.

---

## The four tiers

| Tier | Command | Runs in CI | What belongs here |
|---|---|---|---|
| **unit** — `tests/unit/` | `pnpm test` | yes, per-PR | Pure logic and anything with a decision matrix. Authorization, domain vocabularies, parsers, error envelopes. DB-backed cases run against PGlite. |
| **component** — `tests/component/` | yes, per-PR | yes, per-PR | Branchy *render* logic: a component whose output derives from data (bar widths, chips, diffs). **Not** screens. |
| **functional** — `tests/functional/` | `pnpm test:e2e` | yes, per-PR | Every screen and flow against the mock erserver, on three device projects, plus the visual baselines and the axe a11y sweep. This is the bulk of the coverage. |
| **full-stack** — `tests/e2e/` | `make frontend-e2e` | **no — release gate** | The real journey against a seeded dev stack (Docker, ~10 min). See [The full-stack tier](#the-full-stack-tier). |

**Which tier does a change need?** If it is logic with branches, a unit test. If
it is a component whose markup depends on data, a component test. If it is a
screen, a flow, or anything a user would notice, the functional tier — that is
where Playwright already drives real code against a real BFF.

Do not reach for a component test to cover something the functional tier already
drives. Duplicating a screen in jsdom is slower to run, weaker evidence, and two
places to update.

**The one exception, and why it is one.** `tests/component/structured-data.test.tsx`
asserts on what React's *server* renderer puts on the wire for a `<script>`
text child — that quotes survive unescaped so JSON-LD parses, and that
`</script` is encoded so the payload cannot close its own tag. Playwright
cannot see either property (the browser has already parsed the markup by the
time a test looks), and both are React implementation details a major upgrade
could change, with a failure mode that is invisible in the rendered page and
only breaks crawlers. That is the shape that earns a component test: a wire-format
property, not a screen.

### The axe sweep, and the two things that make a screen invisible to it

`tests/functional/a11y.spec.ts` holds a hardcoded `SCREENS` array. **A screen
not in that array is silently unchecked** — the gate passes having examined
nothing about it. Which routes are currently missing is derived rather than
remembered: `make skills-check` regenerates the list in the `frontend-data-ui`
skill's `references/screens.md` from the route tree.

Two shape rules when adding an entry:

- The sweep **logs in as a fixture user before every screen**, so a public route
  needs `anonymous: true` to skip that step. The landing page is the first entry
  that uses it.
- The sweep is **desktop-only**, on the stated grounds that the desktop DOM is a
  superset of the mobile one. That premise holds while a screen's responsive
  behaviour is the grid-collapses-to-cards pattern, and breaks the moment a
  screen renders *different markup* below `lg` — a mobile nav drawer, for
  instance. A screen that does needs its own mobile pass, which is a reason to
  prefer the collapsing pattern.

### Coverage scope, and why the number is ~49% and not ~70%

`coverage.include` is `src/lib/**` only. Pages, components and BFF route handlers
are exercised by the Playwright tier, which is not instrumented, so including
them would report a misleadingly low number and enforce nothing.

Within that scope, roughly 1,300 of the lines are `src/lib/query/**` — TanStack
Query hooks that need a QueryClient and a backend to say anything, and which the
functional tier already drives. Chasing them in jsdom would raise the number
without raising confidence. 49% is the honest figure for this scope; treat a
proposal to "get to 80%" with suspicion unless it names which modules and why.

---

## Gates, and what each one is for

Run locally with `make frontend` (the PR-equivalent) and `make frontend-audit`
(needs network).

| Gate | Command | Mirrors |
|---|---|---|
| lint | `eslint . --max-warnings 0` | — |
| format | `prettier --check .` | `cargo fmt --check` |
| types | `tsc --noEmit` | — |
| **BFF authz** | `pnpm authz:check` | — |
| unit + component + coverage floor | `vitest run --coverage` | `cargo llvm-cov --fail-under-lines` |
| generated API drift | `pnpm gen:api` + `git diff --exit-code` | `er-entity verify` |
| **advisories** | `pnpm audit:check` | `cargo deny check advisories` |
| **licenses** | `pnpm licenses:check` | `cargo deny check licenses` |
| static analysis | `semgrep --config .semgrep.yml` | — |
| functional + visual + a11y | `pnpm test:e2e` | — |

### The BFF authorization gate

`scripts/check-bff-authz.mjs` asserts that every route handler under
`src/app/api/orgs/**` calls `requireOrgAccess` and every one under
`src/app/api/admin/**` calls `requireSuperAdmin`. 54 handlers, all compliant.

This is the property the whole tenant-isolation model rests on: `src/proxy.ts` is
a cookie-**presence** gate only — it runs where there is no database access — so
a handler that forgets to authorize is reachable by any session, with the BFF
attaching a privileged credential upstream.

It is a file-level textual check on purpose. "Did this file remember to authorize
at all" needs no dataflow analysis, and a cheap check that cannot silently stop
working is worth more here than a clever one.

### Supply chain

`audit:check` fails on HIGH and CRITICAL; moderate and below are reported. The
tree carries a long tail of dev-only transitive DoS advisories and a gate that
cries wolf gets switched off.

Exceptions live in `frontend/audit-allow.json` and follow `rust/deny.toml`'s
convention: **a prose justification per entry naming why the advisory cannot
reach us.** Not "fixing it is inconvenient" — *the vulnerable code path does not
exist in this application*. If you cannot write that sentence honestly, upgrade
the dependency or pin it through `pnpm.overrides`. Stale entries are reported, so
the allowlist does not quietly accumulate.

`licenses:check` is an allowlist, not a denylist — a license it has never seen
fails and gets looked at. That is the point: the risk is a copyleft dependency
arriving unnoticed in a transitive bump.

### The monochrome rule

`docs/frontend-design.md` §3.3: *"Black and white, period… no brand accent, no
colored chips or badges, no palette. This is a standing constraint on every
surface and component, not a launch-phase simplification."*

That was stated in three places and enforced by nothing, and the codebase had
drifted: **12 component files, 47 colour utilities** using emerald, red and amber
to encode meaning. The `no-restricted-syntax` rule in `eslint.config.mjs` now
rejects Tailwind palette utilities in string and template literals.

What to use instead — the semantic tokens in `globals.css`, every one chroma-0 in
both themes. The techniques that replaced hue:

| Was | Now | Why it still reads |
|---|---|---|
| emerald/red waterfall bars | `bg-foreground/70` both | polarity is the bar's **side** of the axis, plus the signed weight |
| emerald/red `always`/`never` chips | filled vs **outlined** | opposites, not better/worse |
| emerald "published" chip | **filled** vs muted | the live version has to be findable at a glance |
| amber impersonation banner | `bg-foreground text-background` | it must still shout; inverted is the loudest a grayscale theme has |
| emerald/amber histogram zones | three **lightness** steps | ordering already carries it |
| emerald success icons | `text-foreground` | the glyph (`CheckCircle2` vs `Circle`) was already saying it |

**Known exception, not yet resolved: `--destructive` is still red.**

After the conversion, an audit of every colour value in the built stylesheet
leaves exactly two that are not grayscale, and both are this one token:

```
--destructive: #bf000f   (light)   rgb(191, 0, 15)
--destructive: #ff6568   (dark)    rgb(255, 101, 104)
```

Everything else in the bundle is `R == G == B`, or `lab()` with zero a/b. So the
app is grayscale apart from `.bg-destructive`, `.text-destructive`,
`.border-destructive` and `.text-destructive-foreground` — 25 files, 54 usages
(error text and destructive buttons).

The lint rule deliberately does not flag it: referencing a semantic token is the
correct thing for a component to do, and whether that token carries hue is a
*theme* decision made in one place. Resolving it is two lines in `globals.css`
plus a design call — desaturating removes the danger affordance from every error
message and destructive action at once, and §3.3's answer ("weight, scale,
spacing, motion") needs applying deliberately rather than by find-and-replace.

To re-run this audit after any theme change:

```sh
cd frontend && pnpm build
CSS=$(find .next -name '*.css' | head -1)
grep -hoE '#[0-9a-f]{6}\b' "$CSS" | sort -u | python3 -c "
import sys
for l in sys.stdin:
    h=l.strip().lstrip('#'); r,g,b=(int(h[i:i+2],16) for i in (0,2,4))
    if not r==g==b: print(f'#{h} rgb({r},{g},{b})')"
```

### Why the visual baselines did not move

The colour conversion changed no committed baseline — all 30 regenerated
byte-identical. That is self-consistent rather than suspicious: the generated
stylesheet now contains **no** emerald, amber or red palette value at all, so if
any baselined screenshot had rendered one of those pixels, removing it would have
changed that screenshot. The converted components are simply not on the ten
baselined screens, or not in the state the fixtures reach (the dashboard's
`CheckCircle2` steps, the staged-decisions banner, the job-success card and the
swipe-card stamps all need state the pixel-asserted fixtures do not produce).

Worth knowing as a gap: the visual tier pixel-asserts ten screens, not every
component state. The functional tier still drives these components behaviorally.

---

## The full-stack tier

`tests/e2e/` (5 specs, 16 cases) runs against a real stack: one Postgres, a local
DuckLake, `er-server`, and the Next.js app built and served on `:3200`.

```
make frontend-dev     # container + erweb migrations + er-server on :8000, blocks
make frontend-seed    # org, api keys, published config, base_10 resolved, logins
cd frontend && corepack pnpm build
make frontend-e2e     # the tier
```

It is **not** on the PR path — it is slow (~10 min) and needs Docker. Treat it as
a release gate and as the thing to reach for when a change spans
BFF → er-server → engine.

### What it was, and what revived it (W0.5b)

It was dormant from the all-Rust cutover until W0.5b, and the reason recorded at
the time — `frontend/dev/seed.py` deleted by `7cc444f`, `make frontend-seed`
dropped by `9a55504` — was only the visible half. The substrate had gone with the
Python tree too: `up.sh` booted `uvicorn erserver.api` and
`python -m erserver.dispatcher`; `env.sh` exported eight `ERSERVER_*` variables
that nothing in the Rust workspace reads; and `compose.yaml` built its MinIO and
catalog-Postgres services from `docker/Dockerfile`, which no longer exists.

The stack is now deliberately smaller: **one** Postgres (control plane + the
`erweb` schema) and a local DuckLake catalog file plus data directory — exactly
what every `lake_e2e` case uses. MinIO and the catalog Postgres are gone. The
honest cost is that a bug which only appears against S3 or a Postgres-backed
catalog will not be caught here; neither local gate caught those before either
(`make check-e2e`'s lake tests are also local), and the real AWS environment
covers them.

`er-server` runs the dispatcher **in-process** (`ER_DISPATCH_IN_PROCESS=1`), so
the half-up state the old `run.sh` had to detect — a live API with nothing
draining the queue — is unreachable.

### The seeding order is forced, not chosen

`frontend/dev/seed.mjs` resolves the first corpus through the `er` **CLI**, before
publishing a config. Both alternatives are closed:

* A first publish is tier C, so it enqueues `train` + `run_all_full`. That rebuild
  is publish-triggered, so W0.3 sets `skip_ingest` on it — it never reads the drop
  directory, and on a new org both jobs fail for want of a corpus.
* `POST /imports` is the route that *does* ingest, and `delivery_preconditions`
  refuses it until a config is already published.

So something has to put a corpus in the lake first, and only the CLI can.

`.state/expected.json` is the ground truth the specs read, in the shape their
`Expected` interface declares: `{ org, metrics: { records, entities,
duplicate_groups } }`. It is derived from the committed `base_10` fixture, not
from the run that just happened — an independent statement of the right answer —
and `metrics` is read as a **floor**, because the dev tenant is long-lived and
every pass appends to it.

### What the first run found

The tier earned its keep immediately. `GET /api/orgs/{org}/jobs` in the BFF did
`forward<unknown[]>(...)` and passed the body through — a type *assertion* about
an HTTP body TypeScript cannot check. er-server returns `{ "jobs": [...] }` (it
wraps every list: `items` for most, named keys for some), while the UI hook types
it `JobRow[]`, the **mock returned a bare array**, and the specs call `.some()` on
it. Three consumers agreed on "array"; only the real backend disagreed.

The damage was not a blank table. `dashboard-content.tsx` does
`(jobs.data ?? []).find(...)`, and `?? []` does not apply to a non-null object, so
`.find` was undefined and **the whole dashboard crashed on render**. One line took
out the app's landing page against the real backend, and the mock's
unfaithfulness hid it through every green functional run.

Fixed on both sides — unwrap in the BFF *and* make the mock return `{jobs}`.
Unwrapping alone would have broken the functional tier, which is the point: the
mock has to be faithful or it manufactures green. **The W0.4a contract gate
compares routes, not response shapes**; a shape-level mock-vs-OpenAPI check would
have caught this and is still unbuilt.

### Known failures, and what each one is

This tier is **red by design right now**: it reports real defects rather than
being tuned to pass. 8 of 15 cases pass. Do not "fix" these in the specs.

| Case | Diagnosis |
|---|---|
| `see-your-data` → provisions a tenant through the UI (×2) | **Product gap.** `create_org` returns 503 "auto-provisioning not configured" without a `config_path` — the path the admin tenant-create UI uses. The Python server provisioned server-side from the dead `ERSERVER_*` variables; the Rust port never reimplemented it. Tenant self-onboarding is broken in the product, not the test. |
| `run-the-business` → suspended tenant refuses work (×2) | **Real bug.** Submitting a job to a suspended org returns **202, not 409**: `create_job` has no suspension guard, while `publish_config_version` and `delivery_preconditions` both have one. The UI promises "a suspended tenant refuses jobs, imports and steward actions" — jobs is the one that does not. |
| `fix-your-data` → assertion library shows the actor | **Product gap.** Expects `steward@acme.dev`; gets `key:<uuid>`. `Principal.actor` is derived solely from the API key and the BFF forwards no end-user identity, so every human action is attributed to a service credential. Fixing it means forwarding the authenticated user for *attribution only* — never for authorization, or it becomes an impersonation vector. |
| `see-your-data` → jobs table lists `run_all` | **Cold-start flake.** The card's text is just its title, meaning `jobs.isPending` was still true and the Skeleton (no text) was rendering. A freshly started `next start` has to initialise the dynamic route inside the assertion's 5 s budget. Needs a longer first-assertion timeout, not a code change. |
| `grow-your-data` → wizard activates a new source | Not yet diagnosed. |

---

## Visual baselines

Linux-only, and committed. Regenerate with `make frontend-baselines`, which runs
`--update-snapshots` inside `mcr.microsoft.com/playwright:v1.63.0-jammy` — the
same image the CI `e2e` job uses, so the comparison environment matches. On other
platforms snapshot comparison is skipped (`ignoreSnapshots`), so local runs never
fight font rendering.

`@playwright/test`'s version and that Docker tag must move together. Dependabot
is configured not to bump the package's major or minor for exactly this reason
(`.github/dependabot.yml`).

**A baseline diff is a question, not a chore.** Regenerating is correct when you
changed the design on purpose. If you did not, the diff is the finding.

**Scope a screenshot to a section, not `fullPage`, on anything long.** The
marketing surface is the case that forced the rule: a landing page captured
whole is one very tall PNG per device project, so a copy edit anywhere rewrites
all three and the diff gives no indication of what moved. Per-section captures
keyed to a `data-testid` mean an FAQ edit touches the FAQ baseline alone.

**The trap this tier sets for a macOS author.** `ignoreSnapshots` means a local
run reports green having compared *nothing*, so a PR that adds a
`toHaveScreenshot` call and no `*-linux.png` files looks fine locally and fails
on CI's first run. Adding a visual assertion and running
`make frontend-baselines` are one step, not two.

---

## Proving a gate works

Every gate in this file has been observed **failing** on a deliberate violation,
not just passing. This is a rule, not a nicety, and here is what it caught:

The BFF authorization invariant was first written as a semgrep `pattern-not`
rule. Semgrep **rejected the pattern as invalid TypeScript while still reporting
the rule as run with zero findings** — so two deliberately unauthorized route
handlers sailed through. A false green on the most important property in the
application. It is now plain code with tests that plant violations and require
them to be reported.

So, when adding a gate:

1. Write it.
2. Break the thing it protects. Watch it fail, and read the message it prints.
3. Restore. Watch it pass.
4. Where the gate is code, keep step 2 as a test (`tests/unit/bff-authz-gate.test.ts`
   is the model — it plants an unauthorized handler and requires the catch).

For a test rather than a gate, the equivalent is mutation: change the behavior the
test claims to protect and confirm the test fails. The authorization matrix in
`tests/unit/bff-access.test.ts` was verified this way — turning the non-member
404 into a 403 fails 4 cases, and deleting the impersonation org confinement
fails exactly the case written for it.

---

## Change log

- 2026-10-20 — **The gate became blocking** (#187). `detect` job replaces the
  `paths:` filter; `lint + types + unit`, `playwright functional tier` and
  `pnpm audit + licenses` added to the required contexts, verified not to
  deadlock a backend-only PR. New: `pnpm audit:check`, `pnpm licenses:check`,
  `pnpm authz:check`, `.semgrep.yml`, the `gen:api` drift gate, pnpm-store and
  `.next` caches, npm + cargo Dependabot, and the five restored `make
  frontend-*` targets. The audit gate's first run found 4 HIGH advisories:
  `next` 16.3.6 → 16.3.8 (six advisories, incl. a HIGH SSRF in image
  optimization), `sharp` → 0.35.5, `source-map-js` → 1.2.2. Gate:
  `pnpm check` + the functional tier + all four Rust contexts.
- 2026-10-20 — **Security headers and three fixed vulnerabilities** (#190).
  Five enforced headers plus a report-only CSP and its sink. Fixed: an open
  redirect via `?next=` (confirmed exploitable — reverting the fix fails the new
  Playwright case), CSV formula injection in both export routes, and an
  unvalidated org slug reaching ~50 raw path interpolations. Gate: `pnpm check`
  + 13 new Playwright cases.
- 2026-10-20 — **The unit tier** (#201). `requireOrgAccess`'s full matrix,
  `bff/members.ts` against PGlite, the component tier created, the browser fetch
  helper. Coverage 30.9% → 49.3% of lines; 62 → 272 tests. Verified by mutation.
  Gate: `pnpm check`.
- 2026-10-20 — **Coverage on the PR path, the monochrome rule, this doc.**
  Floor set to lines 48 / functions 33 / statements 48 / branches 49, just under
  the measurement. 12 component files converted to grayscale and 30 visual
  baselines regenerated in the pinned Linux image. Gate: `pnpm check` (coverage
  enforced) + the functional tier.
- 2026-10-20 — **`detect` now filters pushes to main as well**, not just PRs.
  Removing the `paths:` filter (needed to make the checks required) had made
  every backend-only merge run the full frontend gate, Playwright container
  included, re-testing a tree the just-merged PR had already tested. Fails safe
  on an all-zeroes or unresolvable `event.before`. Gate: all eight event paths
  exercised against real commits (#206 as a frontend-touching merge, #207 as a
  Rust-only one) before pushing.
- 2026-10-20 — **Capped cargo Dependabot at 2 open PRs.** Adding the cargo
  ecosystem in #187 opened five at once; each carries a different `Cargo.lock`,
  so rust-cache misses and the workspace compiles near-cold, and five in
  parallel starved the runners enough to time out `fmt + clippy + docs` (30 min)
  on the dependabot branches and on main. A side effect of this program, fixed
  here rather than by raising the timeout. npm stays at 5 — those PRs do not
  compile Rust.
