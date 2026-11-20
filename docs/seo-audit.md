# SEO and AI-crawler audit

Baseline taken 2026-11-20 against a production build (`next build` + `next
start`) of `main` at `ba99d0b4`, and against the live site
(`https://dupezero.com`, which runs `main`). "After" is the `feat/seo` branch.
Nothing here promises a ranking or a citation; it records what crawlers
receive.

## Baseline: indexed-intended URLs (32, from `MARKETING_PATHS`)

`/`, `/product`, `/features/{matching,review,golden-records}`, `/solutions`,
`/solutions/roles/{salesforce-admins,revenue-operations,marketing,it-and-security}`,
`/solutions/industries/{technology,nonprofit,financial-services,healthcare-and-life-sciences,education,real-estate,manufacturing}`,
`/integrations`, `/security`, `/pricing`, `/faq`, `/resources`,
`/resources/{causes-of-salesforce-duplicates,preparing-for-a-bulk-merge,matching-rules-explained}`,
`/blog`, `/blog/{estimating-what-duplicates-cost,leads-contacts-and-the-person-in-between,every-match-should-come-with-its-reasons}`,
`/about`, `/contact`, `/docs`. Unlisted and `noindex`: `/legal/privacy`, `/legal/terms`.

## 1. Rendering

**Every marketing page is prerendered static HTML** (`○`/`●` in the build
output, `force-static`). Every page returns its full text, its one H1 and its
headings in the initial HTML, with no JavaScript needed. No page depends on
client-side rendering for key content.

- `/pricing`: the interactive calculator is a client component, but its server
  render already includes a real `<table>` of every tier, record cap and annual
  price for each billing term, and the records-only rule in the intro and
  footnotes. No change needed.
- `/` (home): the story animations are client-side; all copy is in the HTML.

## 2. Crawlability

| Check          | Baseline                                                                                          | After                                                                                                                                                              |
| -------------- | ------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| robots.txt     | One `*` group; private paths disallowed; sitemap referenced                                       | Separate groups for search/retrieval and AI-training bots (section 5)                                                                                              |
| sitemap.xml    | All 32 indexed URLs, absolute, canonical; **no `lastmod`**                                        | `lastmod` from each page's content date (`PAGE_UPDATED`, blog dates)                                                                                               |
| Canonical tags | Self-referencing on all 32                                                                        | Unchanged                                                                                                                                                          |
| `noindex`      | Only the two legal pages (intended)                                                               | Unchanged; 404 page `noindex` (Next adds it)                                                                                                                       |
| HTTP → HTTPS   | 301                                                                                               | Unchanged                                                                                                                                                          |
| `www` → apex   | 301, but `Location: https://dupezero.com:443/` carries an explicit `:443`                         | Not changed in code: the redirect is the load balancer's (`infra/terraform/envs/prod/edge.tf`); harmless, but a cleaner `Location` is an infra edit                |
| Trailing slash | `/pricing/` → 308 → `/pricing`                                                                    | Unchanged (consistent policy: no trailing slash)                                                                                                                   |
| Query strings  | `/pricing?utm_source=x` → 200, canonical `/pricing`                                               | Unchanged (canonical handles it)                                                                                                                                   |
| Case           | `/PRICING` → 307 to `/login`                                                                      | See "Unknown URLs"                                                                                                                                                 |
| HSTS           | `max-age=63072000; includeSubDomains`                                                             | Unchanged                                                                                                                                                          |
| Unknown URLs   | `/nonexistent` → **307 to `/login`** (soft 404); `/blog/nope` → 404 with Next's bare default page | New `app/not-found.tsx`: real 404, `noindex`, marketing header/footer, links to key pages. Single-segment unknown paths still go to `/login` by design (see below) |

**Unknown URLs, by design.** The app has a catch-all `/[org]` route, so
`/anything` may be a customer workspace. The proxy must send a signed-out
visitor there to login, and it cannot tell a typo from a real workspace without
a session. `/login` is disallowed in robots.txt, so crawlers do not index the
redirect target. Multi-segment unknown paths and every `notFound()` now get the
proper 404 page.

## 3. Per-page metadata (baseline)

All 32 pages: one H1, a logical H2 structure, a unique title and description,
a self-canonical, and `og:title`/`og:description`/`og:url` plus a Twitter card.

