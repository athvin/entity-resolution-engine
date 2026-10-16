"use client";

import { useState } from "react";
import Link from "next/link";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { bffFetch, BffRequestError } from "@/lib/api/client";

/**
 * Request a password-reset link. The BFF (/api/auth/password-reset) is
 * deliberately enumeration-safe — it answers 202 with the same copy whether or
 * not the address has an account — so this page shows one confirmation
 * regardless and never reveals who has an account. The link it emails lands on
 * /reset/[token].
 */
export default function RequestResetPage() {
  const [sent, setSent] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function onSubmit(event: React.SyntheticEvent<HTMLFormElement, SubmitEvent>) {
    event.preventDefault();
    const form = new FormData(event.currentTarget);
    setPending(true);
    setError(null);
    try {
      await bffFetch("/api/auth/password-reset", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ email: form.get("email") }),
      });
      setSent(true);
    } catch (cause) {
      setError(
        cause instanceof BffRequestError ? cause.message : "something went wrong — try again",
      );
    } finally {
      setPending(false);
    }
  }

  return (
    <main className="bg-muted/40 flex min-h-dvh items-center justify-center p-4">
      <Card className="w-full max-w-sm" data-testid="reset-request-card">
        <CardHeader className="space-y-2 text-center">
          <CardTitle className="text-2xl">Reset your password</CardTitle>
          <CardDescription>
            {sent ? "Check your email." : "Enter your email and we'll send a reset link."}
          </CardDescription>
        </CardHeader>
        <CardContent>
          {sent ? (
            <div className="grid gap-4">
              <p className="text-muted-foreground text-sm" data-testid="reset-request-sent">
                If that address has an account, a reset link is on its way.
              </p>
              <p className="text-center text-sm">
                <Link href="/login" className="underline underline-offset-4">
                  Back to sign in
                </Link>
              </p>
            </div>
          ) : (
            <form onSubmit={(event) => void onSubmit(event)} className="grid gap-4">
              <div className="grid gap-2">
                <Label htmlFor="email">Email</Label>
                <Input
                  id="email"
                  name="email"
                  type="email"
                  autoComplete="email"
                  placeholder="you@example.com"
                  required
                  data-testid="reset-email"
                />
              </div>
              {error && (
                <p className="text-destructive text-sm" role="alert" data-testid="reset-error">
                  {error}
                </p>
              )}
              <Button
                type="submit"
                className="w-full"
                disabled={pending}
                data-testid="reset-request-submit"
              >
                {pending ? "Sending…" : "Send reset link"}
              </Button>
              <p className="text-muted-foreground text-center text-sm">
                <Link href="/login" className="underline underline-offset-4">
                  Back to sign in
                </Link>
              </p>
            </form>
          )}
        </CardContent>
      </Card>
    </main>
  );
}
