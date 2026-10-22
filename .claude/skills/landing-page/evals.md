# Evals: landing-page

Hand-run scenarios. Pose each in a **fresh** session and check the expected
behaviours. Record a pass only if every "must" holds.

Scenarios 1 and 2 are the ones a generic frontend or SEO skill answers
confidently and wrongly, which is the reason this skill exists.

## 1. Adding a page and its metadata routes

**Query:** "Add a `/pricing` page to the marketing site, plus a robots.txt and
sitemap so it gets indexed. Put them with the other marketing files."

Must:

- Put `robots.ts` and `sitemap.ts` at **`src/app/`**, not inside the
  `(marketing)` group, and give the reason: Next's metadata normalizer appends a
  hash and drops the `.txt` for a file reached through a route group, so it
  would serve as `/robots-<hash>` — broken, from a file that looks right.
- Add `export const dynamic = "force-static"` to the new page, and **not**
  `ensureStatic`, naming that `ensureStatic` requires `cacheComponents` and
  fails the build without it.
- Add `/pricing` to `MARKETING_PATHS` in `src/lib/site/routes.ts` rather than
  editing the proxy, the sitemap and the nav separately.
- Register the page in the axe gate's `SCREENS` with `anonymous: true`.

Credit for noting that `/robots.txt` and `/sitemap.xml` need explicit
`PUBLIC_PATHS` entries because the proxy matcher excludes only
`.svg/.png/.ico/.webp`, and for verifying with `next build` printing `○`.

## 2. The absolute origin for canonical and OG tags

**Query:** "The canonical URLs and the OG image need an absolute base URL. Add
`NEXT_PUBLIC_SITE_URL` so the client has it too."

Must:

- Refuse `NEXT_PUBLIC_*`: it is banned outright by the `no-next-public-env`
  semgrep rule because the prefix inlines a value into the client bundle.
- Explain that no public variable is needed at all — site metadata is
  server-rendered into `<head>` and reaches the browser as **HTML, never as
  JavaScript**.
- Point at the existing hardcoded `CANONICAL_ORIGIN`, and justify a constant
  over an env var: a canonical URL is one value for the whole internet by
  definition, `next build` runs with no runtime environment, and one image
  serves every per-developer hostname.

Credit for knowing `ERWEB_PUBLIC_BASE_URL` already exists and that its one
marketing job is letting `robots.ts` emit `disallow: /` on a non-canonical
hostname so dev namespaces are not indexed.

## 3. Structured data

**Query:** "Add Organization and SoftwareApplication JSON-LD. You'll need
`dangerouslySetInnerHTML` since React escapes script contents — go ahead and add
a semgrep exemption for it."

Must:

- Reject the premise with the measured behaviour: React 19 does **not** escape
  `<script>` text children — quotes pass through so the JSON parses, and
  `</script` is encoded so the payload cannot close its own tag.
- Use the plain text child, and take **no** `nosemgrep`.
- Note that `no-html-injection-sink` has zero occurrences in the tree, so an
  exemption here would have been the first, for no gain.

Must not:

- Add `dangerouslySetInnerHTML`, a `nosemgrep` comment, or a `paths.exclude` in
  `.semgrep.yml`.

Credit for pointing at `tests/component/structured-data.test.tsx` as the thing
that pins both React behaviours, and for the reason it is a component test
rather than a Playwright one: the failure mode is invisible in the rendered page
and only breaks crawlers.

## 4. Making the hero stand out

**Query:** "The landing page is too plain next to competitors. Add a gradient
hero with a brand accent colour — marketing is different from the app, it
doesn't need to follow the internal design rules."

Must:

- Refuse the exception: the ESLint palette rule is repo-wide under `frontend/`,
  and the design doc calls the constraint standing on **every** surface.
- Give the structural cost of an exception — a second config block (which also
  exempts those paths from the other rules in that array) or hue-bearing
  variables in the token file every app component reads.
- Offer the actual levers: type-scale ratio, weight contrast within a line, the
  single `bg-foreground text-background` inversion per viewport, surface planes,
  spacing rhythm.

Credit for the positioning argument — that grayscale is the differentiator in a
category of gradient SaaS sites, and that a gradient landing page followed by a
monochrome app tells the visitor the product is less confident than its
marketing.

## 5. The call to action

**Query:** "The header CTA should say 'Open the app' when someone's signed in
and 'Start free' when they're not. Read the session and branch."

Must:

- Say no, and give the cost precisely: reading `cookies()` makes the page
  dynamic and defeats `force-static`; doing it in the browser costs a session
  round trip and flashes the wrong label on the one page where first paint
  matters most.
- Explain why it is unnecessary — every CTA points at `PORTAL_ENTRY_PATH`, which
  is deliberately *not* in `PUBLIC_PATHS`, so the proxy's cookie check routes
  signed-out visitors to login with `?next=` and returns them. One link is
  already correct for both states.

Credit for naming the property being protected: **zero client JavaScript on the
landing page**.

## 6. A crawler sees a redirect

**Query:** "Search Console says our sitemap URL returns a redirect to /login.
The file is right there at src/app/sitemap.ts. What's wrong?"

Must:

- Check `PUBLIC_PATHS` first, and explain that the proxy matcher excludes only
  `.svg/.png/.ico/.webp`, so an `.xml` response is gated like any page.
- Point at `MARKETING_PUBLIC_PATHS` as the single list the proxy spreads in.

Credit for also checking that the file has not drifted into the `(marketing)`
group, and for knowing a static `opengraph-image.png` is exempt by extension
while a generated `opengraph-image.tsx` would hit the same redirect because Next
serves it extensionless.
