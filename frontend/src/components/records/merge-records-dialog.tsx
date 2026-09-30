"use client";

import { useEffect, useId, useMemo, useState } from "react";
import { GitMerge, Search } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { GOLDEN_ATTRIBUTES, displayName } from "@/lib/domain/attributes";
import { diffFields } from "@/lib/domain/field-diff";
import { useAddAssertion } from "@/lib/query/duplicates";
import {
  useEntityDetail,
  useGoldenRecords,
  type EntityDetail,
  type GoldenRecord,
} from "@/lib/query/hooks";
import { cn } from "@/lib/utils";

/**
 * Ad-hoc "merge these two entities" (design §5.3): search a target, compare
 * the two golden records field by field, and confirm — which writes one
 * durable `always` assertion between a member record of each entity. One edge
 * joins the connected components, so the first member of each side suffices;
 * the reconcile job realizes the merge and survivorship shapes the result.
 */
export function MergeRecordsDialog({
  org,
  source,
  open,
  onClose,
  onDone,
}: {
  org: string;
  source: EntityDetail;
  open: boolean;
  onClose: () => void;
  onDone: (message: string) => void;
}) {
  const [q, setQ] = useState("");
  const [debounced, setDebounced] = useState("");
  const [targetId, setTargetId] = useState<string | null>(null);
  const headingId = useId();
  const assertion = useAddAssertion(org);

  useEffect(() => {
    const handle = setTimeout(() => {
      setDebounced(q);
    }, 250);
    return () => {
      clearTimeout(handle);
    };
  }, [q]);

  useEffect(() => {
    if (open) {
      setQ("");
      setDebounced("");
      setTargetId(null);
    }
  }, [open]);

  const search = useGoldenRecords(org, debounced);
  const target = useEntityDetail(org, targetId ?? "");
  const candidates = useMemo(
    () =>
      (search.data?.pages ?? [])
        .flatMap((page) => page.items)
        .filter((row) => row.entity_id !== source.golden.entity_id)
        .slice(0, 8),
    [search.data, source.golden.entity_id],
  );

  if (!open) return null;

  const comparison =
    targetId !== null && target.data
      ? diffFields(
          GOLDEN_ATTRIBUTES,
          source.golden as unknown as Record<string, unknown>,
          target.data.golden as unknown as Record<string, unknown>,
        )
      : null;

  function confirmMerge() {
    const a = source.members[0]?.record_key;
    const b = target.data?.members[0]?.record_key;
    if (!a || !b) return;
    assertion.mutate(
      { kind: "always", a, b, note: "ad-hoc merge from records" },
      {
        onSuccess: (result) => {
          onDone(
            result.status === "applied"
              ? "always-rule written — the merge lands with the reconcile job"
              : "always-rule staged — queued behind the current run",
          );
          onClose();
        },
      },
    );
  }

  return (
    <div
      className="fixed inset-0 z-50 flex items-end justify-center bg-black/50 p-4 lg:items-center"
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
        className="flex max-h-[85vh] w-full max-w-2xl flex-col gap-3 overflow-y-auto rounded-lg border bg-background p-5 shadow-xl"
        data-testid="merge-dialog"
      >
        <h2 id={headingId} className="flex items-center gap-2 text-base font-semibold">
          <GitMerge className="size-4" /> Merge{" "}
          {displayName(source.golden) || source.golden.entity_id} with…
        </h2>

        {targetId === null ? (
          <>
            <label className="relative block">
              <Search className="text-muted-foreground absolute left-2.5 top-1/2 size-4 -translate-y-1/2" />
              <Input
                autoFocus
                value={q}
                onChange={(event) => {
                  setQ(event.target.value);
                }}
                placeholder="Search records by name or email"
                className="pl-8"
                aria-label="Search for the entity to merge with"
                data-testid="merge-search"
              />
            </label>
            {search.isPending && debounced ? (
              <Skeleton className="h-24 w-full" />
            ) : (
              <div className="overflow-hidden rounded-md border">
                {candidates.length === 0 ? (
                  <p className="text-muted-foreground p-4 text-center text-sm">
                    {debounced ? "No other entities match." : "Type to search."}
                  </p>
                ) : (
                  candidates.map((row: GoldenRecord) => (
                    <button
                      key={row.entity_id}
                      onClick={() => {
                        setTargetId(row.entity_id);
                      }}
                      className="hover:bg-accent flex w-full items-center gap-2 border-b px-3 py-2 text-left text-sm last:border-0"
                      data-testid={`merge-candidate-${row.entity_id}`}
                    >
                      <span className="min-w-0 flex-1 truncate">
                        {displayName(row) || row.entity_id}
                      </span>
                      <span className="text-muted-foreground truncate text-xs">
                        {row.email ?? ""}
                      </span>
                    </button>
                  ))
                )}
              </div>
            )}
          </>
        ) : target.isPending || !comparison ? (
          <Skeleton className="h-40 w-full" />
        ) : (
          <>
            <div className="overflow-hidden rounded-md border text-sm" data-testid="merge-compare">
              <div className="bg-muted/50 grid grid-cols-[8rem_1fr_1fr] gap-2 border-b px-3 py-2 text-xs font-medium">
                <span>Field</span>
                <span className="truncate">{displayName(source.golden) || "this entity"}</span>
                <span className="truncate">
                  {target.data ? displayName(target.data.golden) || "target" : "target"}
                </span>
              </div>
              {comparison.map((row) => (
                <div
                  key={row.key}
                  className={cn(
                    "grid grid-cols-[8rem_1fr_1fr] gap-2 border-b px-3 py-1.5 last:border-0",
                    !row.same && "bg-amber-500/10",
                  )}
                >
                  <span className="text-muted-foreground text-xs">{row.label}</span>
                  <span className="truncate">{row.a ?? "—"}</span>
                  <span className="truncate">{row.b ?? "—"}</span>
                </div>
              ))}
            </div>
            <p className="text-muted-foreground text-xs">
              Confirming writes a durable always-rule between the two entities&apos; records;
              survivorship decides each golden field. Differing fields are highlighted.
            </p>
            <div className="flex justify-end gap-2">
              <Button
                variant="outline"
                onClick={() => {
                  setTargetId(null);
                }}
              >
                Back
              </Button>
              <Button
                disabled={assertion.isPending}
                onClick={confirmMerge}
                data-testid="merge-confirm"
              >
                <GitMerge /> Merge entities
              </Button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
