"use client";

import { useQuery } from "@tanstack/react-query";

import { bffFetch } from "@/lib/api/client";
import type { SourceRecord } from "@/lib/query/reviews";
import { queryKeys } from "./keys";

/** How many members one group view will fetch values for.
 *
 * A household or a company can have dozens of source records, and the merge
 * editor is a comparison a human reads — past a handful of columns nobody is
 * comparing anything. The cap is reported by the caller rather than hidden, so
 * a large group says so instead of appearing to show everything. */
export const MAX_COMPARED_MEMBERS = 8;

/**
 * Every member record's values, for the group merge editor (F1).
 *
 * The same `?key=` endpoint `useRecordPair` uses for the review inbox's
 * two-record compare — generalised to N. That reuse is the point: a group
 * editor built on a different read would drift from the pair adjudicator's
 * idea of what a record's values are.
 */
export function useMemberValues(org: string, recordKeys: readonly string[]) {
  const keys = recordKeys.slice(0, MAX_COMPARED_MEMBERS);
  return useQuery({
    queryKey: queryKeys.memberValues(org, keys),
    queryFn: () => {
      const params = new URLSearchParams();
      for (const key of keys) params.append("key", key);
      return bffFetch<{ items: SourceRecord[]; snapshot: number }>(
        `/api/orgs/${org}/records?${params.toString()}`,
      );
    },
    enabled: keys.length > 0,
    staleTime: 60_000,
  });
}

/**
 * One attribute across the master and every member.
 *
 * `differs` is computed against the MASTER, not between members: the question
 * the editor answers is "is the golden value the one I want", and two members
 * disagreeing with each other while both matching the master is not something
 * a reader needs flagged.
 */
export interface FieldAcrossMembers {
  attribute: string;
  master: string | null;
  values: { recordKey: string; value: string | null; differs: boolean }[];
  /** True when at least one member holds something the master does not. */
  hasAlternative: boolean;
}

function text(value: unknown): string | null {
  if (typeof value === "string") return value.trim() === "" ? null : value.trim();
  if (typeof value === "number") return String(value);
  return null;
}

export function compareAcrossMembers(
  attributes: readonly string[],
  master: Record<string, unknown>,
  members: readonly SourceRecord[],
): FieldAcrossMembers[] {
  return attributes.map((attribute) => {
    const masterValue = text(master[attribute]);
    const values = members.map((member) => {
      const value = text(member.attributes[attribute]);
      return { recordKey: member.record_key, value, differs: value !== masterValue };
    });
    return {
      attribute,
      master: masterValue,
      values,
      // An alternative is a member value that is BOTH different and present —
      // a member missing a field the master has is not an offer to make the
      // master empty, and showing it as one invites a destructive click.
      hasAlternative: values.some((entry) => entry.differs && entry.value !== null),
    };
  });
}
