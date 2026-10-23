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
| **wire contract** | `tsc --noEmit` (`tests/unit/wire-contract.test.ts`) | `er-server/tests/mock_contract.rs` |
| **mock fabrication** | `vitest run` (`tests/unit/mock-fabrication.test.ts`) | `er-server/tests/mock_contract.rs` |
| unit + component + coverage floor | `vitest run --coverage` | `cargo llvm-cov --fail-under-lines` |
| generated API drift | `pnpm gen:api` + `git diff --exit-code` | `er-entity verify` |
| **advisories** | `pnpm audit:check` | `cargo deny check advisories` |
| **licenses** | `pnpm licenses:check` | `cargo deny check licenses` |
| static analysis | `semgrep --config .semgrep.yml` | — |
| functional + visual + a11y | `pnpm test:e2e` | — |

### The wire-contract gate

`tests/unit/wire-contract.test.ts` asserts that a response shape the UI declares
does not invent fields the backend does not have. The authority is
`src/lib/entities/bindings/*`, generated by `er-entity-gen` from
`entities/*/entity.yaml` and drift-gated by `er-entity verify`, so it cannot
disagree with what er-server serializes.

It is a **compile-time** check, not a runtime one:
`NoUndeclaredFields<Ui, Entity, Composed>` resolves to `true` when the UI shape's
keys are a subset of the entity's, and otherwise to an error object naming the
offending keys — so `tsc --noEmit` fails inside the already-required
`lint + types + unit` context, with a message like
`'{ UNDECLARED_WIRE_FIELDS: "progress" }'`.

Two rules when adding a pair to `PAIRS`:

- **A BFF-composed field is declared, not assumed.** Put it in the third type
  argument with a comment naming where it comes from. `AdminOrgRow` legitimately
  carries `display_name`/`registered` (the erweb tenant registry — a different
  database) and `has_credentials` (the credential vault); er-server knows about
  none of them, correctly. The gate's value is that such a field is written down
  rather than being indistinguishable from a fabrication.
- **Do not delete an assertion to make a failure go away.** The `it()` block
  asserts the exact pair list for that reason.

The file also proves its own non-vacuity: a `@ts-expect-error` case requires that
an invented field does *not* type-check, so if the type ever collapses to a
tautology the directive goes unused and `tsc` reports TS2578. That assertion
earned its keep immediately — it caught its own first draft, where the directive
sat at the top of a multi-line comment and so suppressed a comment line instead
of the statement below it.

**What it does not cover:** envelope and ordering. `{jobs: […]}` read as a bare
array (W0.5b) and a mock serving receipts newest-first for an endpoint that pages
oldest-first (W0.5d) are per-path facts, and `src/lib/api/openapi.json` documents
**zero** paths — `er-server/src/openapi.rs` registers `components.schemas` only,
on purpose. Those 12 schemas are these same entity structs, which is why the
field half is buildable today and the envelope half needs `#[utoipa::path]` on
the handlers first.

### The mock-fabrication gate

`tests/unit/mock-fabrication.test.ts` is the other half of the wire contract.
The wire-contract gate asks whether a **UI type** declares a field its entity
lacks; this one asks whether the **mock emits** one. Neither that gate nor
`mock_contract.rs` looked at what the mock actually sends, and the mock is where
the defect is born: it encodes the frontend's belief about the API, so a field
invented there reads as real in all three mock-backed tiers.

It walks every object literal in `tests/mock-erserver/server.mjs` with the
TypeScript AST, matches each against the entity it best fits, and fails on any
key that entity does not declare. The authority is `entities/*/entity.yaml`
rather than the generated bindings, because the lake-only entities (`assertion`
among them) have no TypeScript binding and their rows would otherwise be
misattributed to whichever entity happened to share the most fields — that one
choice removed thirteen false positives.

Server-composed fields are legitimate and belong in `EXPLAINED` with a note on
where each comes from: `run.auto_merged_pairs` is derived on read,
`assertion.assertion_id` is a wire rename of the entity's `id`. As with
wire-contract's third type argument, the point is that such a field is
*accounted for* rather than indistinguishable from a fabrication. A companion
assertion fails on a **stale** `EXPLAINED` entry, since a note kept after the
field stopped being emitted also silently permits its return.

