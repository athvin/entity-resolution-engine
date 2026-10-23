/**
 * The labelling queue's decision logic.
 *
 * These three functions are extracted from `labeling-content.tsx` rather than
 * inlined in its handlers, because each encodes a rule that was got wrong once
 * and is invisible when wrong:
 *
 * - advancing by index against a list that shifts under you skips a pair;
 * - a permission check written as a negation treats a *loading* session as
 *   permission;
 * - a verdict that drops `model_version`/`score_at_label` is still a valid
 *   label and still trains the model, but can never be measured.
 *
 * None of the three throws, logs, or renders differently when broken. That is
 * what makes them worth a unit test rather than only an end-to-end one.
 */
import { describe, expect, it } from "vitest";

import { parseLabelInput } from "@/lib/bff/labels";
import {
  canRecordLabel,
  labelPayloadFor,
  nextUnskipped,
  pairKey,
  type LabelCandidate,
} from "@/lib/query/labeling";

function candidate(recA: string, recB: string, probability: number): LabelCandidate {
  return {
    rec_a_key: recA,
    rec_b_key: recB,
    match_probability: probability,
    model_version: "v0001",
    run_id: "01jm0000000000000000000002",
    scored_at: "2026-09-24T06:01:00Z",
  };
}

describe("which pair to ask about", () => {
  it("offers the head of the queue when nothing is skipped", () => {
    const items = [candidate("a", "b", 0.94), candidate("c", "d", 0.91)];
    expect(nextUnskipped(items, new Set())?.rec_a_key).toBe("a");
  });

  it("steps over skipped pairs in order", () => {
    const items = [candidate("a", "b", 0.94), candidate("c", "d", 0.91)];
    const skipped = new Set([pairKey("a", "b")]);
    expect(nextUnskipped(items, skipped)?.rec_a_key).toBe("c");
  });

  it("does not skip a pair when the list shifts under it", () => {
    // The bug this replaced: the component advanced an index on a successful
    // label while the server removed that pair from the queue, so the list
    // moved down AND the cursor moved up — one pair never got asked about.
    // Here the labelled head is simply gone and nothing was skipped, so the
    // next ask is the new head rather than the one after it.
    const before = [
      candidate("a", "b", 0.94),
      candidate("c", "d", 0.91),
      candidate("e", "f", 0.88),
    ];
    const afterServerExcludedTheLabelledPair = before.slice(1);
    expect(nextUnskipped(afterServerExcludedTheLabelledPair, new Set())?.rec_a_key).toBe("c");
  });

  it("returns undefined once every pair is skipped", () => {
    const items = [candidate("a", "b", 0.94)];
    expect(nextUnskipped(items, new Set([pairKey("a", "b")]))).toBeUndefined();
    expect(nextUnskipped([], new Set())).toBeUndefined();
  });
});

describe("who may record a verdict", () => {
  it("allows a steward and an admin", () => {
    expect(canRecordLabel("steward")).toBe(true);
    expect(canRecordLabel("admin")).toBe(true);
  });

  it("refuses a viewer", () => {
    expect(canRecordLabel("viewer")).toBe(false);
  });

  it("refuses a session that has not resolved", () => {
    // `effectiveRole` returns null while loading. The obvious spelling,
    // `role !== "viewer"`, is TRUE here — so a viewer was offered the label
    // buttons for as long as the session took to settle.
    expect(canRecordLabel(null)).toBe(false);
  });
});

describe("the verdict sent for a pair", () => {
  it("carries the model version and the score the labeller saw", () => {
    // The contract with per-version evaluation. Without these two fields the
    // label is still valid and still trains, but
    // `/models/{name}/versions/{v}/evaluation` can never count it — it reports
    // "no label can be scored against this version" with nothing to explain
    // why. Nothing fails loudly if they go missing, which is the point.
    const payload = labelPayloadFor(candidate("billing:L-200", "crm:L-100", 0.94), "match");
    expect(payload).toEqual({
      rec_a_key: "billing:L-200",
      rec_b_key: "crm:L-100",
      verdict: "match",
      model_version: "v0001",
      score_at_label: 0.94,
    });
  });

  it("sends the score the model gave, not the threshold or a rounding of it", () => {
    const payload = labelPayloadFor(candidate("a", "b", 0.9375), "no_match");
    expect(payload.score_at_label).toBe(0.9375);
    expect(payload.verdict).toBe("no_match");
  });

  it("passes a null model version through rather than inventing one", () => {
    // A pair scored before the registry existed has no version. Substituting
    // the active one would attribute a score to a model that never produced it.
    const item = { ...candidate("a", "b", 0.9), model_version: null };
    expect(labelPayloadFor(item, "match").model_version).toBeNull();
  });
});

describe("the BFF's verdict validation", () => {
  const valid = {
    rec_a_key: "billing:L-200",
    rec_b_key: "crm:L-100",
    verdict: "match",
    model_version: "v0001",
    score_at_label: 0.94,
  };

  it("accepts a well-formed verdict", () => {
    const parsed = parseLabelInput(valid);
    expect(parsed.ok).toBe(true);
    if (parsed.ok) expect(parsed.value.score_at_label).toBe(0.94);
  });

  it("accepts a pair with no model version or score", () => {
    // A pair scored before the registry existed has neither. Still valid
    // training signal; simply not measurable per version.
    const parsed = parseLabelInput({
      rec_a_key: "a:1",
      rec_b_key: "b:2",
      verdict: "no_match",
    });
    expect(parsed.ok).toBe(true);
  });

  it("refuses a verdict outside the closed set", () => {
    const parsed = parseLabelInput({ ...valid, verdict: "maybe" });
    expect(parsed.ok).toBe(false);
    if (!parsed.ok) expect(parsed.reason).toContain("verdict");
  });

  it("refuses a score that is not a probability", () => {
    // The range matters more than the type: a value outside [0,1] would make
    // every precision figure computed from it meaningless, and silently.
    for (const score of [1.5, -0.1]) {
      const parsed = parseLabelInput({ ...valid, score_at_label: score });
      expect(parsed.ok, `score ${String(score)} should be refused`).toBe(false);
    }
  });

  it("refuses a record labelled against itself", () => {
    const parsed = parseLabelInput({ ...valid, rec_a_key: "a:1", rec_b_key: "a:1" });
    expect(parsed.ok).toBe(false);
    if (!parsed.ok) expect(parsed.reason).toContain("itself");
  });

  it("refuses an empty key", () => {
    expect(parseLabelInput({ ...valid, rec_a_key: "" }).ok).toBe(false);
  });
});