**Defect found: no page had an `og:image` or `twitter:image`.** A page-level
`openGraph` object replaces the layout's instead of merging with it, so the
root `opengraph-image.png` never reached a page. **Fixed:** `pageMetadata`,
home, pricing and docs now set the 1200×630 share image explicitly.

**Proposed, not made (copy changes need your approval):** most titles are
short of the 50 to 60 character guideline, for example "Pricing · DupeZero"
(18), "FAQ · DupeZero" (14), "Product · DupeZero" (18). Suggested titles:

| Page                 | Current                                          | Proposed                                                        |
| -------------------- | ------------------------------------------------ | --------------------------------------------------------------- |
| `/product`           | Product · DupeZero                               | Salesforce duplicate records, resolved with evidence · DupeZero |
| `/pricing`           | Pricing · DupeZero                               | Deduplication pricing based on total records · DupeZero         |
| `/faq`               | FAQ · DupeZero                                   | Salesforce deduplication questions answered · DupeZero          |
| `/features/matching` | Matching · DupeZero                              | How DupeZero matches duplicate records · DupeZero               |
| `/features/review`   | Review · DupeZero                                | Review duplicate matches before they merge · DupeZero           |
| `/blog`              | Blog · DupeZero                                  | Notes on cleaner Salesforce and CRM data · DupeZero             |
| `/contact`           | Request a demo or ask a question · DupeZero (36) | Fine as is                                                      |

Descriptions are 105 to 190 characters; the shortest (`/contact` 86,
`/solutions/industries/education` 105) could be lengthened toward 140.

## 4. Structured data

| Where              | Baseline                                                                 | After                                                                                                                                            |
| ------------------ | ------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| `/`                | Organization + SoftwareApplication (description not visible on the page) | Organization (name, URL, logo, contact page) + WebSite. SoftwareApplication moved to where its facts are visible                                 |
| `/product`         | none                                                                     | SoftwareApplication; `description` is the page's own lead sentence; no `offers` (no prices on the page)                                          |
| `/pricing`         | none                                                                     | SoftwareApplication with `AggregateOffer` (USD, low $1,875, high $7,500, 3 offers), built from `PRICING_CONFIG`, so it matches the visible table |
| `/faq`             | FAQPage (built from the same array as the visible list)                  | Unchanged                                                                                                                                        |
| `/blog/*`          | none                                                                     | BlogPosting: headline, description, datePublished/dateModified, author "DupeZero team" (the visible byline)                                      |
| `/resources/*`     | none                                                                     | Article, plus a visible "Last updated" date                                                                                                      |
| All interior pages | none                                                                     | BreadcrumbList matching visible breadcrumbs                                                                                                      |

No `aggregateRating`, `review`, award or certification anywhere; a test fails
if one appears. `sameAs` is omitted until real profiles exist.

**Validation.** The Schema.org validator and Google's Rich Results Test are web
services that need the deployed URLs; run them on the live pages after deploy.
Offline, `tests/component/seo.test.tsx` parses every block, checks types and
required fields, and compares breadcrumbs, product description and pricing
offer with what the page renders.

## 5. Crawler access

**Live check (2026-11-20).** `/` and `/docs` requested as Googlebot, Bingbot,
OAI-SearchBot, ChatGPT-User, PerplexityBot, Claude-SearchBot, Claude-User,
GPTBot, ClaudeBot, CCBot, Meta-ExternalAgent and Applebot: **all 200**, with
the same full HTML a browser gets (65 KB and 731 KB) and one H1. No 403, 429,
challenge or empty response. These requests came from this machine's IP, not
the bots' published ranges, so an IP-based block would not show here.

**WAF (`infra/terraform/envs/prod/edge.tf`).** AWS WAF on the load balancer:
rate limits only on three unauthenticated write paths (`/api/auth/login`,
`/api/auth/password-reset`, `/api/invites`); `AWSManagedRulesAmazonIpReputationList`
(blocks); the Common and Known-Bad-Inputs groups in count mode. **No Bot
Control rule group and no CDN**, so nothing in the repository blocks legitimate
crawlers. If someone later adds `AWSManagedRulesBotControlRuleSet`, set its
`CategorySearchEngine` and `CategoryAI` (or the specific verified-bot labels)
to Allow, or retrieval bots will be challenged.