**This replaced an earlier decision.** The two known fabrications on jobs
(`outcome`, `exit_code` — 11 sites each, absent from er-server, read by no
component) were previously left in the mock on purpose, as the fixture the
envelope half would be built against. They are now removed: a field-level gate
exists, and a sabotage run that re-adds `exit_code` and watches the gate name it
is a better fixture than a fabrication living in the shipped mock, which is a
trap for the next person to surface a job field. The envelope half still needs
`#[utoipa::path]` and still has no fixture; that is the honest state.

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

The control-plane Postgres binds **5433**, which is also the port the engine's
own lake-E2E instructions use, so a machine already running one of those gets
`Bind for 0.0.0.0:5433 failed: port is already allocated` and nothing else.
Export `ER_DEV_PG_PORT` on all four commands to move it — `env.sh` threads it
into both DSNs and `playwright.e2e.config.ts` reads `ERWEB_DATABASE_URL` from
the environment before falling back, so the override reaches every consumer:

```
ER_DEV_PG_PORT=5470 make frontend-dev
ER_DEV_PG_PORT=5470 make frontend-seed
ER_DEV_PG_PORT=5470 ERWEB_DATABASE_URL=postgresql://postgres:er@localhost:5470/postgres \
  make frontend-e2e
```

Two things to know before trusting a result. `next start` serves whatever is in
`.next`, so **rebuild after every source change** or the tier silently tests the
previous build. And the tier is stateful: each wizard pass adds a source
carrying its own duplicate pair, so a corpus grown by repeated runs is a
different corpus — which is how the `ingest_batches` window bug and the unmerge
spec's over-strict count were both found, but also means an exact-count
assertion over the corpus is a latent flake.

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

### What this tier found, and what each finding became

It was revived red at 8 of 15 and left that way on purpose — the failures were
real defects, not spec drift. Every one has since been fixed, each in its own PR
and each locked by a test observed failing first:

