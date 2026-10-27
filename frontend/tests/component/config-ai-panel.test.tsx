import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ConfigAiPanel, type ConfigAiPanelProps } from "@/components/config/config-ai-panel";

/**
 * The config AI panel's render branches (V7).
 *
 * The branch that matters most is the **split permission**: drafting writes a
 * version and is admin-only, while explaining writes nothing and is open to
 * everyone. Getting that backwards — or collapsing it to one gate — would put
 * the rules learning curve behind the permission that presumes you have
 * already climbed it, which is the complaint this feature exists to answer.
 *
 * The second is that a **draft reads as a draft**. This panel sits directly
 * above editors that DO take effect on publish, so if its copy ever reads as
 * "done", someone approves a model's guess without looking at the diff.
 *
 * `declined` is its own branch rather than an error, because the authoring
 * prompt explicitly asks the model to leave the config alone when a request is
 * ambiguous. Rendering that as a failure would train people to rephrase until
 * something — anything — lands.
 */

function props(over: Partial<ConfigAiPanelProps> = {}): ConfigAiPanelProps {
  return {
    canDraft: true,
    instruction: "",
    block: "blocking",
    explanation: null,
    draftSummary: null,
    declined: null,
    drafting: false,
    explaining: false,
    failure: null,
    onInstructionChange: vi.fn(),
    onBlockChange: vi.fn(),
    onDraft: vi.fn(),
    onExplain: vi.fn(),
    ...over,
  };
}

describe("ConfigAiPanel", () => {
  it("says up front that a described change is only a draft", () => {
    render(<ConfigAiPanel {...props()} />);
    expect(screen.getByTestId("config-ai").textContent).toContain("draft");
    expect(screen.getByTestId("config-ai").textContent).toContain("nothing is published");
  });

  it("offers explaining to a non-admin and drafting to nobody else", () => {
    render(<ConfigAiPanel {...props({ canDraft: false })} />);
    // The half that writes nothing stays available…
    expect(screen.getByTestId("config-ai-explain")).toBeDefined();
    expect(screen.getByTestId("config-ai-block")).toBeDefined();
    // …and the half that writes is absent, not merely disabled.
    expect(screen.queryByTestId("config-ai-instruction")).toBeNull();
    expect(screen.queryByTestId("config-ai-draft")).toBeNull();
    expect(screen.getByTestId("config-ai-read-only").textContent).toContain("anyone can ask");
  });

  it("gives an admin both halves", () => {
    render(<ConfigAiPanel {...props()} />);
    expect(screen.getByTestId("config-ai-draft")).toBeDefined();
    expect(screen.getByTestId("config-ai-explain")).toBeDefined();
    expect(screen.queryByTestId("config-ai-read-only")).toBeNull();
  });

  it("will not draft from an empty or whitespace-only description", () => {
    for (const instruction of ["", "   "]) {
      const view = render(<ConfigAiPanel {...props({ instruction })} />);
      expect(screen.getByTestId("config-ai-draft"), JSON.stringify(instruction)).toBeDisabled();
      view.unmount();
    }
  });

  it("enables drafting once something is described", () => {
    render(<ConfigAiPanel {...props({ instruction: "raise auto-merge to 0.97" })} />);
    expect(screen.getByTestId("config-ai-draft")).toBeEnabled();
  });

  it("reports a decline as an outcome, not as an error", () => {
    // No `role=alert`, different test id, and the model's own words.
    render(
      <ConfigAiPanel
        {...props({ declined: "survivorship cannot express 'prefer the longest value'" })}
      />,
    );
    const declined = screen.getByTestId("config-ai-declined");
    expect(declined.textContent).toContain("No change made");
    expect(declined.textContent).toContain("longest value");
    expect(declined).not.toHaveAttribute("role", "alert");
    expect(screen.queryByTestId("config-ai-error")).toBeNull();
  });

  it("shows what a successful draft changed", () => {
    render(<ConfigAiPanel {...props({ draftSummary: "Raised auto-merge to 0.97." })} />);
    expect(screen.getByTestId("config-ai-summary").textContent).toContain("Raised auto-merge");
  });

  it("renders an explanation only once there is one", () => {
    const empty = render(<ConfigAiPanel {...props()} />);
    expect(screen.queryByTestId("config-ai-explanation")).toBeNull();
    empty.unmount();

    render(<ConfigAiPanel {...props({ explanation: "It compares emails exactly." })} />);
    expect(screen.getByTestId("config-ai-explanation").textContent).toContain("compares emails");
  });

  it("disables each control while its own request is in flight", () => {
    const drafting = render(<ConfigAiPanel {...props({ instruction: "x", drafting: true })} />);
    expect(screen.getByTestId("config-ai-draft")).toBeDisabled();
    // Explaining is a different request and stays usable — they do not share a
    // spinner because they do not share a cost.
    expect(screen.getByTestId("config-ai-explain")).toBeEnabled();
    drafting.unmount();

    render(<ConfigAiPanel {...props({ instruction: "x", explaining: true })} />);
    expect(screen.getByTestId("config-ai-explain")).toBeDisabled();
    expect(screen.getByTestId("config-ai-draft")).toBeEnabled();
  });

  it("surfaces a real failure as an alert", () => {
    render(<ConfigAiPanel {...props({ failure: "rule authoring is unavailable" })} />);
    const alert = screen.getByTestId("config-ai-error");
    expect(alert).toHaveAttribute("role", "alert");
    expect(alert.textContent).toContain("unavailable");
  });

  it("offers every explainable block and no others", () => {
    render(<ConfigAiPanel {...props()} />);
    const options = [...screen.getByTestId("config-ai-block").querySelectorAll("option")].map(
      (option) => option.value,
    );
    expect(options).toContain("blocking");
    expect(options).toContain("survivorship");
    // `sources` is config but not a RULE; explaining it would get an answer
    // invented for a block the prompt has no description of.
    expect(options).not.toContain("sources");
    expect(options).not.toContain("entity_type");
  });
});
