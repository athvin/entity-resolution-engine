"use client";

import { useParams } from "next/navigation";

/**
 * The dataset segment of the current URL.
 *
 * Falls back to `default` on org-level screens (settings, the datasets manage
 * page): every org is guaranteed a dataset named `default` — the convergence
 * sweep creates it for pre-dataset orgs and provisioning creates it for new
 * ones — so navigation rendered outside a dataset context still has somewhere
 * real to point. The last-used-dataset cookie replaces this fallback in the
 * management-UX phase.
 */
export function useDatasetSlug(): string {
  const params = useParams<{ ds?: string }>();
  return typeof params.ds === "string" && params.ds.length > 0 ? params.ds : "default";
}
