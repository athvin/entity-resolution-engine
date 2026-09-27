"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useRouter } from "next/navigation";

import { bffFetch } from "@/lib/api/client";
import type { OrgRole } from "@/lib/auth/types";
import { queryKeys } from "./keys";
import type { JobRow } from "./hooks";

export interface AdminOrgRow {
  name: string;
  state: string;
  active_config_version: number | null;
  created_at: string;
  display_name: string;
  registered: boolean;
  has_credentials: boolean;
}

export interface AdminOrgDetail extends Record<string, unknown> {
  name: string;
  state: string;
  active_config_version: number | null;
  registered: boolean;
  display_name: string;
  has_credentials: boolean;
}

export interface ProvisionResult {
  name: string;
  state: string;
  job_id?: string;
  admin_key?: string;
  admin_key_id?: string;
}

export function useAdminOrgs() {
  return useQuery({
    queryKey: queryKeys.adminOrgs(),
    queryFn: () => bffFetch<AdminOrgRow[]>("/api/admin/orgs"),
    refetchInterval: 30_000,
  });
}

export function useAdminOrg(org: string, options: { poll?: boolean } = {}) {
  return useQuery({
    queryKey: queryKeys.adminOrg(org),
    queryFn: () => bffFetch<AdminOrgDetail>(`/api/admin/orgs/${org}`),
    refetchInterval: (query) =>
      options.poll && query.state.data?.state === "provisioning" ? 2_000 : false,
  });
}

export function useAdminJobs() {
  return useQuery({
    queryKey: queryKeys.adminJobs(),
    queryFn: () => bffFetch<(JobRow & { org: string })[]>("/api/admin/jobs?limit=100"),
    refetchInterval: 5_000,
  });
}

export function useProvisionOrg() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: { name: string; display_name?: string | undefined }) =>
      bffFetch<ProvisionResult>("/api/admin/orgs", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      }),
    onSettled: () => void queryClient.invalidateQueries({ queryKey: queryKeys.adminOrgs() }),
  });
}

export function useRegisterOrg(org: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () => bffFetch<unknown>(`/api/admin/orgs/${org}/register`, { method: "POST" }),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.adminOrgs() });
      void queryClient.invalidateQueries({ queryKey: queryKeys.adminOrg(org) });
    },
  });
}

/** Enter/exit "view as tenant". Clears the whole cache: the identity changed. */
export function useImpersonation() {
  const queryClient = useQueryClient();
  const router = useRouter();
  const enter = useMutation({
    mutationFn: (body: { org: string; role: OrgRole }) =>
      bffFetch<{ org: string; role: OrgRole; expires_at: string }>("/api/impersonation", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      }),
    onSuccess: (data) => {
      queryClient.clear();
      router.push(`/${data.org}/dashboard`);
      router.refresh();
    },
  });
  const exit = useMutation({
    mutationFn: () => bffFetch<unknown>("/api/impersonation", { method: "DELETE" }),
    onSuccess: () => {
      queryClient.clear();
      router.push("/admin/tenants");
      router.refresh();
    },
  });
  return { enter, exit };
}
