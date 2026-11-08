"use client";

import { useQuery } from "@tanstack/react-query";

import { AuditTable, type AuditRow } from "@/components/audit/audit-table";
import { bffFetch, BffRequestError } from "@/lib/api/client";
import { queryKeys } from "@/lib/query/keys";

/** The tenant's own trail (design §7.20): who did what, with person attribution. */
export function TenantAuditFeed({ org }: { org: string }) {
  const audit = useQuery({
    queryKey: [...queryKeys.org(org), "audit"],
    queryFn: () => bffFetch<AuditRow[]>(`/api/orgs/${org}/audit?limit=100`),
    refetchInterval: 30_000,
    retry: false,
  });

  if (audit.error instanceof BffRequestError && audit.error.status === 403) {
    return (
      <p className="text-muted-foreground py-16 text-center text-sm" data-testid="audit-forbidden">
        The audit trail is admin-only.
      </p>
    );
  }

  return (
    <div
      className="mx-auto flex w-full max-w-5xl min-w-0 flex-col gap-4"
      data-testid="tenant-audit"
    >
      <div className="flex items-start justify-between gap-4">
        <div>
          <h1 className="text-xl font-semibold lg:text-2xl">Audit</h1>
          <p className="text-muted-foreground mt-1 text-sm">
            Every mutation in this workspace, with the person behind it — merges carry names, not
            key ids.
          </p>
        </div>
        {/* A plain same-origin anchor: the session cookie rides along, and the
            BFF answers with content-disposition, so no client-side blob dance
            is needed. Exports the same page the table shows (limit 100). */}
        <a
          href={`/api/orgs/${org}/audit?format=csv&limit=100`}
          download
          data-testid="audit-export"
          className="text-sm font-medium underline underline-offset-4"
        >
          Export CSV
        </a>
      </div>
      <AuditTable rows={audit.data} pending={audit.isPending} showOrg={false} testId="audit-rows" />
    </div>
  );
}
