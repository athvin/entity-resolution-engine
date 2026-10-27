"use client";

import { ArrowLeftRight } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Label } from "@/components/ui/label";
import type { DiffLine } from "@/lib/domain/config";
import { cn } from "@/lib/utils";

export interface PromotePanelProps {
  /** Orgs this person administers, excluding the current one. */
  candidates: string[];
  target: string;
  diff: DiffLine[] | null;
  /** Which version of the other org the diff came from. */
  theirVersion: number | null;
  comparing: boolean;
  importing: boolean;
  note: string | null;
  failure: string | null;
  onTargetChange: (value: string) => void;
  onCompare: () => void;
  onImport: () => void;
}

/**
 * Promote a config between orgs (O9).
 *
 * The dev→prod story. Two controls with a deliberate asymmetry: **compare is
 * always available, import is gated behind having compared**. Copying a
 * configuration from one tenant into another without having read the diff is
 * the mistake this surface exists to prevent — the two tenants' sources may
 * differ, and the engine will not say so until publish.
 *
 * Only orgs the person ADMINISTERS are offered. The server checks both sides
 * independently, so this list is convenience rather than control; offering an
 * unreadable org would just produce a 404 nobody can interpret.
 */
export function PromotePanel({
  candidates,
  target,
  diff,
  theirVersion,
  comparing,
  importing,
  note,
  failure,
  onTargetChange,
  onCompare,
  onImport,
}: PromotePanelProps) {
  if (candidates.length === 0) {
    return (
      <Card data-testid="promote-panel">
        <CardHeader>
          <CardTitle className="flex items-center gap-2 text-base">
            <ArrowLeftRight className="size-4" aria-hidden />
            Promote a config
          </CardTitle>
        </CardHeader>
        <CardContent>
          <p className="text-muted-foreground text-sm" data-testid="promote-none">
            You administer only this workspace, so there is nothing to promote between.
          </p>
        </CardContent>
      </Card>
    );
  }

  return (
    <Card data-testid="promote-panel">
      <CardHeader>
        <CardTitle className="flex items-center gap-2 text-base">
          <ArrowLeftRight className="size-4" aria-hidden />
          Promote a config
        </CardTitle>
        <CardDescription>
          Compare this workspace&apos;s published config against another you administer, and copy it
          in as a <strong>draft</strong> — nothing publishes until you say so.
        </CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <div className="flex flex-wrap items-end gap-2">
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="promote-target">Other workspace</Label>
            <select
              id="promote-target"
              className="border-input bg-background h-9 rounded-md border px-2 text-sm"
              value={target}
              onChange={(event) => {
                onTargetChange(event.target.value);
              }}
              data-testid="promote-target"
            >
              {candidates.map((org) => (
                <option key={org} value={org}>
                  {org}
                </option>
              ))}
            </select>
          </div>
          <Button
            variant="outline"
            size="sm"
            onClick={onCompare}
            disabled={comparing}
            data-testid="promote-compare"
          >
            {comparing ? "Comparing…" : "Compare"}
          </Button>
          <Button
            size="sm"
            onClick={onImport}
            // Import is deliberately unavailable until a diff has been read:
            // copying a config between tenants unseen is the mistake here.
            disabled={importing || diff === null}
            title={
              diff === null ? "Compare first, so the diff is read before it is copied" : undefined
            }
            data-testid="promote-import"
          >
            {importing ? "Copying…" : "Copy in as a draft"}
          </Button>
        </div>

        {note !== null && (
          <p className="text-muted-foreground text-sm" data-testid="promote-note">
            {note}
          </p>
        )}
        {failure !== null && (
          <p className="text-sm font-medium" role="alert" data-testid="promote-error">
            {failure}
          </p>
        )}

        {diff !== null &&
          (diff.length === 0 ? (
            <p className="text-muted-foreground text-sm" data-testid="promote-identical">
              Identical — these two workspaces publish the same configuration
              {theirVersion === null ? "" : ` (theirs is v${String(theirVersion)})`}.
            </p>
          ) : (
            <pre
              className="border-border max-h-80 overflow-auto rounded-md border p-2 font-mono text-xs"
              data-testid="promote-diff"
            >
              {diff.map((line, index) => (
                <div
                  key={`${String(index)}:${line.text}`}
                  // Shape, not hue: a leading +/- and a weight change, because
                  // the palette is unavailable (design §3.3).
                  className={cn(
                    line.kind === "added" && "font-medium",
                    line.kind === "removed" && "text-muted-foreground line-through",
                  )}
                >
                  {line.kind === "added" ? "+ " : line.kind === "removed" ? "- " : "  "}
                  {line.text}
                </div>
              ))}
            </pre>
          ))}
      </CardContent>
    </Card>
  );
}
