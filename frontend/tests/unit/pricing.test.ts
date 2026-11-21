import { describe, expect, it } from "vitest";

import {
  applyRate,
  billingOptions,
  calculate,
  competitorPrice,
  customEstimate,
  formatCompactRecords,
  formatRecords,
  formatUsd,
  parseRecords,
  recordsToSlider,
  referenceGrid,
  sliderToRecords,
  TERMS,
  tierFor,
  type Calculation,
} from "@/lib/pricing/calculator";
import { PRICING_CONFIG, type PricingConfig } from "@/lib/pricing/config";

const WITH_ESTIMATE: PricingConfig = { ...PRICING_CONFIG, SHOW_CUSTOM_ESTIMATE: true };
const WITH_COMPETITOR: PricingConfig = { ...PRICING_CONFIG, SHOW_COMPETITOR_COMPARISON: true };

function priced(result: Calculation) {
  if (result.status !== "priced") throw new Error(`expected a price, got ${result.status}`);
  return result;
}

const pro = (support: boolean) =>
  priced(calculate({ records: 1_000_000, term: "yearly", support })).options;

describe("the brief's Professional cases (A = $4,500)", () => {
  it("Monthly, no support: $450/month, $5,400/year, $900 due at signup", () => {
    const m = pro(false).monthly;
    expect(formatUsd(m.perMonthCents)).toBe("$450");
    expect(formatUsd(m.perYearCents)).toBe("$5,400");
    expect(formatUsd(m.dueAtSignupCents)).toBe("$900");
    expect(formatUsd(m.totalCents)).toBe("$5,850");
    expect(m.onboarding).toBe("charged");
    expect(m.savingsPct).toBe(0);
  });

  it("Monthly, with support: $517.50/month, $6,210/year, $967.50 due at signup", () => {
    const m = pro(true).monthly;
    expect(formatUsd(m.perMonthCents)).toBe("$517.50");
    expect(formatUsd(m.perYearCents)).toBe("$6,210");
    expect(formatUsd(m.dueAtSignupCents)).toBe("$967.50");
    expect(m.support).toBe("added");
  });

  it("Yearly, no support: $4,500 a year and at signup, onboarding free, 17% saved", () => {
    const y = pro(false).yearly;
    expect(formatUsd(y.perYearCents)).toBe("$4,500");
    expect(formatUsd(y.dueAtSignupCents)).toBe("$4,500");
    expect(formatUsd(y.perMonthCents)).toBe("$375");
    expect(y.onboarding).toBe("free");
    expect(y.onboardingCents).toBe(0);
    expect(y.savingsPct).toBe(17);
  });

  it("Yearly, with support: $5,175 a year and at signup, 17% saved", () => {
    const y = pro(true).yearly;
    expect(formatUsd(y.perYearCents)).toBe("$5,175");
    expect(formatUsd(y.dueAtSignupCents)).toBe("$5,175");
    expect(y.savingsPct).toBe(17);
  });

  it("2 years: $9,000 upfront, $4,500/year, $375/month, support included, onboarding free", () => {
    for (const support of [false, true]) {
      const t = pro(support).twoYear;
      expect(formatUsd(t.dueAtSignupCents)).toBe("$9,000");
      expect(formatUsd(t.totalCents)).toBe("$9,000");
      expect(formatUsd(t.perYearCents)).toBe("$4,500");
      expect(formatUsd(t.perMonthCents)).toBe("$375");
      expect(t.support).toBe("included");
      expect(t.onboarding).toBe("free");
      expect(t.onboardingCents).toBe(0);
    }
  });

  it("onboarding is charged once on Monthly only; every upfront term gets it free", () => {
    for (const support of [false, true]) {
      const all = pro(support);
      expect(all.monthly.onboarding).toBe("charged");
      expect(all.monthly.onboardingCents).toBe(45_000);
      for (const term of [all.yearly, all.twoYear, all.threeYear]) {
        expect(term.onboarding).toBe("free");
        expect(term.onboardingCents).toBe(0);
      }
    }
  });

  it("3 years: $10,800 upfront, $3,600/year, $300/month; 33% saved, 42% against monthly with support", () => {
    const plain = pro(false).threeYear;
    expect(formatUsd(plain.dueAtSignupCents)).toBe("$10,800");
    expect(formatUsd(plain.perYearCents)).toBe("$3,600");
    expect(formatUsd(plain.perMonthCents)).toBe("$300");
    expect(plain.savingsPct).toBe(33);
    expect(pro(true).threeYear.savingsPct).toBe(42);
  });

  it("3 years upfront is the cheapest per year", () => {
    expect(
      priced(calculate({ records: 1_000_000, term: "monthly", support: false })).cheapest,
    ).toBe("threeYear");
  });
});

