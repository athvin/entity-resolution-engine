/**
 * The pricing rules: pure functions from a record count and a support choice
 * to every billing option's figures. No DOM, no state, no I/O; every number
 * comes from `config.ts`, and every amount is integer cents.
 *
 * The UI renders what `calculate` and `referenceGrid` return and formats it
 * with `formatUsd` / `formatRecords`; it does no arithmetic of its own.
 */

import { PRICING_CONFIG, type PricingConfig, type TierId } from "./config";

export const TERMS = ["monthly", "yearly", "twoYear", "threeYear"] as const;
export type Term = (typeof TERMS)[number];

export const TERM_LABELS: Record<Term, string> = {
  monthly: "Monthly",
  yearly: "Yearly upfront",
  twoYear: "2 years upfront",
  threeYear: "3 years upfront",
};

export interface OptionResult {
  term: Term;
  /** Billed per month (Monthly) or once upfront (every other term). */
  billing: "monthly" | "upfront";
  /** The term in months: 12 for Monthly, which is shown over a year. */
  months: number;
  dueAtSignupCents: number;
  /** Recurring cost per year, excluding onboarding. */
  perYearCents: number;
  perMonthCents: number;
  /** Monthly: twelve months plus onboarding. Upfront terms: the upfront price. */
  totalCents: number;
  onboardingCents: number;
  /** Charged once on Monthly; free on every upfront term (1, 2 or 3 years). */
  onboarding: "charged" | "free";
  support: "added" | "not-added" | "included";
  /** Saving in cost per year against Monthly with the same support setting. */
  savingsPct: number;
}

export type TierKind = TierId | "custom";

export type Calculation =
  | {
      status: "priced";
      records: number;
      tier: { id: TierKind; name: string; capRecords: number | null };
      /** True when the price is a custom-band estimate rather than a list price. */
      estimate: boolean;
      annualListCents: number;
      selected: Term;
      options: Record<Term, OptionResult>;
      /** The term with the lowest cost per year. */
      cheapest: Term;
      competitor: CompetitorComparison | null;
    }
  | {
      /** Above the largest tier with custom estimates switched off. */
      status: "contact-us";
      records: number;
      selected: Term;
    }
  | {
      /** Above the largest record count any estimate covers. */
      status: "contact-sales";
      records: number;
      selected: Term;
    };

export interface CompetitorComparison {
  competitorCents: number;
  /** Our annual list price against theirs: negative means we are cheaper. */
  differencePct: number;
}

/** `cents * bp / 10000`, rounded to the cent with halves up. Exact for integers. */
export function applyRate(cents: number, bp: number): number {
  return Math.floor((cents * bp + 5_000) / 10_000);
}

/** Integer division rounded with halves up. */
function divideRounded(cents: number, by: number): number {
  return Math.floor((2 * cents + by) / (2 * by));
}

function ceilDiv(value: number, by: number): number {
  return Math.ceil(value / by);
}

/** Whole percent with halves up, of `part` against `whole`. */
function percent(part: number, whole: number): number {
  return Math.floor((part * 100) / whole + 0.5);
}

function assertRecords(records: number): void {
  if (!Number.isSafeInteger(records) || records < 1) {
    throw new RangeError(`records must be a positive integer, got ${String(records)}`);
  }
}

/** The custom-band estimate in cents before and after rounding, or null outside the bands. */
export function customEstimate(
  records: number,
  config: PricingConfig = PRICING_CONFIG,
): { unroundedCents: number; cents: number } | null {
  const band = config.customBands.find((b) => records > b.fromRecords && records <= b.toRecords);
  if (!band) return null;
  const increments = ceilDiv(records - band.fromRecords, config.CUSTOM_INCREMENT_RECORDS);
  const unroundedCents = band.baseCents + increments * band.perIncrementCents;
  const cents =
    ceilDiv(unroundedCents, config.CUSTOM_ROUND_UP_CENTS) * config.CUSTOM_ROUND_UP_CENTS;
  return { unroundedCents, cents };
}

/** The competitor's comparable annual list price for this many records. */
export function competitorPrice(records: number, config: PricingConfig = PRICING_CONFIG): number {
  const c = config.competitor;
  if (records <= c.flatUpToRecords) return c.flatCents;
  const increments = ceilDiv(records - c.incrementFromRecords, c.incrementRecords);
  // The last step is unbounded, so a step always matches.
  const step = c.steps.find((s) => records <= s.upToRecords) ?? c.steps[c.steps.length - 1];
  return step ? step.baseCents + increments * step.perIncrementCents : c.flatCents;
}

function supportFee(subscriptionCents: number, config: PricingConfig): number {
  return Math.max(applyRate(subscriptionCents, config.SUPPORT_RATE_BP), config.MIN_SUPPORT_FEE);
}

