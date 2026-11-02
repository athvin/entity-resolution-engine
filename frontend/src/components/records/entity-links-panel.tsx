"use client";

import { useState } from "react";
import Link from "next/link";
import { Link2, Trash2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { BffRequestError } from "@/lib/api/client";
import {
  counterpart,
  isParent,
  LINK_KINDS,
  LINK_LABELS,
  type LinkKind,
  useCreateEntityLink,
  useDeleteEntityLink,
  useEntityLinks,
} from "@/lib/query/entity-links";

import { useDatasetSlug } from "@/lib/use-dataset-slug";
/**
 * Relationships that are NOT merges (E11b).
 *
 * The product has one answer to "these two belong together" — merge them — and
 * it is the wrong answer for a husband and wife at one address, or a subsidiary
 * and its parent. Merging loses the distinction and no later run recovers it;
 * recording nothing loses the relationship. This panel is the third option.
 *
 * **It sits below the merge editor, not beside it.** Someone looking at an
 * entity is usually deciding whether its members belong together; linking is
 * the answer when they have decided the members are RIGHT and some other entity
 * is related. Putting it above would offer the subtler tool first.
 */
export function EntityLinksPanel({
  org,
  entityId,
  canEdit,
}: {
  org: string;
  entityId: string;
  canEdit: boolean;
}) {
  const ds = useDatasetSlug();
  const links = useEntityLinks(org, ds, entityId);
  const create = useCreateEntityLink(org, ds, entityId);
  const remove = useDeleteEntityLink(org, ds, entityId);
  const [target, setTarget] = useState("");
  const [kind, setKind] = useState<LinkKind>("household");
  const [failure, setFailure] = useState<string | null>(null);

  function submit() {
    setFailure(null);
    const trimmed = target.trim();
    if (trimmed === "") return;
    create.mutate(
      // The viewed entity is the PARENT — it is the one whose page you are on,
      // so "this household contains that person" is the sentence being written.
      { parent_entity_id: entityId, child_entity_id: trimmed, kind },
      {
        onSuccess: () => {
          setTarget("");
        },
        onError: (error) => {
          setFailure(error instanceof BffRequestError ? error.message : "could not link");
        },
      },
    );
  }

  return (
    <Card data-testid="entity-links">
      <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
        <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
          Related entities
        </CardTitle>
        <CardDescription>
          A link records that two entities belong together without merging them — a household, a
          parent company. Both keep their own records and their own golden values.
        </CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-3 p-4 pt-2 lg:p-6 lg:pt-2">
        {links.isPending ? (
          <Skeleton className="h-12 w-full" />
        ) : (links.data?.items.length ?? 0) === 0 ? (
          <p className="text-muted-foreground text-sm" data-testid="entity-links-empty">
            Not linked to anything yet.
          </p>
        ) : (
          <ul className="flex flex-col gap-2" data-testid="entity-links-list">
            {links.data?.items.map((link) => {
              const other = counterpart(link, entityId);
              return (
                <li
                  key={link.id}
                  className="flex items-center justify-between gap-3 border-b pb-2 text-sm last:border-0 last:pb-0"
                  data-testid={`entity-link-${link.id}`}
                >
                  <span className="flex min-w-0 items-center gap-2">
                    <Link2 className="text-muted-foreground size-3.5 shrink-0" aria-hidden />
                    <span className="font-medium">{LINK_LABELS[link.kind]}</span>
                    {/* Which end the viewed entity is on. The same row read
                        from two sides is two different sentences, and saying
                        the same one for both would be wrong half the time. */}
                    <span className="text-muted-foreground text-xs">
                      {isParent(link, entityId) ? "contains" : "belongs to"}
                    </span>
                    <Link
                      href={`/${org}/${ds}/records/${encodeURIComponent(other)}`}
                      className="truncate font-mono text-xs underline underline-offset-2"
                    >
                      {other}
                    </Link>
                    {/* A derived link is a suggestion, a manual one is a
                        decision — a surface that cannot tell them apart
                        presents a guess as a judgement. */}
                    {link.origin !== "manual" && (
                      <span className="text-muted-foreground text-xs">({link.origin})</span>
                    )}
                  </span>
                  {canEdit && (
                    <Button
                      variant="ghost"
                      size="sm"
                      className="h-6 px-1"
                      disabled={remove.isPending}
                      onClick={() => {
                        remove.mutate(link.id);
                      }}
                      aria-label={`Remove the ${LINK_LABELS[link.kind]} link to ${other}`}
                      data-testid={`entity-link-remove-${link.id}`}
                    >
                      <Trash2 className="size-3.5" aria-hidden />
                    </Button>
                  )}
                </li>
              );
            })}
          </ul>
        )}

        {canEdit && (
          <div className="flex flex-wrap items-center gap-2">
            <Input
              value={target}
              onChange={(event) => {
                setTarget(event.target.value);
              }}
              placeholder="Entity id to link"
              className="h-8 max-w-xs font-mono text-xs"
              data-testid="entity-link-target"
            />
            <select
              value={kind}
              onChange={(event) => {
                setKind(event.target.value as LinkKind);
              }}
              className="border-input bg-background h-8 rounded-md border px-2 text-xs"
              aria-label="Link kind"
              data-testid="entity-link-kind"
            >
              {LINK_KINDS.map((value) => (
                <option key={value} value={value}>
                  {LINK_LABELS[value]}
                </option>
              ))}
            </select>
            <Button
              size="sm"
              variant="outline"
              disabled={create.isPending || target.trim() === ""}
              onClick={submit}
              data-testid="entity-link-add"
            >
              Link
            </Button>
          </div>
        )}

        {failure !== null && (
          <span className="text-xs font-medium" role="alert" data-testid="entity-link-error">
            {failure}
          </span>
        )}
      </CardContent>
    </Card>
  );
}
