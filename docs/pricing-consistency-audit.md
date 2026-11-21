# Pricing consistency audit: records only

**The rule.** Price depends only on total records, counted across every
system a customer connects. Not orgs, systems, connectors, integrations,
CRMs or users. Bespoke integration work for one customer's own system is a
separate professional-services quote, never in the calculator.

**Method.** `git grep -i` over the whole repository for: per org, per
Salesforce org, per organisation/organization, additional (production) org,
extra org, connector, per connector, per system, per integration, per CRM,
per seat, per user, seat, sandbox, Salesforce org, ADDITIONAL_ORG; then a
second pass over every pricing summary (`price|pricing|priced|tier|unlimited
users|record volume`) in site copy, metadata and the pricing code. Lock files,
Rust, entities and infrastructure were searched too; none touch pricing.

**Outcome.** Every proposed change is a wording change, or the removal of a
config value that only feeds a footnote (it never enters a price). So the
edits went ahead without a pause, as the brief allows.

## Conflicts, and the change made

| #   | File : line                                                            | Current text or logic                                                                                                              | Why it conflicts                                                       | Change                                                                                                                                              |
| --- | ---------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1   | `frontend/src/lib/pricing/config.ts:105`                               | `ADDITIONAL_ORG_RATE_BP: 5_000` (50%)                                                                                              | An extra-org charge. Used only by footnote #4; never in a price        | Removed                                                                                                                                             |
| 2   | `frontend/src/components/marketing/pricing/pricing-calculator.tsx:456` | Footnote "Additional production orgs are 50% of that org's tier price."                                                            | Per-org charge                                                         | Removed; footnotes rewritten to the rule                                                                                                            |
| 3   | `pricing-calculator.tsx:181`                                           | Label "Records across all scanned objects"                                                                                         | Does not say records are totalled across systems                       | "Total records across all connected systems", helper "Count records from every system you connect, added together."                                 |
| 4   | `pricing-calculator.tsx:220`                                           | Slider hint "Total across every object we scan, per org"                                                                           | Per org                                                                | "Total across every connected system"                                                                                                               |
| 5   | `pricing-calculator.tsx:446`                                           | "Your Salesforce sandbox org is always free."                                                                                      | Salesforce-only, org-shaped                                            | "Records in free sandbox or test environments do not count toward your tier."                                                                       |
| 6   | `pricing-calculator.tsx:448-461` (footnotes)                           | Unlimited users and features; 10% soft buffer, true-up at renewal; additional orgs                                                 | Incomplete and per-org                                                 | Footnotes state the full rule (see below)                                                                                                           |
| 7   | `pricing-calculator.tsx` (contact panels)                              | No mention of bespoke integrations                                                                                                 | Rule requires the custom-integration line                              | Added "Need an integration built for your own system? Contact us for a project quote." under the CTA, no price                                      |
| 8   | `frontend/src/components/marketing/content.ts:130`                     | Pricing intro "Choose by the records you scan, per Salesforce org, with unlimited users."                                          | Per Salesforce org                                                     | "Priced on the total records you connect, across every system. Every tier has every feature and unlimited users."                                   |
| 9   | `content.ts:132`                                                       | Meta/OG description "…for Salesforce deduplication… priced per org by record volume…"                                              | Per org; Salesforce-only                                               | "…priced only on the total records you connect across every system, with every feature and unlimited users on every tier…"                          |
| 10  | `frontend/src/components/marketing/pages/faq-content.ts:56`            | "Pricing is per Salesforce org, in tiers by the number of records included."                                                       | Per Salesforce org (also in FAQPage JSON-LD, built from the same text) | "Pricing is based on the total number of records you connect, across every system, in tiers…"; new Q&A on connectors and users                      |
| 11  | `frontend/src/components/marketing/interactive-story.tsx:151-155`      | Home: "Our pricing direction is per-run compute. Record volume, configuration, and the work a run performs shape that discussion…" | A different pricing model                                              | "Pricing is based on total records… never on systems, connectors or users." Link to /pricing                                                        |
| 12  | `frontend/src/app/(marketing)/docs/page.tsx:403-414`                   | Docs "Usage and pricing": "The current pricing direction is per-run compute… Record volume alone cannot predict runtime or price…" | A different pricing model                                              | Rewritten to the rule; the runtime caveat kept, separated from price                                                                                |
| 13  | `frontend/src/app/(marketing)/integrations/page.tsx`                   | No pricing statement                                                                                                               | Rule requires it                                                       | Added: standard connectors are included at no extra charge and priced by records only; setup is part of onboarding; bespoke work is a project quote |
| 14  | `frontend/src/lib/pricing/README.md:34`                                | "Footnote figures: `SOFT_BUFFER_BP`, `ADDITIONAL_ORG_RATE_BP`"                                                                     | Per-org config                                                         | Removed; "Pricing rule: records only" section added                                                                                                 |
| 15  | `docs/feature-inventory.md:155-156` (claims #7, #8)                    | Lists the sandbox and per-org claims as claims to fix                                                                              | Now fixed                                                              | Marked fixed                                                                                                                                        |
| 16  | `.claude/skills/landing-page/SKILL.md`                                 | "Home's usage section still describes per-run compute pricing"                                                                     | Stale after #11                                                        | Updated                                                                                                                                             |

## Not changed: legal and contract text (needs your review)

| File : line                                            | Text                                                                                   | Conflict           |
| ------------------------------------------------------ | -------------------------------------------------------------------------------------- | ------------------ |
| `frontend/src/app/(marketing)/legal/terms/page.tsx:79` | "DupeZero is priced per Salesforce org, with tiers by the number of records included…" | Per Salesforce org |
| `frontend/src/app/(marketing)/legal/terms/page.tsx`    | No statement of the counting rule, the soft buffer, sandbox exclusion or bespoke work  | Silent on the rule |
| `frontend/src/app/(marketing)/legal/privacy/page.tsx`  | No pricing text                                                                        | No conflict        |

No order forms, contracts or email templates exist in this repository.

## Fine as they are

| File                                                                                                                                                                                                                    | Text                                                                         | Why it is fine                                                                 |
| ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------- | ------------------------------------------------------------------------------ |
| `frontend/src/lib/pricing/savings.ts:8` and the savings calculator                                                                                                                                                      | "users × cost per user"                                                      | The visitor's staff cost in the ROI estimate, not our price                    |
| `frontend/src/lib/last-dataset.ts`, `lib/db/schema.ts`, `lib/domain/attributes.ts`, `app/api/.../models/route.ts`, `tests/unit/webhook-wire.test.ts`, `tests/functional/runs.spec.ts`, `tests/mock-erserver/server.mjs` | "per org"                                                                    | A tenant organisation in application code, not pricing                         |
| Product, golden records, integrations, FAQ, solutions copy                                                                                                                                                              | "your Salesforce org"                                                        | Names the system records come from and go back to; no price attached           |
| `frontend/src/components/marketing/pages/resources-content.ts:113`                                                                                                                                                      | "Run a sample in a sandbox first"                                            | Merge advice, not pricing                                                      |
| `docs/feature-inventory.md:52-54`, `130`                                                                                                                                                                                | Connections per org, sandbox orgs, roles per organisation                    | Product capability notes, not pricing                                          |
| `.claude/skills/er-server-control-plane/references/backend-design.md`, `research-cloudingo.md`, `connectors.md`                                                                                                         | A competitor "licensed per org"; one connection per org; sandbox connections | Internal research and connector design, never shown to customers               |
| `.github/workflows/*.lock.yml`, `CLAUDE.md`, scripts and other skills                                                                                                                                                   | "sandbox"                                                                    | Agent and CI sandboxing, unrelated                                             |
| `frontend/src/lib/pricing/config.ts` `competitor` block                                                                                                                                                                 | A competitor's flat price by records                                         | Off behind `SHOW_COMPETITOR_COMPARISON`, which needs legal review; not per org |

## Integration claims that are not verified in code

The brief says not to claim a connector exists unless the code verifies it.
These claims were already on the site at the owner's instruction (dated in
`docs/feature-inventory.md`) and were **left in place**; none was added here:

- The DupeZero app on the Salesforce AppExchange (no package source in this repo).
- Connecting other systems through their APIs (connection routes exist but are API-only, with no screen).
- Writing clean records back to the source systems (no write path exists in this repo).
- Home page source buttons for Google Sheets, Shopify and SFTP (not built), and ZoomInfo (disclosed as proposed).

The new integrations-page pricing line names no connector.