/** Every billing option for an annual list price `a`, before savings are known. */
function optionsFor(
  a: number,
  support: boolean,
  config: PricingConfig,
): Record<Term, Omit<OptionResult, "savingsPct">> {
  const onboardingCents = Math.max(
    applyRate(a, config.ONBOARDING_RATE_BP),
    config.MIN_ONBOARDING_FEE,
  );

  const baseMonth = applyRate(a, config.MONTHLY_RATE_BP);
  const month = support ? baseMonth + supportFee(baseMonth, config) : baseMonth;
  const year = support ? a + supportFee(a, config) : a;
  const two = applyRate(a, config.TWO_YEAR_PRICE_BP);
  const three = applyRate(a, config.THREE_YEAR_PRICE_BP);
  const supportState = support ? "added" : "not-added";

  return {
    monthly: {
      term: "monthly",
      billing: "monthly",
      months: 12,
      dueAtSignupCents: month + onboardingCents,
      perYearCents: month * 12,
      perMonthCents: month,
      totalCents: month * 12 + onboardingCents,
      onboardingCents,
      onboarding: "charged",
      support: supportState,
    },
    yearly: {
      term: "yearly",
      billing: "upfront",
      months: 12,
      dueAtSignupCents: year,
      perYearCents: year,
      perMonthCents: divideRounded(year, 12),
      totalCents: year,
      onboardingCents: 0,
      onboarding: "free",
      support: supportState,
    },
    twoYear: {
      term: "twoYear",
      billing: "upfront",
      months: 24,
      dueAtSignupCents: two,
      perYearCents: divideRounded(two, 2),
      perMonthCents: divideRounded(two, 24),
      totalCents: two,
      onboardingCents: 0,
      onboarding: "free",
      support: "included",
    },
    threeYear: {
      term: "threeYear",
      billing: "upfront",
      months: 36,
      dueAtSignupCents: three,
      perYearCents: divideRounded(three, 3),
      perMonthCents: divideRounded(three, 36),
      totalCents: three,
      onboardingCents: 0,
      onboarding: "free",
      support: "included",
    },
  };
}

/** Every billing option for an annual list price, with savings against Monthly. */
export function billingOptions(
  annualListCents: number,
  support: boolean,
  config: PricingConfig = PRICING_CONFIG,
): Record<Term, OptionResult> {
  const raw = optionsFor(annualListCents, support, config);
  const monthlyYear = raw.monthly.perYearCents;
  const withSavings = (o: Omit<OptionResult, "savingsPct">): OptionResult => ({
    ...o,
    savingsPct: percent(monthlyYear - o.perYearCents, monthlyYear),
  });
  return {
    monthly: withSavings(raw.monthly),
    yearly: withSavings(raw.yearly),
    twoYear: withSavings(raw.twoYear),
    threeYear: withSavings(raw.threeYear),
  };
}

/** The tier a record count falls in, by inclusive cap; null above the largest. */
export function tierFor(records: number, config: PricingConfig = PRICING_CONFIG) {
  return config.tiers.find((t) => records <= t.maxRecords) ?? null;
}

export interface CalculateInput {
  records: number;
  term: Term;
  support: boolean;
}

/** Price a record count under every billing option. Throws on a non-positive or non-integer count. */
export function calculate(
  { records, term, support }: CalculateInput,
  config: PricingConfig = PRICING_CONFIG,
): Calculation {
  assertRecords(records);
  if (records > config.MAX_ESTIMATE_RECORDS) {
    return { status: "contact-sales", records, selected: term };
  }

  const tier = tierFor(records, config);
  let annualListCents: number;
  let tierInfo: { id: TierKind; name: string; capRecords: number | null };
  if (tier) {
    annualListCents = tier.annualCents;
    tierInfo = { id: tier.id, name: tier.name, capRecords: tier.maxRecords };
  } else {
    const estimate = customEstimate(records, config);
    if (!config.SHOW_CUSTOM_ESTIMATE || !estimate) {
      return { status: "contact-us", records, selected: term };
    }
    annualListCents = estimate.cents;
    tierInfo = { id: "custom", name: "Custom", capRecords: null };
  }

  const options = billingOptions(annualListCents, support, config);
  const cheapest = TERMS.reduce((best, t) =>
    options[t].perYearCents < options[best].perYearCents ? t : best,
  );
  const competitor = config.SHOW_COMPETITOR_COMPARISON
    ? (() => {
        const competitorCents = competitorPrice(records, config);
        return {
          competitorCents,
          differencePct: percent(annualListCents - competitorCents, competitorCents),
        };
      })()
    : null;

  return {
    status: "priced",
    records,
    tier: tierInfo,
    estimate: tier === null,
    annualListCents,
    selected: term,
    options,
    cheapest,
    competitor,
  };
}

export type GridMetric = "dueAtSignup" | "perYear" | "perMonth";

export const GRID_METRIC_LABELS: Record<GridMetric, string> = {
  dueAtSignup: "Due at signup",
  perYear: "Cost per year",
  perMonth: "Cost per month",
};

