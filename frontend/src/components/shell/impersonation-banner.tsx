"use client";

import { useEffect, useState } from "react";
import { Eye } from "lucide-react";

import { Button } from "@/components/ui/button";
import type { OrgRole } from "@/lib/auth/types";
import { useImpersonation } from "@/lib/query/admin";

export interface ImpersonationState {
  org: string;
  role: OrgRole;
  expiresAt: string;
}

function remaining(expiresAt: string): string {
  const ms = Date.parse(expiresAt) - Date.now();
  if (!Number.isFinite(ms) || ms <= 0) return "expired";
  const minutes = Math.ceil(ms / 60_000);
  return `${String(minutes)}m left`;
}

/** The unmissable "you are the tenant right now" strip (design §2.3). */
export function ImpersonationBanner({ impersonation }: { impersonation: ImpersonationState }) {
  const { exit } = useImpersonation();
  const [countdown, setCountdown] = useState(() => remaining(impersonation.expiresAt));

  useEffect(() => {
    const timer = setInterval(() => {
      setCountdown(remaining(impersonation.expiresAt));
    }, 30_000);
    return () => {
      clearInterval(timer);
    };
  }, [impersonation.expiresAt]);

  return (
    <div
      className="bg-foreground text-background flex items-center gap-2 px-4 py-1.5 text-sm font-medium"
      data-testid="impersonation-banner"
    >
      <Eye className="size-4 shrink-0" />
      <span className="min-w-0 truncate">
        Viewing <strong>{impersonation.org}</strong> as <strong>{impersonation.role}</strong> —
        actions are audited under both identities · {countdown}
      </span>
      <span className="flex-1" />
      <Button
        size="sm"
        variant="outline"
        className="border-background/40 text-background hover:bg-background/15 h-7 bg-transparent"
        onClick={() => {
          exit.mutate();
        }}
        data-testid="impersonation-exit"
      >
        Exit
      </Button>
    </div>
  );
}
