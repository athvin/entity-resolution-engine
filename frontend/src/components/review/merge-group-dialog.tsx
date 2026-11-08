"use client";

import { useEffect, useMemo, useState } from "react";
import { Check, GitMerge, Pencil, Undo2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { goldenAttributes, type AttributeDescriptor } from "@/lib/domain/attributes";
import {
  effectiveValue,
  isCopyOffered,
  newDraft,
  pinCount,
  reassignMaster,
  revertValue,
  stageValue,
  toOverridePayload,
  type MergeDraft,
} from "@/lib/domain/merge-draft";
import { BffRequestError } from "@/lib/api/client";
import { useOrgEntityType } from "@/lib/query/config";
import { useMergeGroup, useMergeGroupSubmit, type GroupMember } from "@/lib/query/merge-group";
import type { ReviewRow } from "@/lib/query/reviews";
import { cn } from "@/lib/utils";
import { useDatasetSlug } from "@/lib/use-dataset-slug";

/** One cell in the master's column: the effective value (a pin when staged,
 * the record's own value otherwise), with the edit and revert affordances. */
function MasterCell({
  descriptor,
  member,
  draft,
  onStage,
  onRevert,
}: {
  descriptor: AttributeDescriptor;
  member: GroupMember;
  draft: MergeDraft;
  onStage: (attribute: string, value: string | null) => void;
  onRevert: (attribute: string) => void;
}) {
  const [editing, setEditing] = useState(false);
  const naturalRaw = member.attributes[descriptor.key];
  const natural = typeof naturalRaw === "string" ? naturalRaw : null;
  const value = effectiveValue(draft, descriptor.key, natural);
  const pinned = descriptor.key in draft.pins;

  if (editing) {
    return (
      <Input
        autoFocus
        type={descriptor.kind === "date" ? "date" : "text"}
        defaultValue={value ?? ""}
        aria-label={`Edit ${descriptor.label}`}
        className="h-7 text-xs"
        data-testid={`merge-edit-${descriptor.key}`}
        onKeyDown={(event) => {
          if (event.key === "Enter") {
            onStage(descriptor.key, event.currentTarget.value);
            setEditing(false);
          } else if (event.key === "Escape") {
            setEditing(false);
          }
        }}
        onBlur={(event) => {
          onStage(descriptor.key, event.currentTarget.value);
          setEditing(false);
        }}
      />
    );
  }

  return (
    <span className="flex min-h-7 items-center gap-1">
      <span
        className={cn("truncate", pinned && "font-semibold underline underline-offset-4")}
        data-testid={`merge-master-${descriptor.key}`}
      >
        {value ?? "—"}
      </span>
      {pinned && (
        <>
          {/* Real text, not just the weight: the staged state has to survive a
              screen reader, same rule as the compare grid's differs marker. */}
          <span className="sr-only">(staged for the merge)</span>
          <button
            type="button"
            className="text-muted-foreground hover:text-foreground focus-visible:ring-ring flex size-6 shrink-0 items-center justify-center rounded focus-visible:ring-2 focus-visible:outline-none"
            aria-label={`Revert ${descriptor.label} to the master's own value`}
            data-testid={`merge-revert-${descriptor.key}`}
            onClick={() => {
              onRevert(descriptor.key);
            }}
          >
            <Undo2 className="size-3" aria-hidden />
          </button>
        </>
      )}
      <button
        type="button"
        className="text-muted-foreground hover:text-foreground focus-visible:ring-ring flex size-6 shrink-0 items-center justify-center rounded focus-visible:ring-2 focus-visible:outline-none"
        aria-label={`Edit ${descriptor.label}`}
        data-testid={`merge-edit-open-${descriptor.key}`}
        onClick={() => {
          setEditing(true);
        }}
      >
        <Pencil className="size-3" aria-hidden />
      </button>
    </span>
  );
}

/**
 * The group grid, master on the left (spec 2.8): every golden field across
 * every member, click-to-copy from a member into the master, click-again (or
 * the master's undo) to revert, inline edit on the master's cells, and a radio
 * per non-master column to reassign.
 *
 * **No hue, by the house rule.** The spec's red/green conflict/edit colours
 * become weight, underline, tint and explicit text here: a differing member
 * value sits on a `bg-muted` row; a staged value is semibold + underlined +
 * announced as staged; the master carries the one inverted chip plus the word
 * "master".
 *
 * Kept pure (state in, callbacks out) so the render branches are
 * component-testable without mocking the query layer.
 */
export function MergeGroupGrid({
  members,
  draft,
  attributes,
  editable,
  onDraft,
}: {
  members: GroupMember[];
  draft: MergeDraft;
  attributes: readonly AttributeDescriptor[];
  /** False for account orgs: the server's overridable set is the person golden
   * columns, so the copy/edit affordances stay off rather than fail at submit. */
  editable: boolean;
  onDraft: (next: MergeDraft) => void;
}) {
  const master = members.find((member) => member.record_key === draft.master);
  if (!master) return null;
  // Master column first ("master on the left, duplicates right"), the rest in
  // the server's group order.
  const columns = [master, ...members.filter((member) => member.record_key !== draft.master)];
  return (
    <div className="overflow-x-auto rounded-md border">
      <table className="w-full text-xs" data-testid="merge-group-table">
        <thead>
          <tr className="text-muted-foreground border-b text-left">
            <th className="py-1.5 pr-3 pl-3 font-medium">Field</th>
            {columns.map((member) => (
              <th key={member.record_key} className="py-1.5 pr-3 font-medium">
                <span className="flex items-center gap-2">
                  <span className="font-mono text-xs">{member.record_key}</span>
                  {member.record_key === draft.master ? (
                    <span
                      className="bg-foreground text-background rounded-full px-2 py-0.5 text-[10px] font-medium"
                      data-testid="merge-master-mark"
                    >
                      master
                    </span>
                  ) : (
                    <label className="flex items-center gap-1 font-normal">
                      <input
                        type="radio"
                        name="merge-master"
                        className="size-3.5 accent-current"
                        checked={false}
                        onChange={() => {
                          onDraft(reassignMaster(draft, member.record_key));
                        }}
                        data-testid={`merge-make-master-${member.record_key}`}
                      />
                      master
                    </label>
                  )}
                </span>
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {attributes.map((descriptor) => {
            const naturalRaw = master.attributes[descriptor.key];
            const natural = typeof naturalRaw === "string" ? naturalRaw : null;
            const masterEffective = effectiveValue(draft, descriptor.key, natural);
            const rowDiffers = columns.some(
              (member) =>
                member.record_key !== draft.master &&
                typeof member.attributes[descriptor.key] === "string" &&
                member.attributes[descriptor.key] !== masterEffective,
            );
            return (
              <tr
                key={descriptor.key}
                className={cn("border-b last:border-0", rowDiffers && "bg-muted")}
                data-testid={`merge-row-${descriptor.key}`}
              >
                <td className="text-muted-foreground py-1.5 pr-3 pl-3">{descriptor.label}</td>
                {columns.map((member) => {
                  if (member.record_key === draft.master) {
                    return (
                      <td key={member.record_key} className="py-1.5 pr-3">
                        {editable ? (
                          <MasterCell
                            descriptor={descriptor}
                            member={member}
                            draft={draft}
                            onStage={(attribute, value) => {
                              onDraft(stageValue(draft, attribute, value, natural));
                            }}
                            onRevert={(attribute) => {
                              onDraft(revertValue(draft, attribute));
                            }}
                          />
                        ) : (
                          <span className="truncate">{natural ?? "—"}</span>
                        )}
                      </td>
                    );
                  }
                  const raw = member.attributes[descriptor.key];
                  const memberValue = typeof raw === "string" ? raw : null;
                  const stagedHere =
                    editable && memberValue !== null && draft.pins[descriptor.key] === memberValue;
                  // A STAGED value stays clickable even though it is no longer
                  // "offered" (it now equals the master's effective value):
                  // clicking it again is the revert, per the spec's
                  // click-to-copy / click-again-to-revert.
                  const offered =
                    editable &&
                    (stagedHere || isCopyOffered(draft, descriptor.key, memberValue, natural));
                  return (
                    <td key={member.record_key} className="py-1.5 pr-3">
                      {offered ? (
                        <button
                          type="button"
                          className={cn(
                            "hover:bg-accent focus-visible:ring-ring -mx-1 flex min-h-7 items-center gap-1 rounded px-1 text-left focus-visible:ring-2 focus-visible:outline-none",
                            stagedHere && "font-semibold underline underline-offset-4",
                          )}
                          aria-label={
                            stagedHere
                              ? `Revert ${descriptor.label} (staged from ${member.record_key})`
                              : `Use ${member.record_key}'s ${descriptor.label}`
                          }
                          data-testid={`merge-copy-${descriptor.key}-${member.record_key}`}
                          onClick={() => {
                            onDraft(
                              stagedHere
                                ? revertValue(draft, descriptor.key)
                                : stageValue(draft, descriptor.key, memberValue, natural),
                            );
                          }}
                        >
                          {memberValue}
                          {stagedHere && <Check className="size-3" aria-hidden />}
                        </button>
                      ) : (
                        <span className="truncate">{memberValue ?? "—"}</span>
                      )}
                    </td>
                  );
                })}
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

/**
 * The editable review merge (spec unit 2.8): the GROUP the merge would combine
 * — both review sides' whole entities, not just the pair — with a designated
 * master, click-to-copy from any member into the master, click-again to
 * revert, and inline edit on the master's cells.
 *
 * Submitting stages ONE bundle (the assertion star + the master's protect +
 * the pins + the review's own match resolution) through `POST /merges`; the
 * drain applies them together, and the record-keyed pins land on whatever
 * entity the merge mints.
 */
export function MergeGroupDialog({
  org,
  review,
  open,
  onClose,
  onMerged,
}: {
  org: string;
  review: ReviewRow;
  open: boolean;
  onClose: () => void;
  onMerged: (message: string) => void;
}) {
  const ds = useDatasetSlug();
  const entityType = useOrgEntityType(org, ds);
  const group = useMergeGroup(org, ds, open ? review.id : null);
  const submit = useMergeGroupSubmit(org, ds);
  const [draft, setDraft] = useState<MergeDraft | null>(null);
  const [failure, setFailure] = useState<string | null>(null);

  const members = useMemo(() => group.data?.items ?? [], [group.data]);
  // The draft initializes from the loaded group: the review's left side is the
  // default master, matching the pair grid's layout.
  const first = members[0];
  useEffect(() => {
    if (first !== undefined) {
      setDraft((current) => current ?? newDraft(review.rec_a_key ?? first.record_key));
    }
  }, [first, review.rec_a_key]);

  // Field overrides exist for the person attribute model only (the server's
  // overridable set is the person golden columns); for an account org the
  // dialog still merges the group and pins the master.
  const editable = entityType === "person";
  const attributes = goldenAttributes(entityType);

  function close() {
    setDraft(null);
    setFailure(null);
    onClose();
  }

  function confirm() {
    if (!draft) return;
    submit.mutate(
      {
        records: members.map((member) => member.record_key),
        master: draft.master,
        overrides: toOverridePayload(draft),
        review_id: review.id,
      },
      {
        onSuccess: () => {
          onMerged(
            `merge of ${String(members.length)} records staged — it applies with the next reconcile`,
          );
          close();
        },
        onError: (error) => {
          setFailure(
            error instanceof BffRequestError ? error.message : "could not stage the merge",
          );
        },
      },
    );
  }

  const pins = draft ? pinCount(draft) : 0;

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) close();
      }}
    >
      <DialogContent data-testid="merge-group-dialog">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <GitMerge className="size-4" aria-hidden /> Merge {members.length} record
            {members.length === 1 ? "" : "s"}
          </DialogTitle>
          <DialogDescription>
            Choose the master and the field values the merged record keeps. Everything stages
            together and applies with the next reconcile.
          </DialogDescription>
        </DialogHeader>

        {group.isPending && <Skeleton className="h-56 w-full" />}
        {group.isError && (
          <p className="text-muted-foreground text-xs" role="alert">
            The records behind this review could not be read — they may have been retired.
          </p>
        )}
        {draft !== null && (
          <>
            {group.data !== undefined && group.data.total > members.length && (
              <p className="text-muted-foreground text-xs" data-testid="merge-group-truncated">
                showing {members.length} of {group.data.total} records in this group
              </p>
            )}
            <MergeGroupGrid
              members={members}
              draft={draft}
              attributes={attributes}
              editable={editable}
              onDraft={setDraft}
            />
          </>
        )}

        {failure !== null && (
          <span className="text-xs font-medium" role="alert" data-testid="merge-group-error">
            {failure}
          </span>
        )}

        <DialogFooter>
          <Button variant="outline" onClick={close} disabled={submit.isPending}>
            Cancel
          </Button>
          <Button
            onClick={confirm}
            disabled={draft === null || submit.isPending || members.length < 2}
            data-er-action="review.merge.confirm"
            data-testid="merge-group-confirm"
          >
            <GitMerge aria-hidden />
            {pins > 0
              ? `Merge ${String(members.length)} records with ${String(pins)} pinned value${pins === 1 ? "" : "s"}`
              : `Merge ${String(members.length)} records`}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
