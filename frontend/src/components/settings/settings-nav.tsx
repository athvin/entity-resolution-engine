"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";

import { cn } from "@/lib/utils";

export function SettingsNav({ org }: { org: string }) {
  const pathname = usePathname();
  const tabs = [
    { href: `/${org}/settings/config`, label: "Config studio" },
    { href: `/${org}/settings/members`, label: "Members" },
    { href: `/${org}/settings/profile`, label: "Profile" },
    { href: `/${org}/settings/audit`, label: "Audit" },
  ];
  return (
    <nav className="flex gap-1" aria-label="Settings">
      {tabs.map((tab) => (
        <Link
          key={tab.href}
          href={tab.href}
          className={cn(
            "rounded-md px-3 py-1.5 text-sm font-medium",
            pathname.startsWith(tab.href)
              ? "bg-accent text-accent-foreground"
              : "text-muted-foreground hover:bg-accent/50",
          )}
          data-testid={`settings-tab-${tab.label.toLowerCase().replaceAll(" ", "-")}`}
        >
          {tab.label}
        </Link>
      ))}
    </nav>
  );
}