| Case | Finding | Fix |
|---|---|---|
| `run-the-business` → suspended tenant refuses work (×2) | Submitting a job to a suspended org returned **202, not 409**. `create_job`, `add_assertion` and `record_overlay_action` all went role-check → org-exists → `pg::enqueue`, so a suspended tenant could be made to spend compute and mutate its own lake — while the admin UI promised otherwise. `publish_config_version`, `delivery_preconditions` and `put_lexicon` each already refused, in three different idioms. | D6 — a shared `refuse_unless_active` on the three that had none |
| `see-your-data` → provisions a tenant through the UI (×2) | `create_org` returned 503 without a `config_path` — the path the tenant-create UI uses, so self-onboarding was broken in the product. The `orgs.state` enum, the mock, the registry's `provision_job_id` and the UI's progress poller all already assumed the flow; only the server was missing. Six of the dead `ERSERVER_*` variables were its configuration. | D8 — a declared provisioning profile (local / Postgres) plus a `provision` job |
| `fix-your-data` → assertion library shows the actor | Expected `steward@acme.dev`, got `key:<uuid>`. The BFF had **always** sent `x-acting-user` and its own comment promised `user:<email> via key:…`; er-server had zero references to the header. | D7 — honour it for attribution only, never for authorization |
| `grow-your-data` → wizard activates a new source | `?limit=3` returned every job the org had ever had: the handler had no `Query` extractor and `pg::list_jobs` passed `i64::MAX`, while the BFF forwarded `limit` and `state` and the UI asked for `?limit=20`. The spec's three-kind assertion could never hold. | a `Query` extractor routing to the existing `pg::list_jobs_all` |
| `grow-your-data` → wizard activates a new source (2nd cause) | The activation panel sat on "Create draft" until the 120 s `waitForURL` expired, having **sent no request and shown no error**. `activate()` opened with `if (!activeConfig.data) return;` — a silent return inside an async pipeline whose UI had already been set to `running` — and the button was never gated on that read, so a click landing before `GET /config` resolved was a no-op. The mock answers in ~1 ms and always won the race; a real BFF → er-server → fresh-connection Postgres round trip does not. | gate both actions on the base config, and turn all three silent returns into errors |
| `grow-your-data` → the run page after activation | **The whole run-detail page threw** against the real backend: `data.progress.stages` with no optional chaining, where er-server sends no `progress` key at all. Per-stage progress has no producer — the `run` ledger is one row per run and the stage-level tracking went with the Python tree — while `JobRow` declared `progress` **required** and the mock fabricated it at 11 sites. Same field, same shape, two more crash sites: the runs list (`lastStage`) and the dashboard's active-job line. | make the field optional so `tsc` finds every reader, strip the fabrication from the mock, and retarget the spec that asserted a mock-only stage name |
| `grow-your-data` → the delivery receipt for the new source | **Real bug.** `ingest_batch/read/page.sql` is a forward keyset cursor (`ORDER BY id`, `id > $after`), and the "did my file do anything?" panel forwarded to it with `?limit=20` — so it rendered the **oldest** 20 receipts and a tenant past 20 ingest batches could never see a new source land. Two more consumers read the same endpoint as newest-first: the assistant's `get_import_receipts`, whose declaration says "newest first", and the mock, which served the fixtures descending regardless — which is why every tier was green. | `ingest_batch/read/recent` + `GET /imports/recent`, mirroring `runs_list`; the mock now copies each endpoint's real ordering |
| `fix-your-data` → unmerge splits a merged entity | Asserted exactly `memberCount - 1` members after peeling one record. Re-clustering runs from the full edge set, so if the peeled record was the only hop holding another member in, that member leaves too — a 5-member group legitimately lands at 3. The spec failed on correct behaviour once repeated tier runs grew the corpus. | assert strictly fewer, which is what the comment above it already claimed |
| `see-your-data` → jobs table lists `run_all` | A **cold-route** flake, not slow data: `next start` initialises the dynamic route on first hit and the default 5 s expired while `jobs.isPending` was true, with the Skeleton rendering no text — so the card's whole content was its own title and the failure read as "element not found". | a 60 s budget on that first assertion |

Most of them were the same shape — **the frontend states a contract the backend
never fulfilled** — which is the class `mock_contract.rs` was built for and can
only partly see; it compares route and method, not response shapes, headers or
query parameters. Every one of those was *also* green in the mock tier, because
the mock asserted the frontend's belief rather than the server's behaviour: a
bare `[…]` for `{jobs: […]}`, a fabricated `progress` object, a stage name no
producer emits. That gap is the strongest remaining argument for a shape-level
gate, and `progress` is the sharpest case for it — the field was declared
**required** in `JobRow`, read unguarded at three call sites, and sent by nothing.

Two fabrications survive in the mock on purpose, as a marker for that gate:
`outcome` and `exit_code` are served on jobs, exist nowhere in er-server, and
are read by no component — harmless today, and exactly what a shape diff should
flag before someone starts reading them.

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

### The tolerance, and why it moved

`maxDiffPixelRatio` was **0.01** — about 16,800 pixels on a 1440x1170 full-page
shot. That is not a rounding allowance; it is a whole region of the screen.

Found by adding a navigation item and watching **no baseline change at all**,
which should have been impossible. Measured:

| | pixels |
|---|---|
| a whole nav entry on the dashboard | ~1,050 |
| the old tolerance | ~16,800 |

Two things followed from it, and the second is worse than the first:

1. A real change smaller than the tolerance **passed**.
2. `--update-snapshots` defaults to `changed`, which honours the same
   tolerance — so that change was neither failed nor rewritten. The committed
   baseline silently stopped depicting the app, and went on masking the next
   change too. Switching the regen to **`=all`** rewrote **20** stale
   baselines, among them `marketing-hero-*` (which has no nav at all, so that
   was pure accumulated drift) and the dashboard's job strip, still showing the
   fabricated `progress` stage text D9 had removed.

