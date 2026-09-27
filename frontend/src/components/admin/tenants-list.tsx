"use client";

import Link from "next/link";
import { Building2, Eye, Plus } from "lucide-react";

import { StateChip } from "@/components/runs/runs-content";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Skeleton } from "@/components/ui/skeleton";
import type { OrgRole } from "@/lib/auth/types";
import { useAdminOrgs, useImpersonation } from "@/lib/query/admin";

const VIEW_AS_ROLES: OrgRole[] = ["admin", "steward", "viewer"];

export function TenantsList() {
  const orgs = useAdminOrgs();
  const { enter } = useImpersonation();

  return (
    <div className="flex flex-col gap-4" data-testid="tenants-list">
      <div className="flex items-center justify-between">
        <h1 className="text-xl font-semibold lg:text-2xl">Tenants</h1>
        <Button size="sm" asChild data-testid="new-tenant">
          <Link href="/admin/tenants/new">
            <Plus /> New tenant
          </Link>
        </Button>
      </div>

      {orgs.isPending ? (
        <Skeleton className="h-48 w-full" />
      ) : (orgs.data ?? []).length === 0 ? (
        <Card>
          <CardContent className="text-muted-foreground p-8 text-center text-sm">
            No tenants yet — provision the first one.
          </CardContent>
        </Card>
      ) : (
        <Card>
          <CardContent className="overflow-x-auto p-0">
            <table className="w-full min-w-[40rem] text-sm">
              <thead>
                <tr className="text-muted-foreground border-b text-left text-xs uppercase">
                  <th className="px-4 py-2.5 font-medium">Tenant</th>
                  <th className="px-4 py-2.5 font-medium">State</th>
                  <th className="px-4 py-2.5 font-medium">Config</th>
                  <th className="px-4 py-2.5 font-medium">Keys</th>
                  <th className="px-4 py-2.5 font-medium">Plan</th>
                  <th className="px-4 py-2.5 text-right font-medium">Actions</th>
                </tr>
              </thead>
              <tbody>
                {(orgs.data ?? []).map((org) => (
                  <tr
                    key={org.name}
                    className="border-b last:border-0"
                    data-testid={`tenant-${org.name}`}
                  >
                    <td className="px-4 py-2.5">
                      <Link
                        href={`/admin/tenants/${org.name}`}
                        className="flex items-center gap-2 font-medium hover:underline"
                      >
                        <Building2 className="text-muted-foreground size-4" />
                        {org.display_name}
                        {org.display_name !== org.name && (
                          <span className="text-muted-foreground font-mono text-xs">
                            {org.name}
                          </span>
                        )}
                      </Link>
                    </td>
                    <td className="px-4 py-2.5">
                      <StateChip state={org.state} />
                    </td>
                    <td className="text-muted-foreground px-4 py-2.5">
                      {org.active_config_version === null
                        ? "—"
                        : `v${String(org.active_config_version)}`}
                    </td>
                    <td className="text-muted-foreground px-4 py-2.5 text-xs">
                      {org.has_credentials ? "vaulted" : "not registered"}
                    </td>
                    {/* Billing is the Stripe phase; the column is a deliberate placeholder. */}
                    <td className="text-muted-foreground px-4 py-2.5">—</td>
                    <td className="px-4 py-2.5 text-right">
                      <DropdownMenu>
                        <DropdownMenuTrigger asChild>
                          <Button
                            variant="outline"
                            size="sm"
                            disabled={org.state !== "active" || !org.has_credentials}
                            data-testid={`view-as-${org.name}`}
                          >
                            <Eye /> View as
                          </Button>
                        </DropdownMenuTrigger>
                        <DropdownMenuContent align="end">
                          <DropdownMenuLabel>View {org.name} as</DropdownMenuLabel>
                          {VIEW_AS_ROLES.map((role) => (
                            <DropdownMenuItem
                              key={role}
                              data-testid={`view-as-${org.name}-${role}`}
                              onSelect={() => {
                                enter.mutate({ org: org.name, role });
                              }}
                            >
                              {role}
                            </DropdownMenuItem>
                          ))}
                        </DropdownMenuContent>
                      </DropdownMenu>
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
