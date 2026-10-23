"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { bffFetch } from "@/lib/api/client";
import type { Tag } from "@/lib/entities/bindings/Tag";
import { queryKeys } from "./keys";

/** What a tag can be attached to. The closed set er-server enforces. */
export type TagSubject = "entity" | "review";

/**
 * Whether a role may name or apply tags.
 *
 * An allowlist, deliberately. `effectiveRole` returns `null` while the session
 * is loading, so the obvious `role !== "viewer"` is true for a *loading*
 * session and flashes the controls at a viewer before settling. The server
 * refuses either way; a UI must not offer what it knows will be rejected.
 */
export function canApplyTags(role: "viewer" | "steward" | "admin" | null): boolean {
  return role === "steward" || role === "admin";
}

/**
 * The tags the picker should offer: the vocabulary minus what is already on
 * this subject.
 *
 * Subtracted by id rather than by name, because a retired tag frees its name
 * for reuse — so two tags can share a name across time and only the id
 * identifies the row. Offering an already-applied tag is not merely untidy: the
 * apply is idempotent, so clicking it would appear to do nothing at all.
 */
export function offeredTags(vocabulary: readonly Tag[], applied: readonly Tag[]): Tag[] {
  const appliedIds = new Set(applied.map((tag) => tag.id));
  return vocabulary.filter((tag) => !appliedIds.has(tag.id));
}

/** The tenant's live vocabulary, alphabetical. */
export function useTags(org: string) {
  return useQuery({
    queryKey: queryKeys.tags(org),
    queryFn: () => bffFetch<Tag[]>(`/api/orgs/${org}/tags`),
  });
}

/**
 * The live tags on one subject — the chips.
 *
 * A separate query from the vocabulary rather than a client-side filter of it,
 * because the server already answers it with one indexed read and the chips on
 * a page change independently of the vocabulary.
 */
export function useSubjectTags(org: string, subjectType: TagSubject, subjectId: string) {
  return useQuery({
    queryKey: queryKeys.subjectTags(org, subjectType, subjectId),
    queryFn: () =>
      bffFetch<Tag[]>(
        `/api/orgs/${org}/tags?subject_type=${subjectType}&subject_id=${encodeURIComponent(
          subjectId,
        )}`,
      ),
    enabled: subjectId !== "",
  });
}

/**
 * Everything that changes tags invalidates the vocabulary AND the subject's
 * chips.
 *
 * Both, because applying a tag changes one subject's chips while creating or
 * retiring one changes the list offered on every subject — and a stale
 * vocabulary would keep offering a tag that no longer exists, which is a 404 on
 * the next click rather than a missing row.
 */
function useTagMutation<T, V>(
  org: string,
  run: (input: V) => Promise<T>,
  subject?: { type: TagSubject; id: string },
) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: run,
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.tags(org) });
      if (subject) {
        void queryClient.invalidateQueries({
          queryKey: queryKeys.subjectTags(org, subject.type, subject.id),
        });
      }
    },
  });
}

/** Add a label to the vocabulary. Steward+. */
export function useCreateTag(org: string, subject?: { type: TagSubject; id: string }) {
  return useTagMutation(
    org,
    (body: { name: string; description?: string }) =>
      bffFetch<Tag>(`/api/orgs/${org}/tags`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      }),
    subject,
  );
}

/** Apply or remove a tag on one subject. Steward+. */
export function useTagApplication(org: string, subject: { type: TagSubject; id: string }) {
  return useTagMutation(
    org,
    ({ tagId, apply }: { tagId: string; apply: boolean }) =>
      bffFetch<unknown>(
        `/api/orgs/${org}/tags/${encodeURIComponent(tagId)}/${apply ? "apply" : "unapply"}`,
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ subject_type: subject.type, subject_id: subject.id }),
        },
      ),
    subject,
  );
}

/** Retire a tag. Admin+ — it affects every subject the tag touched. */
export function useRetireTag(org: string) {
  return useTagMutation(org, (tagId: string) =>
    bffFetch<unknown>(`/api/orgs/${org}/tags/${encodeURIComponent(tagId)}`, { method: "DELETE" }),
  );
}
