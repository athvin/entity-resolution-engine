import { describe, expect, it } from "vitest";

import {
  POOL_SOURCES,
  POOL_TOTAL,
  rulerPosition,
} from "@/components/marketing/pricing/records-pool";
import { tierFor } from "@/lib/pricing/calculator";
import { PRICING_CONFIG } from "@/lib/pricing/config";

/*
 * /pricing's records-to-one-total figure (#1262) shows the records-only rule:
 * every system's records add to one total, and the total alone sets the tier.
 * These hold it to the pricing config, so the figure cannot disagree with the
 * calculator beneath it.
 */
describe("the records-to-one-total figure", () => {
  it("adds up to the count the calculator starts on", () => {
    expect(POOL_TOTAL).toBe(POOL_SOURCES.reduce((sum, source) => sum + source.records, 0));
    expect(POOL_TOTAL).toBe(PRICING_CONFIG.DEFAULT_RECORDS);
  });

  it("lands on the tier the calculator would price", () => {
    const tier = tierFor(POOL_TOTAL);
    expect(tier).not.toBeNull();
    const index = PRICING_CONFIG.tiers.findIndex((each) => each.id === tier?.id);
    const position = rulerPosition(POOL_TOTAL);
    // Inside that tier's equal-width share of the ruler.
    expect(position).toBeGreaterThan(index / PRICING_CONFIG.tiers.length);
    expect(position).toBeLessThanOrEqual((index + 1) / PRICING_CONFIG.tiers.length);
  });

  it("places counts along the ruler in order, each tier's cap at its boundary", () => {
    const counts = [1, 150_000, 300_000, 300_001, 650_000, 1_000_000, 2_500_000];
    const positions = counts.map(rulerPosition);
    expect([...positions].sort((a, b) => a - b)).toEqual(positions);
    PRICING_CONFIG.tiers.forEach((tier, index) => {
      expect(rulerPosition(tier.maxRecords)).toBeCloseTo((index + 1) / PRICING_CONFIG.tiers.length);
    });
    expect(rulerPosition(PRICING_CONFIG.MAX_ESTIMATE_RECORDS)).toBe(1);
  });
});
