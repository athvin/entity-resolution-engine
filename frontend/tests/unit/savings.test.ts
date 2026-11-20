import { describe, expect, it } from "vitest";

import { PRICING_CONFIG } from "@/lib/pricing/config";
import {
  calculateSavings,
  formatHours,
  parseWhole,
  wholeErrorMessage,
} from "@/lib/pricing/savings";

const D = PRICING_CONFIG.savings.defaults;
const base = { ...D, records: 750_000, annualCostCents: 450_000 };

describe("calculateSavings", () => {
  it("prices the defaults against Professional yearly", () => {
    // 25 × $100,000 × 10% = $250,000; × 40% = $100,000; × 80% = $80,000.
    const r = calculateSavings(base);
    expect(r.timeLostCents).toBe(25_000_000);
    expect(r.duplicateCostCents).toBe(10_000_000);
    expect(r.recoveredCents).toBe(8_000_000);
    expect(r.hoursRecovered).toBe(1_664);
    expect(r.duplicateRecords).toBe(75_000);
    expect(r.resolvedRecords).toBe(60_000);
    expect(r.netCents).toBe(7_550_000);
    expect(r.roiPct).toBe(1_678);
    // $4,500 / ($80,000 / 12) = 0.675 months, rounded up to 0.7.
    expect(r.paybackMonths).toBe(0.7);
  });

  it("goes negative rather than hiding a loss", () => {
    const r = calculateSavings({ ...base, users: 1, timeLostBp: 100 });
    // 1 × $100,000 × 1% × 40% × 80% = $320.
    expect(r.recoveredCents).toBe(32_000);
    expect(r.netCents).toBe(32_000 - 450_000);
    expect(r.roiPct).toBe(-93);
    expect(r.paybackMonths).toBe(168.8);
  });

  it("has no net, return or payback without a price", () => {
    const r = calculateSavings({ ...base, annualCostCents: null });
    expect(r.recoveredCents).toBe(8_000_000);
    expect([r.netCents, r.roiPct, r.paybackMonths]).toEqual([null, null, null]);
  });

  it("has no payback when nothing is recovered", () => {
    const r = calculateSavings({ ...base, resolvedBp: 0 });
    expect(r.recoveredCents).toBe(0);
    expect(r.hoursRecovered).toBe(0);
    expect(r.paybackMonths).toBeNull();
    expect(r.roiPct).toBe(-100);
  });

  it("leaves record counts out when the price calculator has none", () => {
    const r = calculateSavings({ ...base, records: null });
    expect(r.duplicateRecords).toBeNull();
    expect(r.resolvedRecords).toBeNull();
  });

  it("stays exact at the input bounds", () => {
    const r = calculateSavings({
      ...base,
      users: PRICING_CONFIG.savings.MAX_USERS,
      costPerUserCents: PRICING_CONFIG.savings.MAX_COST_PER_USER_CENTS,
      timeLostBp: 10_000,
      duplicateShareBp: 10_000,
      resolvedBp: 10_000,
    });
    expect(r.recoveredCents).toBe(5_000 * 50_000_000);
    expect(Number.isSafeInteger(r.recoveredCents * 10_000)).toBe(true);
    expect(r.hoursRecovered).toBe(5_000 * 2_080);
  });
});

describe("parseWhole", () => {
  const bounds = { min: 1, max: 5_000 };
  it("accepts commas, spaces and a dollar sign, and floors decimals", () => {
    expect(parseWhole(" $1,250 ", bounds)).toEqual({ ok: true, value: 1_250 });
    expect(parseWhole("12.9", bounds)).toEqual({ ok: true, value: 12 });
  });
  it("names each failure", () => {
    expect(parseWhole("", bounds)).toEqual({ ok: false, error: "empty" });
    expect(parseWhole("ten", bounds)).toEqual({ ok: false, error: "not-a-number" });
    expect(parseWhole("0", bounds)).toEqual({ ok: false, error: "too-small" });
    expect(parseWhole("-4", bounds)).toEqual({ ok: false, error: "too-small" });
    expect(parseWhole("5,001", bounds)).toEqual({ ok: false, error: "too-large" });
  });
  it("has a message for every failure", () => {
    expect(wholeErrorMessage("empty", "1", "5,000")).toBe("Enter a number.");
    expect(wholeErrorMessage("not-a-number", "1", "5,000")).toMatch(/digits/);
    expect(wholeErrorMessage("too-small", "1", "5,000")).toBe("Enter at least 1.");
    expect(wholeErrorMessage("too-large", "1", "5,000")).toBe("Enter at most 5,000.");
  });
});

describe("formatHours", () => {
  it("groups thousands", () => {
    expect(formatHours(10_400_000)).toBe("10,400,000");
  });
});
