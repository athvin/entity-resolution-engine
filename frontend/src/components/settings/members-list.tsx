"use client";

import { useState } from "react";
import { Copy, Mail, UserPlus, UserX } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Skeleton } from "@/components/ui/skeleton";
import { useToast } from "@/components/ui/toaster";
import { BffRequestError } from "@/lib/api/client";
import { effectiveRole, useSession } from "@/lib/query/hooks";
import {
  useChangeMemberRole,
  useInviteMember,
  useMembers,
  useRemoveMember,
  type InviteResult,
  type OrgRole,
} from "@/lib/query/members";

const ROLES: OrgRole[] = ["viewer", "steward", "admin"];

/** Workspace membership (design §2.2): who is in, at what role, and who is invited. */
export function MembersList({ org }: { org: string }) {
  const query = useMembers(org);
  const session = useSession();
  const invite = useInviteMember(org);
  const changeRole = useChangeMemberRole(org);
  const remove = useRemoveMember(org);
  const toast = useToast();
  const isAdmin = effectiveRole(session.data, org) === "admin";
  const selfId = session.data?.user.id;

  const [email, setEmail] = useState("");
  const [role, setRole] = useState<OrgRole>("steward");
  const [error, setError] = useState<string | null>(null);
  const [lastInvite, setLastInvite] = useState<InviteResult | null>(null);

  function fail(cause: unknown) {
    setError(cause instanceof BffRequestError ? cause.message : "that did not work");
  }

  return (
    <div className="flex w-full flex-col gap-4 lg:gap-6" data-testid="members-page">
      <Card data-testid="members-table">
        <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
          <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
            Members
          </CardTitle>
        </CardHeader>
        <CardContent className="overflow-x-auto p-0">
          {query.isPending ? (
            <Skeleton className="m-4 h-32" />
          ) : (
            <table className="w-full min-w-[34rem] text-sm">
              <thead>
                <tr className="text-muted-foreground border-b text-left text-xs uppercase">
                  <th className="px-4 py-2.5 font-medium">Person</th>
                  <th className="px-4 py-2.5 font-medium">Role</th>
                  <th className="px-4 py-2.5 font-medium">Joined</th>
                  <th className="px-4 py-2.5 text-right font-medium" />
                </tr>
              </thead>
              <tbody>
                {(query.data?.members ?? []).map((member) => {
                  const isSelf = member.user_id === selfId;
                  return (
                    <tr
                      key={member.user_id}
                      className="border-b last:border-0"
                      data-testid={`member-${member.email}`}
                    >
                      <td className="px-4 py-2.5">
                        <div className="font-medium">{member.display_name}</div>
                        <div className="text-muted-foreground text-xs">{member.email}</div>
                      </td>
                      <td className="px-4 py-2.5">
                        <select
                          value={member.role}
                          // Your own role is never yours to change: undoing it
                          // would need the power you just gave away.
                          disabled={!isAdmin || isSelf || changeRole.isPending}
                          aria-label={`Role for ${member.email}`}
                          data-testid={`member-role-${member.email}`}
                          onChange={(event) => {
                            setError(null);
                            changeRole.mutate(
                              { userId: member.user_id, role: event.target.value as OrgRole },
                              { onError: fail },
                            );
                          }}
                          className="border-input bg-background h-9 rounded-md border px-2 text-sm disabled:opacity-60"
                        >
                          {ROLES.map((value) => (
                            <option key={value} value={value}>
                              {value}
                            </option>
                          ))}
                        </select>
                      </td>
                      <td className="text-muted-foreground px-4 py-2.5 text-xs">
                        {member.joined_at.slice(0, 10)}
                      </td>
                      <td className="px-4 py-2.5 text-right">
                        {isAdmin && !isSelf && (
                          <Button
                            variant="ghost"
                            size="icon"
                            aria-label={`Remove ${member.email}`}
                            data-testid={`member-remove-${member.email}`}
                            onClick={() => {
                              setError(null);
                              remove.mutate(member.user_id, { onError: fail });
                            }}
                          >
                            <UserX className="size-4" />
                          </Button>
                        )}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          )}
        </CardContent>
      </Card>

      {(query.data?.invites ?? []).length > 0 && (
        <Card data-testid="pending-invites">
          <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
            <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
              Pending invitations
            </CardTitle>
          </CardHeader>
          <CardContent className="flex flex-col gap-2 p-4 pt-2 text-sm lg:p-6 lg:pt-2">
            {(query.data?.invites ?? []).map((pending) => (
              <div key={pending.invite_id} className="flex flex-wrap items-center gap-2">
                <Mail className="text-muted-foreground size-4" />
                <span className="font-medium">{pending.email}</span>
                <span className="text-muted-foreground text-xs">as {pending.role}</span>
                <span className="text-muted-foreground ml-auto text-xs">
                  expires {pending.expires_at.slice(0, 10)}
                </span>
              </div>
            ))}
          </CardContent>
        </Card>
      )}

      {isAdmin && (
        <Card data-testid="invite-card">
          <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
            <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
              Invite someone
            </CardTitle>
          </CardHeader>
          <CardContent className="flex flex-col gap-3 p-4 pt-2 lg:p-6 lg:pt-2">
            <form
              className="flex flex-wrap items-end gap-3"
              onSubmit={(event) => {
                event.preventDefault();
                setError(null);
                setLastInvite(null);
                invite.mutate(
                  { email, role },
                  {
                    onSuccess: (result) => {
                      setLastInvite(result);
                      setEmail("");
                      toast({
                        title: result.emailed ? "Invitation sent" : "Invitation created",
                        description: result.emailed
                          ? "The link is on its way by email."
                          : "Email is not configured — copy the link below.",
                      });
                    },
                    onError: fail,
                  },
                );
              }}
            >
              <div className="grid gap-1.5">
                <Label htmlFor="invite-email">Email</Label>
                <Input
                  id="invite-email"
                  type="email"
                  required
                  value={email}
                  onChange={(event) => {
                    setEmail(event.target.value);
                  }}
                  placeholder="person@example.com"
                  className="w-64"
                  data-testid="invite-email"
                />
              </div>
              <div className="grid gap-1.5">
                <Label htmlFor="invite-role">Role</Label>
                <select
                  id="invite-role"
                  value={role}
                  onChange={(event) => {
                    setRole(event.target.value as OrgRole);
                  }}
                  className="border-input bg-background h-11 rounded-md border px-3 text-sm lg:h-9"
                  data-testid="invite-role"
                >
                  {ROLES.map((value) => (
                    <option key={value} value={value}>
                      {value}
                    </option>
                  ))}
                </select>
              </div>
              <Button type="submit" disabled={invite.isPending} data-testid="invite-submit">
                <UserPlus /> {invite.isPending ? "Inviting…" : "Send invitation"}
              </Button>
            </form>

            {lastInvite && (
              <div
                className="flex flex-wrap items-center gap-2 rounded-md border p-2 text-xs"
                data-testid="invite-link"
              >
                <code className="min-w-0 flex-1 truncate font-mono">{lastInvite.link}</code>
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => {
                    void navigator.clipboard.writeText(lastInvite.link);
                    toast({ title: "Link copied" });
                  }}
                >
                  <Copy /> Copy
                </Button>
              </div>
            )}
            {error && (
              <p className="text-destructive text-sm" role="alert" data-testid="members-error">
                {error}
              </p>
            )}
            <p className="text-muted-foreground text-xs">
              The invitation expires in seven days and can be accepted once. Re-inviting the same
              address replaces the outstanding link.
            </p>
          </CardContent>
        </Card>
      )}

      {!isAdmin && (
        <p className="text-muted-foreground text-xs">
          Managing members needs the admin role — you can see who is here, not change it.
        </p>
      )}
    </div>
  );
}