**robots.txt policy (after).** Three groups: `*`; search and answer-retrieval
bots (Googlebot, Bingbot, DuckDuckBot, Applebot, OAI-SearchBot, ChatGPT-User,
PerplexityBot, Perplexity-User, Claude-SearchBot, Claude-User); AI-training
crawlers (GPTBot, ClaudeBot, Google-Extended, CCBot, Applebot-Extended,
Meta-ExternalAgent, Bytespider). Each repeats the private paths. No
`Crawl-delay`. Training bots are **allowed**, by the owner's decision of
2026-11-20, controlled by `ALLOW_AI_TRAINING` at the top of `src/app/robots.ts`.

**Access logs (not available today).** Neither the load balancer nor the WAF
logs requests, and the app does not record user agents, so there is no way yet
to see which crawlers visit. To enable it (an infrastructure change, not made
here): in `infra/terraform/envs/prod/edge.tf`, add `access_logs { bucket = …,
enabled = true }` to `aws_lb.edge` with a bucket policy for the ELB log
delivery principal. Then, in Athena over the ALB log table:

```sql
SELECT user_agent, request_url, elb_status_code, count(*) AS hits
FROM alb_logs
WHERE time > date_format(current_date - interval '30' day, '%Y-%m-%d')
  AND regexp_like(user_agent, '(?i)googlebot|bingbot|gptbot|oai-searchbot|chatgpt-user|claudebot|claude-searchbot|claude-user|perplexity|ccbot|google-extended|applebot|meta-externalagent|bytespider')
GROUP BY 1, 2, 3
ORDER BY hits DESC;
```

## 6. Performance (baseline)

**Lighthouse could not run.** There is no working browser in this environment
(system libraries missing; installing them needs approval), and the keyless
PageSpeed Insights API had exhausted its shared daily quota. Run
<https://pagespeed.web.dev/> on `/`, `/product`, `/pricing` and `/docs` (mobile)
and record the numbers here.

Measured from the build instead (gzip level 6):

| Page       | JavaScript                            | HTML                    | CSS        |
| ---------- | ------------------------------------- | ----------------------- | ---------- |
| `/`        | 15 files, 932 KB raw, **278 KB gzip** | 102 KB (18 KB gzip)     | 22 KB gzip |
| `/product` | 12 files, 843 KB raw, **247 KB gzip** | 69 KB (12 KB gzip)      | 17 KB gzip |
| `/pricing` | 13 files, 875 KB, 257 KB gzip         | 60 KB (10 KB gzip)      | 17 KB gzip |
| `/blog`    | 12 files, 843 KB, 247 KB gzip         | 47 KB (7 KB gzip)       | 17 KB gzip |
| `/docs`    | 13 files, 849 KB, 250 KB gzip         | **737 KB (83 KB gzip)** | 16 KB gzip |

- **Every page ships ~247 KB gzip of JavaScript**, most of it the signed-in
  app's providers (React Query, the toaster, BFF error handling,
  OpenTelemetry), because the root layout wraps the whole site in them. Being
  addressed in a separate pull request.
- `/docs` HTML is 737 KB (the full API reference inline). Acceptable for a
  docs page; worth splitting the reference out if it grows.
- Images: the logo SVGs total 56 KB; the 1200×630 share PNG is 64 KB; product
  screenshots (130 to 170 KB PNG) are lazy-loaded with width and height set,
  and served by `next/image`.
- No web fonts are loaded from third parties; no analytics or tag-manager
  scripts exist.

## 7. Internal linking

Every indexed page is linked from the header or footer, except blog posts and
guides, which are linked from `/blog` and `/resources` (both in the header's
Resources menu). Every page is within two clicks of `/`; none is orphaned.
Breadcrumbs now add a link up from every interior page.

## Awaiting the owner

- Title changes in section 3 (copy).
- Search Console and Bing Webmaster Tools: verify by **DNS TXT record** on
  `dupezero.com` (only you can obtain the token), submit
  `https://dupezero.com/sitemap.xml` in both, and import the Search Console
  property into Bing to save a step. IndexNow is not set up: it needs a key
  file and a ping on each deploy, which is not worth it for a site that
  changes this rarely.
- Analytics: none exist, so there is nothing to attribute AI referral traffic
  in yet. Adding one is a new tracker and needs your decision.
- `llms.txt`: skipped by decision; weak evidence it is read.
- Access logs: the infrastructure change in section 5.
