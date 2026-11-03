"use client";

import Link from "next/link";
import { Layers, Pause, Play, Plus } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { useToast } from "@/components/ui/toaster";
import { effectiveRole, useSession } from "@/lib/query/hooks";
import { useDatasetLifecycle, useDatasets, type DatasetSummary } from "@/lib/query/datasets";
import { cn } from "@/lib/utils";

/**
 * A dataset's state, told without hue (design §3.3): `provisioning` pulses
 * because it is going to change on its own, `suspended` is struck through
 * because it is deliberately stopped, `purging` is dimmed because it is on
 * its way out, and `active` says nothing at all — the default needs no mark.
 */
function StateCell({ state }: { state: DatasetSummary["state"] }) {
  if (state === "active") {
    return <span className="text-muted-foreground text-xs">active</span>;
  }
  return (
    <span
      className={cn(
        "text-xs font-medium",
        state === "provisioning" && "animate-pulse",
        state === "suspended" && "text-muted-foreground line-through decoration-1",
        state === "purging" && "text-muted-foreground",
      )}
    >
      {state}
    </span>
  );
}

function formatDate(iso: string): string {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? "—" : d.toLocaleDateString();
}

/**
 * The manage screen (`/{org}/datasets`): every resolution domain in the org,
 * what it resolves, whether it is running, and the lifecycle controls.
 *
 * Org-level on purpose — it is the one tenant screen that is ABOUT datasets
 * rather than inside one, which is why it sits beside settings rather than
 * under `[ds]`.
 */
export function DatasetsContent({ org }: { org: string }) {
  const datasets = useDatasets(org);
  const lifecycle = useDatasetLifecycle(org);
  const session = useSession();
  const role = effectiveRole(session.data, org);
  const isAdmin = role === "admin";
  const toast = useToast();

  function setState(ds: string, action: "suspend" | "resume") {
    lifecycle.mutate(
      { ds, action },
      {
        // The verb on the button is the verb in the toast.
        onSuccess: () => {
          toast({ title: action === "suspend" ? "Suspended" : "Resumed", description: ds });
        },
        onError: (error: unknown) => {
          toast({
            title: `Could not ${action} ${ds}`,
            description: error instanceof Error ? error.message : "unknown error",
          });
        },
      },
    );
  }

  return (
    <div className="mx-auto flex w-full max-w-5xl min-w-0 flex-col gap-4" data-testid="datasets">
      <div className="flex items-center justify-between gap-3">
        <div>
          <h1 className="text-xl font-semibold lg:text-2xl">Datasets</h1>
          <p className="text-muted-foreground mt-1 text-sm">
            Each dataset is its own corpus, configuration and models. Records never match across
            them.
          </p>
        </div>
        {isAdmin && (
          <Button size="sm" asChild data-testid="new-dataset">
            <Link href={`/${org}/datasets/new`}>
              <Plus /> New dataset
            </Link>
          </Button>
        )}
      </div>

      {datasets.isPending ? (
        <Skeleton className="h-48 w-full" />
      ) : datasets.isError ? (
        <Card>
          <CardContent className="p-8 text-center text-sm" data-testid="datasets-error">
            <p className="font-medium">Could not load this workspace’s datasets.</p>
            <Button
              variant="outline"
              size="sm"
              className="mt-3"
              onClick={() => void datasets.refetch()}
            >
              Try again
            </Button>
          </CardContent>
        </Card>
      ) : datasets.data.length === 0 ? (
        <Card>
          <CardContent className="text-muted-foreground p-8 text-center text-sm">
            No datasets yet — create the first one to start resolving.
          </CardContent>
        </Card>
      ) : (
        <Card>
          <CardContent className="overflow-x-auto p-0">
            <table className="w-full min-w-[44rem] text-sm">
              <thead>
                <tr className="text-muted-foreground border-b text-left text-xs uppercase">
                  <th className="px-4 py-2.5 font-medium">Dataset</th>
                  <th className="px-4 py-2.5 font-medium">Resolves</th>
                  <th className="px-4 py-2.5 font-medium">State</th>
                  <th className="px-4 py-2.5 font-medium">Config</th>
                  <th className="px-4 py-2.5 font-medium">Created</th>
                  <th className="px-4 py-2.5 text-right font-medium">Actions</th>
                </tr>
              </thead>
              <tbody>
                {datasets.data.map((dataset) => (
                  <tr
                    key={dataset.name}
                    className="border-b last:border-0"
                    data-testid={`dataset-row-${dataset.name}`}
                  >
                    <td className="px-4 py-3">
                      <Link
                        href={`/${org}/${dataset.name}/dashboard`}
                        className="flex items-center gap-2 font-medium hover:underline"
                      >
                        <Layers className="text-muted-foreground size-4 shrink-0" />
                        <span className="min-w-0">
                          <span className="block truncate">
                            {dataset.display_name ?? dataset.name}
                          </span>
                          {dataset.display_name !== null && (
                            <span className="text-muted-foreground block truncate font-mono text-xs">
                              {dataset.name}
                            </span>
                          )}
                        </span>
                      </Link>
                    </td>
                    <td className="text-muted-foreground px-4 py-3 text-xs tracking-wide uppercase">
                      {dataset.entity_type}
                    </td>
                    <td className="px-4 py-3">
                      <StateCell state={dataset.state} />
                    </td>
                    <td className="px-4 py-3 tabular-nums">
                      {dataset.active_config_version === null ? (
                        <span className="text-muted-foreground">not published</span>
                      ) : (
                        `v${String(dataset.active_config_version)}`
                      )}
                    </td>
                    <td className="text-muted-foreground px-4 py-3 tabular-nums">
                      {formatDate(dataset.created_at)}
                    </td>
                    <td className="px-4 py-3">
                      <div className="flex justify-end gap-2">
                        {isAdmin && dataset.state === "active" && (
                          <Button
                            variant="outline"
                            size="sm"
                            disabled={lifecycle.isPending}
                            data-testid={`suspend-${dataset.name}`}
                            onClick={() => {
                              setState(dataset.name, "suspend");
                            }}
                          >
                            <Pause /> Suspend
                          </Button>
                        )}
                        {isAdmin && dataset.state === "suspended" && (
                          <Button
                            variant="outline"
                            size="sm"
                            disabled={lifecycle.isPending}
                            data-testid={`resume-${dataset.name}`}
                            onClick={() => {
                              setState(dataset.name, "resume");
                            }}
                          >
                            <Play /> Resume
                          </Button>
                        )}
                        {isAdmin && (
                          <Button
                            variant="outline"
                            size="sm"
                            disabled
                            data-testid={`purge-${dataset.name}`}
                            title="purge (drop the dataset's lake and its rows) is deliberately unimplemented — roadmap, as at the org level"
                          >
                            Purge
                          </Button>
                        )}
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </CardContent>
        </Card>
      )}
    </div>
  );
}
