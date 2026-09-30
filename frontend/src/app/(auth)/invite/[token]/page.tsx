"use client";

import { use, useEffect, useState } from "react";
import { useRouter } from "next/navigation";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { bffFetch, BffRequestError } from "@/lib/api/client";

interface InvitePreview {
  org: string;
  email: string;
  role: string;
  needs_account: boolean;
}

/**
 * Accepting an invitation. The token in the URL is the credential, so this page
 * is unauthenticated: it previews what is being joined, collects a password
 * only when the person has no account yet, and signs them in on success.
 */
export default function AcceptInvitePage({ params }: { params: Promise<{ token: string }> }) {
  const { token } = use(params);
  const router = useRouter();
  const [preview, setPreview] = useState<InvitePreview | null>(null);
  const [invalid, setInvalid] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    bffFetch<InvitePreview>(`/api/invites/${token}`)
      .then(setPreview)
      .catch(() => {
        setInvalid(true);
      });
  }, [token]);

  async function accept(event: React.SyntheticEvent<HTMLFormElement, SubmitEvent>) {
    event.preventDefault();
    const form = new FormData(event.currentTarget);
    setPending(true);
    setError(null);
    try {
      const result = await bffFetch<{ org: string }>(`/api/invites/${token}`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          password: form.get("password") ?? undefined,
          display_name: form.get("display_name") ?? undefined,
        }),
      });
      router.push(`/${result.org}/dashboard`);
      router.refresh();
    } catch (cause) {
      setError(
        cause instanceof BffRequestError ? cause.message : "something went wrong — try again",
      );
      setPending(false);
    }
  }

  return (
    <main className="bg-muted/40 flex min-h-dvh items-center justify-center p-4">
      <Card className="w-full max-w-sm" data-testid="invite-card">
        <CardHeader className="space-y-2 text-center">
          <CardTitle className="text-2xl">You&apos;re invited</CardTitle>
          <CardDescription>
            {invalid
              ? "This invitation is no longer valid."
              : preview
                ? `Join ${preview.org} as ${preview.role}, as ${preview.email}.`
                : "Checking the invitation…"}
          </CardDescription>
        </CardHeader>
        <CardContent>
          {invalid ? (
            <Button
              className="w-full"
              onClick={() => {
                router.push("/login");
              }}
            >
              Go to sign in
            </Button>
          ) : preview ? (
            <form className="flex flex-col gap-4" onSubmit={(event) => void accept(event)}>
              {preview.needs_account && (
                <>
                  <div className="grid gap-1.5">
                    <Label htmlFor="display_name">Your name</Label>
                    <Input
                      id="display_name"
                      name="display_name"
                      autoComplete="name"
                      data-testid="invite-name"
                    />
                  </div>
                  <div className="grid gap-1.5">
                    <Label htmlFor="password">Choose a password</Label>
                    <Input
                      id="password"
                      name="password"
                      type="password"
                      required
                      minLength={12}
                      autoComplete="new-password"
                      data-testid="invite-password"
                    />
                    <p className="text-muted-foreground text-xs">At least 12 characters.</p>
                  </div>
                </>
              )}
              {!preview.needs_account && (
                <p className="text-muted-foreground text-sm">
                  You already have an account for this address. Sign in as {preview.email} first,
                  then open this link again — a link alone can&apos;t speak for an existing account.
                </p>
              )}
              <Button
                type="submit"
                className="w-full"
                disabled={pending}
                data-testid="invite-accept"
              >
                {pending ? "Joining…" : "Accept invitation"}
              </Button>
              {error && (
                <p className="text-destructive text-sm" role="alert" data-testid="invite-error">
                  {error}
                </p>
              )}
            </form>
          ) : null}
        </CardContent>
      </Card>
    </main>
  );
}
