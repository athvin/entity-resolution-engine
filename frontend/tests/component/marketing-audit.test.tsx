import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";

import { AuditStory } from "@/components/marketing/demo/audit-story";

/**
 * jsdom has no IntersectionObserver, matchMedia or Element.animate, so the
 * film never plays itself here: these tests drive it the way a visitor with
 * a keyboard could, through the step rail and the in-stage controls. The
 * timing is the Playwright tier's (marketing-scenes.spec.ts).
 */
describe("the home audit", () => {
  afterEach(cleanup);

  // Without JavaScript the poster is the whole story, and no control that
  // needs a script is in the HTML to do nothing.
  it("server-renders the poster with the whole story and no dead controls", () => {
    const html = renderToStaticMarkup(<AuditStory onRunning={() => undefined} />);
    expect(html).toContain('data-scene="holds"');
    expect(html).toContain('data-still="true"');
    expect(html).toContain("Kept apart by your never-match rule");
    expect(html).toContain("unmerged Meridian Supply Ltd");
    expect(html).not.toContain("<button");
    expect(html).not.toContain('type="checkbox"');
  });

  it("takes a match through the unmerge to the next run", () => {
    render(<AuditStory onRunning={vi.fn()} />);
    const audit = screen.getByTestId("marketing-audit");
    // Hydrated with motion allowed, it waits at its first frame to be seen.
    expect(audit).toHaveAttribute("data-scene", "zone");
    expect(screen.getByRole("button", { name: "Play the audit" })).toBeVisible();

    fireEvent.click(screen.getByRole("button", { name: "Step 2: Your call" }));
    expect(audit).toHaveAttribute("data-scene", "call");
    expect(screen.getByRole("status")).toHaveTextContent(
      "Your call. 0.93 sits in your review band",
    );
    fireEvent.click(screen.getByRole("button", { name: "Match" }));
    expect(audit).toHaveAttribute("data-scene", "called");
    const history = screen.getByRole("complementary", { name: "History" });
    expect(history).toHaveTextContent("matched Salesforce ↔ API");

    fireEvent.click(screen.getByRole("button", { name: "Step 3: Unmerge" }));
    const unmerge = screen.getByRole("button", { name: /^Unmerge .*selected$/ });
    expect(unmerge).toBeDisabled();
    fireEvent.click(screen.getByRole("checkbox", { name: /Meridian Supply Ltd/ }));
    expect(unmerge).toBeEnabled();
    expect(unmerge).toHaveTextContent("Unmerge 1 selected");
    fireEvent.click(unmerge);
    expect(audit).toHaveAttribute("data-scene", "unmerged");
    expect(screen.getByRole("status")).toHaveTextContent("One never-match rule is written");
    expect(history).toHaveTextContent("unmerged Meridian Supply Ltd");

    fireEvent.click(screen.getByRole("button", { name: "Step 4: It holds" }));
    expect(audit).toHaveAttribute("data-scene", "holds");
    expect(screen.getByText("Joined by your always-match rule")).toBeVisible();
    expect(screen.getByText("Kept apart by your never-match rule")).toBeVisible();
  });

  it("keeps the pair apart when the visitor says it is not a match", () => {
    render(<AuditStory onRunning={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "Step 2: Your call" }));
    fireEvent.click(screen.getByRole("button", { name: "Not a match" }));
    expect(screen.getByRole("status")).toHaveTextContent("Kept apart.");
    fireEvent.click(screen.getByRole("button", { name: "Step 4: It holds" }));
    expect(screen.getByText(/Kept apart by your never-match rule, as E-2077/)).toBeVisible();
  });

  it("asks the AI only when the visitor does, and leaves the decision with them", () => {
    render(<AuditStory onRunning={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "Step 2: Your call" }));
    const ask = screen.getByRole("button", { name: "Ask AI for a recommendation" });
    expect(ask).toHaveAttribute("aria-expanded", "false");
    fireEvent.click(ask);
    expect(ask).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText(/The decision stays yours/)).toBeVisible();
  });

  // Only the current shot can be reached: the others keep their place in the
  // window, out of the accessibility tree and the tab order.
  it("hides every shot but the current one from assistive technology", () => {
    render(<AuditStory onRunning={vi.fn()} />);
    expect(screen.queryByRole("button", { name: "Match" })).toBeNull();
    const shots = document.querySelectorAll<HTMLElement>(".audit-shot");
    expect(shots).toHaveLength(4);
    const active = [...shots].filter((shot) => shot.dataset.active === "true");
    expect(active).toHaveLength(1);
    for (const shot of shots) {
      if (shot.dataset.active === "true") continue;
      expect(shot).toHaveAttribute("aria-hidden", "true");
      expect(shot).toHaveAttribute("inert");
    }
    const rail = screen.getByRole("list", { name: "The audit, step by step" });
    expect(within(rail).getByRole("button", { name: "Step 1: The gray zone" })).toHaveAttribute(
      "aria-current",
      "step",
    );
  });
});
