---
name: landing-page
description: Explains this repo's public marketing surface — the `(marketing)` route group that prerenders statically inside the same app that serves the BFF, why static `export` output and `ensureStatic` are unavailable, the metadata routes that must sit at the app root, the PUBLIC_PATHS entries without which crawlers are redirected to login, how a public page gets an absolute origin without `NEXT_PUBLIC_*`, the JSON-LD approach, and the grayscale rule it inherits. Use when editing or adding a marketing page or its copy, touching robots, sitemap, canonical URLs, Open Graph or structured data, changing where the portal CTA points, or when a crawler sees a redirect instead of a page. For tokens, type scale and copy voice see frontend-design-system; for a table, chart or stat on a marketing section see frontend-data-ui; for the proxy and the browser/server line see frontend-bff; for the axe gate and visual baselines see frontend-testing.
---

# The marketing surface

The public landing page is a `(marketing)` route group **inside the main Next
app**, statically prerendered. It is not a separate project, and the reasons are
worth knowing before anyone proposes making it one:

> One design system, one gate, one deploy. A second project would mean a second
> copy of every chroma-0 token, a second five-tool gate plus Playwright matrix
> plus audit allowlist, and a hosting target that **does not exist** — the ALB
> is internal and there is no CDN in the Terraform.

`output: 'export'` is unavailable for the full app: the same app serves the BFF
route handlers and needs a server. Marketing previews use a disposable copy of
the marketing routes instead; `frontend/preview/README.md` describes the export,
CI artifact and optional Cloudflare Pages deployment. The production build
continues to use the normal configuration.

## The page is static, and that is structural

Every marketing page carries `export const dynamic = "force-static"`. This is
not decoration — it makes `cookies()`, `headers()` and `useSearchParams()`
return empty values, so a later edit that reaches for the session **cannot**
quietly turn the landing page into a per-request render.

**Do not reach for `ensureStatic`.** It is Next 16.4's natural "this must stay
static" assertion and it looks like the right tool, but it requires
`cacheComponents`, which `next.config.ts` does not set, and the **build fails**
rather than warning. The legacy segment config is what applies here.

Verify with the build output rather than by inspection: `next build` prints
`○ (Static)` beside `/`. If it prints `ƒ`, something in the tree reached for a
request-time API.

## Four traps, each of which fails silently

These are the ones that look fine in the tree and are wrong in production.

**1. `robots.ts` and `sitemap.ts` belong at `src/app/`, never in the route
group.** Next's metadata normalizer appends a hash suffix to a metadata file
reached through a route group, and only appends `.txt` when the page path is
exactly `/robots`. Inside `(marketing)` it is served as `/robots-<hash>` with no
extension — a robots.txt that does not exist, from a file that looks correct.

**2. `/robots.txt` and `/sitemap.xml` need explicit `PUBLIC_PATHS` entries.**
The proxy matcher excludes only `.svg`, `.png`, `.ico` and `.webp`, so a `.txt`
or `.xml` response is gated like any page and every crawler gets a 307 to
`/login`. Those two are literal entries in `MARKETING_PUBLIC_PATHS` in
`src/lib/site/routes.ts`, which is the list the proxy spreads in. **A page goes
in `MARKETING_PATHS`, not that list** — `MARKETING_PUBLIC_PATHS` spreads
`MARKETING_PATHS`, and `src/app/sitemap.ts` reads it directly, so one edit there
reaches both consumers. Adding a page to the public list instead makes it
reachable while leaving it out of the sitemap.

**The nav is not one of those consumers.** The header and footer read
`src/components/marketing/site-links.ts`, which is hand-maintained. A page added
to `MARKETING_PATHS` is reachable and in the sitemap and **linked from nowhere**
until it is added there; `tests/component/marketing-pages.test.tsx` fails when
an indexed path has no link. Placeholder pages (article outlines, legal pages
awaiting counsel) go in `MARKETING_UNLISTED_PATHS` instead: public, out of the
sitemap, and `noindex` through `pageMetadata({ unlisted: true })`.

