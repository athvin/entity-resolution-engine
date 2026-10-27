"use client";

import { useState } from "react";
import { Bookmark, X } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import type { SavedView } from "@/lib/query/views";
import type { ViewDefinition } from "@/lib/views/define";
import { isEmptyDefinition } from "@/lib/views/define";
import { cn } from "@/lib/utils";

export interface ViewsBarProps {
  views: SavedView[];
  /** The filter currently on screen, so Save captures what the reader sees. */
  current: ViewDefinition;
  /** Which view is applied, so the active one is legible. */
  activeId: string | null;
  canSave: boolean;
  busy: boolean;
  failure: string | null;
  onApply: (view: SavedView) => void;
  onSave: (name: string) => void;
  onDelete: (view: SavedView) => void;
}

/**
 * Saved views for a browsable surface (O12a).
 *
 * The product's filter state lives in `useState` and cannot be linked or
 * bookmarked — a gap the frontend-data-ui skill names explicitly. This does not
 * close that gap (the URL still carries nothing); it makes the filter
 * *re-reachable*, which is the part a steward actually needs: "the filter I use
 * every Monday" survives a reload and is there for a colleague.
 *
 * **Save is disabled when the filter narrows nothing**, with the reason shown.
 * A view of "everything" is the state the screen already opens in, and the name
 * someone gives it makes it look deliberate to whoever finds it later.
 */
export function ViewsBar({
  views,
  current,
  activeId,
  canSave,
  busy,
  failure,
  onApply,
  onSave,
  onDelete,
}: ViewsBarProps) {
  const [naming, setNaming] = useState(false);
  const [name, setName] = useState("");
  const empty = isEmptyDefinition(current);

  return (
    <div className="flex flex-col gap-1.5" data-testid="views-bar">
      <div className="flex flex-wrap items-center gap-1.5">
        <Bookmark className="text-muted-foreground size-3.5 shrink-0" aria-hidden />
        {views.length === 0 ? (
          <span className="text-muted-foreground text-xs" data-testid="views-empty">
            No saved views yet
          </span>
        ) : (
          views.map((view) => (
            <span key={view.id} className="flex items-center">
              <button
                type="button"
                onClick={() => {
                  onApply(view);
                }}
                // Applied = filled, available = outlined. The only channel
                // available with no palette (design §3.3).
                className={cn(
                  "rounded-l-md border px-2 py-0.5 text-xs",
                  view.id === activeId
                    ? "bg-foreground text-background border-foreground"
                    : "border-border hover:bg-accent/50",
                )}
                data-testid={`view-${view.name}`}
              >
                {view.name}
              </button>
              {canSave && (
                <button
                  type="button"
                  onClick={() => {
                    onDelete(view);
                  }}
                  className="border-border hover:bg-accent/50 rounded-r-md border border-l-0 px-1 py-0.5"
                  aria-label={`Delete the view ${view.name}`}
                  data-testid={`view-delete-${view.name}`}
                >
                  <X className="size-3" aria-hidden />
                </button>
              )}
            </span>
          ))
        )}

        {canSave &&
          (naming ? (
            <span className="flex items-center gap-1">
              <Input
                autoFocus
                value={name}
                onChange={(event) => {
                  setName(event.target.value);
                }}
                placeholder="name this view"
                className="h-7 w-40 text-xs"
                data-testid="view-name"
              />
              <Button
                size="sm"
                className="h-7"
                disabled={busy || name.trim() === ""}
                onClick={() => {
                  onSave(name.trim());
                  setName("");
                  setNaming(false);
                }}
                data-testid="view-save"
              >
                Save
              </Button>
            </span>
          ) : (
            <Button
              variant="outline"
              size="sm"
              className="h-7"
              disabled={empty}
              title={empty ? "Set a filter first — a view of everything is the default" : undefined}
              onClick={() => {
                setNaming(true);
              }}
              data-testid="view-add"
            >
              Save this view
            </Button>
          ))}
      </div>
      {failure !== null && (
        <span className="text-xs font-medium" role="alert" data-testid="views-error">
          {failure}
        </span>
      )}
    </div>
  );
}
