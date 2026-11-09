"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { bffFetch } from "@/lib/api/client";

export interface IgnoreWord {
  word: string;
  dictionary: string;
}

const key = (org: string, ds: string) => ["org", org, "ds", ds, "ignore-words"] as const;
const path = (org: string, ds: string) => `/api/orgs/${org}/datasets/${ds}/ignore-words`;

export function useIgnoreWords(org: string, ds: string) {
  return useQuery({
    queryKey: key(org, ds),
    queryFn: () => bffFetch<{ rows: IgnoreWord[] }>(path(org, ds)),
  });
}

export function useSaveIgnoreWords(org: string, ds: string) {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (rows: IgnoreWord[]) =>
      bffFetch<{ rows: IgnoreWord[]; apply_job: string }>(path(org, ds), {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ rows }),
      }),
    onSuccess: (result) => {
      client.setQueryData(key(org, ds), { rows: result.rows });
    },
  });
}