describe("tier selection, with inclusive boundaries", () => {
  it.each([
    [1, "Standard"],
    [250_000, "Standard"],
    [300_000, "Standard"],
    [300_001, "Professional"],
    [1_000_000, "Professional"],
    [1_000_001, "Enterprise"],
    [2_500_000, "Enterprise"],
  ])("%i records is %s", (records, name) => {
    const result = priced(calculate({ records, term: "yearly", support: false }));
    expect(result.tier.name).toBe(name);
    expect(result.estimate).toBe(false);
    expect(result.annualListCents).toBe(tierFor(records)?.annualCents);
  });

  it("there is no tier above the largest cap", () => {
    expect(tierFor(2_500_001)).toBeNull();
  });

  it("prices each tier at its list price", () => {
    expect(
      formatUsd(
        priced(calculate({ records: 300_000, term: "yearly", support: false })).annualListCents,
      ),
    ).toBe("$1,875");
    expect(
      formatUsd(
        priced(calculate({ records: 2_500_000, term: "yearly", support: false })).annualListCents,
      ),
    ).toBe("$7,500");
  });
});

describe("the custom band", () => {
  it.each([
    [2_600_000, 762_000, 775_000],
    [5_000_000, 1_050_000, 1_050_000],
    [7_000_000, 1_230_000, 1_250_000],
    [12_500_000, 1_650_000, 1_650_000],
    [25_000_000, 2_400_000, 2_400_000],
  ])("%i records: unrounded %i cents, shown as %i", (records, unrounded, shown) => {
    expect(customEstimate(records)).toEqual({ unroundedCents: unrounded, cents: shown });
    const result = priced(calculate({ records, term: "yearly", support: false }, WITH_ESTIMATE));
    expect(result.annualListCents).toBe(shown);
    expect(result.estimate).toBe(true);
    expect(result.tier).toEqual({ id: "custom", name: "Custom", capRecords: null });
  });

  it("counts a started increment as a whole one", () => {
    expect(customEstimate(2_500_001)?.unroundedCents).toBe(762_000);
    expect(customEstimate(5_000_001)?.unroundedCents).toBe(1_059_000);
    expect(customEstimate(10_000_001)?.unroundedCents).toBe(1_506_000);
  });

  it("is continuous at each band boundary", () => {
    expect(customEstimate(5_000_000)?.unroundedCents).toBe(1_050_000);
    expect(customEstimate(10_000_000)?.unroundedCents).toBe(1_500_000);
  });

  it("has no estimate outside the bands", () => {
    expect(customEstimate(2_500_000)).toBeNull();
    expect(customEstimate(25_000_001)).toBeNull();
  });

  it("asks to contact us, with no number, when estimates are off (the default)", () => {
    expect(PRICING_CONFIG.SHOW_CUSTOM_ESTIMATE).toBe(false);
    expect(calculate({ records: 2_500_001, term: "monthly", support: true })).toEqual({
      status: "contact-us",
      records: 2_500_001,
      selected: "monthly",
    });
  });

  it("sends anything above 25,000,000 records to sales, whatever the flag", () => {
    for (const config of [PRICING_CONFIG, WITH_ESTIMATE]) {
      expect(
        calculate({ records: 25_000_001, term: "yearly", support: false }, config).status,
      ).toBe("contact-sales");
    }
    expect(
      calculate({ records: 25_000_000, term: "yearly", support: false }, WITH_ESTIMATE).status,
    ).toBe("priced");
  });
});

