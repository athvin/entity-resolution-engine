/**
 * The bulk-adjudication cost cap (V6).
 *
 * The cap is the only thing between "ask about the selected pairs" and an
 * unbounded spend on a tenant's model account, and the way it fails is the
 * point: **truncating silently** would report success for pairs that were never
 * asked about, leaving a blank badge that reads as "the model had no opinion"
 * rather than "we did not ask". So over-cap is a refusal, not a trim.
 */
import { describe, expect, it } from "vitest";

import { MAX_BULK, parseBulkAdjudicate } from "@/lib/bff/adjudicate-bulk";

const ids = (n: number) =>
  Array.from({ length: n }, (_, i) => `01REVIEW${String(i).padStart(3, "0")}`);

describe("the cost cap", () => {
  it("accepts exactly the cap", () => {
    const parsed = parseBulkAdjudicate({ review_ids: ids(MAX_BULK) });
    expect(parsed.ok).toBe(true);
    if (parsed.ok) expect(parsed.value.review_ids).toHaveLength(MAX_BULK);
  });

  it("REFUSES past the cap rather than truncating", () => {
    // The whole point. A trimmed list would answer 200 for ids nobody asked
    // the model about.
    const parsed = parseBulkAdjudicate({ review_ids: ids(MAX_BULK + 1) });
    expect(parsed.ok).toBe(false);
    if (!parsed.ok) expect(parsed.reason).toContain("review_ids");
  });

  it("is a count, not a token budget", () => {
    // A number a steward can decide about before pressing the button; a token
    // estimate is not, and the UI could only guess at it.
    expect(Number.isInteger(MAX_BULK)).toBe(true);
    expect(MAX_BULK).toBeGreaterThan(0);
  });
});

describe("the request shape", () => {
  it("refuses an empty list", () => {
    // Zero pairs is a caller bug, not a no-op worth a round trip.
    expect(parseBulkAdjudicate({ review_ids: [] }).ok).toBe(false);
  });

  it("refuses a missing or non-array field", () => {
    for (const body of [{}, { review_ids: "01REVIEW000" }, null, [], 7]) {
      expect(parseBulkAdjudicate(body).ok, JSON.stringify(body)).toBe(false);
    }
  });

  it("refuses a blank id rather than asking about nothing", () => {
    expect(parseBulkAdjudicate({ review_ids: ["01REVIEW000", "   "] }).ok).toBe(false);
  });

  it("de-duplicates, so a repeated id does not spend a second call", () => {
    // Also keeps the `asked` count the caller sees equal to the number of pairs
    // actually judged.
    const parsed = parseBulkAdjudicate({
      review_ids: ["01REVIEW000", "01REVIEW000", "01REVIEW001"],
    });
    expect(parsed.ok).toBe(true);
    if (parsed.ok) expect(parsed.value.review_ids).toEqual(["01REVIEW000", "01REVIEW001"]);
  });

  it("de-duplicates AFTER the cap check, so duplicates cannot smuggle a longer list", () => {
    // Were it the other way round, 20 ids that collapse to 10 would pass — and
    // the caller's intent was still a 20-pair request.
    expect(parseBulkAdjudicate({ review_ids: ids(MAX_BULK).concat(ids(MAX_BULK)) }).ok).toBe(false);
  });

  it("names the offending field rather than a generic message", () => {
    const parsed = parseBulkAdjudicate({ review_ids: ids(MAX_BULK + 5) });
    expect(parsed.ok).toBe(false);
    if (!parsed.ok) expect(parsed.reason).toContain("review_ids");
  });
});
