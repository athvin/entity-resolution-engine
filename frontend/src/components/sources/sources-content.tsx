"use client";

import { useRef, useState } from "react";
import Link from "next/link";
import { useQuery } from "@tanstack/react-query";
import { Database, Loader2, Upload } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { bffFetch, BffRequestError } from "@/lib/api/client";
import { effectiveRole, useSession } from "@/lib/query/hooks";
import { queryKeys } from "@/lib/query/keys";
import { useImportFile, useSources } from "@/lib/query/steward";
import { cn } from "@/lib/utils";

interface ReceiptRow {
  ingest_batch_id: string;
  run_id: string;
  source_system: string;
  new_count: number;
  changed_count: number;
  unchanged_count: number;
  tombstone_count: number;
  resurrected_count: number;
  created_at: string;
}

function ReceiptsPanel({ org }: { org: string }) {
  const receipts = useQuery({
    queryKey: [...queryKeys.org(org), "receipts"],
    queryFn: () => bffFetch<{ items: ReceiptRow[] }>(`/api/orgs/${org}/imports/receipts?limit=20`),
    refetchInterval: 30_000,
  });
  return (
    <Card data-testid="receipts">
      <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
        <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
          Delivery receipts — did my file do anything?
        </CardTitle>
      </CardHeader>
      <CardContent className="overflow-x-auto p-4 pt-2 lg:p-6 lg:pt-2">
        {receipts.isPending ? (
          <Skeleton className="h-20 w-full" />
        ) : (receipts.data?.items.length ?? 0) === 0 ? (
          <p className="text-muted-foreground text-sm">No deliveries recorded yet.</p>
        ) : (
          <table className="w-full min-w-[36rem] text-sm">
            <thead>
              <tr className="text-muted-foreground border-b text-left text-xs uppercase">
                <th className="py-1.5 pr-4 font-medium">Source</th>
                <th className="py-1.5 pr-4 font-medium">New</th>
                <th className="py-1.5 pr-4 font-medium">Changed</th>
                <th className="py-1.5 pr-4 font-medium">Unchanged</th>
                <th className="py-1.5 pr-4 font-medium">Removed</th>
                <th className="py-1.5 font-medium">Delivered</th>
              </tr>
            </thead>
            <tbody>
              {receipts.data?.items.map((receipt) => {
                const noop =
                  receipt.new_count === 0 &&
                  receipt.changed_count === 0 &&
                  receipt.tombstone_count === 0;
                return (
                  <tr key={receipt.ingest_batch_id} className="border-b last:border-0">
                    <td className="py-2 pr-4 font-medium">{receipt.source_system}</td>
                    <td className="py-2 pr-4 tabular-nums">{receipt.new_count}</td>
                    <td className="py-2 pr-4 tabular-nums">{receipt.changed_count}</td>
                    <td className="text-muted-foreground py-2 pr-4 tabular-nums">
                      {receipt.unchanged_count}
                      {noop && " — identical delivery, not a failure"}
                    </td>
                    <td className="py-2 pr-4 tabular-nums">{receipt.tombstone_count}</td>
                    <td className="text-muted-foreground py-2 text-xs">{receipt.created_at}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
      </CardContent>
    </Card>
  );
}

const ROLE_RANK = { viewer: 0, steward: 1, admin: 2 } as const;

function SourceCard({ org, name, isSteward }: { org: string; name: string; isSteward: boolean }) {
  const importFile = useImportFile(org);
  const inputRef = useRef<HTMLInputElement>(null);
  const [dragging, setDragging] = useState(false);
  const [message, setMessage] = useState<string | null>(null);

  function deliver(file: File) {
    setMessage(null);
    importFile.mutate(
      { source: name, file },
      {
        onSuccess: (result) => {
          setMessage(`queued — job ${result.job.job_id.slice(-8)} runs the import`);
        },
        onError: (error) => {
          setMessage(error instanceof BffRequestError ? error.message : "upload failed");
        },
      },
    );
  }

  return (
    <Card
      data-testid={`source-${name}`}
      className={cn(dragging && "border-primary")}
      onDragOver={(event) => {
        if (!isSteward) return;
        event.preventDefault();
        setDragging(true);
      }}
      onDragLeave={() => {
        setDragging(false);
      }}
      onDrop={(event) => {
        if (!isSteward) return;
        event.preventDefault();
        setDragging(false);
        const file = event.dataTransfer.files.item(0);
        if (file) deliver(file);
      }}
    >
      <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
        <CardTitle className="flex items-center gap-2 text-base">
          <Database className="text-muted-foreground size-4" />
          {name}
        </CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-3 p-4 pt-2 lg:p-6 lg:pt-2">
        <p className="text-muted-foreground text-sm">
          {isSteward
            ? "Drop a CSV here or choose a file — the import runs as an incremental job."
            : "Read-only: importing needs the steward role."}
        </p>
        {isSteward && (
          <div className="flex items-center gap-3">
            <input
              ref={inputRef}
              type="file"
              accept=".csv,text/csv"
              className="hidden"
              data-testid={`source-${name}-file`}
              onChange={(event) => {
                const file = event.target.files?.item(0);
                if (file) deliver(file);
                event.target.value = "";
              }}
            />
            <Button
              variant="outline"
              size="sm"
              disabled={importFile.isPending}
              onClick={() => inputRef.current?.click()}
              data-testid={`source-${name}-upload`}
            >
              {importFile.isPending ? <Loader2 className="animate-spin" /> : <Upload />}
              Import file
            </Button>
            {message && (
              <span className="text-muted-foreground text-xs" data-testid={`source-${name}-status`}>
                {message}
              </span>
            )}
          </div>
        )}
      </CardContent>
    </Card>
  );
}

export function SourcesContent({ org }: { org: string }) {
  const sources = useSources(org);
  const session = useSession();
  const role = effectiveRole(session.data, org);
  const isSteward = role !== null && ROLE_RANK[role] >= ROLE_RANK.steward;

  return (
    <div className="mx-auto flex max-w-4xl flex-col gap-4 lg:gap-6" data-testid="sources-page">
      <div className="flex items-center justify-between gap-3">
        <h1 className="text-xl font-semibold lg:text-2xl">Sources</h1>
        <span className="flex items-center gap-3">
          {sources.data && (
            <span className="text-muted-foreground text-xs">
              config v{sources.data.config_version}
            </span>
          )}
          {role === "admin" && (
            <Button size="sm" variant="outline" asChild data-testid="new-source">
              <Link href={`/${org}/sources/new`}>New source</Link>
            </Button>
          )}
        </span>
      </div>

      {sources.isPending ? (
        <Skeleton className="h-48 w-full" />
      ) : (sources.data?.sources.length ?? 0) === 0 ? (
        <Card>
          <CardContent className="text-muted-foreground p-8 text-center text-sm">
            No sources configured yet.
          </CardContent>
        </Card>
      ) : (
        <div className="grid gap-4 lg:grid-cols-2">
          {sources.data?.sources.map((source) => (
            <SourceCard key={source.name} org={org} name={source.name} isSteward={isSteward} />
          ))}
        </div>
      )}

      <ReceiptsPanel org={org} />

      <Card>
        <CardContent className="text-muted-foreground flex flex-wrap items-center gap-2 p-4 text-xs">
          Adding a brand-new source (guided mapping, match preview) is the M4 wizard;
          <Link href={`/${org}/runs`} className="underline underline-offset-2">
            watch imports land in Runs
          </Link>
          meanwhile.
        </CardContent>
      </Card>
    </div>
  );
}
