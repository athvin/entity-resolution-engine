import { describe, expect, it } from "vitest";

import { MERIDIAN_PAIRS } from "@/components/marketing/scene/meridian";
import { DEFAULT_LEVELS, PAIR_FIELDS, scorePair } from "@/components/marketing/scene/pair-model";
import { formatProbability } from "@/components/marketing/scene/score";

describe("Try a pair's scoring", () => {
  // The explorer opens on the pair the page's scene scores, so the two must
  // agree weight for weight, and on the 0.93 both print.
  it("opens on the close Meridian pair, weight for weight", () => {
    const outcome = scorePair(DEFAULT_LEVELS);
    expect(outcome.comparisons).toEqual([...MERIDIAN_PAIRS.close]);
    expect(formatProbability(outcome.probability)).toBe("0.93");
    expect(outcome.decision).toBe("review");
    expect(outcome.ruledOut).toBe(false);
  });

  it("joins a pair whose phones also agree", () => {
    const outcome = scorePair({ ...DEFAULT_LEVELS, phone: "exact" });
    expect(outcome.decision).toBe("join");
  });

  it("keeps records with different account IDs apart, whatever the score", () => {
    const best = Object.fromEntries(PAIR_FIELDS.map((f) => [f.key, f.levels[0].key])) as Parameters<
      typeof scorePair
    >[0];
    const outcome = scorePair({ ...best, account: "different" });
    expect(outcome.ruledOut).toBe(true);
    // The score alone would still have joined them: the rule is what decides.
    expect(outcome.decision).toBe("join");
  });

  it("leaves a pair that disagrees everywhere separate", () => {
    const worst = Object.fromEntries(
      PAIR_FIELDS.map((f) => [f.key, f.levels[f.levels.length - 1]?.key ?? f.levels[0].key]),
    ) as Parameters<typeof scorePair>[0];
    expect(scorePair(worst).decision).toBe("separate");
  });
});
