/**
 * The arithmetic behind every match score a marketing scene shows.
 *
 * Fellegi–Sunter in its usual presentation, the same shape the app's evidence
 * waterfall draws: each comparison contributes a log2 match weight, a prior
 * weight accounts for how rare a true match is among candidate pairs, and the
 * total converts to a probability. The inputs in `meridian.ts` are
 * illustrative; the arithmetic is real, so a number on one page can never
 * contradict the bars drawn beside it on another.
 */

/** Log2 of the prior odds: about one true match in 65 candidate pairs. */
export const PRIOR_WEIGHT = -6;

export function totalWeight(weights: readonly number[], prior = PRIOR_WEIGHT): number {
  return weights.reduce((sum, weight) => sum + weight, prior);
}

export function matchProbability(weights: readonly number[], prior = PRIOR_WEIGHT): number {
  const odds = 2 ** totalWeight(weights, prior);
  return odds / (1 + odds);
}

export type Decision = "join" | "review" | "separate";

/** What the two thresholds do with a score, as /features/matching describes them. */
export function decide(
  probability: number,
  thresholds: { autoJoin: number; review: number },
): Decision {
  if (probability >= thresholds.autoJoin) return "join";
  if (probability >= thresholds.review) return "review";
  return "separate";
}

/** Two decimals, but never "1.00" for something short of certain. */
export function formatProbability(probability: number): string {
  return probability >= 0.995 ? "> 0.99" : probability.toFixed(2);
}

/** A signed weight with a real minus sign, so the sign never depends on hue. */
export function formatWeight(weight: number): string {
  if (weight === 0) return "0.0";
  return `${weight > 0 ? "+" : "−"}${Math.abs(weight).toFixed(1)}`;
}
