"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { bffFetch } from "@/lib/api/client";
import { queryKeys } from "./keys";

export type OrgRole = "viewer" | "steward" | "admin";

export interface MemberRow {
  user_id: string;
  email: string;
  display_name: string;
  role: OrgRole;
  joined_at: string;
}

export interface PendingInvite {
  invite_id: string;
  email: string;
  role: OrgRole;
  invited_by: string;
  expires_at: string;
}

const membersKey = (org: string) => [...queryKeys.org(org), "members"] as const;

export function useMembers(org: string) {
  return useQuery({
    queryKey: membersKey(org),
    queryFn: () =>
      bffFetch<{ members: MemberRow[]; invites: PendingInvite[] }>(`/api/orgs/${org}/members`),
  });
}

export interface InviteResult {
  invite_id: string;
  expires_at: string;
  /** Always returned so an admin can hand the link over directly. */
  link: string;
  emailed: boolean;
}

export function useInviteMember(org: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: { email: string; role: OrgRole }) =>
      bffFetch<InviteResult>(`/api/orgs/${org}/members`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      }),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: membersKey(org) });
    },
  });
}

export function useChangeMemberRole(org: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ userId, role }: { userId: string; role: OrgRole }) =>
      bffFetch<{ user_id: string; role: OrgRole }>(`/api/orgs/${org}/members/${userId}`, {
        method: "PATCH",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ role }),
      }),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: membersKey(org) });
    },
  });
}

export function useRemoveMember(org: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (userId: string) =>
      bffFetch<{ removed: boolean }>(`/api/orgs/${org}/members/${userId}`, { method: "DELETE" }),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: membersKey(org) });
    },
  });
}

export function useChangePassword() {
  return useMutation({
    mutationFn: (body: { current_password: string; new_password: string }) =>
      bffFetch<{ changed: boolean }>("/api/auth/password", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      }),
  });
}