**The tolerance was masking a real instability, which had to be fixed first.**

The obvious move is a much smaller ratio — 0.0005 is ~840 pixels on that shot,
which would catch a nav item with margin. Within a single container run that
looks safe: all 39 visual tests pass at ratio **0**, so there is no same-image
nondeterminism at all.

Across runs there is. Running the tier five times against freshly regenerated
baselines produced an intermittent failure in roughly two runs out of five, and
it is not antialiasing:

```
Expected an image 393px by 2237px, received 393px by 2257px.
2947 pixels (ratio 0.00335 of all image pixels) are different.
```

A **twenty-pixel height swing** between identical runs of the config studio on
mobile-safari. That is a layout nondeterminism — most likely a font or an
async element settling after the screenshot is taken — and it is three
orders of magnitude larger than rasterization noise.

So 0.01 was not merely loose; it was **masking a real instability**. Anything
tight enough to catch a nav item is also tight enough to make the tier flaky,
and a flaky visual gate gets ignored — which is worse than a loose one.

**What it was.** A per-element probe (print every child's height across six
runs) ruled out the first suspect — the band estimate renders identical text
and a constant 16px every time — and landed on the **version-history card**:
62px while its query was in flight, 82px once a version arrived. It rendered
its heading with *nothing underneath*, which also states something false, since
the config does have a published v1.

It was not the only one. Grepping the class — a component rendering
`query.data ?? []` with no `isPending` branch — turned up a second in the card
directly above: the threshold editor's band estimate read **"≈0 of the 0
recently scored pairs fall in this review band"** before its scores arrived.
Most call sites in the tree already use the `isPending ? … : empty ? …` shape;
these two did not.

Two fixes for the version history, because it was two bugs:

1. the card now renders a `Skeleton` while pending and a sentence when the
   history is genuinely empty, per the loading/empty conventions above;
2. the visual spec waits for `version-history` as well as the band estimate.
   It had waited only on the latter — **a different query** — so the screenshot
   could be taken while the history card was still empty. A readiness check has
   to name every async region the shot contains, not the first one that is
   convenient.

The band estimate's loading copy deliberately avoids the phrase "recently
scored pairs", which is what the visual spec waits on — so an existing wait
becomes a real readiness check for that panel instead of one that matches both
states.

With all of it in place, **five consecutive regenerate-and-compare cycles pass
at ratio 0**, so the tolerance is now **0.0005** (~840 pixels on that shot) and
what remains is margin for cross-architecture rasterization — baselines are
regenerated on arm64, CI compares on amd64 — and nothing else.

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

### A DB-backed hook needs slack, and its failure reads as a skip

`vitest.config.ts` sets `hookTimeout: 30_000`. The DB-backed unit files open
PGlite and run every drizzle migration in a `beforeAll`, and the 10 s default is
uncomfortably close: `bff-members.test.ts`'s hook was measured at **10202 ms**
in CI against it, while warm locally it is ~1.2 s.

Worth knowing what that failure looks like, because it is misleading rather than
silent: vitest fails the whole **file** and reports its cases as `skipped`. The
run still exits non-zero, so the gate blocks — but the summary reads like a
smaller suite passed, and nothing points at the hook. Verified by forcing it
(`--hookTimeout=1`): 20 skipped, exit 1.

### Visual baselines are invisible to a local gate

`playwright.config.ts` sets `ignoreSnapshots: process.platform !== "linux"`, so
every `@visual` assertion is **ignored** off Linux. A green `make frontend` on
macOS is not evidence about a baseline, and the failure mode is the usual one —
it looks like proof, and it is not. Only `make frontend-baselines` (the pinned
Linux image) or CI can tell you.

Observed: removing a fabricated field from the mock emptied one table column.
Local reported 362 passed; CI reported `runs.spec.ts:92` failing on
mobile-safari. Any change to rendered content needs `make frontend-baselines`
and a *review of the diff* — it rewrites every `@visual` baseline, so one
unexpected file in that diff is itself a finding.

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
