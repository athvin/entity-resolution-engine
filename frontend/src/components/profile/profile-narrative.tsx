"use client";

import { Sparkles } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Working } from "@/components/ui/working";
import type { ProfileNarrative as Narrative } from "@/lib/query/profile";

/**
 * The optional plain-language briefing (§8).
 *
 * **Deterministic first, prose second** — and the layout says so: this sits
 * BELOW the findings panel, and it is empty until asked. The findings have
 * already decided what is wrong and said it with numbers; this explains what
 * they mean together, which is the one thing a registry cannot do.
 *
 * Three properties the copy carries deliberately:
 *
 * 1. **It is a request, not a feature that happened.** Nobody should pay a
 *    model call for a page view, and a narrative that appeared on its own would
 *    read as part of the measurement rather than as commentary on it.
 * 2. **It says what was sent.** "Statistics only — no values from your data"
 *    is the property the redaction enforces, and stating it is how a reader can
 *    hold the product to it.
 * 3. **The blocking suggestions are drafts.** They are rendered as text to copy
 *    into the config studio, not as a button that changes a config. The profile
 *    recommends; `er-compile`'s own constants are calibrated per corpus and
 *    moving any of them moves every committed golden, so nothing here applies
 *    itself.
 */
export function ProfileNarrative({
  narrative,
  pending,
  error,
  onAsk,
}: {
  narrative: Narrative | undefined;
  pending: boolean;
  error: string | null;
  onAsk: () => void;
}) {
  if (pending) {
    // `Working` is decorative by construction (aria-hidden, no live region of
    // its own) and says so: the caller owns the announcement. So the text
    // carries the status and the dots are punctuation beside it.
    return (
      <p className="text-muted-foreground flex items-center gap-2 text-xs" role="status">
        Reading the profile
        <Working />
      </p>
    );
  }

  if (!narrative) {
    return (
      <div className="flex flex-col items-start gap-2" data-testid="profile-narrative-idle">
        <p className="text-muted-foreground text-xs">
          A short briefing on what these numbers mean together, and which column to block on.
          Statistics only — no values from your data are sent.
        </p>
        <Button size="sm" variant="outline" onClick={onAsk} data-testid="profile-narrative-ask">
          <Sparkles className="size-3.5" aria-hidden />
          Explain this profile
        </Button>
        {error !== null && (
          <p className="text-xs" data-testid="profile-narrative-error">
            {error}
          </p>
        )}
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-3" data-testid="profile-narrative">
      {/* `whitespace-pre-line` because the model is asked for plain prose in
          paragraphs and no markdown — so the newlines it sends are the only
          structure there is, and rendering them as a single run would collapse
          four paragraphs into a wall. */}
      <p className="text-xs leading-relaxed whitespace-pre-line">{narrative.summary}</p>

      {narrative.blocking.length > 0 && (
        <div className="flex flex-col gap-1" data-testid="profile-narrative-blocking">
          <span className="text-muted-foreground text-[11px] tracking-wide uppercase">
            Suggested blocking keys — a draft to review, not a change
          </span>
          <ul className="flex flex-col gap-0.5">
            {narrative.blocking.map((rule) => (
              <li key={rule} className="font-mono text-[11px]">
                {rule}
              </li>
            ))}
          </ul>
          <p className="text-muted-foreground text-[10px]">
            Nothing is applied. Blocking changes the cost and the recall of every run, so these go
            through the config studio like any other edit.
          </p>
        </div>
      )}

      <p className="text-muted-foreground text-[10px]" data-testid="profile-narrative-provenance">
        Written by a model from the statistics above. Every claim should trace to a number on this
        page — if one does not, trust the number.
      </p>
    </div>
  );
}
