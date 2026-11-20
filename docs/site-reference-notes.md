# Reference-site notes

## Crawl status: blocked, not attempted further

On 2026-11-20 a single request for the reference site's `robots.txt`, sent with
a user agent naming itself a research bot, returned **HTTP 403 Forbidden**. An
earlier fetch of its pricing page returned 403 as well. The brief says to stop
if the site blocks us, so no page was crawled, no sitemap was read, and nothing
was retried with a different user agent. This file therefore holds no notes
about that site's pages, sections, forms or FAQ topics.

## Gap analysis, without the crawl

Business-software marketing sites commonly have the page types below. The
column on the right is our own site as of commit `fee431b6` (Home, Docs and
Pricing) and what `docs/feature-inventory.md` can support.

| Page type                             | Ours today                            | Can we fill it with VERIFIED features?                                                      |
| ------------------------------------- | ------------------------------------- | ------------------------------------------------------------------------------------------- |
| Product overview                      | No (Home is a story, not an overview) | Yes                                                                                         |
| Feature pages                         | No                                    | Yes: matching, review, golden records                                                       |
| How it works                          | Partly (Home's story, Docs' workflow) | Yes, from CSV upload to an exported golden record                                           |
| Solutions by role                     | No                                    | Yes, framed by the verified features each role uses                                         |
| Integrations                          | No                                    | Only CSV in, CSV out and webhooks. No direct Salesforce connection is customer-reachable    |
| Pricing                               | Yes (`/pricing`)                      | Already built                                                                               |
| Security and data handling            | No                                    | Partly: isolation, encryption, roles, audit log. No certification, no data-deletion process |
| FAQ                                   | On Home only                          | Yes                                                                                         |
| Resources / articles                  | No                                    | Outlines only; articles need writing                                                        |
| About                                 | No                                    | Placeholders: no company facts are in the repo                                              |
| Contact / demo request                | No (a `mailto:` link on Pricing)      | Yes, as a form with no backend yet                                                          |
| Legal (privacy, terms)                | No                                    | Placeholders only: legal text is not ours to invent                                         |
| Customer stories, testimonials, logos | No                                    | No: none supplied. Left out rather than faked                                               |
| Comparison pages                      | No                                    | Out of scope: the brief forbids competitor comparisons                                      |
