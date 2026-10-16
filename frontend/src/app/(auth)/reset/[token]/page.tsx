"use client";

import { use, useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { bffFetch, BffRequestError } from "@/lib/api/client";

/**
 * Set a new password from a reset link. The token in the URL is the credential,
 * so this page is unauthenticated (see /reset for the request half and
 * PUBLIC_PATHS in src/proxy.ts). On success the BFF has already revoked every
 * session for the account, so the only way forward is a fresh sign-in.
 */
export default function ConfirmResetPage({ params }: { params: Promise<{ token: string }> }) {
  const { token } = use(params);
  const router = useRouter();
  const [done, setDone] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function onSubmit(event: React.SyntheticEvent<HTMLFormElement, SubmitEvent>) {
    event.preventDefault();
    const form = new FormData(event.currentTarget);
    const password = form.get("password");
    const confirm = form.get("confirm");
    if (password !== confirm) {
      setError("those passwords don't match");
      return;
    }
    setPending(true);
    setError(null);
    try {
      await bffFetch("/api/auth/password-reset", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ token, new_password: password }),
      });
      setDone(true);
    } catch (cause) {
      setError(
        cause instanceof BffRequestError ? cause.message : "something went wrong — try again",
      );
      setPending(false);
    }
  }

  return (
    <main className="bg-muted/40 flex min-h-dvh items-center justify-center p-4">
      <Card className="w-full max-w-sm" data-testid="reset-confirm-card">
        <CardHeader className="space-y-2 text-center">
          <CardTitle className="text-2xl">Choose a new password</CardTitle>
          <CardDescription>
            {done ? "Your password has been reset." : "Enter a new password for your account."}
          </CardDescription>
        </CardHeader>
        <CardContent>
          {done ? (
            <div className="grid gap-4" data-testid="reset-done">
              <p className="text-muted-foreground text-sm">
                All other sessions were signed out. Sign in with your new password.
              </p>
              <Button
                className="w-full"
                onClick={() => {
                  router.push("/login");
                }}
              >
                Go to sign in
              </Button>
            </div>
          ) : (
            <form onSubmit={(event) => void onSubmit(event)} className="grid gap-4">
              <div className="grid gap-2">
                <Label htmlFor="password">New password</Label>
                <Input
                  id="password"
                  name="password"
                  type="password"
                  required
                  minLength={12}
                  autoComplete="new-password"
                  data-testid="reset-password"
                />
                <p className="text-muted-foreground text-xs">At least 12 characters.</p>
              </div>
              <div className="grid gap-2">
                <Label htmlFor="confirm">Confirm new password</Label>
                <Input
                  id="confirm"
                  name="confirm"
                  type="password"
                  required
                  minLength={12}
                  autoComplete="new-password"
                  data-testid="reset-confirm"
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
                data-testid="reset-confirm-submit"
              >
                {pending ? "Saving…" : "Set new password"}
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
