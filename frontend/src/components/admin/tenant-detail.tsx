"use client";

import Link from "next/link";
import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, Cpu, Eye, KeyRound, Pause, Play } from "lucide-react";

import { bffFetch } from "@/lib/api/client";
import { queryKeys } from "@/lib/query/keys";

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
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Skeleton } from "@/components/ui/skeleton";
import type { OrgRole } from "@/lib/auth/types";
import {
  useAdminOrg,
  useImpersonation,
  useRegisterOrg,
  useUpdateResources,
  type OrgResources,
} from "@/lib/query/admin";

const VIEW_AS_ROLES: OrgRole[] = ["admin", "steward", "viewer"];

export function TenantDetail({ org }: { org: string }) {
  const detail = useAdminOrg(org, { poll: true });
  const register = useRegisterOrg(org);
  const { enter } = useImpersonation();
  const queryClient = useQueryClient();
  const lifecycle = useMutation({
    mutationFn: (action: "suspend" | "resume") =>
      bffFetch<{ state: string }>(`/api/admin/orgs/${org}/lifecycle`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ action }),
      }),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.adminOrg(org) });
      void queryClient.invalidateQueries({ queryKey: queryKeys.adminOrgs() });
    },
  });

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

      <ResourcesCard org={org} resources={data.resources} />

      <Card data-testid="lifecycle-card">
        <CardContent className="flex flex-wrap items-center gap-3 p-4 text-sm">
          <span className="text-muted-foreground text-xs">
            Lifecycle — a suspended tenant refuses jobs, imports and steward actions until resumed;
            its data is untouched.
          </span>
          {data.state === "active" && (
            <Button
              variant="outline"
              size="sm"
              disabled={lifecycle.isPending}
              data-testid="suspend-org"
              onClick={() => {
                lifecycle.mutate("suspend");
              }}
            >
              <Pause /> Suspend
            </Button>
          )}
          {data.state === "suspended" && (
            <Button
              variant="outline"
              size="sm"
              disabled={lifecycle.isPending}
              data-testid="resume-org"
              onClick={() => {
                lifecycle.mutate("resume");
              }}
            >
              <Play /> Resume
            </Button>
          )}
          <Button
            variant="outline"
            size="sm"
            disabled
            title="purge (drop database + delete lake prefix) is deliberately unimplemented — roadmap"
          >
            Purge
          </Button>
        </CardContent>
      </Card>
    </div>
  );
}

function ResourcesCard({ org, resources }: { org: string; resources: OrgResources }) {
  const update = useUpdateResources(org);
  const [threads, setThreads] = useState(resources.duckdb_threads?.toString() ?? "");
  const [memory, setMemory] = useState(resources.duckdb_memory_limit ?? "");

  const threadsNum = Number(threads);
  const threadsValid =
    threads === "" || (Number.isInteger(threadsNum) && threadsNum >= 1 && threadsNum <= 32);
  const memoryValid = memory === "" || /^[1-9][0-9]*[MG]B$/.test(memory);
  const dirty =
    threads !== (resources.duckdb_threads?.toString() ?? "") ||
    memory !== (resources.duckdb_memory_limit ?? "");

  function save() {
    const body: { duckdb_threads?: number; duckdb_memory_limit?: string } = {};
    if (threads !== "" && threadsNum !== resources.duckdb_threads) body.duckdb_threads = threadsNum;
    if (memory !== "" && memory !== resources.duckdb_memory_limit)
      body.duckdb_memory_limit = memory;
    if (body.duckdb_threads === undefined && body.duckdb_memory_limit === undefined) return;
    update.mutate(body);
  }

  return (
    <Card data-testid="resources-card">
      <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
        <CardTitle className="text-muted-foreground flex items-center gap-1.5 text-xs font-medium tracking-wide uppercase">
          <Cpu className="size-3.5" /> Pipeline resources
        </CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-3 p-4 pt-2 text-sm lg:p-6 lg:pt-2">
        <span className="text-muted-foreground text-xs">
          DuckDB parallelism for this tenant&apos;s pipeline. More threads speed matching and
          training; scoring is deterministic across thread counts, so no retrain follows a change —
          it applies to the tenant&apos;s next job.
        </span>
        <div className="flex flex-wrap items-end gap-4">
          <div className="flex flex-col gap-1">
            <Label htmlFor="duckdb-threads" className="text-xs">
              DuckDB threads
            </Label>
            <Input
              id="duckdb-threads"
              data-testid="duckdb-threads"
              inputMode="numeric"
              className="w-28"
              value={threads}
              placeholder="default"
              onChange={(event) => {
                setThreads(event.target.value.trim());
              }}
            />
          </div>
          <div className="flex flex-col gap-1">
            <Label htmlFor="duckdb-memory" className="text-xs">
              DuckDB memory
            </Label>
            <Input
              id="duckdb-memory"
              data-testid="duckdb-memory"
              className="w-32"
              value={memory}
              placeholder="e.g. 6GB"
              onChange={(event) => {
                setMemory(event.target.value.trim());
              }}
            />
          </div>
          <Button
            size="sm"
            variant="outline"
            data-testid="save-resources"
            disabled={!dirty || !threadsValid || !memoryValid || update.isPending}
            onClick={save}
          >
            {update.isPending ? "Saving…" : "Save"}
          </Button>
        </div>
        {!threadsValid && (
          <span className="text-destructive text-xs">Threads must be a whole number 1–32.</span>
        )}
        {!memoryValid && (
          <span className="text-destructive text-xs">Memory must look like 6GB or 512MB.</span>
        )}
        {update.isError && (
          <span className="text-destructive text-xs" data-testid="resources-error">
            {update.error instanceof Error ? update.error.message : "Could not update resources."}
          </span>
        )}
      </CardContent>
    </Card>
  );
}
