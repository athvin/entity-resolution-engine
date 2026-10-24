import { describe, expect, it } from "vitest";

import { MAX_STAGGER_STEPS, stagger } from "@/lib/motion";

/**
 * The assertions here pin the two properties the rest of the motion system
 * depends on and which are easy to break without noticing: that the delay is
 * expressed in terms of `--stagger-step` rather than a hardcoded duration, and
 * that a hostile index cannot emit a value that never resolves.
 *
 * The second matters more than it looks. `animation-delay` with an unparseable
 * value is dropped, which is harmless — but an index of `NaN` or `Infinity`
 * reaching a `calc()` makes the whole declaration invalid, and the element
 * keeps whatever `animate-*` set, i.e. it animates immediately and out of
 * sequence. That is a visual bug with no error attached to it.
 */
describe("stagger", () => {
  it("expresses the delay in terms of the shared scale", () => {
    // Not `0ms`/`55ms`: the point of the helper is that changing
    // --stagger-step in globals.css retimes every staggered group at once.
    expect(stagger(0).animationDelay).toBe("calc(var(--stagger-step) * 0)");
    expect(stagger(3).animationDelay).toBe("calc(var(--stagger-step) * 3)");
  });

  it("caps the delay so a long list's tail stays part of one gesture", () => {
    expect(stagger(MAX_STAGGER_STEPS).animationDelay).toBe(
      `calc(var(--stagger-step) * ${String(MAX_STAGGER_STEPS)})`,
    );
    // A 60-item list must not put its last row 3.3 seconds after its first.
    expect(stagger(60).animationDelay).toBe(stagger(MAX_STAGGER_STEPS).animationDelay);
  });

  it("floors a negative index at zero rather than emitting a negative delay", () => {
    // A negative animation-delay is legal CSS and means "start mid-animation",
    // so this would silently render a partially-faded row.
    expect(stagger(-1).animationDelay).toBe("calc(var(--stagger-step) * 0)");
  });

  it("emits an integer multiplier for any finite input", () => {
    expect(stagger(2.7).animationDelay).toBe("calc(var(--stagger-step) * 2)");
  });

  it("never emits NaN or Infinity into the calc()", () => {
    for (const bad of [Number.NaN, Number.POSITIVE_INFINITY, Number.NEGATIVE_INFINITY]) {
      const delay = stagger(bad).animationDelay ?? "";
      expect(delay).not.toContain("NaN");
      expect(delay).not.toContain("Infinity");
    }
  });
});
