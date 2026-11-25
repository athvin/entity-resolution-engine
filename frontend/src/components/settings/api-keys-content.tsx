"use client";

import { useState } from "react";
import { Check, Copy, KeyRound, Plus, Trash2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Label } from "@/components/ui/label";
import { Skeleton } from "@/components/ui/skeleton";
import { BffRequestError } from "@/lib/api/client";
import {
  MINTABLE_ROLES,
  useApiKeys,
  useCreateApiKey,
  useRevokeApiKey,
  type ApiKeyCreated,
  type ApiKeyRow,
  type MintableRole,
} from "@/lib/query/api-keys";
import { effectiveRole, useSession } from "@/lib/query/hooks";
import { formatStamp } from "@/lib/webhooks/format";

export interface ApiKeysViewProps {
  org: string;
  /** Only an org admin mints, lists or revokes keys; everyone else is told so. */
  isAdmin: boolean;
  keys: ApiKeyRow[] | undefined;
  loading: boolean;
  /** The key just minted. Its plaintext is rendered here and nowhere else. */
  created: ApiKeyCreated | null;
  copied: boolean;
  role: MintableRole;
  busy: boolean;
  failure: string | null;
  onRoleChange: (role: MintableRole) => void;
  onCreate: () => void;
  onCopy: () => void;
  onRevoke: (keyId: string) => void;
}

/**
 * The API-key screen: mint, see the plaintext once, list, revoke.
 *
 * Before #866 an API key came only from the operator, so "connect your own
 * system" started with a support ticket. The show-once shape is the webhook
 * secret's (`webhooks-content.tsx`): the plaintext exists in one response, so
 * the screen has to show it at that moment, say so, and never again.
 */