**3. The OG image is a committed PNG, not `opengraph-image.tsx`.** A generated
one is served **extensionless**, so the proxy intercepts it and a crawler
fetching the card gets a redirect. Two further reasons not to generate it: the
`@vercel/og` default font is Geist, so the card would render in a typeface the
site does not use, and `ImageResponse` pulls wasm into a standalone bundle whose
Dockerfile copies no traced extras. Regenerate the committed card from its SVG
source rather than reaching for runtime generation.

**4. A prerendered route must not call `env()`.** It runs during `next build`,
where the schema's four required secrets are deliberately unset, and the whole
export step fails. `robots.ts` reads `publicBaseUrl()` instead — the one narrow
accessor for a single optional variable. Anything needing a _required_ value
must not be prerendered.

## The canonical origin is a constant, and that is not a `NEXT_PUBLIC_` evasion

`CANONICAL_ORIGIN` is hardcoded in `src/lib/site/routes.ts`. Three reasons, in
order of force:

1. A canonical URL is **one value for the whole internet, forever** — that is
   what canonical means. Per-environment is the bug.
2. `metadataBase` is mandatory once a page declares a canonical tag or an OG
   image, and `next build` runs in the Docker image with no runtime environment.
   One image serves every `{developer}.dev.*` hostname.
3. The `NEXT_PUBLIC_*` ban exists because that prefix **inlines a value into the
   client bundle**. Site metadata is server-rendered into `<head>` and reaches
   the browser as HTML, never as JavaScript. No public variable is needed, and
   adding one would trip semgrep for no benefit.

`ERWEB_PUBLIC_BASE_URL` keeps its existing job (invite and reset email links)
and gains exactly one more: `robots.ts` compares it to `CANONICAL_ORIGIN` and
emits `disallow: /` when they differ, so a developer namespace never competes
with the real site for indexing.

## JSON-LD needs no injection sink

The standard advice is `dangerouslySetInnerHTML`, on the grounds that React
escapes text children. **Measured against React 19, that is not true of
`<script>`:** quotes pass through so the JSON parses, and `</script` is encoded
as a legal JSON escape so a payload cannot close its own tag. The plain text
child is therefore byte-identical for valid payloads and _safer_ for hostile
ones.

That matters because `.semgrep.yml`'s `no-html-injection-sink` bans that sink at
ERROR with zero occurrences in the tree. Following the usual advice would have
taken the first exemption for no gain.

Both properties are pinned by `tests/component/structured-data.test.tsx`,
because they are React implementation details whose failure mode is invisible —
the page still renders, and only crawlers break.

## Search and AI crawlers

`src/components/marketing/seo.tsx` holds the share image, `JsonLd`,
`Breadcrumbs` (visible trail plus a matching BreadcrumbList) and
`PAGE_UPDATED`, the content dates that feed both the sitemap's `lastmod` and
the visible "Last updated" lines; change a page's date when its content
changes, never per deploy. Structured data repeats what the page shows:
Organization + WebSite on `/`, SoftwareApplication on `/product` (its lead as
the description) and `/pricing` (an AggregateOffer built from
`PRICING_CONFIG`), BlogPosting and Article, FAQPage. No ratings, reviews,
awards or certifications; `tests/component/seo.test.tsx` fails on them. A
page-level `openGraph` replaces the layout's, so `pageMetadata` sets the image
itself. `robots.ts` has separate search/retrieval and AI-training groups, with
`ALLOW_AI_TRAINING` at the top. The audit and its baseline are in
`docs/seo-audit.md`.

## Marketing follows the product tokens, with three explicit exceptions

All were requested by the owner, and all live in `story.css` under
`.marketing-site`, never in `globals.css`; the application palette and the
palette-utility lint rule remain unchanged.

- **The audit demonstration.** Home's audit film rings the member that does
  not belong in `--story-flag` and marks the two corrected entities with
  `--story-corrected`. Labels and shapes carry the same meaning.