const GRID_METRIC_FIELD = {
  dueAtSignup: "dueAtSignupCents",
  perYear: "perYearCents",
  perMonth: "perMonthCents",
} as const satisfies Record<GridMetric, keyof OptionResult>;

export interface GridRow {
  label: string;
  records: number;
  /** Cents per term, or null where the row asks to contact us. */
  cells: Record<Term, number | null>;
}

/** Example record counts for the custom rows of the reference grid. */
export const GRID_CUSTOM_EXAMPLES = [5_000_000, 10_000_000, 25_000_000] as const;

/** All tiers and the custom examples across every term, for one metric. */
export function referenceGrid(
  { support, metric }: { support: boolean; metric: GridMetric },
  config: PricingConfig = PRICING_CONFIG,
): GridRow[] {
  const field = GRID_METRIC_FIELD[metric];
  const row = (label: string, records: number): GridRow => {
    const result = calculate({ records, term: "yearly", support }, config);
    const cells = Object.fromEntries(
      TERMS.map((t) => [t, result.status === "priced" ? result.options[t][field] : null]),
    ) as Record<Term, number | null>;
    return { label, records, cells };
  };
  return [
    ...config.tiers.map((t) => row(t.name, t.maxRecords)),
    ...GRID_CUSTOM_EXAMPLES.map((r) => row(`${formatCompactRecords(r)} records`, r)),
  ];
}

/** "$4,500", "$517.50": cents shown only when present (see SHOW_CENTS_WHEN_FRACTIONAL). */
export function formatUsd(cents: number, config: PricingConfig = PRICING_CONFIG): string {
  const fractional = cents % 100 !== 0;
  if (fractional && config.SHOW_CENTS_WHEN_FRACTIONAL) {
    return new Intl.NumberFormat("en-US", {
      style: "currency",
      currency: config.currency,
      minimumFractionDigits: 2,
      maximumFractionDigits: 2,
    }).format(cents / 100);
  }
  const dollars = Math.floor((cents + 50) / 100);
  return new Intl.NumberFormat("en-US", {
    style: "currency",
    currency: config.currency,
    maximumFractionDigits: 0,
  }).format(dollars);
}

export function formatRecords(records: number): string {
  return new Intl.NumberFormat("en-US").format(records);
}

/** "5M", "2.5M", "300K". */
export function formatCompactRecords(records: number): string {
  if (records >= 1_000_000) return `${String(records / 1_000_000)}M`;
  return `${String(records / 1_000)}K`;
}

export type RecordsError = "empty" | "not-a-number" | "not-an-integer" | "zero" | "negative";

export const RECORDS_ERROR_MESSAGES: Record<RecordsError, string> = {
  empty: "Enter a record count.",
  "not-a-number": "Enter a number, like 750,000.",
  "not-an-integer": "Enter a whole number of records.",
  zero: "Enter at least 1 record.",
  negative: "Record counts can’t be negative.",
};

/** Parse what someone typed: thousands separators and spaces are allowed. */
export function parseRecords(
  input: string,
): { ok: true; records: number } | { ok: false; error: RecordsError } {
  const cleaned = input.replace(/[,\s_]/g, "");
  if (cleaned === "") return { ok: false, error: "empty" };
  if (!/^[+-]?\d*\.?\d+$/.test(cleaned)) return { ok: false, error: "not-a-number" };
  const value = Number(cleaned);
  if (value < 0) return { ok: false, error: "negative" };
  if (!Number.isInteger(value) || !Number.isSafeInteger(value)) {
    return { ok: false, error: "not-an-integer" };
  }
  if (value === 0) return { ok: false, error: "zero" };
  return { ok: true, records: value };
}

/** Slider position (0..SLIDER_STEPS) to a record count, on a log scale, to 3 significant figures. */
export function sliderToRecords(position: number, config: PricingConfig = PRICING_CONFIG): number {
  const lo = Math.log10(config.SLIDER_MIN_RECORDS);
  const hi = Math.log10(config.SLIDER_MAX_RECORDS);
  const t = Math.min(1, Math.max(0, position / config.SLIDER_STEPS));
  const raw = 10 ** (lo + t * (hi - lo));
  const magnitude = 10 ** (Math.floor(Math.log10(raw)) - 2);
  const rounded = Math.round(raw / magnitude) * magnitude;
  return Math.min(config.SLIDER_MAX_RECORDS, Math.max(config.SLIDER_MIN_RECORDS, rounded));
}

/** A record count to the nearest slider position, clamped to the slider's range. */
export function recordsToSlider(records: number, config: PricingConfig = PRICING_CONFIG): number {
  const lo = Math.log10(config.SLIDER_MIN_RECORDS);
  const hi = Math.log10(config.SLIDER_MAX_RECORDS);
  const clamped = Math.min(config.SLIDER_MAX_RECORDS, Math.max(config.SLIDER_MIN_RECORDS, records));
  return Math.round(((Math.log10(clamped) - lo) / (hi - lo)) * config.SLIDER_STEPS);
}
