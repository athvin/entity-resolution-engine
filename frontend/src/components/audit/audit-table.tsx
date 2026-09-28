"use client";

import { ScrollText } from "lucide-react";

import { Card, CardContent } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";

export interface AuditRow {
  id: number | string;
  at: string;
  actor?: string;
  org?: string | null;
  action: string;
  detail?: unknown;
}

export function AuditTable({
  rows,
  pending,
  showOrg,
  testId,
}: {
  rows: AuditRow[] | undefined;
  pending: boolean;
  showOrg: boolean;
  testId: string;
}) {
  if (pending) return <Skeleton className="h-48 w-full" />;
  if (!rows || rows.length === 0) {
    return (
      <Card>
        <CardContent className="text-muted-foreground flex items-center gap-2 p-8 text-sm">
          <ScrollText className="size-4" />
          No audit entries yet.
        </CardContent>
      </Card>
    );
  }
  return (
    <Card data-testid={testId}>
      <CardContent className="overflow-x-auto p-0">
        <table className="w-full min-w-[40rem] text-sm">
          <thead>
            <tr className="text-muted-foreground border-b text-left text-xs uppercase">
              <th className="px-4 py-2.5 font-medium">When</th>
              <th className="px-4 py-2.5 font-medium">Action</th>
              {showOrg && <th className="px-4 py-2.5 font-medium">Org</th>}
              <th className="px-4 py-2.5 font-medium">Actor</th>
              <th className="px-4 py-2.5 font-medium">Detail</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => (
              <tr key={String(row.id)} className="border-b last:border-0">
                <td className="text-muted-foreground px-4 py-2 text-xs whitespace-nowrap">
                  {row.at}
                </td>
                <td className="px-4 py-2 font-medium">{row.action}</td>
                {showOrg && (
                  <td className="text-muted-foreground px-4 py-2 text-xs">{row.org ?? "—"}</td>
                )}
                <td className="text-muted-foreground max-w-56 truncate px-4 py-2 text-xs">
                  {row.actor ?? "—"}
                </td>
                <td className="text-muted-foreground max-w-64 truncate px-4 py-2 font-mono text-[11px]">
                  {row.detail && Object.keys(row.detail).length > 0
                    ? JSON.stringify(row.detail)
                    : "—"}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </CardContent>
    </Card>
  );
}
