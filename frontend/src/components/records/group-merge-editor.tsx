"use client";

import { ArrowUp } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import type { FieldAcrossMembers } from "@/lib/query/member-values";
import { cn } from "@/lib/utils";

export interface GroupMergeEditorProps {
  fields: FieldAcrossMembers[];
  memberKeys: string[];
  /** How many members exist beyond the ones compared, if any. */
  notShown: number;
  pending: boolean;
  canEdit: boolean;
  /** Which attribute is being written, so only that row shows as busy. */
  applying: string | null;
  failure: string | null;
  onAdopt: (attribute: string, value: string) => void;
  /** Only rows where a member offers something different. */
  differencesOnly: boolean;
  onToggleDifferences: (value: boolean) => void;
}

/**
 * The master-editing half of a merge (F1).
 *
 * The review inbox is a PAIR adjudicator — it answers "are these the same".
 * This answers the question that comes after: the group is right, but is the
 * golden record holding the best value from it? Every field is shown across
 * every member, and adopting one writes through the same audited
 * `override_field` path a typed edit uses.
 *
 * **Show-differences-only defaults ON.** A person opening this is looking for
 * disagreements; a full grid of fields where every member agrees is a wall to
 * scroll past, and the one row that matters is in it somewhere.
 *
 * **A member's EMPTY value is never offered.** It differs from a populated
 * master, but adopting it would clear the field — and a one-click control that
 * silently deletes a value is the destructive mistake this surface could most
 * easily make.
 */
export function GroupMergeEditor({
  fields,
  memberKeys,
  notShown,
  pending,
  canEdit,
  applying,
  failure,
  onAdopt,
  differencesOnly,
  onToggleDifferences,
}: GroupMergeEditorProps) {
  if (pending) return <Skeleton className="h-40 w-full" />;

  const shown = differencesOnly ? fields.filter((field) => field.hasAlternative) : fields;

  return (
    <div className="flex flex-col gap-2" data-testid="group-merge-editor">
      <div className="flex flex-wrap items-center gap-3">
        <label className="flex items-center gap-2 text-xs">
          <input
            type="checkbox"
            className="size-3.5 shrink-0 accent-current"
            checked={differencesOnly}
            onChange={(event) => {
              onToggleDifferences(event.target.checked);
            }}
            data-testid="merge-differences-only"
          />
          Only fields where a member disagrees
        </label>
        {notShown > 0 && (
          <span className="text-muted-foreground text-xs" data-testid="merge-not-shown">
            comparing {memberKeys.length} of {memberKeys.length + notShown} members
          </span>
        )}
      </div>

      {shown.length === 0 ? (
        <p className="text-muted-foreground text-xs" data-testid="merge-no-differences">
          Every member agrees with the golden record on all compared fields.
        </p>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full text-xs" data-testid="merge-table">
            <thead>
              <tr className="text-muted-foreground text-left">
                <th className="py-1 pr-3 font-medium">Field</th>
                <th className="py-1 pr-3 font-medium">Golden</th>
                {memberKeys.map((key) => (
                  <th key={key} className="py-1 pr-3 font-mono font-medium">
                    {key}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {shown.map((field) => (
                <tr key={field.attribute} data-testid={`merge-row-${field.attribute}`}>
                  <td className="py-1 pr-3">{field.attribute}</td>
                  <td className="py-1 pr-3 font-medium">{field.master ?? "—"}</td>
                  {field.values.map((entry) => (
                    <td key={entry.recordKey} className="py-1 pr-3">
                      <span className={cn(entry.differs && entry.value !== null && "font-medium")}>
                        {entry.value ?? "—"}
                      </span>
                      {/* Offered only when the member has something the master
                          does not: adopting an empty value would CLEAR the
                          field, and a one-click control that silently deletes
                          is the mistake this surface could most easily make. */}
                      {canEdit && entry.differs && entry.value !== null && (
                        // `adopt` binds the narrowed value: the guard above
                        // narrows at render, but the closure is called later
                        // and TypeScript is right that `entry.value` is
                        // `string | null` by then.
                        <Button
                          variant="outline"
                          size="sm"
                          className="ml-1 h-5 px-1"
                          disabled={applying !== null}
                          onClick={() => {
                            if (entry.value !== null) onAdopt(field.attribute, entry.value);
                          }}
                          aria-label={`Use ${entry.recordKey}'s ${field.attribute}`}
                          data-testid={`merge-adopt-${field.attribute}-${entry.recordKey}`}
                        >
                          <ArrowUp className="size-3" aria-hidden />
                        </Button>
                      )}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <p className="text-muted-foreground text-[11px]">
        Adopting a value sets it as an override on the golden record — audited like any edit, and
        the member records are untouched.
      </p>

      {failure !== null && (
        <span className="text-xs font-medium" role="alert" data-testid="merge-error">
          {failure}
        </span>
      )}
    </div>
  );
}
