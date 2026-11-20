# Marketing page plan

**Status: not approved by the owner.** The owner asked for best judgement, so
this plan follows the brief's fallback: describe only VERIFIED items from
`docs/feature-inventory.md`.

## Positioning decision

The audience is Salesforce admins and revenue operations. Today a customer
cannot connect a Salesforce org to DupeZero, and DupeZero cannot write to one.
The pages therefore describe the workflow that works today:

1. Export records from Salesforce, or any other system, as CSV.
2. Upload and resolve them in DupeZero.
3. Review the uncertain pairs.
4. Export the golden records as CSV, or receive webhooks.

There is no self-serve sign-up (accounts come from invitations), so the
primary call to action is **Request a demo** (`/contact`), not a free trial.
The pages make no promise of a direct connection, writeback, or merging
inside Salesforce, and use no "coming soon" wording. Prices stay on `/pricing`.
New pages do not repeat "every feature in every tier", because `/pricing`
already says it.

## Pages

Pages marked "unlisted" are public but `noindex` and left out of the sitemap,
because their content is a placeholder.

| Route                                       | Purpose                                                                                                        | Visitor            | Primary CTA          | Verified features                                                                            | Outline                                                                                                                                                 |
| ------------------------------------------- | -------------------------------------------------------------------------------------------------------------- | ------------------ | -------------------- | -------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `/product`                                  | Overview                                                                                                       | Anyone evaluating  | Request a demo       | Datasets, CSV upload, matching, review, golden records, export, webhooks                     | Problem → what DupeZero does (4 cards linking to feature pages) → workflow summary → what it does not do yet → CTA                                      |
| `/features/matching`                        | How records are compared                                                                                       | Admin, data team   | See how review works | Value cleanup, comparison types, hard rules, trained model, rare-value weighting, thresholds | Problem → cleanup → comparisons → rules → learning → thresholds → limits → CTA                                                                          |
| `/features/review`                          | Human review                                                                                                   | Admin, sales ops   | Request a demo       | Review queue, match evidence, AI recommendation, manual merge, unmerge, always/never rules   | Problem → queue → evidence → AI suggestion → corrections → CTA                                                                                          |
| `/features/golden-records`                  | The resulting record                                                                                           | Ops, data team     | See integrations     | Golden records, field rules, source trace, history, export                                   | Problem → one record per person/company → which value wins → trace → export → CTA                                                                       |
| `/solutions`                                | Overview: use cases, roles, industries                                                                         | Each role          | Request a demo       | Same features, framed per reader                                                             | Use cases → by role (4 cards) → by industry (7 cards) → CTA                                                                                             |
| `/solutions/roles/{slug}` ×4                | Salesforce admins, revenue operations, marketing, IT and security                                              | That role          | Request a demo       | Matching, review, golden records, security, per role                                         | Where duplicates hurt → how DupeZero helps (each linked to its feature page) → use cases → the limit that matters → CTA                                 |
| `/solutions/industries/{slug}` ×7           | Technology, nonprofit, financial services, healthcare and life sciences, education, real estate, manufacturing | That industry      | Request a demo       | Same, framed per industry                                                                    | Same outline. Each states that no compliance certification is held; healthcare, financial services and education carry a TODO for what data may be held |
| `/integrations`                             | What connects today                                                                                            | Admin, data team   | Read the docs        | CSV upload, CSV export, signed webhooks                                                      | Today → Salesforce via CSV → webhooks → custom needs → CTA                                                                                              |
| `/security`                                 | Data handling                                                                                                  | Security reviewer  | Contact              | Separate data per customer, encryption, roles, audit log, password handling, signed webhooks | Practices → access → audit → what we do not claim (TODOs) → contact                                                                                     |
| `/faq`                                      | Answers                                                                                                        | Anyone             | Contact              | Inventory-backed answers only                                                                | Q&A list, FAQPage JSON-LD of the same visible text                                                                                                      |
| `/blog` and `/blog/{slug}` ×3               | Blog posts                                                                                                     | Admins, ops        | Request a demo       | Review evidence, matching, savings calculator                                                | Index → posts dated on publication                                                                                                                      |
| `/resources`                                | Article index                                                                                                  | Admins researching | Read an outline      | —                                                                                            | 3 article cards                                                                                                                                         |
| `/resources/{slug}` ×3 (unlisted)           | Article outlines                                                                                               | —                  | Back to resources    | —                                                                                            | Draft banner, outline headings, TODO                                                                                                                    |
| `/about`                                    | Company                                                                                                        | Buyers             | Contact              | —                                                                                            | Mission in our words, TODO placeholders for company facts                                                                                               |
| `/contact`                                  | Contact and demo request                                                                                       | Buyers             | Submit               | —                                                                                            | Form: name, work email, company, reason, message. Hands off to the visitor's email client until a backend exists                                        |
| `/legal/privacy`, `/legal/terms` (unlisted) | Legal placeholders                                                                                             | —                  | —                    | —                                                                                            | TODO banner; text must come from counsel                                                                                                                |

The three articles:

- What causes duplicate records in Salesforce
- How to prepare before a bulk merge
- Matching rules, explained

## Navigation and footer

- **Header:** Product, Solutions (dropdown: by role, by industry), Resources (dropdown: Docs, Blog, ROI calculator), Pricing, Contact. How it works is a section of `/product`; `/how-it-works` redirects there. Below 640px these
  move into a Menu disclosure, because five links do not fit 320px. This adds
  the site's first mobile menu.
- **Footer:** link columns.
  - Product: Overview, Matching, Review, Golden records, Integrations
  - Solutions
  - Resources: Docs, Resources, FAQ, Pricing
  - Company: About, Contact, Security
  - Legal: Privacy, Terms
- **Active state:** the same as today, `aria-current="page"` plus weight and
  underline.

## Left out, and why

- Customer quotes, logos, statistics, awards and case studies: none supplied.
- Direct Salesforce connection, writeback, AppExchange, scheduling of
  Salesforce pulls: not customer-reachable or not built.
- S3 auto-ingest, the API, the embeddable widget: need staff setup.
- SFTP, Google Sheets, Shopify: not built.
- In code but not marketed (protect/suppress, bulk edit, conflict check,
  campaigns, assistant, data profile, run preview): awaiting the owner's
  confirmation.