describe("billing options", () => {
  it("returns every term, and the selected one", () => {
    const result = priced(calculate({ records: 50_000, term: "twoYear", support: false }));
    expect(Object.keys(result.options)).toEqual([...TERMS]);
    expect(result.selected).toBe("twoYear");
  });

  it("charges Monthly as 10% of A per month, which is A/12 plus 20%", () => {
    const m = billingOptions(450_000, false).monthly;
    expect(m.perMonthCents).toBe(45_000);
    // A/12 = 37,500 cents; plus 20% = 45,000.
    expect((450_000 / 12 / 100) * 120).toBe(m.perMonthCents);
  });

  it("keeps fractional cents exact and rounds halves up", () => {
    // Standard monthly with support: $187.50 * 1.15 = $215.625 -> $215.63.
    const m = billingOptions(187_500, true).monthly;
    expect(m.perMonthCents).toBe(21_563);
    expect(formatUsd(m.perMonthCents)).toBe("$215.63");
    // Yearly with support per month: $2,156.25 / 12 = $179.6875 -> $179.69.
    expect(billingOptions(187_500, true).yearly.perMonthCents).toBe(17_969);
  });

  it("applies the minimum fees when they exceed the percentage", () => {
    const config: PricingConfig = {
      ...PRICING_CONFIG,
      MIN_SUPPORT_FEE: 100_000,
      MIN_ONBOARDING_FEE: 200_000,
    };
    const options = billingOptions(450_000, true, config);
    expect(options.yearly.perYearCents).toBe(550_000);
    expect(options.monthly.perMonthCents).toBe(145_000);
    expect(options.monthly.onboardingCents).toBe(200_000);
    expect(options.monthly.dueAtSignupCents).toBe(345_000);
  });

  it("does not add support to multi-year terms, where it is included", () => {
    expect(billingOptions(450_000, true).twoYear.perYearCents).toBe(450_000);
    expect(billingOptions(450_000, false).yearly.support).toBe("not-added");
  });

  it("rounds a rate to the cent with halves up", () => {
    expect(applyRate(5, 1_000)).toBe(1); // 0.5 -> 1
    expect(applyRate(4, 1_000)).toBe(0); // 0.4 -> 0
  });
});

describe("input validation", () => {
  it("rejects input that is empty, zero, negative, fractional or not a number", () => {
    expect(parseRecords("")).toEqual({ ok: false, error: "empty" });
    expect(parseRecords("  ")).toEqual({ ok: false, error: "empty" });
    expect(parseRecords("0")).toEqual({ ok: false, error: "zero" });
    expect(parseRecords("-5")).toEqual({ ok: false, error: "negative" });
    expect(parseRecords("1.5")).toEqual({ ok: false, error: "not-an-integer" });
    expect(parseRecords("abc")).toEqual({ ok: false, error: "not-a-number" });
    expect(parseRecords("1e6")).toEqual({ ok: false, error: "not-a-number" });
    expect(parseRecords("99999999999999999999")).toEqual({ ok: false, error: "not-an-integer" });
  });

  it("accepts thousands separators and spaces", () => {
    expect(parseRecords("1,000,000")).toEqual({ ok: true, records: 1_000_000 });
    expect(parseRecords(" 300 001 ")).toEqual({ ok: true, records: 300_001 });
    expect(parseRecords("1")).toEqual({ ok: true, records: 1 });
  });

  it("refuses to price a count that is not a positive integer", () => {
    expect(() => calculate({ records: 0, term: "yearly", support: false })).toThrow(RangeError);
    expect(() => calculate({ records: 1.5, term: "yearly", support: false })).toThrow(RangeError);
  });
});

