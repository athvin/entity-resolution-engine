/**
 * Every number the pricing calculator uses, in one place.
 *
 * Change prices here; `calculator.ts` holds only the rules that combine them.
 * See README.md beside this file for which field moves which figure.
 *
 * Money is integer US cents. Rates are integer basis points (1% = 100 bp), so
 * no figure is ever a binary fraction and every result is exact to the cent.
 */

export type TierId = "standard" | "professional" | "enterprise";

export interface TierConfig {
  id: TierId;
  name: string;
  /** Inclusive upper bound on total records across all scanned objects. */
  maxRecords: number;
  /** Annual list price, in cents. */
  annualCents: number;
}

export interface CustomBandConfig {
  /** Exclusive lower bound: the band starts at `fromRecords + 1`. */
  fromRecords: number;
  /** Inclusive upper bound. */
  toRecords: number;
  /** Price at `fromRecords`, in cents. */
  baseCents: number;
  /** Added per started increment of `CUSTOM_INCREMENT_RECORDS` above `fromRecords`. */
  perIncrementCents: number;
}

export interface CompetitorStepConfig {
  /** Inclusive upper bound on records this step prices. */
  upToRecords: number;
  baseCents: number;
  perIncrementCents: number;
}

export const PRICING_CONFIG = {
  currency: "USD",

  /** Ordered by `maxRecords`; the first tier whose cap covers the count wins. */
  tiers: [
    { id: "standard", name: "Standard", maxRecords: 300_000, annualCents: 187_500 },
    { id: "professional", name: "Professional", maxRecords: 1_000_000, annualCents: 450_000 },
    { id: "enterprise", name: "Enterprise", maxRecords: 2_500_000, annualCents: 750_000 },
  ] satisfies TierConfig[],

  /** Above the largest tier: an estimate, or a request to contact us. */
  SHOW_CUSTOM_ESTIMATE: false,
  customBands: [
    { fromRecords: 2_500_000, toRecords: 5_000_000, baseCents: 750_000, perIncrementCents: 12_000 },
    {
      fromRecords: 5_000_000,
      toRecords: 10_000_000,
      baseCents: 1_050_000,
      perIncrementCents: 9_000,
    },
    {
      fromRecords: 10_000_000,
      toRecords: 25_000_000,
      baseCents: 1_500_000,
      perIncrementCents: 6_000,
    },
  ] satisfies CustomBandConfig[],
  CUSTOM_INCREMENT_RECORDS: 100_000,
  /** A custom estimate is rounded up to the next multiple of this. */
  CUSTOM_ROUND_UP_CENTS: 25_000,
  /** Above this, no estimate at all: "Contact sales". */
  MAX_ESTIMATE_RECORDS: 25_000_000,

  /** Monthly billing: this share of the annual list price, per month. */
  MONTHLY_RATE_BP: 1_000,
  /** Support add-on: this share of the subscription it is added to. */
  SUPPORT_RATE_BP: 1_500,
  /** Onboarding: this share of the annual list price, charged once. */
  ONBOARDING_RATE_BP: 1_000,
  /** Floors on the two fees, in cents. Support's floor applies per billing period. */
  MIN_SUPPORT_FEE: 0,
  MIN_ONBOARDING_FEE: 0,
  /** Upfront multi-year prices, as a multiple of the annual list price. */
  TWO_YEAR_PRICE_BP: 20_000,
  THREE_YEAR_PRICE_BP: 24_000,

  /**
   * Display: amounts with a cents part show it ($517.50); whole amounts do not
   * ($4,500). When false, every amount rounds to the nearest dollar, half up.
   */
  SHOW_CENTS_WHEN_FRACTIONAL: true,

  /** The records slider: logarithmic between these. */
  SLIDER_MIN_RECORDS: 10_000,
  SLIDER_MAX_RECORDS: 25_000_000,
  SLIDER_STEPS: 1_000,
  DEFAULT_RECORDS: 750_000,

  /** Calls to action. Placeholders: replace before launch. */
  TRIAL_URL: "/app",
  SALES_URL: "mailto:sales@dupezero.com",
  TRIAL_DAYS: 14,

  /** Footnote figures. */
  SOFT_BUFFER_BP: 1_000,

  /**
   * Comparison against a competitor's list price. Do not enable without legal
   * review of comparative-pricing claims.
   */
  SHOW_COMPETITOR_COMPARISON: false,
  competitor: {
    /** Flat price up to and including this many records. */
    flatUpToRecords: 300_000,
    flatCents: 250_000,
    /** Increments are counted above this record count in every step. */
    incrementFromRecords: 300_000,
    incrementRecords: 100_000,
    steps: [
      { upToRecords: 1_000_000, baseCents: 600_000, perIncrementCents: 10_000 },
      { upToRecords: Number.POSITIVE_INFINITY, baseCents: 1_000_000, perIncrementCents: 10_000 },
    ] satisfies CompetitorStepConfig[],
  },

  /**
   * The savings calculator's starting assumptions and input bounds. The
   * defaults are assumptions for a visitor to replace with their own figures,
   * not measurements: the page says so beside the result.
   */
  savings: {
    WORK_HOURS_PER_YEAR: 2_080,
    defaults: {
      /** People whose work depends on the CRM's records. */
      users: 25,
      /** Fully loaded annual cost of one of them, in cents. */
      costPerUserCents: 10_000_000,
      /** Share of their time lost to bad data. */
      timeLostBp: 1_000,
      /** Share of that lost time caused by duplicate records. */
      duplicateShareBp: 4_000,
      /** Share of records that are duplicates. */
      duplicateRateBp: 1_000,
      /** Share of duplicates DupeZero resolves. */
      resolvedBp: 8_000,
    },
    /** Bounds keep every product below 2^53, so the maths stays exact. */
    MAX_USERS: 5_000,
    MAX_COST_PER_USER_CENTS: 50_000_000,
  },
};

export type PricingConfig = typeof PRICING_CONFIG;
