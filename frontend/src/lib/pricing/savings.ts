import { applyRate } from "./calculator";
import { PRICING_CONFIG, type PricingConfig } from "./config";

/**
 * The savings calculator: what duplicate records cost a team, and what
 * resolving them is worth against the price of DupeZero.
 *
 *   time lost to bad data   = users × cost per user × time lost
 *   caused by duplicates    = that × duplicate share
 *   value recovered / year  = that × share resolved
 *   net benefit / year      = value recovered − DupeZero cost per year
 *
 * Integer cents and basis points throughout, as in `calculator.ts`. Every
 * default lives in `PRICING_CONFIG.savings`.
 */

export interface SavingsInput {
  users: number;
  costPerUserCents: number;
  timeLostBp: number;
  duplicateShareBp: number;
  duplicateRateBp: number;
  resolvedBp: number;
  /** Total records, from the price calculator; null when it has none. */
  records: number | null;
  /** DupeZero cost per year for the selected option; null when not priced. */
  annualCostCents: number | null;
}

export interface SavingsResult {
  /** Annual cost of the time lost to bad data. */
  timeLostCents: number;
  /** The part of it caused by duplicates. */
  duplicateCostCents: number;
  /** The part of that DupeZero recovers: the headline figure. */
  recoveredCents: number;
  hoursRecovered: number;
  duplicateRecords: number | null;
  resolvedRecords: number | null;
  /** Null when there is no price to compare against. */
  netCents: number | null;
  /** Net benefit as a whole percent of the cost. */
  roiPct: number | null;
  /** Months of recovered value that pay for a year, to one decimal; null if never. */
  paybackMonths: number | null;
}

export function calculateSavings(
  input: SavingsInput,
  config: PricingConfig = PRICING_CONFIG,
): SavingsResult {
  const timeLostCents = applyRate(input.users * input.costPerUserCents, input.timeLostBp);
  const duplicateCostCents = applyRate(timeLostCents, input.duplicateShareBp);
  const recoveredCents = applyRate(duplicateCostCents, input.resolvedBp);
  const hoursRecovered =
    input.costPerUserCents > 0
      ? Math.floor(
          (2 * recoveredCents * config.savings.WORK_HOURS_PER_YEAR + input.costPerUserCents) /
            (2 * input.costPerUserCents),
        )
      : 0;

  const duplicateRecords =
    input.records === null ? null : applyRate(input.records, input.duplicateRateBp);
  const resolvedRecords =
    duplicateRecords === null ? null : applyRate(duplicateRecords, input.resolvedBp);

  const cost = input.annualCostCents;
  if (cost === null || cost <= 0) {
    return {
      timeLostCents,
      duplicateCostCents,
      recoveredCents,
      hoursRecovered,
      duplicateRecords,
      resolvedRecords,
      netCents: null,
      roiPct: null,
      paybackMonths: null,
    };
  }
  const netCents = recoveredCents - cost;
  return {
    timeLostCents,
    duplicateCostCents,
    recoveredCents,
    hoursRecovered,
    duplicateRecords,
    resolvedRecords,
    netCents,
    roiPct: Math.round((netCents * 100) / cost),
    // Rounded up: "pays for itself in 0.9 months" must never flatter the figure.
    paybackMonths: recoveredCents > 0 ? Math.ceil((cost * 120) / recoveredCents) / 10 : null,
  };
}

export type WholeError = "empty" | "not-a-number" | "too-small" | "too-large";

/**
 * Parses a whole number a visitor typed: commas, spaces and a leading `$` are
 * allowed; decimals are rounded down to the whole unit the field asks for.
 */
export function parseWhole(
  text: string,
  { min, max }: { min: number; max: number },
): { ok: true; value: number } | { ok: false; error: WholeError } {
  const cleaned = text.replace(/[\s,$]/g, "");
  if (cleaned === "") return { ok: false, error: "empty" };
  if (!/^-?\d+(\.\d+)?$/.test(cleaned)) return { ok: false, error: "not-a-number" };
  const value = Math.floor(Number(cleaned));
  if (value < min) return { ok: false, error: "too-small" };
  if (value > max) return { ok: false, error: "too-large" };
  return { ok: true, value };
}

export function wholeErrorMessage(error: WholeError, min: string, max: string): string {
  switch (error) {
    case "empty":
      return "Enter a number.";
    case "not-a-number":
      return "Use digits only, for example 25.";
    case "too-small":
      return `Enter at least ${min}.`;
    case "too-large":
      return `Enter at most ${max}.`;
  }
}

/** Hours, as "1,664". */
export function formatHours(hours: number): string {
  return new Intl.NumberFormat("en-US").format(hours);
}