describe("the logarithmic slider", () => {
  it("spans 10,000 to 25,000,000", () => {
    expect(sliderToRecords(0)).toBe(10_000);
    expect(sliderToRecords(PRICING_CONFIG.SLIDER_STEPS)).toBe(25_000_000);
    expect(sliderToRecords(-10)).toBe(10_000);
    expect(sliderToRecords(5_000)).toBe(25_000_000);
  });

  it("round-trips a position through a record count", () => {
    for (const position of [0, 137, 500, 812, 1_000]) {
      expect(Math.abs(recordsToSlider(sliderToRecords(position)) - position)).toBeLessThanOrEqual(
        1,
      );
    }
  });

  it("clamps record counts outside its range", () => {
    expect(recordsToSlider(1)).toBe(0);
    expect(recordsToSlider(99_000_000)).toBe(PRICING_CONFIG.SLIDER_STEPS);
  });

  it("lands on round numbers", () => {
    expect(sliderToRecords(500) % 1_000).toBe(0);
  });
});

describe("the reference grid", () => {
  it("lists every tier and the 5M, 10M and 25M examples", () => {
    const rows = referenceGrid({ support: false, metric: "perYear" });
    expect(rows.map((r) => r.label)).toEqual([
      "Standard",
      "Professional",
      "Enterprise",
      "5M records",
      "10M records",
      "25M records",
    ]);
    expect(rows[1]?.cells).toEqual({
      monthly: 540_000,
      yearly: 450_000,
      twoYear: 450_000,
      threeYear: 360_000,
    });
  });

  it("shows 'Contact us' (null) for custom rows while estimates are off", () => {
    const rows = referenceGrid({ support: false, metric: "dueAtSignup" });
    expect(rows[3]?.cells.yearly).toBeNull();
    const shown = referenceGrid({ support: false, metric: "dueAtSignup" }, WITH_ESTIMATE);
    expect(shown[5]?.cells.yearly).toBe(2_400_000);
  });

  it("respects the support toggle and the chosen metric", () => {
    const due = referenceGrid({ support: true, metric: "dueAtSignup" });
    expect(due[1]?.cells.monthly).toBe(96_750);
    const month = referenceGrid({ support: true, metric: "perMonth" });
    expect(month[1]?.cells.monthly).toBe(51_750);
  });
});

describe("the competitor comparison (off by default)", () => {
  it("is absent unless the flag is on", () => {
    expect(
      priced(calculate({ records: 1_000_000, term: "yearly", support: false })).competitor,
    ).toBeNull();
  });

  it.each([
    [300_000, 250_000],
    [300_001, 610_000],
    [1_000_000, 670_000],
    [1_000_001, 1_080_000],
    [2_500_000, 1_220_000],
  ])("prices %i records at %i cents", (records, cents) => {
    expect(competitorPrice(records)).toBe(cents);
  });

  it("reports our list price's difference from theirs", () => {
    const result = priced(
      calculate({ records: 1_000_000, term: "yearly", support: false }, WITH_COMPETITOR),
    );
    // $4,500 against $6,700: 32.8% less, shown as 33%.
    expect(result.competitor).toEqual({ competitorCents: 670_000, differencePct: -33 });
  });
});

describe("formatting", () => {
  it("shows cents only when there are any", () => {
    expect(formatUsd(450_000)).toBe("$4,500");
    expect(formatUsd(51_750)).toBe("$517.50");
  });

  it("rounds to the nearest dollar, halves up, when cents are hidden", () => {
    const config: PricingConfig = { ...PRICING_CONFIG, SHOW_CENTS_WHEN_FRACTIONAL: false };
    expect(formatUsd(51_750, config)).toBe("$518");
    expect(formatUsd(51_749, config)).toBe("$517");
  });

  it("formats record counts", () => {
    expect(formatRecords(2_500_000)).toBe("2,500,000");
    expect(formatCompactRecords(2_500_000)).toBe("2.5M");
    expect(formatCompactRecords(300_000)).toBe("300K");
  });
});
