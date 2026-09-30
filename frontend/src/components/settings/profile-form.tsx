"use client";

import { useState } from "react";
import { KeyRound } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { useToast } from "@/components/ui/toaster";
import { BffRequestError } from "@/lib/api/client";
import { useSession } from "@/lib/query/hooks";
import { useChangePassword } from "@/lib/query/members";

/** Your own account: who you are here, and your password. */
export function ProfileForm() {
  const session = useSession();
  const change = useChangePassword();
  const toast = useToast();
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [error, setError] = useState<string | null>(null);

  const user = session.data?.user;

  return (
    <div className="flex w-full flex-col gap-4 lg:gap-6" data-testid="profile-page">
      <Card data-testid="profile-identity">
        <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
          <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
            You
          </CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-1 p-4 pt-2 text-sm lg:p-6 lg:pt-2">
          <div className="font-medium">{user?.displayName ?? "—"}</div>
          <div className="text-muted-foreground text-xs">{user?.email ?? ""}</div>
          <div className="text-muted-foreground mt-1 text-xs">
            Workspaces:{" "}
            {(session.data?.memberships ?? [])
              .map((membership) => `${membership.org} (${membership.role})`)
              .join(", ") || "none"}
          </div>
        </CardContent>
      </Card>

      <Card data-testid="password-card">
        <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
          <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
            Change password
          </CardTitle>
        </CardHeader>
        <CardContent className="p-4 pt-2 lg:p-6 lg:pt-2">
          <form
            className="flex flex-col gap-3"
            onSubmit={(event) => {
              event.preventDefault();
              setError(null);
              change.mutate(
                { current_password: current, new_password: next },
                {
                  onSuccess: () => {
                    setCurrent("");
                    setNext("");
                    toast({
                      title: "Password changed",
                      description: "Your other sessions have been signed out.",
                    });
                  },
                  onError: (cause) => {
                    setError(
                      cause instanceof BffRequestError ? cause.message : "that did not work",
                    );
                  },
                },
              );
            }}
          >
            <div className="flex flex-wrap items-end gap-3">
              <div className="grid gap-1.5">
                <Label htmlFor="current-password">Current password</Label>
                <Input
                  id="current-password"
                  type="password"
                  required
                  autoComplete="current-password"
                  value={current}
                  onChange={(event) => {
                    setCurrent(event.target.value);
                  }}
                  className="w-56"
                  data-testid="current-password"
                />
              </div>
              <div className="grid gap-1.5">
                <Label htmlFor="new-password">New password</Label>
                <Input
                  id="new-password"
                  type="password"
                  required
                  minLength={12}
                  autoComplete="new-password"
                  value={next}
                  onChange={(event) => {
                    setNext(event.target.value);
                  }}
                  className="w-56"
                  data-testid="new-password"
                />
              </div>
              <Button type="submit" disabled={change.isPending} data-testid="password-submit">
                <KeyRound /> {change.isPending ? "Changing…" : "Change password"}
              </Button>
            </div>
            {error && (
              <p className="text-destructive text-sm" role="alert" data-testid="password-error">
                {error}
              </p>
            )}
            <p className="text-muted-foreground text-xs">
              At least 12 characters. Changing it signs out every other session; this one stays.
            </p>
          </form>
        </CardContent>
      </Card>
    </div>
  );
}
