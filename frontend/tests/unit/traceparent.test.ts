import { describe, expect, it } from "vitest";

import {
  TRACEPARENT_LENGTH,
  isAcceptableTraceparent,
  mintTraceparent,
  resolveTraceparent,
} from "@/lib/telemetry/traceparent";

/**
 * These assertions mirror `er_server::acceptable_traceparent`.
 *
 * They matter more than they look. erserver does not reject a bad traceparent
 * loudly — it stores `None`, so `jobs.trace_context` ends up empty and the
 * trace stops at the BFF with nothing logged on either side. If this rule and
 * the Rust one drift apart, the BFF forwards contexts that silently vanish.
 *
 * Each case below names the Rust clause it covers, so a future change to that
 * function has somewhere obvious to land.
 */
describe("isAcceptableTraceparent mirrors erserver's rule", () => {
  const valid = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

  it("accepts the W3C level-1 example", () => {
    expect(valid).toHaveLength(TRACEPARENT_LENGTH);
    expect(isAcceptableTraceparent(valid)).toBe(true);
  });

  it("rejects a version other than 00, including future ones", () => {
    // The Rust comment is explicit that this is deliberate: a `01-` context may
    // carry fields whose meaning we would be guessing at, so it is refused
    // rather than stored uninterpreted. This is the case a newer browser
    // tracing library would actually produce.
    expect(isAcceptableTraceparent(valid.replace(/^00/, "01"))).toBe(false);
    expect(isAcceptableTraceparent(valid.replace(/^00/, "ff"))).toBe(false);
  });

  it("rejects an all-zero trace id or span id", () => {
    // Both are invalid per the spec.
    expect(isAcceptableTraceparent(`00-${"0".repeat(32)}-00f067aa0ba902b7-01`)).toBe(false);
    expect(
      isAcceptableTraceparent(`00-4bf92f3577b34da6a3ce929d0e0e4736-${"0".repeat(16)}-01`),
    ).toBe(false);
  });

  it("rejects uppercase, so one context cannot arrive as two strings", () => {
    expect(isAcceptableTraceparent(valid.toUpperCase())).toBe(false);
    expect(isAcceptableTraceparent("00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01")).toBe(
      false,
    );
  });

  it("rejects wrong lengths and wrong field counts", () => {
    expect(isAcceptableTraceparent(valid.slice(0, -1))).toBe(false);
    expect(isAcceptableTraceparent(`${valid}0`)).toBe(false);
    expect(isAcceptableTraceparent(`${valid}-extra`)).toBe(false);
    expect(isAcceptableTraceparent("")).toBe(false);
  });

  it("rejects non-hex characters that are still the right length", () => {
    expect(isAcceptableTraceparent("00-4bf92f3577b34da6a3ce929d0e0e473g-00f067aa0ba902b7-01")).toBe(
      false,
    );
  });
});

describe("mintTraceparent", () => {
  it("produces something erserver accepts", () => {
    for (let i = 0; i < 50; i += 1) {
      const minted = mintTraceparent();
      expect(isAcceptableTraceparent(minted), minted).toBe(true);
    }
  });

  it("produces a distinct trace id each time", () => {
    const ids = new Set(Array.from({ length: 50 }, () => mintTraceparent().split("-")[1]));
    expect(ids.size).toBe(50);
  });

  it("marks the context sampled", () => {
    // The sampling decision belongs to the collector's tail_sampling, which
    // cannot judge a trace that never arrives.
    expect(mintTraceparent().endsWith("-01")).toBe(true);
  });
});

describe("resolveTraceparent", () => {
  const valid = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

  it("inherits an acceptable incoming context", () => {
    expect(resolveTraceparent(valid)).toEqual({ traceparent: valid, inherited: true });
  });

  it("tolerates surrounding whitespace", () => {
    expect(resolveTraceparent(`  ${valid}  `)).toEqual({ traceparent: valid, inherited: true });
  });

  it("mints when there is nothing to inherit", () => {
    for (const incoming of [null, undefined, "", "   "]) {
      const resolved = resolveTraceparent(incoming);
      expect(resolved.inherited).toBe(false);
      expect(isAcceptableTraceparent(resolved.traceparent)).toBe(true);
    }
  });

  it("mints rather than forwarding a context erserver would drop", () => {
    // The whole point: a new trace beats a broken one. Forwarding this would
    // produce a request whose trace_context is silently NULL downstream.
    const unusable = valid.replace(/^00/, "01");
    const resolved = resolveTraceparent(unusable);
    expect(resolved.inherited).toBe(false);
    expect(resolved.traceparent).not.toBe(unusable);
    expect(isAcceptableTraceparent(resolved.traceparent)).toBe(true);
  });
});
