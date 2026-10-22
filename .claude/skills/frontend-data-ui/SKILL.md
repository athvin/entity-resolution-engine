---
name: frontend-data-ui
description: Explains how this repo renders dense entity-resolution data — the charting policy that every chart is hand-rolled DOM/CSS rather than a library or canvas and why, the virtualization threshold and its one worked example, numeric column conventions, how scores and match evidence are presented under the explain-don't-gate rule, the loading/empty/error conventions and the deliberate absence of route-level `loading.tsx` and `error.tsx`, and the obligation to register a new screen in the axe gate. Use when building or changing a table, list, chart, score display, merge or review surface, when a view must render more rows than fit on screen, when presenting a distribution or a confidence value, or when adding a screen. This supersedes any generic data-visualization skill's chart-library and palette guidance. For tokens, primitives and copy voice see frontend-design-system; for the public marketing surface see landing-page; for wiring to data see frontend-bff; for the test tiers and baselines see frontend-testing.
---

# Dense data surfaces

The product's hard screens are a golden-records browser over an unbounded
corpus, a duplicates triage queue, and a merge-review surface that has to
explain a probability to a human who will act on it. The design doc calls merge
review "the killer screen", and it is where the rules below were learned.

> **Clarity is the feature.** Every score, tier label and state chip is
> hoverable or expandable to plain language, and the honest intermediate states
> (staged vs applied vs reconciled) are rendered as what they are rather than
> flattened into a boolean.

## Charts are hand-rolled DOM and CSS. There is no chart library.

No charting dependency is installed — not Recharts, not ECharts, not visx, not
D3. That is a decision, not an omission, and the reasoning is recorded in the
one chart that exists:

> Deliberately hand-rolled DOM/CSS (no canvas): **pixel-deterministic for
> visual baselines and legible to assistive tech.**
> — `src/components/review/waterfall-chart.tsx`

Both halves matter. A canvas is one opaque bitmap to a screen reader, and this
screen's whole job is explaining evidence. And a library that renders through
canvas or animates on mount produces a different image run to run, which breaks
the Linux pixel baselines the frontend gate depends on.

`docs/frontend-design.md` §8 recommends ECharts and visx. **That
recommendation was never adopted.** Proposing a chart library is therefore a
decision to escalate to a human, not an implementation detail to settle inside a
task — and a generic data-visualization skill will reach for one by default.

### How to build one instead

`waterfall-chart.tsx` is the pattern to copy, and it is short enough to read
whole before starting:

- a flex or grid column of rows, one row per datum;
- magnitude as a **width percentage carried in a CSS custom property**
  (`w-(--bar-w)` with `style={{ "--bar-w": … }}`), so there is no inline width
  string to drift and no layout animation;
- a centre axis as a 1px `bg-border` span, with polarity expressed as **which
  side of it the bar sits on** plus a signed number;
- `data-testid` on the container and on each bar, because that is how the test
  tier asserts on it;
- an explicit empty branch returning a sentence, never an empty chart frame.

Pick the form from the data, not from a gallery: bars or a histogram for a
distribution of cluster sizes or match scores, a bar-from-axis for signed
contributions, a sparkline for a trend in a tile. **Never a pie chart** — the
quantities here are compared, not apportioned.

Colour is not available as an encoding channel. See the frontend-design-system
skill for what replaces it; for charts specifically, the channels are bar
length, side of axis, sign, order, and `bg-foreground` against
`bg-foreground/70`.

## Anything that can exceed a few hundred rows is virtualized

`@tanstack/react-virtual` is the installed tool, and
`src/components/records/records-browser.tsx` is the **only** virtualized surface
in the tree — so it is both the worked example and the thing to copy:

- `useVirtualizer` with a fixed `estimateSize` and `overscan`, against a
  scroll-container ref;
- the spacer height driven by `getTotalSize()` through a CSS custom property,
  the same trick the chart uses;
- rows rendered from `getVirtualItems()` and positioned absolutely.

The threshold is the design doc's: "virtualized tables for anything that can
exceed a few hundred rows". Judge it by what the data **can** be for a real
tenant, not by what the fixture holds — a corpus browser that is fast on 800
fixture records and dies on 2M is the failure this rule prevents.

