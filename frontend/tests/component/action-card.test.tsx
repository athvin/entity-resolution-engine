import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ActionCard, type ActionCardProps } from "@/components/assistant/action-card";
import type { ProposedAction } from "@/lib/agent-actions/propose";

/**
 * The confirm card (V9).
 *
 * **This component is the security boundary**, which makes its copy
 * load-bearing in a way most UI copy is not. The assistant's tools return
 * record data; a record can contain text shaped like an instruction; the entire
 * defence is that such an instruction reaches, at most, a card a person reads
 * and declines. A card that reads as "apply suggestion" gives them nothing to
 * disagree with, and the attack succeeds on inattention alone.
 *
 * So these assert what the reader is told: the effect in plain language, the
 * subject by name, the rationale visibly attributed to the model rather than to
 * the product, and a decline control weighted equally with confirm.
 */

const action: ProposedAction = {
  kind: "suppress_record",
  subject: "crm:C001",
  rationale: "duplicate of crm:C002 and not updated since 2019",
};

function props(over: Partial<ActionCardProps> = {}): ActionCardProps {
  return {
    action,
    state: "pending",
    failure: null,
    onConfirm: vi.fn(),
    onDecline: vi.fn(),
    ...over,
  };
}

describe("ActionCard", () => {
  it("states the EFFECT, not the action's name", () => {
    render(<ActionCard {...props()} />);
    const effect = screen.getByTestId("action-effect").textContent;
    expect(effect).toContain("stops feeding golden records");
    // The wire name must not be what the reader has to interpret.
    expect(effect).not.toContain("suppress_record");
  });

  it("names the subject, so the action is never ambiguous about which record", () => {
    render(<ActionCard {...props()} />);
    expect(screen.getByTestId("action-effect").textContent).toContain("crm:C001");
  });

  it("attributes the rationale to the assistant rather than to the product", () => {
    // This text passed through a model that read customer records. Presenting
    // it unattributed would make an injected sentence read as our advice.
    render(<ActionCard {...props()} />);
    expect(screen.getByTestId("action-card-suppress_record").textContent).toContain(
      "The assistant says",
    );
  });

  it("weights decline equally with confirm", () => {
    // Not an X in a corner: declining is the expected outcome for a proposal
    // someone disagrees with, and it has to be as easy as agreeing.
    render(<ActionCard {...props()} />);
    expect(screen.getByTestId("action-confirm")).toBeEnabled();
    expect(screen.getByTestId("action-decline")).toBeEnabled();
  });

  it("says the action runs as the person, not as the assistant", () => {
    render(<ActionCard {...props()} />);
    expect(screen.getByTestId("action-card-suppress_record").textContent).toContain(
      "the assistant cannot do this itself",
    );
  });

  it("disables both controls while running, so nothing double-fires", () => {
    render(<ActionCard {...props({ state: "running" })} />);
    expect(screen.getByTestId("action-confirm")).toBeDisabled();
    expect(screen.getByTestId("action-decline")).toBeDisabled();
  });

  it("confirms completion by saying where the record of it is", () => {
    render(<ActionCard {...props({ state: "done" })} />);
    expect(screen.getByTestId("action-done").textContent).toContain("audit log");
    // The controls are gone — a done action must not be re-runnable from a
    // stale card.
    expect(screen.queryByTestId("action-confirm")).toBeNull();
  });

  it("says plainly that a decline changed nothing", () => {
    render(<ActionCard {...props({ state: "declined" })} />);
    expect(screen.getByTestId("action-declined").textContent).toContain("nothing was changed");
    expect(screen.queryByTestId("action-confirm")).toBeNull();
  });

  it("surfaces a failure as an alert and leaves the action retryable", () => {
    render(<ActionCard {...props({ state: "failed", failure: "record not found" })} />);
    expect(screen.getByTestId("action-error")).toHaveAttribute("role", "alert");
    // Still offerable: a 404 from a stale id is worth seeing, not worth losing
    // the card over.
    expect(screen.getByTestId("action-confirm")).toBeEnabled();
  });

  it("renders without a rationale rather than showing an empty quote", () => {
    render(<ActionCard {...props({ action: { ...action, rationale: "" } })} />);
    expect(screen.getByTestId("action-card-suppress_record").textContent).not.toContain(
      "The assistant says",
    );
  });

  it("describes a merge as a merge", () => {
    render(
      <ActionCard
        {...props({
          action: { kind: "resolve_review", subject: "01REV1", resolution: "match", rationale: "" },
        })}
      />,
    );
    expect(screen.getByTestId("action-effect").textContent).toContain("Merge");
  });
});
