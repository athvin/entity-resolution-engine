import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import {
  RecommendationBadge,
  type RecommendationBadgeProps,
} from "@/components/review/recommendation-badge";
import type { ReviewRecommendation } from "@/lib/query/adjudicate";

/**
 * The AI recommendation badge's render branches (V6).
 *
 * Two things are being guarded, and neither would throw if it broke.
 *
 * The first is **the advisory framing**. This badge sits directly above the
 * three resolve buttons, and if it ever reads as a decision already taken, a
 * steward rubber-stamps a merge the model guessed at. The word "suggests", the
 * explicit confidence and the model's name are the whole defence, so they are
 * asserted rather than assumed.
 *
 * The second is the **grayscale rule** (`docs/frontend-design.md` §3.3). Three
 * verdicts is the most tempting place in the product to reach for
 * green/red/grey, and with no palette the only channel is the glyph — so the
 * glyph must differ per verdict and must match the button the steward will
 * press.
 */

function recommendation(over: Partial<ReviewRecommendation> = {}): ReviewRecommendation {
  return {
    review_id: "01REVIEW001",
    verdict: "match",
    confidence: 0.86,
    reasoning: "Same email and the same phone.",
    model: "claude-opus-5",
    match_probability: 0.81,
    requested_by: "steward@acme.dev",
    created_at: "2026-10-24T09:00:00Z",
    ...over,
  };
}

function props(over: Partial<RecommendationBadgeProps> = {}): RecommendationBadgeProps {
  return {
    recommendation: recommendation(),
    canRequest: true,
    busy: false,
    failure: null,
    onRequest: vi.fn(),
    ...over,
  };
}

describe("RecommendationBadge", () => {
  it("frames the verdict as a suggestion, not a decision", () => {
    render(<RecommendationBadge {...props()} />);
    const badge = screen.getByTestId("recommendation");
    expect(badge.textContent).toContain("AI suggests");
    // The explicit reminder that the steward's own resolve is the real act.
    expect(badge.textContent).toContain("your resolve below is what counts");
  });

  it("names the model on every answer", () => {
    // An accuracy claim that does not say whose it is cannot be checked later.
    render(<RecommendationBadge {...props()} />);
    expect(screen.getByTestId("recommendation-model").textContent).toBe("claude-opus-5");
  });

  it("shows confidence as a bounded whole percentage", () => {
    render(<RecommendationBadge {...props()} />);
    expect(screen.getByTestId("recommendation-confidence").textContent).toBe("86% sure");
  });

  it("renders a distinct glyph per verdict, since colour is unavailable", () => {
    const seen = new Set<string>();
    for (const verdict of ["match", "no_match", "skip"]) {
      const view = render(
        <RecommendationBadge {...props({ recommendation: recommendation({ verdict }) })} />,
      );
      const svg = screen.getByTestId("recommendation-verdict").querySelector("svg");
      const name = svg?.getAttribute("class") ?? "";
      expect(name, verdict).not.toBe("");
      seen.add(name);
      view.unmount();
    }
    // Three verdicts, three different marks — not one shape distinguished only
    // by a hue that does not exist here.
    expect(seen.size).toBe(3);
  });

  it("puts each verdict in plain words rather than the wire value", () => {
    const cases: [string, string][] = [
      ["match", "same entity"],
      ["no_match", "different entities"],
      ["skip", "not settled"],
    ];
    for (const [verdict, words] of cases) {
      const view = render(
        <RecommendationBadge {...props({ recommendation: recommendation({ verdict }) })} />,
      );
      expect(screen.getByTestId("recommendation-verdict").textContent, verdict).toContain(words);
      view.unmount();
    }
  });

  it("renders an unknown verdict as itself rather than hiding it", () => {
    // Forward compatibility: a verdict this build does not know is still shown,
    // because silently dropping it would look like no recommendation exists.
    render(
      <RecommendationBadge {...props({ recommendation: recommendation({ verdict: "link" }) })} />,
    );
    expect(screen.getByTestId("recommendation-verdict").textContent).toContain("link");
  });

  it("offers the ask control to a steward when nothing has been asked", () => {
    render(<RecommendationBadge {...props({ recommendation: undefined })} />);
    expect(screen.getByTestId("recommendation-request")).toBeDefined();
  });

  it("tells a viewer there is none, rather than offering a button they cannot use", () => {
    // Reading an opinion is free; asking for one spends a tenant's tokens.
    render(<RecommendationBadge {...props({ recommendation: undefined, canRequest: false })} />);
    expect(screen.queryByTestId("recommendation-request")).toBeNull();
    expect(screen.getByTestId("recommendation-none")).toBeDefined();
  });

  it("hides the re-ask control from a viewer but still shows them the answer", () => {
    render(<RecommendationBadge {...props({ canRequest: false })} />);
    expect(screen.getByTestId("recommendation")).toBeDefined();
    expect(screen.queryByTestId("recommendation-redo")).toBeNull();
  });

  it("disables the controls while a request is in flight", () => {
    const asked = render(<RecommendationBadge {...props({ busy: true })} />);
    expect(screen.getByTestId("recommendation-redo")).toBeDisabled();
    asked.unmount();

    render(<RecommendationBadge {...props({ recommendation: undefined, busy: true })} />);
    expect(screen.getByTestId("recommendation-request")).toBeDisabled();
  });

  it("surfaces a failure as an alert in both states", () => {
    const empty = render(
      <RecommendationBadge
        {...props({ recommendation: undefined, failure: "adjudication is unavailable" })}
      />,
    );
    expect(screen.getByTestId("recommendation-error")).toHaveAttribute("role", "alert");
    empty.unmount();

    // Also while a previous answer is on screen: a failed re-ask must not look
    // like it succeeded and produced the old answer again.
    render(<RecommendationBadge {...props({ failure: "the model could not be reached" })} />);
    expect(screen.getByTestId("recommendation-error").textContent).toContain(
      "could not be reached",
    );
  });
});