A virtualized list needs a stable row height to stay smooth, which means the
row design and the virtualization are one decision. Deciding row contents first
and discovering they wrap is the usual way this goes wrong.

## Numbers in columns

- **`tabular-nums` on every numeric column**, so digits align down the column
  rather than jittering by glyph width. It is already used across the dashboard
  tiles, the score-band browser, the runs list and the chart.
- **Fixed decimal places** via `toFixed`, and a monospace face for scores and
  weights. This is a deliberate exception to the general "use `Intl.*`" advice:
  alignment down a column beats locale-aware grouping for values a human is
  comparing row to row. Dates and user-facing counts still want `Intl`.
- **Signed values carry an explicit `+`**, because the sign is the information.

## Scores must explain themselves

A bare probability is a defect on this product. The rule from
`docs/frontend-design.md` §3.3, stated as a requirement:

> "0.93 — just below your auto-merge threshold of 0.95; that's why it's asking
> you."

So a score is rendered with its threshold, its band, or its evidence reachable
without leaving the row. The match-evidence waterfall, the score-band browser
and the pre-merge report all exist to serve that rule. When adding a new place a
score appears, the question to answer is "where does the reader find out what
this number means" — and if the answer is "nowhere", the surface is not done.

## Loading, empty and error are per-component by convention

There are **no `loading.tsx`, `error.tsx` or `not-found.tsx` files anywhere** in
`src/app`. No route-level Suspense or error boundaries exist. That is the
current convention, so follow it rather than introducing a route-level file for
one screen and leaving the rest inconsistent:

| State | Convention |
|---|---|
| loading | the `Skeleton` primitive, inline, shaped like the content it replaces |
| empty | a sentence in the house voice — `No X yet — <next action>` |
| not found | a **404 is an empty or pending state**, not an error; `entity-view` and `config-studio` both special-case it |
| error | narrow `BffRequestError` from `@/lib/api/client` at the call site, with a short lowercase fallback message |

A skeleton should match the shape of what is arriving. A spinner where a table
will appear tells the reader nothing about what they are waiting for.

## Filters are component state, and that is a known gap

Filter and search state lives in `useState` with a debounce —
`records-browser.tsx` is the example. **Nothing in the tenant surface reads or
writes `searchParams`.** The only `useSearchParams` in the tree is `/login`
reading its `next` parameter.

The consequence is real and worth saying out loud when working on these
screens: a filtered or searched view **cannot be linked, bookmarked or shared**,
and the back button does not undo a filter. The vendored Web Interface
Guidelines rule "URL reflects state, deep-linking" is a rule this repo does not
currently meet.

Treat it as a gap to close deliberately on a screen you are already changing,
not as a convention to imitate — and not as a licence to retrofit every screen
in a task about something else.

## A new screen must be registered in the axe gate

`tests/functional/a11y.spec.ts` holds a hardcoded `SCREENS` array and sweeps
each entry with `@axe-core/playwright`. **A screen that is not in that array is
silently unchecked** — the gate passes, having examined nothing about it.

Which routes are currently missing from it is derived rather than listed:
[references/screens.md](references/screens.md). Adding a screen means adding an
entry with a `ready` predicate that waits for real content, which in turn means
the screen needs a stable `data-testid`.

Virtualization and table patterns in detail, with the `records-browser`
walkthrough: [references/dense-surfaces.md](references/dense-surfaces.md).

## Common workflows

**Charting a distribution.** Do not install a library and do not use canvas.
Read `waterfall-chart.tsx`, pick the form from the data, encode with length and
position, add `data-testid`s, and write the empty branch first.

**A list that could get long.** Virtualize it with `@tanstack/react-virtual`,
copying `records-browser.tsx`. Fix the row height before designing the row.

**Showing a score.** Render the number with `tabular-nums` and fixed decimals,
and give the reader a path to its meaning in the same row.

**Adding a screen.** Build it, give its main region a `data-testid`, add it to
`SCREENS` in `a11y.spec.ts`, then run `make skills-check` and confirm the
derived gap list no longer names it.

**Asked for a route-level loading or error boundary.** Note that none exist
anywhere and that the convention is inline. If introducing one is genuinely
right, it is a convention change for the whole surface, not one screen.
