"use client";

import { CircleCheck, TriangleAlert } from "lucide-react";

import { Button } from "@/components/ui/button";
import { describeAction, type ProposedAction } from "@/lib/agent-actions/propose";

export type ActionState = "pending" | "running" | "done" | "failed" | "declined";

export interface ActionCardProps {
  action: ProposedAction;
  state: ActionState;
  failure: string | null;
  onConfirm: () => void;
  onDecline: () => void;
}

/**
 * One proposed steward action, awaiting a human (V9).
 *
 * **This card is the security boundary.** The assistant's tools return record
 * data, and a record can contain text shaped like an instruction; the whole
 * defence is that such an instruction can, at most, reach a card a person reads
 * and declines. So the card has to be legible in a way a rubber-stamp button is
 * not:
 *
 * - it states the EFFECT in plain language (`describeAction`), not the action's
 *   name — "apply suggestion" gives a reader nothing to disagree with;
 * - it names the subject, so "suppress a record" is never ambiguous about which;
 * - the model's rationale is shown as a QUOTE and visibly attributed, because
 *   it is attacker-influenced text and must not read as the product's own
 *   recommendation;
 * - declining is a real, equally-weighted control rather than an X in a corner.
 */
export function ActionCard({ action, state, failure, onConfirm, onDecline }: ActionCardProps) {
  const settled = state === "done" || state === "declined";
  return (
    <div
      className="border-border flex flex-col gap-2 rounded-md border p-3"
      data-testid={`action-card-${action.kind}`}
    >
      <div className="flex items-start gap-2">
        <TriangleAlert className="mt-0.5 size-3.5 shrink-0" aria-hidden />
        <span className="text-sm font-medium" data-testid="action-effect">
          {describeAction(action)}
        </span>
      </div>

      {action.rationale !== "" && (
        // Attributed and quoted. This text came through a model that read
        // customer records, so it must never present as the product speaking.
        <p className="text-muted-foreground border-border border-l-2 pl-2 text-xs italic">
          The assistant says: “{action.rationale}”
        </p>
      )}

      {state === "done" ? (
        <span
          className="text-muted-foreground flex items-center gap-1.5 text-xs"
          data-testid="action-done"
        >
          <CircleCheck className="size-3.5" aria-hidden />
          Done — it ran under your own session and is in the audit log.
        </span>
      ) : state === "declined" ? (
        <span className="text-muted-foreground text-xs" data-testid="action-declined">
          Declined — nothing was changed.
        </span>
      ) : (
        <div className="flex flex-wrap items-center gap-2">
          <Button
            size="sm"
            disabled={state === "running"}
            onClick={onConfirm}
            data-testid="action-confirm"
          >
            {state === "running" ? "Running…" : "Confirm"}
          </Button>
          <Button
            size="sm"
            variant="outline"
            disabled={state === "running"}
            onClick={onDecline}
            data-testid="action-decline"
          >
            Decline
          </Button>
          <span className="text-muted-foreground text-xs">
            Runs as you — the assistant cannot do this itself.
          </span>
        </div>
      )}

      {failure !== null && !settled && (
        <span className="text-xs font-medium" role="alert" data-testid="action-error">
          {failure}
        </span>
      )}
    </div>
  );
}
