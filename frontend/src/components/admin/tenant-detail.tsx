"use client";

import Link from "next/link";
import { ArrowLeft, Eye, KeyRound } from "lucide-react";

import { StateChip } from "@/components/runs/runs-content";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Skeleton } from "@/components/ui/skeleton";
import type { OrgRole } from "@/lib/auth/types";
import { useAdminOrg, useImpersonation, useRegisterOrg } from "@/lib/query/admin";

const VIEW_AS_ROLES: OrgRole[] = ["admin", "steward", "viewer"];

export function TenantDetail({ org }: { org: string }) {
  const detail = useAdminOrg(org, { poll: true });
  const register = useRegisterOrg(org);
  const { enter } = useImpersonation();

  if (detail.isPending || !detail.data) {
    return <Skeleton className="h-64 w-full" />;
  }
  const data = detail.data;

  return (
    <div className="flex flex-col gap-4" data-testid="tenant-detail">
      <div>
        <Link
          href="/admin/tenants"
          className="text-muted-foreground mb-2 inline-flex items-center gap-1 text-sm hover:underline"
        >
          <ArrowLeft className="size-3.5" /> Tenants
        </Link>
        <div className="flex flex-wrap items-center gap-3">
          <h1 className="text-xl font-semibold lg:text-2xl">{data.display_name}</h1>
          <StateChip state={data.state} />
          <span className="flex-1" />
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button
                variant="outline"
                size="sm"
                disabled={data.state !== "active" || !data.has_credentials}
                data-testid="detail-view-as"
              >
                <Eye /> View as
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuLabel>View {org} as</DropdownMenuLabel>
              {VIEW_AS_ROLES.map((role) => (
                <DropdownMenuItem
                  key={role}
                  onSelect={() => {
                    enter.mutate({ org, role });
                  }}
                >
                  {role}
                </DropdownMenuItem>
              ))}
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </div>

      <Card>
        <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
          <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
            Control plane
          </CardTitle>
        </CardHeader>
        <CardContent className="grid gap-x-6 gap-y-3 p-4 pt-2 text-sm sm:grid-cols-2 lg:p-6 lg:pt-2">
          <div>
            <div className="text-muted-foreground text-xs">Slug</div>
            <div className="font-mono">{data.name}</div>
          </div>
          <div>
            <div className="text-muted-foreground text-xs">Active config version</div>
            <div>
              {data.active_config_version === null ? "—" : String(data.active_config_version)}
            </div>
          </div>
          <div>
            <div className="text-muted-foreground text-xs">Service keys</div>
            <div>{data.has_credentials ? "vaulted (viewer / steward / admin)" : "not minted"}</div>
          </div>
          <div>
            <div className="text-muted-foreground text-xs">Registered in console</div>
            <div>{data.registered ? "yes" : "no"}</div>
          </div>
        </CardContent>
      </Card>

      {!data.has_credentials && (
        <Card>
          <CardContent className="flex flex-wrap items-center gap-3 p-4 text-sm">
            <KeyRound className="text-muted-foreground size-4" />
            <span>
              This org was provisioned outside the console. Mint service keys so the app can act for
              it.
            </span>
            <Button
              size="sm"
              variant="outline"
              onClick={() => {
                register.mutate();
              }}
              disabled={register.isPending}
              data-testid="register-org"
            >
              {register.isPending ? "Registering…" : "Register"}
            </Button>
          </CardContent>
        </Card>
      )}

      <Card>
        <CardContent className="text-muted-foreground flex flex-wrap gap-3 p-4 text-xs">
          <span>Suspend, resume and purge arrive with the lifecycle routes (M5) —</span>
          <Button variant="outline" size="sm" disabled title="backend route pending (M5)">
            Suspend
          </Button>
          <Button variant="outline" size="sm" disabled title="backend route pending (M5)">
            Purge
          </Button>
        </CardContent>
      </Card>
    </div>
  );
}
