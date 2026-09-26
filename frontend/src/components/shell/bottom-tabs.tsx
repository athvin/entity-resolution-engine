"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { MoreHorizontal } from "lucide-react";

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { MOBILE_TAB_IDS, NAV_ITEMS } from "@/lib/nav";
import { cn } from "@/lib/utils";

/** Mobile navigation: a fixed bottom tab bar (hidden at lg and up). */
export function BottomTabs({ org }: { org: string }) {
  const pathname = usePathname();
  const tabIds: readonly string[] = MOBILE_TAB_IDS;
  const tabs = NAV_ITEMS.filter((item) => tabIds.includes(item.id)).sort(
    (a, b) => tabIds.indexOf(a.id) - tabIds.indexOf(b.id),
  );
  const overflow = NAV_ITEMS.filter((item) => !tabIds.includes(item.id));

  return (
    <nav
      aria-label="Primary"
      data-testid="bottom-tabs"
      className="bg-background/95 fixed inset-x-0 bottom-0 z-40 flex border-t pb-[env(safe-area-inset-bottom)] backdrop-blur lg:hidden"
    >
      {tabs.map((item) => {
        const href = item.path(org);
        const active = pathname.startsWith(href);
        const className = cn(
          "flex min-h-14 flex-1 flex-col items-center justify-center gap-1 text-[11px] font-medium",
          active ? "text-foreground" : "text-muted-foreground",
          !item.enabled && "opacity-50",
        );
        return item.enabled ? (
          <Link
            key={item.id}
            href={href}
            aria-current={active ? "page" : undefined}
            className={className}
          >
            <item.icon className="size-5" />
            {item.label}
          </Link>
        ) : (
          <span key={item.id} aria-disabled="true" className={className}>
            <item.icon className="size-5" />
            {item.label}
          </span>
        );
      })}
      <DropdownMenu>
        <DropdownMenuTrigger
          className="text-muted-foreground flex min-h-14 flex-1 flex-col items-center justify-center gap-1 text-[11px] font-medium"
          aria-label="More"
        >
          <MoreHorizontal className="size-5" />
          More
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" side="top">
          {overflow.map((item) =>
            item.enabled ? (
              <DropdownMenuItem key={item.id} asChild>
                <Link href={item.path(org)}>
                  <item.icon />
                  {item.label}
                </Link>
              </DropdownMenuItem>
            ) : (
              <DropdownMenuItem key={item.id} disabled>
                <item.icon />
                {item.label}
                <span className="text-muted-foreground ml-auto text-[10px] uppercase">soon</span>
              </DropdownMenuItem>
            ),
          )}
        </DropdownMenuContent>
      </DropdownMenu>
    </nav>
  );
}
