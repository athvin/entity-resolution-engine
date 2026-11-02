"use client";

import { useDataset } from "@/lib/query/datasets";

/**
 * Gates a dataset's screens on the dataset actually being usable.
 *
 * Renders the children only for an `active` dataset; a provisioning one shows
 * a pending state (and the detail hook polls until it flips), a suspended or
 * purging one says so, and a name the org does not have reads as not found.
 * State is signalled with weight and tone only — the design system has no
 * status colours to reach for.
 */
export function DatasetGuard({
  org,
  ds,
  children,
}: {
  org: string;
  ds: string;
  children: React.ReactNode;
}) {
  const dataset = useDataset(org, ds);

  if (dataset.isPending) {
    return (
      <div className="text-muted-foreground flex min-h-48 items-center justify-center text-sm">
        Loading…
      </div>
    );
  }

  if (dataset.isError) {
    return (
      <div
        className="mx-auto flex min-h-48 max-w-md flex-col items-center justify-center gap-2 text-center"
        data-testid="dataset-not-found"
      >
        <p className="text-lg font-semibold">No dataset “{ds}”</p>
        <p className="text-muted-foreground text-sm">
          This workspace has no dataset by that name. Pick another from the switcher in the top bar.
        </p>
      </div>
    );
  }

  const state = dataset.data.state;
  if (state === "provisioning") {
    return (
      <div
        className="mx-auto flex min-h-48 max-w-md flex-col items-center justify-center gap-2 text-center"
        data-testid="dataset-provisioning"
      >
        <p className="animate-pulse text-lg font-semibold">Setting up “{ds}”…</p>
        <p className="text-muted-foreground text-sm">
          The dataset’s lake and starter configuration are being created. This page will continue on
          its own once it is ready.
        </p>
      </div>
    );
  }
  if (state === "suspended" || state === "purging") {
    return (
      <div
        className="mx-auto flex min-h-48 max-w-md flex-col items-center justify-center gap-2 text-center"
        data-testid="dataset-suspended"
      >
        <p className="text-muted-foreground text-lg font-semibold line-through decoration-1">
          {ds}
        </p>
        <p className="text-muted-foreground text-sm">
          {state === "suspended"
            ? "This dataset is suspended. An administrator can resume it from the datasets page."
            : "This dataset is being purged."}
        </p>
      </div>
    );
  }

  return <>{children}</>;
}
