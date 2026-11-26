import { describe, expect, it } from "vitest";

import {
  MERIDIAN_GOLDEN,
  MERIDIAN_PAIRS,
  MERIDIAN_SOURCES,
  MERIDIAN_THRESHOLDS,
} from "@/components/marketing/scene/meridian";
import {
  decide,
  formatProbability,
  formatWeight,
  matchProbability,
  PRIOR_WEIGHT,
  totalWeight,
} from "@/components/marketing/scene/score";

const weights = (pair: readonly { weight: number }[]) => pair.map((c) => c.weight);

describe("marketing scene scoring", () => {
  it("adds the prior to every comparison's weight", () => {
    expect(totalWeight([])).toBe(PRIOR_WEIGHT);
    expect(totalWeight([1.5, -0.5], 0)).toBe(1);
  });

  it("turns a total weight into a probability: zero is even odds", () => {
    expect(matchProbability([], 0)).toBe(0.5);
    expect(matchProbability([1], 0)).toBeCloseTo(2 / 3, 10);
    expect(matchProbability([-1], 0)).toBeCloseTo(1 / 3, 10);
  });

  // The scenes say these numbers out loud; the arithmetic has to agree with them.
  it("puts the close Meridian pair in review at 0.93, under the auto-join threshold", () => {
    const p = matchProbability(weights(MERIDIAN_PAIRS.close));
    expect(formatProbability(p)).toBe("0.93");
    expect(decide(p, MERIDIAN_THRESHOLDS)).toBe("review");
  });

  it("joins the clear Meridian pair without a person", () => {
    const p = matchProbability(weights(MERIDIAN_PAIRS.clear));
    expect(formatProbability(p)).toBe("> 0.99");
    expect(decide(p, MERIDIAN_THRESHOLDS)).toBe("join");
  });

  it("keeps pairs under the review band separate", () => {
    expect(decide(0.5, MERIDIAN_THRESHOLDS)).toBe("separate");
  });

  it("signs a weight with a real minus, never by hue", () => {
    expect(formatWeight(4.1)).toBe("+4.1");
    expect(formatWeight(-2.6)).toBe("−2.6");
    expect(formatWeight(0)).toBe("0.0");
  });

  // The scene strikes through every source not named in `held`, and the
  // explorer marks every one that is: the data has to be true for both.
  it("names, for each golden value, exactly the sources that hold it", () => {
    for (const value of MERIDIAN_GOLDEN) {
      for (const source of MERIDIAN_SOURCES) {
        const key = value.field.toLowerCase() as keyof (typeof source)["raw"];
        const holds = source.raw[key] === value.value || source.clean[key] === value.value;
        expect(
          (value.held as readonly string[]).includes(source.key),
          `${value.field} / ${source.key}`,
        ).toBe(holds);
      }
    }
  });

  it("draws every golden value from a source that exists", () => {
    const keys = new Set<string>(MERIDIAN_SOURCES.map((s) => s.key));
    for (const value of MERIDIAN_GOLDEN) expect(keys.has(value.from)).toBe(true);
    // Field-level rules: the winners come from more than one source.
    expect(new Set(MERIDIAN_GOLDEN.map((v) => v.from)).size).toBeGreaterThan(1);
  });
});
