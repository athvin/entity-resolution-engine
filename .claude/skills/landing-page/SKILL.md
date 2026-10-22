---
name: landing-page
description: Explains this repo's public marketing surface — the `(marketing)` route group that prerenders statically inside the same app that serves the BFF, why `output: 'export'` and `ensureStatic` are both unavailable here, the metadata routes that must sit at the app root to get their file extension, the PUBLIC_PATHS entries without which crawlers are redirected to login, how a public page gets an absolute origin without `NEXT_PUBLIC_*`, the JSON-LD approach that needs no HTML-injection sink, and the fact that the surface inherits the grayscale rule. Use when editing or adding a marketing page or its copy, touching robots, sitemap, canonical URLs, Open Graph or structured data, changing where the portal CTA points, or when a crawler sees a redirect instead of a page. For tokens, type scale and copy voice see frontend-design-system; for the proxy and the browser/server line see frontend-bff; for the axe gate and visual baselines see frontend-testing.
---

# The marketing surface

The public landing page is a `(marketing)` route group **inside the main Next
app**, statically prerendered. It is not a separate project, and the reasons are
worth knowing before anyone proposes making it one:

> One design system, one gate, one deploy. A second project would mean a second
> copy of every chroma-0 token, a second five-tool gate plus Playwright matrix
> plus audit allowlist, and a hosting target that **does not exist** — the ALB
> is internal and there is no CDN in the Terraform.

`output: 'export'` is unavailable regardless: the same app serves the BFF route
handlers and needs a server.

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
`/login`. Both come from `MARKETING_PUBLIC_PATHS` in `src/lib/site/routes.ts`,
which the proxy spreads in — add a page there and all three consumers follow.

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
accessor for a single optional variable. Anything needing a *required* value
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
child is therefore byte-identical for valid payloads and *safer* for hostile
ones.

That matters because `.semgrep.yml`'s `no-html-injection-sink` bans that sink at
ERROR with zero occurrences in the tree. Following the usual advice would have
taken the first exemption for no gain.

Both properties are pinned by `tests/component/structured-data.test.tsx`,
because they are React implementation details whose failure mode is invisible —
the page still renders, and only crawlers break.

## It inherits the grayscale rule

The ESLint palette rule is repo-wide under `frontend/`, and the design doc calls
the constraint standing "on every surface". Do not carve an exception: it would
need either a second config block (which also exempts those paths from the other
rules in that array) or hue-bearing variables in the token file every app
component reads.

The positive case is stronger anyway. In a category of indigo-gradient SaaS
sites, grayscale *is* the distinctive identity, and a landing page that opens on
a gradient before handing the visitor a black-and-white app has said the product
is less confident than its marketing.

So: a 6–8× type-scale ratio, weight contrast within a line,
`bg-foreground text-background` inversion spent **once per viewport** on the
primary CTA, the three surface planes, `py-20 lg:py-28` rhythm, and one honest
product image. The channel-by-channel table is in the frontend-design-system
skill.

## Section order, and the one CTA

The page has one job: send a qualified visitor into the portal. The shipped
order is hero → problem → how it works → proof → FAQ → closing CTA, and it is a
sequence rather than a menu — the numbered markers in "how it works" are
legitimate because resolve/review/reconcile genuinely is one.

**Every CTA points at `PORTAL_ENTRY_PATH`, and nothing else.** That route is the
signed-in dispatcher and is deliberately *not* public, so the proxy's cookie
check turns an anonymous visitor away before its database query runs and returns
them after login. The result is one CTA that is correct for both states with no
session read — and therefore **no client JavaScript on the landing page**, which
is the property to protect when adding anything.

Reading `cookies()` to vary the label would make the page dynamic; doing it in
the browser costs a round trip and flashes the wrong label on the one page where
first paint matters most.

Copy lives in `src/components/marketing/content.ts`, outside `src/lib/**` on
purpose: the coverage floor measures that tree with Vitest's `all`, so
presentation data there reports 0% and spends the budget.

## Gate obligations

- **Add the route to the axe gate's `SCREENS`** with `anonymous: true` — the
  sweep logs in before every screen otherwise.
- **Scope visual baselines to a section**, never `fullPage`, and regenerate with
  `make frontend-baselines`. `ignoreSnapshots` is true off Linux, so a macOS run
  reports green having compared nothing.
- **Keep the mobile layout a subset of desktop.** The axe sweep is desktop-only
  on the stated grounds that the desktop DOM is a superset; a nav drawer that
  renders different markup below `lg` breaks that premise and earns a mobile
  pass. This is why the header is a plain row at every width.

## Not yet reachable from the internet

The ALB is `scheme: internal` and the Cloudflare records are DNS-only, so this
surface is complete and fully tested but **unpublished**. Two decisions are
open: the production edge topology, and whether marketing lives at the apex with
the portal at `app.` — which would need host-based routing in `src/proxy.ts` —
or both share one hostname, which needs nothing. See the infra-platform skill.

## Common workflows

**Adding a page.** Create it under `(marketing)/`, add
`export const dynamic = "force-static"`, add its path to `MARKETING_PATHS`
(which feeds the proxy, the sitemap and the nav at once), register it in the axe
gate with `anonymous: true`, and confirm `next build` prints `○` beside it.

**Changing copy.** Edit `content.ts`. Then regenerate any baseline whose section
you touched, in the container.

**Changing where the CTA goes.** Change `PORTAL_ENTRY_PATH`. Check it is still
absent from `MARKETING_PUBLIC_PATHS`, or the proxy stops bouncing anonymous
visitors; the unit test asserts exactly that.

**A crawler reports a redirect.** It is almost always a missing
`PUBLIC_PATHS` entry, or a metadata file that drifted into the route group.
