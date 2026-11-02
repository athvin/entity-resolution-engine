"use client";

import { useMutation } from "@tanstack/react-query";

import { bffFetch } from "@/lib/api/client";
import type { ProposedAction } from "@/lib/agent-actions/propose";

/**
 * Perform a confirmed proposal (V9).
 *
 * **Every branch below calls a route that already existed**, with the person's
 * own session, exactly as the review inbox and the records browser call them.
 * That is the design rather than an implementation convenience: there is no
 * agent-specific write path to secure, no second authorization to keep in sync,
 * and the audit log cannot tell this apart from a click on the page — because
 * at this point it IS one.
 *
 * If a future action needs a route that does not exist yet, the route comes
 * first and is reachable from the UI; an endpoint whose only caller is the
 * assistant would be exactly the write tool this design avoids.
 */
export function useRunAction(org: string, ds: string) {
  return useMutation({
    mutationFn: async (action: ProposedAction) => {
      switch (action.kind) {
        case "resolve_review":
          return bffFetch<unknown>(
            `/api/orgs/${org}/datasets/${ds}/reviews/${encodeURIComponent(action.subject)}/resolve`,
            {
              method: "POST",
              headers: { "content-type": "application/json" },
              body: JSON.stringify({ resolution: action.resolution }),
            },
          );
        case "suppress_record":
        case "unsuppress_record":
        case "protect_record":
        case "unprotect_record": {
          const path = {
            suppress_record: "suppress",
            unsuppress_record: "unsuppress",
            protect_record: "protect",
            unprotect_record: "unprotect",
          }[action.kind];
          return bffFetch<unknown>(`/api/orgs/${org}/datasets/${ds}/records/${path}`, {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: JSON.stringify({ record_key: action.subject }),
          });
        }
      }
    },
  });
}
