"use client";

import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { dropPathFor, useS3Ingest } from "@/lib/query/s3-ingest";

/**
 * Auto-ingest state (E9c): where to drop a file, and what has been read.
 *
 * Sits beside the delivery-receipts panel because it answers the half of "did
 * my file do anything" that receipts cannot. An upload gets an HTTP response
 * and a push gets a status code; a dropped object gets a dispatcher log line
 * nobody outside the cluster can read. So this is the only place a tenant can
 * tell "the poller has not run yet" apart from "the poller read it and found
 * nothing".
 *
 * **The drop path is shown per configured source, not as one prefix.** The
 * source comes from the directory, and a file dropped one level up is passed
 * over silently — so the exact path is the one thing worth spelling out rather
 * than describing.
 */
export function AutoIngestPanel({ org, sources }: { org: string; sources: readonly string[] }) {
  const state = useS3Ingest(org);

  return (
    <Card data-testid="auto-ingest">
      <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
        <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
          Auto-ingest — drop a file, no upload
        </CardTitle>
        <CardDescription>
          The dispatcher checks each source&apos;s folder about once a minute and imports the newest
          CSV it has not read. Replacing a file in place counts as new.
        </CardDescription>
      </CardHeader>
      <CardContent className="p-4 pt-2 lg:p-6 lg:pt-2">
        {state.isPending ? (
          <Skeleton className="h-16 w-full" />
        ) : state.data === undefined || state.data.bucket === null ? (
          <p className="text-muted-foreground text-sm" data-testid="auto-ingest-off">
            Not configured — no bucket is being watched for this tenant. Ask an operator to point
            auto-ingest at one, or keep uploading files here.
          </p>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full min-w-[32rem] text-sm" data-testid="auto-ingest-table">
              <thead>
                <tr className="text-muted-foreground border-b text-left text-xs uppercase">
                  <th className="py-1.5 pr-4 font-medium">Source</th>
                  <th className="py-1.5 pr-4 font-medium">Drop files here</th>
                  <th className="py-1.5 pr-4 font-medium">Last read</th>
                  <th className="py-1.5 font-medium">When</th>
                </tr>
              </thead>
              <tbody>
                {sources.map((source) => {
                  const landed = state.data.sources.find((row) => row.source === source);
                  return (
                    <tr
                      key={source}
                      className="border-b last:border-0"
                      data-testid={`auto-ingest-row-${source}`}
                    >
                      <td className="py-2 pr-4 font-medium">{source}</td>
                      <td className="text-muted-foreground py-2 pr-4 font-mono text-xs break-all">
                        {dropPathFor(state.data, source)}
                      </td>
                      {/*
                        An em dash rather than a zero or a blank: "nothing has
                        been dropped here yet" is a different fact from "a file
                        was read and was empty", and the receipts panel already
                        answers the second.
                      */}
                      <td className="py-2 pr-4 font-mono text-xs break-all">
                        {landed?.object_key ?? "— nothing yet"}
                      </td>
                      <td className="text-muted-foreground py-2 text-xs">
                        {landed?.updated_at ?? "—"}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
            {sources.length === 0 && (
              <p className="text-muted-foreground text-sm" data-testid="auto-ingest-no-sources">
                A bucket is being watched, but the published config declares no sources — so there
                is no folder a file could be dropped into yet.
              </p>
            )}
          </div>
        )}
      </CardContent>
    </Card>
  );
}
