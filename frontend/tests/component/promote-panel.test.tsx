import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { PromotePanel, type PromotePanelProps } from "@/components/config/promote-panel";
import { promotableOrgs } from "@/lib/query/promote";

/**
 * The config promotion panel (O9).
 *
 * The branch that carries the feature is **import being gated behind compare**.
 * Copying a configuration from one tenant into another without reading the diff
 * is the mistake this surface exists to prevent: the two tenants' sources may
 * differ, and the engine does not say so until publish — by which point the
 * draft has been approved by someone who never saw what changed.
 *
 * The diff rendering is asserted too, because `DiffLine.kind` is `added` /
 * `removed` and my first draft matched `add` / `remove`. Every line would have
 * rendered as unchanged, with no error anywhere — a diff that silently claims
 * two configs are identical is worse than no diff at all.
 */

function props(over: Partial<PromotePanelProps> = {}): PromotePanelProps {
  return {
    candidates: ["acme-prod", "acme-staging"],
    target: "acme-prod",
    diff: null,
    theirVersion: null,
    comparing: false,
    importing: false,
    note: null,
    failure: null,
    onTargetChange: vi.fn(),
    onCompare: vi.fn(),
    onImport: vi.fn(),
    ...over,
  };
}

describe("PromotePanel", () => {
  it("will not import before a diff has been read", () => {
    render(<PromotePanel {...props()} />);
    expect(screen.getByTestId("promote-compare")).toBeEnabled();
    expect(screen.getByTestId("promote-import")).toBeDisabled();
  });

  it("enables import once a diff is on screen", () => {
    render(<PromotePanel {...props({ diff: [{ kind: "added", text: "review_low: 0.7" }] })} />);
    expect(screen.getByTestId("promote-import")).toBeEnabled();
  });

  it("says WHY import is unavailable rather than just disabling it", () => {
    render(<PromotePanel {...props()} />);
    expect(screen.getByTestId("promote-import").getAttribute("title")).toContain("Compare first");
  });

  it("marks added and removed lines distinctly", () => {
    // `added`/`removed`, not `add`/`remove`. Getting this wrong renders every
    // line as context and the diff silently claims nothing changed.
    render(
      <PromotePanel
        {...props({
          diff: [
            { kind: "same", text: "tenant: acme" },
            { kind: "added", text: "review_low: 0.7" },
            { kind: "removed", text: "review_low: 0.6" },
          ],
        })}
      />,
    );
    const text = screen.getByTestId("promote-diff").textContent;
    expect(text).toContain("+ review_low: 0.7");
    expect(text).toContain("- review_low: 0.6");
    expect(text).toContain("  tenant: acme");
  });

  it("says plainly when the two configs are identical", () => {
    // An empty diff is a real answer — the dev→prod case where promotion has
    // already happened — and an empty <pre> would read as a render failure.
    render(<PromotePanel {...props({ diff: [], theirVersion: 4 })} />);
    expect(screen.getByTestId("promote-identical").textContent).toContain("Identical");
    expect(screen.getByTestId("promote-identical").textContent).toContain("v4");
  });

  it("tells a single-workspace admin there is nothing to promote", () => {
    render(<PromotePanel {...props({ candidates: [] })} />);
    expect(screen.getByTestId("promote-none")).toBeDefined();
    expect(screen.queryByTestId("promote-compare")).toBeNull();
  });

  it("says a copy lands as a draft, not a publish", () => {
    render(<PromotePanel {...props()} />);
    expect(screen.getByTestId("promote-panel").textContent).toContain("draft");
    expect(screen.getByTestId("promote-import").textContent).toContain("draft");
  });

  it("disables each control while its own request is in flight", () => {
    const comparing = render(<PromotePanel {...props({ comparing: true })} />);
    expect(screen.getByTestId("promote-compare")).toBeDisabled();
    comparing.unmount();

    render(<PromotePanel {...props({ diff: [], importing: true })} />);
    expect(screen.getByTestId("promote-import")).toBeDisabled();
  });

  it("surfaces a failure as an alert", () => {
    render(<PromotePanel {...props({ failure: "not found" })} />);
    expect(screen.getByTestId("promote-error")).toHaveAttribute("role", "alert");
  });
});

describe("which orgs may be promoted between", () => {
  const memberships = [
    { org: "acme-dev", role: "admin" },
    { org: "acme-prod", role: "admin" },
    { org: "other-dev", role: "steward" },
    { org: "third-dev", role: "viewer" },
  ];

  it("offers only orgs this person ADMINISTERS", () => {
    // A steward cannot publish, so offering their org would produce a 403 from
    // the far side that reads as a bug.
    expect(promotableOrgs(memberships, "acme-dev")).toEqual(["acme-prod"]);
  });

  it("never offers the current workspace", () => {
    expect(promotableOrgs(memberships, "acme-prod")).toEqual(["acme-dev"]);
  });

  it("returns nothing when this is the only org administered", () => {
    expect(promotableOrgs([{ org: "acme-dev", role: "admin" }], "acme-dev")).toEqual([]);
  });

  it("sorts, so the picker does not reshuffle between renders", () => {
    const shuffled = [
      { org: "zeta", role: "admin" },
      { org: "alpha", role: "admin" },
    ];
    expect(promotableOrgs(shuffled, "acme-dev")).toEqual(["alpha", "zeta"]);
  });
});
