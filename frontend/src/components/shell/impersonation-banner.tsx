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
      className="flex items-center gap-2 bg-amber-500 px-4 py-1.5 text-sm font-medium text-amber-950"
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
        className="h-7 border-amber-900/30 bg-transparent text-amber-950 hover:bg-amber-400"
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
