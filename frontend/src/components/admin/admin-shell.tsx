"use client";

import Link from "next/link";
import { usePathname, useRouter } from "next/navigation";
import { LogOut, ShieldCheck } from "lucide-react";

import { Button } from "@/components/ui/button";
import { bffFetch } from "@/lib/api/client";
import type { SessionUser } from "@/lib/auth/types";
import { cn } from "@/lib/utils";

const TABS = [
  { href: "/admin/tenants", label: "Tenants" },
  { href: "/admin/queue", label: "Global queue" },
  { href: "/admin/audit", label: "Audit" },
];

/** The operator console's own shell — deliberately distinct from tenant space. */
export function AdminShell({ user, children }: { user: SessionUser; children: React.ReactNode }) {
  const pathname = usePathname();
  const router = useRouter();

  async function signOut() {
    await bffFetch("/api/auth/logout", { method: "POST" });
    router.push("/login");
    router.refresh();
  }

  return (
    <div className="flex min-h-dvh flex-col">
      <header className="bg-background/95 sticky top-0 z-40 border-b backdrop-blur">
        {/* Below sm the console tabs drop to their own scrollable row. */}
        <div className="flex flex-wrap items-center gap-x-4 px-4 sm:h-14 sm:flex-nowrap">
          <span className="flex h-14 items-center gap-2 text-sm font-semibold sm:h-auto">
            <ShieldCheck className="size-4" />
            Operator console
          </span>
          <nav
            className="order-last -mx-4 flex w-[calc(100%+2rem)] gap-1 overflow-x-auto px-4 pb-2 sm:order-none sm:mx-0 sm:w-auto sm:p-0"
            aria-label="Console"
          >
            {TABS.map((tab) => (
              <Link
                key={tab.href}
                href={tab.href}
                className={cn(
                  "shrink-0 rounded-md px-3 py-1.5 text-sm font-medium whitespace-nowrap",
                  pathname.startsWith(tab.href)
                    ? "bg-accent text-accent-foreground"
                    : "text-muted-foreground hover:bg-accent/50",
                )}
              >
                {tab.label}
              </Link>
            ))}
          </nav>
          <span className="flex-1" />
          <span className="text-muted-foreground hidden text-xs sm:inline">{user.email}</span>
          <Button
            variant="ghost"
            size="icon"
            aria-label="Sign out"
            data-testid="sign-out"
            onClick={() => void signOut()}
          >
            <LogOut className="size-4" />
          </Button>
        </div>
      </header>
      <main className="mx-auto w-full max-w-6xl flex-1 px-4 py-6 lg:px-8">{children}</main>
    </div>
  );
}
