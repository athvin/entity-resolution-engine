"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { bffFetch } from "@/lib/api/client";
import { queryKeys } from "./keys";

/** What a link can be. Mirrors `entity_link.kind`'s accepted values. */
export const LINK_KINDS = ["household", "company", "related"] as const;
export type LinkKind = (typeof LINK_KINDS)[number];

/** How each kind reads to a person, in the house voice. */
export const LINK_LABELS: Record<LinkKind, string> = {
  household: "Same household",
  company: "Same company",
  related: "Related",
};

export interface EntityLink {
  id: string;
  parent_entity_id: string;
  child_entity_id: string;
  kind: LinkKind;
  /** `manual` is a steward's judgement; `rule` and `llm` are derived. */
  origin: "manual" | "rule" | "llm";
  note: string | null;
  created_by: string;
  created_at: string;
  updated_at: string;
}

export function useEntityLinks(org: string, entityId: string) {
  return useQuery({
    queryKey: [...queryKeys.org(org), "entity-links", entityId],
    queryFn: () =>
      bffFetch<{ items: EntityLink[] }>(
        `/api/orgs/${org}/entities/${encodeURIComponent(entityId)}/links`,
      ),
    enabled: entityId !== "",
  });
}

export interface CreateLinkInput {
  parent_entity_id: string;
  child_entity_id: string;
  kind: LinkKind;
  note?: string;
}

export function useCreateEntityLink(org: string, entityId: string) {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (input: CreateLinkInput) =>
      bffFetch<EntityLink>(`/api/orgs/${org}/entity-links`, {
        method: "POST",
        body: JSON.stringify(input),
      }),
    onSuccess: () => {
      void client.invalidateQueries({
        queryKey: [...queryKeys.org(org), "entity-links", entityId],
      });
    },
  });
}

export function useDeleteEntityLink(org: string, entityId: string) {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (linkId: string) =>
      // `{deleted}`, not void: the route answers with a JSON body like every
      // other DELETE here, and a bare 204 is what made the BFF's `forward`
      // throw "Unexpected end of JSON input".
      bffFetch<{ deleted: string }>(`/api/orgs/${org}/entity-links/${encodeURIComponent(linkId)}`, {
        method: "DELETE",
      }),
    onSuccess: () => {
      void client.invalidateQueries({
        queryKey: [...queryKeys.org(org), "entity-links", entityId],
      });
    },
  });
}

/**
 * The other end of a link, relative to the entity being viewed.
 *
 * The read answers from both sides in one query, so a row can have this entity
 * on either end and the panel has to say which other entity it points at. Done
 * here rather than in the component because getting it backwards would render
 * an entity as its own relation — visibly wrong but easy to write.
 */
export function counterpart(link: EntityLink, entityId: string): string {
  return link.parent_entity_id === entityId ? link.child_entity_id : link.parent_entity_id;
}

/**
 * Is the viewed entity the owning side?
 *
 * Worth distinguishing because "this household contains Jane" and "Jane belongs
 * to this household" are the same row read from two ends, and a panel that said
 * the same sentence for both would be wrong half the time.
 */
export function isParent(link: EntityLink, entityId: string): boolean {
  return link.parent_entity_id === entityId;
}