export function ApiKeysView(props: ApiKeysViewProps) {
  const { org, isAdmin, keys, loading, created } = props;
  if (!isAdmin) {
    return (
      <p className="text-muted-foreground text-sm" data-testid="api-keys-admin-only">
        Only an admin of this organization can create, list or revoke its API keys.
      </p>
    );
  }
  const rows = keys ?? [];
  return (
    <div className="flex w-full flex-col gap-4 lg:gap-6" data-testid="api-keys-page">
      <Card data-testid="api-keys-new">
        <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
          <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
            Create a key
          </CardTitle>
          <CardDescription>
            A key acts as one role in this organization and nowhere else. It can never outrank you,
            and no key is ever an operator.
          </CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-4 p-4 pt-2 lg:p-6 lg:pt-2">
          <div className="flex flex-wrap items-end gap-3">
            <div className="flex flex-col gap-1">
              <Label htmlFor="api-key-role">Role</Label>
              <select
                id="api-key-role"
                data-testid="api-key-role"
                className="border-input bg-background h-9 rounded-md border px-2 text-sm"
                value={props.role}
                onChange={(e) => {
                  props.onRoleChange(e.target.value as MintableRole);
                }}
              >
                {MINTABLE_ROLES.map((r) => (
                  <option key={r} value={r}>
                    {r}
                  </option>
                ))}
              </select>
            </div>
            <Button data-testid="api-key-create" disabled={props.busy} onClick={props.onCreate}>
              <Plus /> {props.busy ? "Creating…" : "Create key"}
            </Button>
          </div>

          {created && (
            <div
              className="bg-muted/50 flex flex-col gap-2 rounded-md border p-3"
              data-testid="api-key-secret"
            >
              <p className="flex items-center gap-1.5 text-sm font-medium">
                <KeyRound className="size-4" aria-hidden /> New {created.role} key
              </p>
              <div className="flex items-start gap-2">
                <code
                  className="min-w-0 flex-1 font-mono text-xs break-all"
                  data-testid="api-key-plaintext"
                >
                  {created.key}
                </code>
                <Button
                  variant="outline"
                  size="sm"
                  data-testid="api-key-copy"
                  onClick={props.onCopy}
                  aria-label="Copy the key"
                >
                  {props.copied ? <Check /> : <Copy />} {props.copied ? "Copied" : "Copy"}
                </Button>
              </div>
              <p className="text-muted-foreground text-xs">
                Shown once and never again: only a hash is stored, so nobody, including us, can show
                it to you later. Store it now in your secret manager.
              </p>
              <p className="text-muted-foreground text-xs">Check it works:</p>
              <pre className="bg-background overflow-x-auto rounded border p-2 font-mono text-xs">
                {`curl -H "Authorization: Bearer $ER_API_KEY" \\\n  "$ER_API_ORIGIN/v1/orgs/${org}"`}
              </pre>
            </div>
          )}
        </CardContent>
      </Card>

      <Card data-testid="api-keys-list">
        <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
          <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
            Keys
          </CardTitle>
          <CardDescription>
            Revoking is immediate and cannot be undone: the next request with that key is refused.
            Keys marked &ldquo;Used by this app&rdquo; are this web app&apos;s own credentials and
            cannot be revoked here.
          </CardDescription>
        </CardHeader>
        <CardContent className="overflow-x-auto p-0">
          {loading ? (
            <Skeleton className="m-4 h-20" />
          ) : rows.length === 0 ? (
            <p className="text-muted-foreground p-4 text-sm">
              This organization has no API keys yet.
            </p>
          ) : (
            <table className="w-full min-w-[32rem] text-sm">
              <thead>
                <tr className="text-muted-foreground border-b text-left text-xs uppercase">
                  <th className="px-4 py-1.5 font-medium">Key</th>
                  <th className="py-1.5 pr-4 font-medium">Role</th>
                  <th className="py-1.5 pr-4 font-medium">Created</th>
                  <th className="py-1.5 pr-4 font-medium">Status</th>
                  <th className="py-1.5 pr-4 font-medium" />
                </tr>
              </thead>
              <tbody>
                {rows.map((k) => (
                  <tr key={k.id} className="border-b last:border-0" data-testid="api-key-row">
                    <td className="px-4 py-2 font-mono text-xs">{k.id}</td>
                    <td className="py-2 pr-4">{k.role}</td>
                    <td className="text-muted-foreground py-2 pr-4 font-mono text-xs">
                      {formatStamp(k.created_at)}
                    </td>
                    <td className="py-2 pr-4">
                      {k.revoked_at === null ? (
                        <span className="font-medium">active</span>
                      ) : (
                        <span className="text-muted-foreground">
                          revoked {formatStamp(k.revoked_at)}
                        </span>
                      )}
                    </td>
                    <td className="py-2 pr-4 text-right">
                      {k.service && (
                        <span
                          className="text-muted-foreground text-xs"
                          data-testid="api-key-service"
                        >
                          Used by this app
                        </span>
                      )}
                      {k.revoked_at === null && !k.service && (
                        <Button
                          variant="ghost"
                          size="sm"
                          data-testid={`api-key-revoke-${k.id}`}
                          disabled={props.busy}
                          onClick={() => {
                            props.onRevoke(k.id);
                          }}
                        >
                          <Trash2 /> Revoke
                        </Button>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </CardContent>
      </Card>

      {props.failure !== null && (
        <p className="text-sm font-medium" data-testid="api-keys-error">
          {props.failure}
        </p>
      )}
    </div>
  );
}

export function ApiKeysContent({ org }: { org: string }) {
  const session = useSession();
  const isAdmin = effectiveRole(session.data, org) === "admin";
  const keys = useApiKeys(org, isAdmin);
  const create = useCreateApiKey(org);
  const revoke = useRevokeApiKey(org);
  const [role, setRole] = useState<MintableRole>("viewer");
  const [created, setCreated] = useState<ApiKeyCreated | null>(null);
  const [copied, setCopied] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);

  function fail(cause: unknown) {
    setFailure(cause instanceof BffRequestError ? cause.message : "that did not work");
  }

  return (
    <ApiKeysView
      org={org}
      isAdmin={isAdmin}
      keys={keys.data}
      loading={keys.isPending}
      created={created}
      copied={copied}
      role={role}
      busy={create.isPending || revoke.isPending}
      failure={failure}
      onRoleChange={setRole}
      onCreate={() => {
        setFailure(null);
        setCopied(false);
        setCreated(null);
        create.mutateAsync(role).then(setCreated).catch(fail);
      }}
      onCopy={() => {
        if (!created) return;
        navigator.clipboard
          .writeText(created.key)
          .then(() => {
            setCopied(true);
          })
          .catch(() => {
            setFailure("could not copy; select the key and copy it by hand");
          });
      }}
      onRevoke={(keyId) => {
        setFailure(null);
        revoke.mutateAsync(keyId).catch(fail);
      }}
    />
  );
}
