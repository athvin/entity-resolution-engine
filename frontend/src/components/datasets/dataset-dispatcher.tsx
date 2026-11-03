"use client";

import { useEffect } from "react";
import { useRouter } from "next/navigation";

import { Skeleton } from "@/components/ui/skeleton";
import { readLastDataset } from "@/lib/last-dataset";
import { useDatasets, type DatasetSummary } from "@/lib/query/datasets";

/**
 * Where `/{org}` actually goes.
 *
 * The chain, in order: the dataset you were last in, then the only one you
 * have, then the manage page to choose from, then the create flow when there
 * is nothing to choose. Every step is checked against the live list rather
 * than taken on trust — a remembered dataset can have been suspended or
 * removed since, and following it blindly would land on the guard's
 * not-found instead of somewhere useful.
 *
 * Resolved on the client, after the list loads, for that reason: a
 * server-side redirect straight from the cookie would be faster and wrong.
 */
export function DatasetDispatcher({ org }: { org: string }) {
  const router = useRouter();
  const datasets = useDatasets(org);
  const rows = datasets.data;

  useEffect(() => {
    if (!rows) return;
    router.replace(`/${org}${destinationFor(rows, readCookie(), org)}`);
  }, [rows, org, router]);

  // The list is one request against the control plane, so this is brief. An
  // error falls through to the manage page, which renders the failure with a
  // retry rather than duplicating that handling here.
  useEffect(() => {
    if (datasets.isError) router.replace(`/${org}/datasets`);
  }, [datasets.isError, org, router]);

  return (
    <div className="mx-auto flex w-full max-w-4xl flex-col gap-4" data-testid="dataset-dispatcher">
      <Skeleton className="h-8 w-48" />
      <Skeleton className="h-48 w-full" />
      <span className="sr-only">Opening your workspace…</span>
    </div>
  );
}

function readCookie(): string {
  return typeof document === "undefined" ? "" : document.cookie;
}

/**
 * The path under `/{org}` to land on. Exported for the unit test: this is
 * four branches of routing policy, and driving them through a browser is a
 * much worse way to check them than calling the function.
 */
export function destinationFor(
  datasets: readonly DatasetSummary[],
  cookieHeader: string,
  org: string,
): string {
  // Suspended datasets stay reachable — their screens explain the freeze and
  // an admin resumes from there — so they are valid destinations. Purging
  // ones are not: that row is on its way out.
  const reachable = datasets.filter((d) => d.state !== "purging");
  if (reachable.length === 0) return "/datasets/new";

  const remembered = readLastDataset(cookieHeader, org);
  if (remembered !== null && reachable.some((d) => d.name === remembered)) {
    return `/${remembered}/dashboard`;
  }
  const [only] = reachable;
  if (only !== undefined && reachable.length === 1) return `/${only.name}/dashboard`;
  return "/datasets";
}
