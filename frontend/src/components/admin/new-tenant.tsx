"use client";

import { useState } from "react";
import Link from "next/link";
import { ArrowLeft, Check, Copy, Loader2 } from "lucide-react";

import { StateChip } from "@/components/runs/runs-content";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { BffRequestError } from "@/lib/api/client";
import { useAdminOrg, useProvisionOrg, type ProvisionResult } from "@/lib/query/admin";

function ProvisionProgress({ result }: { result: ProvisionResult }) {
  const detail = useAdminOrg(result.name, { poll: true });
  const state = detail.data?.state ?? result.state;
  const [copied, setCopied] = useState(false);

  return (
    <div className="flex flex-col gap-4" data-testid="provision-progress">
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-3 text-lg">
            {result.name}
            <StateChip state={state} />
            {state === "provisioning" && <Loader2 className="size-4 animate-spin" />}
          </CardTitle>
          <CardDescription>
            {state === "provisioning"
              ? "Creating the dedicated database and initializing the lake namespace…"
              : state === "active"
                ? "Tenant is live. Service keys are vaulted; the app can act for it now."
                : `State: ${state}`}
          </CardDescription>
        </CardHeader>
        {result.admin_key && (
          <CardContent className="flex flex-col gap-2">
            <p className="text-sm font-medium">
              One-time tenant admin key — hand this to the customer. It is shown exactly once and
              never stored.
            </p>
            <div className="flex items-center gap-2">
              <code
                className="bg-muted min-w-0 flex-1 truncate rounded-md px-3 py-2 font-mono text-xs"
                data-testid="one-time-admin-key"
              >
                {result.admin_key}
              </code>
              <Button
                variant="outline"
                size="icon"
                aria-label="Copy admin key"
                onClick={() => {
                  void navigator.clipboard.writeText(result.admin_key ?? "").then(() => {
                    setCopied(true);
                  });
                }}
              >
                {copied ? <Check className="size-4" /> : <Copy className="size-4" />}
              </Button>
            </div>
          </CardContent>
        )}
      </Card>
      {state === "active" && (
        <div className="flex gap-2">
          <Button asChild data-testid="open-new-tenant">
            <Link href={`/admin/tenants/${result.name}`}>Open tenant</Link>
          </Button>
          <Button variant="outline" asChild>
            <Link href="/admin/tenants">Back to tenants</Link>
          </Button>
        </div>
      )}
    </div>
  );
}

export function NewTenant() {
  const provision = useProvisionOrg();
  const [error, setError] = useState<string | null>(null);

  if (provision.data) return <ProvisionProgress result={provision.data} />;

  return (
    <div
      className="mx-auto flex w-full max-w-lg min-w-0 flex-col gap-4"
      data-testid="new-tenant-form"
    >
      <div>
        <Link
          href="/admin/tenants"
          className="text-muted-foreground mb-2 inline-flex items-center gap-1 text-sm hover:underline"
        >
          <ArrowLeft className="size-3.5" /> Tenants
        </Link>
        <h1 className="text-xl font-semibold lg:text-2xl">New tenant</h1>
        <p className="text-muted-foreground mt-1 text-sm">
          Provisions a dedicated database and lake namespace, seeds config v1, mints the app&apos;s
          service keys, and issues the customer&apos;s one-time admin key.
        </p>
      </div>
      <form
        className="grid gap-4"
        onSubmit={(event) => {
          event.preventDefault();
          setError(null);
          const form = new FormData(event.currentTarget);
          const name = form.get("name");
          const displayName = form.get("display_name");
          provision.mutate(
            {
              name: typeof name === "string" ? name : "",
              display_name:
                typeof displayName === "string" && displayName !== "" ? displayName : undefined,
            },
            {
              onError: (cause) => {
                setError(cause instanceof BffRequestError ? cause.message : "provisioning failed");
              },
            },
          );
        }}
      >
        <div className="grid gap-2">
          <Label htmlFor="name">Workspace slug</Label>
          <Input
            id="name"
            name="name"
            placeholder="acme"
            pattern="[a-z0-9][a-z0-9_-]*"
            title="lowercase letters, digits, - and _"
            required
            data-testid="tenant-name"
          />
        </div>
        <div className="grid gap-2">
          <Label htmlFor="display_name">Display name (optional)</Label>
          <Input id="display_name" name="display_name" placeholder="Acme Corporation" />
        </div>
        {error && (
          <p className="text-destructive text-sm" role="alert" data-testid="provision-error">
            {error}
          </p>
        )}
        <Button type="submit" disabled={provision.isPending} data-testid="provision-submit">
          {provision.isPending ? "Provisioning…" : "Provision tenant"}
        </Button>
      </form>
    </div>
  );
}
