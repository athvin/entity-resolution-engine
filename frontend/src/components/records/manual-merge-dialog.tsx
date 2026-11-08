"use client";

import { useEffect, useId, useMemo, useState } from "react";
import { useQueries } from "@tanstack/react-query";
import { GitMerge } from "lucide-react";

import { bffFetch } from "@/lib/api/client";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { displayName } from "@/lib/domain/attributes";
import { manualMergePayload, type ManualMergeSide } from "@/lib/domain/manual-merge";
import { useOrgEntityType } from "@/lib/query/config";
import type { EntityDetail, GoldenRecord } from "@/lib/query/hooks";
import { queryKeys } from "@/lib/query/keys";
import { useMergeGroupSubmit } from "@/lib/query/merge-group";
import { useDatasetSlug } from "@/lib/use-dataset-slug";

/**
 * Manual group merge (spec 2.10): entities the steward selected in the records
 * browser, merged as one group through the SAME `/merges` bundle the review
 * dialog submits for detected groups — assertion star + master protect, one
 * drain applying it — so survivorship shapes the result identically either
 * way. What the dialog adds is only the things the matcher cannot know: WHICH
 * records, and which side survives. Field-level corrections stay post-merge
 * (the `GroupMergeEditor` on the merged entity) — this dialog does one thing.
 */
export function ManualMergeDialog({
  org,
  selected,
  open,
  onClose,
  onDone,
}: {
  org: string;
  selected: GoldenRecord[];
  open: boolean;
  onClose: () => void;
  onDone: (message: string) => void;
}) {
  const ds = useDatasetSlug();
  const entityType = useOrgEntityType(org, ds);
  const headingId = useId();
  const submit = useMergeGroupSubmit(org, ds);

  const [survivorId, setSurvivorId] = useState<string | null>(null);
  useEffect(() => {
    if (open) setSurvivorId(selected[0]?.id ?? null);
  }, [open, selected]);

  const details = useQueries({
    queries: selected.map((record) => ({
      queryKey: queryKeys.entity(org, ds, record.id),
      queryFn: () =>
        bffFetch<EntityDetail>(`/api/orgs/${org}/datasets/${ds}/golden-records/${record.id}`),
      enabled: open,
      staleTime: 60_000,
    })),
  });

  const sides = useMemo(
    () =>
      selected.flatMap((record, index): ManualMergeSide[] => {
        const detail = details[index]?.data;
        return detail ? [{ record, detail }] : [];
      }),
    [selected, details],
  );
  const pending = details.some((query) => query.isPending);
  const ready = !pending && sides.length === selected.length;

  const payload = ready && survivorId !== null ? manualMergePayload(sides, survivorId) : null;
  const refusal = payload !== null && "error" in payload ? payload.error : null;

  if (!open) return null;

  function confirm() {
    if (payload === null || "error" in payload) return;
    submit.mutate(payload, {
      onSuccess: () => {
        onDone("merge staged — the reconcile job applies it");
        onClose();
      },
    });
  }

  return (
    <div
      className="animate-fade-in fixed inset-0 z-50 flex items-end justify-center bg-black/50 p-4 lg:items-center"
      onKeyDown={(event) => {
        if (event.key === "Escape") onClose();
      }}
      onClick={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby={headingId}
        className="animate-lift-in bg-background flex max-h-[85vh] w-full max-w-xl flex-col gap-3 overflow-y-auto rounded-lg border p-5 shadow-xl"
        data-testid="manual-merge-dialog"
      >
        <h2 id={headingId} className="flex items-center gap-2 text-base font-semibold">
          <GitMerge className="size-4" /> Merge {selected.length} records
        </h2>
        <p className="text-muted-foreground text-sm">
          The matcher never grouped these — you are saying they are the same. The merge runs the
          configured survivorship rules, and the audit trail records it as manual.
        </p>

        {!ready ? (
          <Skeleton className="h-32 w-full" />
        ) : (
          <fieldset className="flex flex-col gap-1" data-testid="manual-merge-sides">
            <legend className="text-muted-foreground mb-1 text-xs">
              Which record survives as the master?
            </legend>
            {sides.map((side) => (
              <label
                key={side.record.id}
                className="hover:bg-accent has-checked:border-foreground flex cursor-pointer items-center gap-3 rounded-md border px-3 py-2 text-sm"
                data-testid={`manual-merge-side-${side.record.id}`}
              >
                <input
                  type="radio"
                  name="manual-merge-survivor"
                  checked={survivorId === side.record.id}
                  onChange={() => {
                    setSurvivorId(side.record.id);
                  }}
                  className="size-3.5 shrink-0 accent-current"
                />
                <span className="min-w-0 flex-1 truncate font-medium">
                  {displayName(side.record, entityType) || side.record.id}
                </span>
                <span className="text-muted-foreground truncate text-xs">
                  {side.detail.members.length}{" "}
                  {side.detail.members.length === 1 ? "record" : "records"}
                </span>
              </label>
            ))}
          </fieldset>
        )}

        {refusal !== null && (
          <p className="text-destructive text-sm" role="alert" data-testid="manual-merge-refusal">
            {refusal}
          </p>
        )}
        {submit.isError && (
          <p className="text-destructive text-sm" role="alert" data-testid="manual-merge-error">
            the merge could not be staged
          </p>
        )}

        <div className="flex justify-end gap-2">
          <Button variant="ghost" onClick={onClose} data-testid="manual-merge-cancel">
            Cancel
          </Button>
          <Button
            onClick={confirm}
            disabled={!ready || refusal !== null || submit.isPending}
            data-testid="manual-merge-confirm"
          >
            {submit.isPending ? "Merging…" : "Merge"}
          </Button>
        </div>
      </div>
    </div>
  );
}
