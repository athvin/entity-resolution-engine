/**
 * Saved views (O12a).
 *
 * Two refusals carry this, and both exist because the failure is a view that
 * looks deliberate to whoever finds it later:
 *
 * - **an empty definition** is a bookmark for "everything", which is the state
 *   the screen already opens in. It is almost always Save pressed before the
 *   filter was set, and the name the person chose makes it look intentional.
 * - **an inverted band** (min above max) matches nothing, and the surface
 *   renders an empty list rather than an error — the silent-wrongness shape
 *   this program keeps finding.
 */
import { describe, expect, it } from "vitest";

import { isEmptyDefinition, parseCreateView, VIEW_SURFACES } from "@/lib/views/define";

const definition = { q: "", min: null, max: null, reason: null };

describe("the surface vocabulary", () => {
  it("is closed to the three browsable screens", () => {
    expect([...VIEW_SURFACES]).toEqual(["records", "duplicates", "reviews"]);
  });

  it("refuses a surface that cannot be filtered", () => {
    // A records filter is meaningless on a screen with no records; offering it
    // would be a dead control.
    for (const surface of ["runs", "settings", "dashboard", ""]) {
      const parsed = parseCreateView({ surface, name: "x", definition: { ...definition, q: "a" } });
      expect(parsed.ok, surface).toBe(false);
    }
  });

  it("accepts each real surface", () => {
    for (const surface of VIEW_SURFACES) {
      const parsed = parseCreateView({
        surface,
        name: "Mine",
        definition: { ...definition, q: "acme" },
      });
      expect(parsed.ok, surface).toBe(true);
    }
  });
});

describe("a view must actually filter", () => {
  it("recognises a definition that narrows nothing", () => {
    expect(isEmptyDefinition(definition)).toBe(true);
  });

  it("recognises each way a definition can narrow", () => {
    expect(isEmptyDefinition({ ...definition, q: "acme" })).toBe(false);
    expect(isEmptyDefinition({ ...definition, min: 0.6 })).toBe(false);
    expect(isEmptyDefinition({ ...definition, max: 0.9 })).toBe(false);
    expect(isEmptyDefinition({ ...definition, reason: "gray_band" })).toBe(false);
  });

  it("treats a whitespace-only search as empty after trimming", () => {
    // zod trims, so `"   "` arrives as `""` and the emptiness check sees it.
    const parsed = parseCreateView({
      surface: "records",
      name: "x",
      definition: { ...definition, q: "   " },
    });
    expect(parsed.ok).toBe(true);
    if (parsed.ok) expect(isEmptyDefinition(parsed.value.definition)).toBe(true);
  });
});

describe("the band must be satisfiable", () => {
  it("REFUSES a minimum above its maximum", () => {
    // Matches nothing, and the surface renders an empty list rather than an
    // error — so it is refused where the reader can still see what they typed.
    const parsed = parseCreateView({
      surface: "reviews",
      name: "Impossible",
      definition: { ...definition, min: 0.9, max: 0.6 },
    });
    expect(parsed.ok).toBe(false);
    if (!parsed.ok) expect(parsed.reason).toContain("minimum is above its maximum");
  });

  it("allows a band with only one side", () => {
    // "Above 0.9" and "below 0.6" are both real filters.
    expect(
      parseCreateView({
        surface: "reviews",
        name: "High",
        definition: { ...definition, min: 0.9 },
      }).ok,
    ).toBe(true);
    expect(
      parseCreateView({
        surface: "reviews",
        name: "Low",
        definition: { ...definition, max: 0.6 },
      }).ok,
    ).toBe(true);
  });

  it("allows a degenerate band where the two sides meet", () => {
    expect(
      parseCreateView({
        surface: "reviews",
        name: "Exact",
        definition: { ...definition, min: 0.7, max: 0.7 },
      }).ok,
    ).toBe(true);
  });

  it("refuses a probability outside 0–1", () => {
    for (const min of [-0.1, 1.5]) {
      expect(
        parseCreateView({ surface: "reviews", name: "x", definition: { ...definition, min } }).ok,
        String(min),
      ).toBe(false);
    }
  });
});

describe("the name", () => {
  it("is required and bounded", () => {
    const d = { ...definition, q: "acme" };
    expect(parseCreateView({ surface: "records", name: "", definition: d }).ok).toBe(false);
    expect(parseCreateView({ surface: "records", name: "   ", definition: d }).ok).toBe(false);
    expect(parseCreateView({ surface: "records", name: "x".repeat(81), definition: d }).ok).toBe(
      false,
    );
  });

  it("is trimmed, so two views cannot differ only by whitespace", () => {
    // The unique index is on the exact string; without trimming, `"Mine"` and
    // `"Mine "` would both be storable and indistinguishable in the picker.
    const parsed = parseCreateView({
      surface: "records",
      name: "  Mine  ",
      definition: { ...definition, q: "acme" },
    });
    expect(parsed.ok && parsed.value.name).toBe("Mine");
  });

  it("names the offending field rather than a generic message", () => {
    const parsed = parseCreateView({ surface: "records", definition });
    expect(parsed.ok).toBe(false);
    if (!parsed.ok) expect(parsed.reason).toContain("name");
  });
});
