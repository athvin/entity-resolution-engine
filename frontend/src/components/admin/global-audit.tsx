"use client";

import { useQuery } from "@tanstack/react-query";

import { AuditTable, type AuditRow } from "@/components/audit/audit-table";
import { bffFetch } from "@/lib/api/client";

interface AppAuditRow {
  id: string;
  at: string;
  userId: string;
  actingOrg: string | null;
  actingRole: string;
  impersonating: boolean;
  action: string;
  detail: unknown;
}

interface GlobalAudit {
  control_plane: AuditRow[];
  app: AppAuditRow[];
}

/** The operator's cross-tenant trail: control plane + the app tier's own rows. */
export function GlobalAuditFeed() {
  const audit = useQuery({
    queryKey: ["admin", "audit"],
    queryFn: () => bffFetch<GlobalAudit>("/api/admin/audit"),
    refetchInterval: 30_000,
  });

  return (
    <div className="flex flex-col gap-6" data-testid="global-audit">
      <h1 className="text-xl font-semibold lg:text-2xl">Audit</h1>

      <section className="flex flex-col gap-2">
        <h2 className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
          Control plane — every authenticated mutation, all tenants
        </h2>
        <AuditTable
          rows={audit.data?.control_plane}
          pending={audit.isPending}
          showOrg
          testId="audit-control-plane"
        />
      </section>

      <section className="flex flex-col gap-2">
        <h2 className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
          App tier — logins, provisioning, impersonation
        </h2>
        <AuditTable
          rows={audit.data?.app.map((row) => ({
            id: row.id,
            at: row.at,
            actor: `${row.userId}${row.impersonating ? " (impersonating)" : ""}`,
            org: row.actingOrg,
            action: row.action,
            detail: row.detail,
          }))}
          pending={audit.isPending}
          showOrg
          testId="audit-app"
        />
      </section>
    </div>
  );
}
