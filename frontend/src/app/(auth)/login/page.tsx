"use client";

import { Suspense, useState } from "react";
import Link from "next/link";
import { useRouter, useSearchParams } from "next/navigation";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { bffFetch, BffRequestError } from "@/lib/api/client";
import { safeNextPath } from "@/lib/auth/redirect";

const NO_WORKSPACE_MESSAGE = "this account has no workspace yet — ask an admin for an invite";

function errorFromParam(param: string | null): string | null {
  return param === "no-workspace" ? NO_WORKSPACE_MESSAGE : null;
}

function LoginForm() {
  const router = useRouter();
  const searchParams = useSearchParams();
  const [error, setError] = useState<string | null>(() =>
    errorFromParam(searchParams.get("error")),
  );
  const [pending, setPending] = useState(false);

  async function onSubmit(event: React.SyntheticEvent<HTMLFormElement, SubmitEvent>) {
    event.preventDefault();
    const form = new FormData(event.currentTarget);
    setPending(true);
    setError(null);
    try {
      const result = await bffFetch<{ ok: boolean; redirect: string }>("/api/auth/login", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ email: form.get("email"), password: form.get("password") }),
      });
      // `next` is attacker-controllable, and `router.push` will happily leave the
      // origin. Anything that is not a same-origin path falls back to the
      // redirect the BFF chose, which is ours.
      const target =
        safeNextPath(searchParams.get("next"), window.location.origin) ?? result.redirect;
      // A redirect back to /login means the session is live but there is
      // nowhere to land; pushing the same route would leave the spinner on.
      if (target.startsWith("/login")) {
        setError(
          errorFromParam(new URL(target, window.location.origin).searchParams.get("error")) ??
            NO_WORKSPACE_MESSAGE,
        );
        setPending(false);
        return;
      }
      router.push(target);
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
      <Card className="w-full max-w-sm" data-testid="login-card">
        <CardHeader className="space-y-2 text-center">
          <CardTitle className="text-2xl">DupeZero</CardTitle>
          <CardDescription>Sign in to your workspace</CardDescription>
        </CardHeader>
        <CardContent>
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
              />
            </div>
            <div className="grid gap-2">
              <Label htmlFor="password">Password</Label>
              <Input
                id="password"
                name="password"
                type="password"
                autoComplete="current-password"
                required
              />
            </div>
            {error && (
              <p className="text-destructive text-sm" role="alert" data-testid="login-error">
                {error}
              </p>
            )}
            <Button type="submit" className="w-full" disabled={pending} data-testid="login-submit">
              {pending ? "Signing in…" : "Sign in"}
            </Button>
            <p className="text-muted-foreground text-center text-sm">
              <Link
                href="/reset"
                className="underline underline-offset-4"
                data-testid="login-forgot"
              >
                Forgot your password?
              </Link>
            </p>
          </form>
        </CardContent>
      </Card>
    </main>
  );
}

export default function LoginPage() {
  return (
    <Suspense>
      <LoginForm />
    </Suspense>
  );
}
