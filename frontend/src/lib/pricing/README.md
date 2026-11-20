# Pricing calculator

The `/pricing` page (`src/app/(marketing)/pricing/page.tsx`) renders
`src/components/marketing/pricing/pricing-calculator.tsx`, which does no
arithmetic. Every rule lives here, and every number lives in one object.

| File                         | Holds                                                                        |
| ---------------------------- | ---------------------------------------------------------------------------- |
| `config.ts`                  | `PRICING_CONFIG`: every price, rate, threshold, flag and CTA URL             |
| `calculator.ts`              | The rules: `calculate`, `referenceGrid`, parsing, slider mapping, formatting |
| `savings.ts`                 | The savings calculator: value recovered, net benefit, return, payback        |
| `tests/unit/pricing.test.ts` | The brief's worked cases, every boundary, both flags                         |
| `tests/unit/savings.test.ts` | The savings defaults worked through, a loss, no price, the input bounds      |

## Changing a price

Edit `PRICING_CONFIG` in `config.ts`, then run
`corepack pnpm exec vitest run tests/unit/pricing.test.ts`. The tests pin the
current figures, so a deliberate price change also updates the expected values
there. That is intended: a price change should be a reviewed diff.

- **Money is integer cents**: `$4,500` is `450_000`.
- **Rates are basis points**: 1% is `100`, so 15% support is `1_500`.

| To change                              | Edit                                                               |
| -------------------------------------- | ------------------------------------------------------------------ |
| A tier's price or record cap           | `tiers[].annualCents`, `tiers[].maxRecords`                        |
| Monthly premium (10% of A per month)   | `MONTHLY_RATE_BP`                                                  |
| Support add-on (15%)                   | `SUPPORT_RATE_BP`, floor `MIN_SUPPORT_FEE`                         |
| Onboarding (10% of A, once)            | `ONBOARDING_RATE_BP`, floor `MIN_ONBOARDING_FEE`                   |
| Multi-year upfront prices (2A, 2.4A)   | `TWO_YEAR_PRICE_BP`, `THREE_YEAR_PRICE_BP`                         |
| Custom bands above the largest tier    | `customBands`, `CUSTOM_INCREMENT_RECORDS`, `CUSTOM_ROUND_UP_CENTS` |
| Where estimates stop ("Contact sales") | `MAX_ESTIMATE_RECORDS`                                             |
| Footnote figures                       | `SOFT_BUFFER_BP`, `ADDITIONAL_ORG_RATE_BP`                         |
| Trial and sales links                  | `TRIAL_URL`, `SALES_URL` (placeholders until launch)               |

## Flags

- `SHOW_CUSTOM_ESTIMATE` (default `false`): above the largest tier, show
  "Estimated from $X per year" instead of "Contact us for pricing".
- `SHOW_COMPETITOR_COMPARISON` (default `false`): **needs legal review of
  comparative-pricing claims before it is turned on.**
- `SHOW_CENTS_WHEN_FRACTIONAL` (default `true`): show `$517.50` rather than
  rounding every amount to the nearest dollar.

## Rules worth knowing

- Tier caps are inclusive: 300,000 records is Standard, 300,001 Professional.
- A started 100K increment in a custom band counts as a whole one, and the
  estimate then rounds up to the next $250.
- Support is optional on Monthly and Yearly only; 2- and 3-year terms include
  it, and onboarding. The support floor applies per billing period.
- "Savings vs monthly" compares cost per year against Monthly with the same
  support setting.

## Savings calculator

`savings-calculator.tsx` sits under the price and takes the record count and
the selected option's cost per year from it, so the net figure always matches
the price on screen. Its starting values are `PRICING_CONFIG.savings.defaults`:
assumptions for the visitor to replace, which the page says beside the result.

- value recovered = people × yearly cost × time lost × duplicate share × resolved
- net benefit = value recovered − DupeZero cost per year; payback rounds up
- the duplicate rate drives only the record counts shown, never the money
- `MAX_USERS` and `MAX_COST_PER_USER_CENTS` keep every product exact (below 2^53)