- **The brand blue from the logo** (`frontend/assets/dupezero-logo-*.svg`).
  `--brand` is the logo's exact `#2F6BFF`, for marks and the flow's moving
  light only: it sits at the AA edge (4.4-4.5:1), so text and filled controls
  use `--brand-ink` and a marketing-scoped `--primary`, which clear 5.8:1 in
  both themes. Eyebrows, story links, focus rings, primary buttons and the
  next-chapter button carry it; everything else stays grayscale. One accent,
  no gradients, no second hue.
- **Source logos on the home page's source buttons** (`demo/source-logo.tsx`).
  Google Sheets, Salesforce, S3, Shopify and HubSpot carry their Simple Icons
  marks (CC0, inlined as path data); ZoomInfo carries its own logomark. Each has
  a `--brand-<key>` colour: a tint when off, the solid colour under
  `--brand-on-fill` ink when on (5.6:1 or better; ZoomInfo's red takes white).
  Scoped to those two pickers by `data-brand`; SFTP and Custom API stay
  grayscale. A brand colour used anywhere else is a new exception to ask for.

The header shows the logo, with both theme variants in the markup from
`public/brand/`; the favicon `src/app/icon.svg` is the mark on a solid tile.

## A visual story and a detailed reference

Home opens with a hero whose opening scene (`scene/hero-scene.tsx`) plays once,
in under five seconds, beside the headline: three versions of Meridian Supply
clean up, line up and collapse into one golden record, then hold. It is CSS on
a server component; Replay is its only island, and reduced motion shows the
payoff. Then come stacked ingress, organization/dataset, dedupe
and egress panels, followed by audit, usage, and a short FAQ and closing CTA.
Measured paths connect actual exit and entry ports across section boundaries;
resize and dataset/source changes must preserve their alignment. The
header links Product, Solutions, Resources, Pricing and Contact through
`marketing-nav.tsx`, a client island so the current page carries
`aria-current`. From `lg` the links sit in the row; below it they move behind a
Menu disclosure (Escape and an outside click close it). The row ends with two
buttons, Login (`PORTAL_ENTRY_PATH`) and Book a demo (`DEMO_PATH`); below `sm`
the header shows the logo mark and Book a demo, and Login moves into the Menu,
so the row fits 320px. There is no theme control: `app/providers.tsx` forces
dark on every `MARKETING_PUBLIC_PATHS` page, so a choice stored by the app's
own toggle cannot strand a visitor in light mode. The
footer has four link columns from the same `site-links.ts`. Solutions is a
click-to-open dropdown (never hover-only) of role and industry pages, and
Resources one of Docs, Blog and the ROI calculator (`/pricing#roi`), each nested
inside the Menu below `lg`; Escape closes the innermost open disclosure and
refocuses that dropdown's own button. How it works is a section of `/product`;
`next.config.ts` redirects the old `/how-it-works` URL there (308). A merged page
keeps its old URL working the same way. Those
pages are data in `pages/solutions-content.ts`, rendered by one template, and
**industry pages make no compliance claim**: a test fails on HIPAA, SOC 2 and
similar terms.

The content pages (product, features, solutions, integrations, security, FAQ,
resources, about, contact) are built from `components/marketing/pages/page-kit.tsx`.
**Their copy may describe only what `docs/feature-inventory.md` marks VERIFIED
or the owner has confirmed** (the AppExchange app and API connections, dated in
the inventory), including writing clean records back to the source systems,
which has no write path in this repo; no sign-up, no certification. Where a fact is
missing (a founding year, a customer quote), write the copy so it does not need
one; never invent it. Product screens are the app's own desktop visual
baselines (rendered in CI with fixture data), copied to `public/screenshots/`
and shown by `ProductScreenshot` in `pages/examples.tsx`; re-copy them when the
app's screens change. Never use a screen that shows storage paths, internal
code names or figures that read as statistics (Sources, Dashboard). Where no
clean screen exists, a labelled example panel stands in. The legal pages
are written from verified data handling but unreviewed by counsel, so they stay
in `MARKETING_UNLISTED_PATHS` (noindex). The contact form hands off to the visitor's
email app through `mailto:`; there is no form backend. `/pricing` is a calculator
whose every figure is in `src/lib/pricing/config.ts` and whose rules are the
pure `calculator.ts` beside it (see its README); the page itself does no
arithmetic. **Pricing is records only**: total records across every connected
system, never per org, system, connector or user (the rule and where it lives
are in `src/lib/pricing/README.md`). `tests/unit/pricing-rule.test.ts` fails on
per-org, per-connector, per-system or per-seat language in marketing copy or
the pricing code; deliberate exceptions go in its `EXCEPTIONS` list.
Home scrolls freely: there is no scroll snapping, by the owner's decision, and
no wheel or touch event is intercepted. Section links are ordinary anchors.
Progression is opt-in through `story-chapters.tsx`: one floating button that
always names the next chapter, and becomes "Back to top" at the end. It is
hidden at the top of the page and fades in once the reader scrolls down; after
the hero's `[data-explore-flow]` link it waits until that scroll has landed.
Both the link and the button move the page through `glide.ts`, one eased,
interruptible scroll on every device, rather than native smooth scrolling,
which story.css disables on touch.
Hidden means `visibility: hidden` plus `inert`, not just transparent. Its `CHAPTERS`
list (`chapters.ts`, React-free so a spec can import it) must name every section
id in page order; `tests/functional/marketing-scenes.spec.ts` fails when it does
not.

## Scenes: one entity, scrubbed by scroll, no JavaScript

Every page tells part of one story: **Meridian Supply** (E-1042), the company
the home story already follows, shown at a different moment on each page.
Its records, golden record and evidence live once in `scene/meridian.ts`, and
every score a scene prints comes from `scene/score.ts`, so a number on one
page cannot contradict the bars drawn on another. The owner chose the grammar
(2026-11-26, epic #1253): below the fold, scenes are **pinned and scrubbed by
scroll position**; the hero plays itself; new scenes stay grayscale (the
audit's red/green stays the audit's).

`scene/scroll-scene.tsx` is the primitive: a tall track with a view timeline,
a sticky `aria-hidden` stage, and the beats as ordinary copy that scrolls past.
It is a server component and ships no script. What breaks it, each found by
probing rather than reading:

- **The global reduced-motion clamp does not stop a scrubbed animation.** It
  shortens `animation-duration`, and a scroll timeline ignores duration —
  measured: the clamp left a probe at 250 of 1,000px at a quarter of the range
  in Chromium. So scene.css puts pinning and scrubbing *inside*
  `prefers-reduced-motion: no-preference` and a view-timeline `@supports`; the
  unconditioned rules are the static poster layout that reduced motion, the
  axe sweep and the baselines all see.
- **Never the `animation` shorthand on a scrubbed element.** It resets
  `animation-timeline`. Mark the element `data-scrub` and give it only an
  `animation-name`; its normal styles are the payoff, the keyframes the states
  before it.
- **Wire a scene's timeline with `:where()`, never `:is()`.** A stage that
  places its parts with per-element `animation-range` (one keyframe set, many
  ranges) needs the shared wiring rule to lose to those ranges. `:is()` takes
  its most specific argument's weight, and it beat every `[data-step]` range:
  each field ran over the whole scroll, and only a filmed frame showed it.
- **A probe page needs `<meta name="viewport">`.** Without it Playwright's
  WebKit reports support and never moves the element; with it, WebKit scrubs
  like Chromium. Next adds the tag to every real page.
- **A scrub can be parked mid-animation**, so only `--foreground` text may take
  opacity; muted text moves instead. Losing values are struck through, never
  dimmed.
- **WebKit pays per keyframe on every scroll frame** (#1220): a handful per
  element, transform and opacity only.
- **A parked frame must read on its own.** Give each moment the frame to itself
  (the evidence leaves before the decision arrives); two half-faded layers over
  each other is the frame a reader stops on. The same goes for things that
  overlap while they move: /product's problem figure deals its records out
  from under one that is already opaque, instead of fading them in as they
  slide apart. Panels that replace each other hold for most of their beat and
  cut in about 2% of the scroll; fades that took most of the beat parked a
  blank window between steps.
- **An unpinned entry figure is only halfway when it is centred.** Its view
  timeline runs from entering at the bottom (`cover` 0%) to leaving at the top
  (100%), so a figure in the middle of the screen sits at 50%. Finish by about
  46%, or a reader who stops with it centred sees it unfinished: the
  golden-records way-back figure read 0.14 opacity there.
- **A figure in the first screen plays itself instead.** A scroll timeline
  there is already most of the way through at load: /pricing's
  records-to-one-total figure starts 496px down a 900px desktop screen, so it
  runs once on first view through `scene/scene-playback.tsx`, with Replay, as
  the hero does.
- **`--story-container-surface` is translucent.** A fill mixed from it is
  still see-through; mix from an opaque base (the surface over
  `--background`) when shapes must hide each other.
- **A phone's pinned stage is short** (about 290px on the iPhone 15 profile).
  Each stage compacts itself under `@container (max-height: 520px)`: fewer
  fields, one-line evidence rows, a rule reduced to its verdict, and the
  moments' offsets in custom properties so they can be respaced, not squeezed.
- **The CSS minifier drops what it cannot parse, silently.** A `translate`
  whose `calc()` held `min(…cqw)` was simply absent from the built stylesheet;
  route such lengths through a custom property, and check the built CSS when a
  part sits where its transform says it should not.

Home's `#dedupe` chapter is the set piece (`scene/dedupe-stage.tsx`). The
trail's step cards stay in normal flow as the scene's beats, because
`TrailDiagram` and `TrailBridges` measure them; the close-up pins beside them.
Each card's `scroll-margin-top` centres it in the band it is read in, and
`glideTo` and the chapter button honour that margin, so `CHAPTERS` can list the
beats and each glide lands on the frame drawn for it. The trail now spans
several screens and an off-screen wire is paused by design: a spec that reads a
dedupe packet has to bring its wire into view first
(`showFirstDedupeWire` in `marketing-interactions.spec.ts`).

Home's `#audit` chapter (06) is a film the visitor takes part in, not a scrub.
`scene/audit-desk.tsx` draws it, `demo/audit-story.tsx` times it, and
`scene/audit-model.ts` holds every fact on it. It is click-driven because a
scroll position cannot wait for a decision.

- **How it plays.** It starts at 40% seen and plays each moment on one WAAPI
  clock (`.story-audit-clock`); pausing, an off-screen section and a hidden tab
  all hold that clock.
- **Its two decisions:**
  - A pair in the review band: Match leaves an always-match rule and Not a
    match a never-match rule, as `resolve_review` does.
  - A member that joined above auto-join and does not belong: select it and
    unmerge, which writes one never-match rule against a kept member, as
    `unmerge_entity` does.
- **Controls.** "Play it for me" makes the usual choices, and the step rail
  jumps, filling in any decision it skips.
- **Layout.** Its four shots share one grid cell, so the window never changes
  height as it cuts; TrailBridges measures the stage. Inactive shots are
  `inert` and `aria-hidden`.
- **The poster** is the end of the usual path. Reduced motion, no JavaScript
  and the baseline all show it.
- **Honesty.** Never claim the product flagged the record by itself: the person
  auditing finds it, and `marketing-audit-model.test.ts` holds the status copy
  to that.

The hero's scene is the self-playing exception, and on phones it sits after the
foot: the iPhone 15 profile shows 659px, and anything above the foot pushed
"Explore the flow" below it. `scene-playback.tsx` holds a scene that is off
screen at hydration until it is first seen, and "seen" means 40% of it on
screen: a solutions figure that started 19px above an iPhone 15's bottom edge
played out unwatched while any sliver counted. The same island drives
`/pricing`'s records figure and `scene/resolution-figure.tsx`, which every
solutions, role, industry and /about page renders from its content data's
`scene` (people and companies only, fictional, scanned by
`solutions-pages.test.tsx`).

Pages cut into each other through React 19.3's `<ViewTransition>` (`cut.tsx`):
page-kit's `Page` wraps every content page, and a page that does not use `Page`
wraps itself in a two-line default export rather than re-indenting a large file
other sessions edit. The layout cannot host it: a layout persists across navigations
and never enters or exits. The global clamp matches `*`, which does not reach
`::view-transition-*`, so `cut.css` zeroes those under reduced motion itself.

`/docs` is a static workflow guide and operation reference. Its committed
`operations.json` is generated from the MCP route manifest and response-envelope
contract by `frontend/scripts/gen-developer-reference.mjs`; the unit test checks
drift. Keep examples grounded in handlers and identify contract gaps. Its walkthrough
(`docs/api-storyboard.tsx`) is a scene that types the overview's six steps as
HTTP exchanges; the values it shows live in `docs/storyboard.ts`, and
`marketing-api-storyboard.test.ts` holds every method, path and field there to
`openapi.public.json`. The article's element styles (`ol` numbering, `li`
margins) reach into any scene placed in it, which is why `docs.css` resets them
under `.docs-article .scene`.

`/docs/api` is the public API reference, rendered in-house from
`openapi.public.json` by `src/lib/docs/openapi-reference.ts` (pure, unit-tested
against the real document) and `components/marketing/docs/api-reference.tsx`.
Two owner decisions on #867 shape it, and both are deliberate:
- No stock viewer (Swagger UI, Redoc, Scalar), because each colour-codes HTTP
  methods.
- No live "try it". The first call is a copyable `curl` built from
  `$ER_API_KEY` and `$ER_API_ORIGIN`, so the page never holds a customer
  credential.

`GET /api/openapi` serves the same document signed-out, listed in the proxy's
`PUBLIC_PATHS` and the authz gate's `UNAUTHENTICATED_DIRS` with that reason.

Conversion CTAs point at `PORTAL_ENTRY_PATH`. Story links navigate to their
sections and Docs links to `/docs`. Small client components own demonstration
state and copy buttons; the page and layout remain static server components.

Reading `cookies()` to vary the label would make the page dynamic; doing it in
the browser costs a round trip and flashes the wrong label on the one page where
first paint matters most. Interactive demos do not read sessions.

Copy lives in `src/components/marketing/content.ts`, outside `src/lib/**` on
purpose: the coverage floor measures that tree with Vitest's `all`, so
presentation data there reports 0% and spends the budget.

## Gate obligations

- **Add the route to the axe gate's `SCREENS`** with `anonymous: true` — the
  sweep logs in before every screen otherwise.
- **Scope visual baselines to a section**, never `fullPage`, and regenerate with
  `make frontend-baselines`. The off-Linux `ignoreSnapshots` false green applies
  here like anywhere else; the frontend-testing skill is the source of truth for
  that trap and the rest of them.
- **Keep the mobile layout a subset of desktop.** App screens are swept on
  desktop only, on the grounds that their desktop DOM is a superset; marketing
  screens (`anonymous: true`) are also swept on both phone projects, which
  checks the phone composition. A nav drawer that renders different markup
  below `lg` still breaks the premise, which is why the header is a plain row at
  every width, and a scene keeps one DOM for both widths.

## Not yet reachable from the internet

The ALB is `scheme: internal` and the Cloudflare records are DNS-only, so this
surface is complete and fully tested but **unpublished**. Two decisions are
open: the production edge topology, and whether marketing lives at the apex with
the portal at `app.` — which would need host-based routing in `src/proxy.ts` —
or both share one hostname, which needs nothing. See the infra-platform skill.

## Common workflows

**Adding a page.** Create it under `(marketing)/`, add
`export const dynamic = "force-static"`, add its path to `MARKETING_PATHS`
(which feeds the proxy and the sitemap at once), **link it from
`site-links.ts` by hand** because no nav derives from that list, register
it in the axe gate with `anonymous: true`, and confirm `next build` prints `○`
beside it.

**Changing copy.** Edit `content.ts`. Then regenerate any baseline whose section
you touched, in the container.

**Changing where the CTA goes.** Change `PORTAL_ENTRY_PATH`. Check it is still
absent from `MARKETING_PUBLIC_PATHS`, or the proxy stops bouncing anonymous
visitors; the unit test asserts exactly that.

**A crawler reports a redirect.** It is almost always a missing
`PUBLIC_PATHS` entry, or a metadata file that drifted into the route group.
