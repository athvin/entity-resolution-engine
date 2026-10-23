"use client";

import { useState } from "react";
import { Plus, X } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { BffRequestError } from "@/lib/api/client";
import { effectiveRole, useSession } from "@/lib/query/hooks";
import type { Tag } from "@/lib/entities/bindings/Tag";
import {
  canApplyTags,
  offeredTags,
  useCreateTag,
  useSubjectTags,
  useTagApplication,
  useTags,
  type TagSubject,
} from "@/lib/query/tags";

/**
 * The tags on one entity or review, with the affordance to change them.
 *
 * Reusable across both subjects because a chip is a chip: the only difference is
 * the `subject` pair, which is also the whole of the server's addressing.
 *
 * **Grayscale, like everything else** (`docs/frontend-design.md` §3.3). A tag
 * system is the single most tempting place to reach for colour, and the design
 * rule forbids a palette — so an applied tag is a FILLED chip and an offered one
 * is OUTLINED, the same opposites-not-better-worse channel the always/never
 * chips use. A tenant who wants red "urgent" tags gets weight instead, and the
 * ESLint rule would reject the hue anyway.
 */
export function TagChips({
  org,
  subjectType,
  subjectId,
}: {
  org: string;
  subjectType: TagSubject;
  subjectId: string;
}) {
  const applied = useSubjectTags(org, subjectType, subjectId);
  const vocabulary = useTags(org);
  const session = useSession();
  const canTag = canApplyTags(effectiveRole(session.data, org));

  const subject = { type: subjectType, id: subjectId };
  const application = useTagApplication(org, subject);
  const create = useCreateTag(org, subject);
  const [picking, setPicking] = useState(false);
  const [draft, setDraft] = useState("");
  const [failure, setFailure] = useState<string | null>(null);

  const appliedTags = applied.data ?? [];
  const offered = offeredTags(vocabulary.data ?? [], appliedTags);

  const fail = (cause: unknown) => {
    setFailure(cause instanceof BffRequestError ? cause.message : "could not change the tags");
  };

  if (applied.isPending) {
    return <Skeleton className="h-6 w-40" data-testid="tag-chips-loading" />;
  }

  return (
    <TagChipsView
      applied={appliedTags}
      offered={offered}
      canTag={canTag}
      picking={picking}
      draft={draft}
      busy={application.isPending || create.isPending}
      failure={failure}
      vocabularyLoaded={vocabulary.data !== undefined}
      onOpenPicker={() => {
        setPicking(true);
      }}
      onClosePicker={() => {
        setPicking(false);
        setDraft("");
        setFailure(null);
      }}
      onDraftChange={setDraft}
      onApply={(tagId) => {
        setFailure(null);
        application.mutate({ tagId, apply: true }, { onError: fail });
      }}
      onRemove={(tagId) => {
        setFailure(null);
        application.mutate({ tagId, apply: false }, { onError: fail });
      }}
      onCreate={() => {
        setFailure(null);
        const name = draft.trim();
        // Created then applied, sequentially and awaited. A steward naming a tag
        // here means it for THIS subject, so making them click it again
        // afterwards is a step with no decision in it. Awaiting rather than
        // nesting puts both failures in one `catch`.
        void (async () => {
          try {
            const tag = await create.mutateAsync({ name });
            setDraft("");
            await application.mutateAsync({ tagId: tag.id, apply: true });
          } catch (cause) {
            fail(cause);
          }
        })();
      }}
    />
  );
}

/**
 * The chips, as a function of data alone.
 *
 * Split out from the container so the branchy render logic is testable in the
 * component tier (`docs/frontend-testing.md`: "branchy RENDER logic... a
 * component whose output derives from data") without a QueryClient or a
 * backend. The container below is the data wiring, which Playwright already
 * drives end to end.
 */
export function TagChipsView({
  applied,
  offered,
  canTag,
  picking,
  draft,
  busy,
  failure,
  vocabularyLoaded,
  onOpenPicker,
  onClosePicker,
  onDraftChange,
  onApply,
  onRemove,
  onCreate,
}: {
  applied: readonly Tag[];
  offered: readonly Tag[];
  canTag: boolean;
  picking: boolean;
  draft: string;
  busy: boolean;
  failure: string | null;
  vocabularyLoaded: boolean;
  onOpenPicker: () => void;
  onClosePicker: () => void;
  onDraftChange: (value: string) => void;
  onApply: (tagId: string) => void;
  onRemove: (tagId: string) => void;
  onCreate: () => void;
}) {
  return (
    <div className="flex flex-col gap-1.5" data-testid="tag-chips">
      <div className="flex flex-wrap items-center gap-1.5">
        {applied.map((tag) => (
          <span
            key={tag.id}
            className="bg-foreground text-background inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-xs font-medium"
            title={tag.description ?? undefined}
            data-testid={`tag-chip-${tag.name}`}
          >
            {tag.name}
            {canTag && (
              <button
                type="button"
                aria-label={`Remove ${tag.name}`}
                className="hover:opacity-70"
                disabled={busy}
                onClick={() => {
                  onRemove(tag.id);
                }}
                data-testid={`tag-remove-${tag.name}`}
              >
                <X className="size-3" />
              </button>
            )}
          </span>
        ))}

        {applied.length === 0 && !picking && (
          <span className="text-muted-foreground text-xs" data-testid="tag-chips-empty">
            No tags yet
          </span>
        )}

        {canTag && !picking && (
          <Button
            variant="ghost"
            size="sm"
            className="h-6 px-1.5 text-xs"
            onClick={onOpenPicker}
            data-testid="tag-add"
          >
            <Plus className="size-3" /> Tag
          </Button>
        )}
      </div>

      {canTag && picking && (
        <div className="flex flex-col gap-1.5" data-testid="tag-picker">
          {offered.length > 0 && (
            <div className="flex flex-wrap items-center gap-1.5">
              {offered.map((tag) => (
                <button
                  key={tag.id}
                  type="button"
                  // OUTLINED: offered, not applied. The opposite of the filled
                  // chips above, carried by border rather than hue.
                  className="border-foreground text-foreground rounded-full border px-2 py-0.5 text-xs font-medium hover:opacity-70"
                  disabled={busy}
                  onClick={() => {
                    onApply(tag.id);
                  }}
                  data-testid={`tag-offer-${tag.name}`}
                >
                  {tag.name}
                </button>
              ))}
            </div>
          )}
          <div className="flex items-center gap-1.5">
            <Input
              value={draft}
              onChange={(event) => {
                onDraftChange(event.target.value);
              }}
              placeholder="New tag name"
              className="h-7 w-44 text-xs"
              data-testid="tag-new-name"
            />
            <Button
              size="sm"
              className="h-7 text-xs"
              disabled={draft.trim() === "" || busy}
              onClick={onCreate}
              data-testid="tag-create"
            >
              Create and apply
            </Button>
            <Button
              variant="ghost"
              size="sm"
              className="h-7 text-xs"
              onClick={onClosePicker}
              data-testid="tag-picker-close"
            >
              Done
            </Button>
          </div>
          {offered.length === 0 && vocabularyLoaded && (
            <p className="text-muted-foreground text-xs">
              {applied.length > 0
                ? "Every tag is already applied — name a new one."
                : "No tags in this workspace yet — name the first one."}
            </p>
          )}
        </div>
      )}

      {failure !== null && (
        <p className="text-destructive text-xs" role="alert" data-testid="tag-error">
          {failure}
        </p>
      )}
    </div>
  );
}
