"use client";

import { AlertTriangle } from "lucide-react";

import { Skeleton } from "@/components/ui/skeleton";
import { GOLDEN_ATTRIBUTES, RECORD_ATTRIBUTES } from "@/lib/domain/attributes";
import { diffFields } from "@/lib/domain/field-diff";
import { useRecordPair, type SourceRecord } from "@/lib/query/reviews";
import { cn } from "@/lib/utils";

/**
 * The side-by-side field comparison (design §5.3): the two records' values,
 * attribute by attribute, with the ones that differ marked.
 *
 * Difference is shown twice on purpose — a tinted row AND an icon — because
 * colour alone fails both colour-blind readers and the a11y gate. Absent
 * values render as an em dash, and two spellings of absence are agreement,
 * not a difference (see `field-diff`).
 */
export function FieldCompareGrid({
  org,
  keyA,
  keyB,
  layout = "grid",
}: {
  org: string;
  keyA: string | null;
  keyB: string | null;
  /** `grid` is the desktop three-column table; `stacked` is the phone card. */
  layout?: "grid" | "stacked";
}) {
  const pair = useRecordPair(org, keyA, keyB);

  if (keyA === null || keyB === null) {
    return (
      <p className="text-muted-foreground text-xs">
        This review is about an entity rather than a pair of records.
      </p>
    );
  }
  if (pair.isPending) return <Skeleton className="h-40 w-full" />;
  if (pair.isError) {
    return (
      <p className="text-muted-foreground text-xs" data-testid="compare-unavailable">
        The records behind this pair could not be read — the values may have been retired.
      </p>
    );
  }

  const byKey = new Map<string, SourceRecord>(
    pair.data.items.map((item) => [item.record_key, item]),
  );
  const left = byKey.get(keyA);
  const right = byKey.get(keyB);
  if (!left || !right) {
    return (
      <p className="text-muted-foreground text-xs" data-testid="compare-unavailable">
        One side of this pair is no longer in the current snapshot.
      </p>
    );
  }

  const rows = diffFields(RECORD_ATTRIBUTES, left.attributes, right.attributes);
  // The headline counts IDENTITY fields only. The validity flags and the source
  // timestamp are shown as context, but `updated_at_source` differs for almost
  // every pair by construction, and counting it would inflate every headline
  // into "these records disagree" when they may match perfectly.
  const identityKeys = new Set<string>(GOLDEN_ATTRIBUTES.map((attribute) => attribute.key));
  const differing = rows.filter((row) => !row.same && identityKeys.has(row.key)).length;

  return (
    <div className="flex flex-col gap-1.5" data-testid="field-compare-grid">
      <p className="text-muted-foreground text-xs">
        {differing === 0
          ? "Every identifying field agrees."
          : `${String(differing)} identifying field${differing === 1 ? "" : "s"} differ.`}
      </p>
      <div className="overflow-hidden rounded-md border text-sm">
        <div
          className={cn(
            "bg-muted/50 border-b px-3 py-1.5 text-xs font-medium",
            layout === "grid"
              ? "grid grid-cols-[7rem_1fr_1fr] gap-2"
              : "flex justify-between gap-2",
          )}
        >
          {layout === "grid" && <span>Field</span>}
          <span className="truncate font-mono">{left.source_system}</span>
          <span className="truncate font-mono">{right.source_system}</span>
        </div>
        {rows.map((row) => (
          <div
            key={row.key}
            className={cn(
              "border-b px-3 py-1.5 last:border-0",
              layout === "grid"
                ? "grid grid-cols-[7rem_1fr_1fr] items-baseline gap-2"
                : "flex flex-col gap-0.5",
              !row.same && "bg-amber-500/10",
            )}
            data-testid={`compare-${row.key}`}
          >
            <span className="text-muted-foreground flex items-center gap-1 text-xs">
              {!row.same && (
                <>
                  <AlertTriangle className="size-3 shrink-0 text-amber-600" aria-hidden />
                  {/* Real text, not just the tint or the icon: the difference
                      has to survive both colour blindness and a screen reader. */}
                  <span className="sr-only">differs — </span>
                </>
              )}
              {row.label}
            </span>
            <span className={cn("truncate", layout === "stacked" && "text-xs")}>
              {row.a ?? "—"}
            </span>
            <span className={cn("truncate", layout === "stacked" && "text-xs")}>
              {row.b ?? "—"}
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}
