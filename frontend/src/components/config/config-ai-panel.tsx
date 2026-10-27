"use client";

import { Sparkles } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Label } from "@/components/ui/label";
import { EXPLAINABLE_BLOCKS } from "@/lib/config-ai/blocks";

export interface ConfigAiPanelProps {
  /** Admin-only: drafting writes a version, explaining does not. */
  canDraft: boolean;
  instruction: string;
  /** Which block the explain control is pointed at. */
  block: string;
  /** Null until something has been explained. */
  explanation: string | null;
  /** The model's summary of a draft it just made. */
  draftSummary: string | null;
  /** The model's reason for declining — an expected answer, not an error. */
  declined: string | null;
  drafting: boolean;
  explaining: boolean;
  failure: string | null;
  onInstructionChange: (value: string) => void;
  onBlockChange: (value: string) => void;
  onDraft: () => void;
  onExplain: () => void;
  /** V8b: propose a whole starting matching config. Admin, like drafting. */
  starting: boolean;
  onStarter: () => void;
}

/**
 * Describe a change, or ask what a rule does (V7).
 *
 * Two halves of the same complaint — the rules learning curve — from opposite
 * directions, and they deliberately carry different permissions. **Drafting is
 * admin** because it writes a version. **Explaining is open to anyone**,
 * because the person who most needs a rule explained is usually the one who
 * cannot edit it, and gating the explanation behind the editing permission
 * would put the learning curve behind the thing it is a curve toward.
 *
 * The copy has to keep saying that a draft is only a draft. Nothing the model
 * writes takes effect until an admin publishes a diff they have read, and this
 * panel sits directly above the editors that do take effect — so the
 * distinction has to be legible at a glance, not inferred.
 */
export function ConfigAiPanel({
  canDraft,
  instruction,
  block,
  explanation,
  draftSummary,
  declined,
  drafting,
  explaining,
  failure,
  onInstructionChange,
  onBlockChange,
  onDraft,
  onExplain,
  starting,
  onStarter,
}: ConfigAiPanelProps) {
  return (
    <Card data-testid="config-ai">
      <CardHeader>
        <CardTitle className="flex items-center gap-2 text-base">
          <Sparkles className="size-4" aria-hidden />
          Ask about a rule, or describe a change
        </CardTitle>
        <CardDescription>
          A described change becomes a <strong>draft</strong> you review below — nothing is
          published until you say so.
        </CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-5">
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="config-ai-explain-block">What does this rule do?</Label>
          <div className="flex flex-wrap items-center gap-2">
            <select
              id="config-ai-explain-block"
              className="border-input bg-background h-9 rounded-md border px-2 text-sm"
              value={block}
              onChange={(event) => {
                onBlockChange(event.target.value);
              }}
              data-testid="config-ai-block"
            >
              {EXPLAINABLE_BLOCKS.map((name) => (
                <option key={name} value={name}>
                  {name}
                </option>
              ))}
            </select>
            <Button
              variant="outline"
              size="sm"
              onClick={onExplain}
              disabled={explaining}
              data-testid="config-ai-explain"
            >
              {explaining ? "Reading…" : "Explain"}
            </Button>
          </div>
          {explanation !== null && (
            <p
              className="text-muted-foreground border-border mt-1 border-l-2 pl-3 text-sm whitespace-pre-line"
              data-testid="config-ai-explanation"
            >
              {explanation}
            </p>
          )}
        </div>

        {canDraft ? (
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="config-ai-instruction">Describe a change</Label>
            <textarea
              id="config-ai-instruction"
              className="border-input bg-background min-h-20 rounded-md border px-3 py-2 text-sm"
              placeholder="e.g. only auto-merge above 0.97, and never merge two records with different birth dates"
              value={instruction}
              onChange={(event) => {
                onInstructionChange(event.target.value);
              }}
              data-testid="config-ai-instruction"
            />
            <div className="flex flex-wrap items-center gap-3">
              <Button
                size="sm"
                onClick={onDraft}
                disabled={drafting || instruction.trim() === ""}
                data-testid="config-ai-draft"
                className="self-start"
              >
                {drafting ? "Drafting…" : "Draft it"}
              </Button>
              <Button
                size="sm"
                variant="outline"
                onClick={onStarter}
                disabled={starting}
                data-testid="config-ai-starter"
              >
                {starting ? "Proposing…" : "Propose a starting config"}
              </Button>
              {draftSummary !== null && (
                <span className="text-muted-foreground text-sm" data-testid="config-ai-summary">
                  Drafted: {draftSummary}
                </span>
              )}
              {declined !== null && (
                <span className="text-muted-foreground text-sm" data-testid="config-ai-declined">
                  No change made — {declined}
                </span>
              )}
            </div>
          </div>
        ) : (
          <p className="text-muted-foreground text-sm" data-testid="config-ai-read-only">
            Only an admin can draft a change, but anyone can ask what a rule does.
          </p>
        )}

        {failure !== null && (
          <p className="text-sm font-medium" role="alert" data-testid="config-ai-error">
            {failure}
          </p>
        )}
      </CardContent>
    </Card>
  );
}
