"use client";

import { History, RotateCcw } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import type { HistoryPoint } from "@/lib/history/snapshots";

export interface FieldHistoryRow extends HistoryPoint {
  /** The value this field held at that snapshot, or null if it had none. */
  value: string | null;
  /** True when it differs from the value immediately newer than it. */
  changed: boolean;
}

export interface FieldHistoryProps {
  attribute: string;
  rows: FieldHistoryRow[];
  pending: boolean;
  /** Steward+: restoring writes through the audited override path. */
  canRestore: boolean;
  restoringSnapshot: number | null;
  failure: string | null;
  onRestore: (row: FieldHistoryRow) => void;
  onClose: () => void;
}

function stamp(iso: string | null): string {
  if (iso === null) return "—";
  return new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" }).format(
    new Date(iso),
  );
}

/**
 * What one golden field used to be, and putting it back (O12c).
 *
 * **Only snapshots where the value CHANGED are offered as restorable.** A
 * history listing every run would be mostly rows saying "still Bob", and the
 * one row that matters would be buried in them — the reader is looking for the
 * moment it became something else.
 *
 * Restoring writes through the existing audited `override_field` path rather
 * than rewriting history: the old value becomes the current one by the same
 * mechanism a human typing it would use, so the audit log records who restored
 * what and the lake keeps both versions. Nothing here edits the past.
 */
export function FieldHistory({
  attribute,
  rows,
  pending,
  canRestore,
  restoringSnapshot,
  failure,
  onRestore,
  onClose,
}: FieldHistoryProps) {
  const changes = rows.filter((row) => row.changed);
  return (
    <div
      className="border-border flex flex-col gap-2 rounded-md border p-3"
      data-testid="field-history"
    >
      <div className="flex items-center gap-2">
        <History className="size-3.5 shrink-0" aria-hidden />
        <span className="text-sm font-medium">History of {attribute}</span>
        <button
          type="button"
          onClick={onClose}
          className="text-muted-foreground hover:text-foreground ml-auto text-xs underline underline-offset-2"
          data-testid="field-history-close"
        >
          close
        </button>
      </div>

      {pending ? (
        <Skeleton className="h-16 w-full" />
      ) : changes.length === 0 ? (
        <p className="text-muted-foreground text-xs" data-testid="field-history-unchanged">
          This value has not changed across the runs still in the lake.
        </p>
      ) : (
        <ul className="flex flex-col gap-1.5">
          {changes.map((row) => (
            <li
              key={row.snapshot}
              className="flex flex-wrap items-baseline gap-x-3 gap-y-1 text-xs"
              data-testid={`field-history-${String(row.snapshot)}`}
            >
              <span className="font-medium">{row.value ?? "— empty —"}</span>
              <span className="text-muted-foreground tabular-nums">
                v{row.snapshot} · {stamp(row.endedAt)}
              </span>
              {canRestore && (
                <Button
                  variant="outline"
                  size="sm"
                  className="ml-auto h-6"
                  disabled={restoringSnapshot !== null}
                  onClick={() => {
                    onRestore(row);
                  }}
                  data-testid={`field-restore-${String(row.snapshot)}`}
                >
                  <RotateCcw className="size-3" aria-hidden />
                  {restoringSnapshot === row.snapshot ? "Restoring…" : "Restore"}
                </Button>
              )}
            </li>
          ))}
        </ul>
      )}

      <p className="text-muted-foreground text-[11px]">
        Restoring sets the value as an override — it is audited like any edit, and nothing in the
        past is rewritten.
      </p>

      {failure !== null && (
        <span className="text-xs font-medium" role="alert" data-testid="field-history-error">
          {failure}
        </span>
      )}
    </div>
  );
}
