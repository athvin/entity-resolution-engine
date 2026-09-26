"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";

import { NAV_ITEMS } from "@/lib/nav";
import { cn } from "@/lib/utils";

/** Desktop navigation: a slim left rail (hidden below lg). */
export function NavRail({ org }: { org: string }) {
  const pathname = usePathname();
  return (
    <nav
      aria-label="Primary"
      className="sticky top-14 hidden h-[calc(100dvh-3.5rem)] w-52 shrink-0 flex-col gap-1 border-r p-3 lg:flex"
      data-testid="nav-rail"
    >
      {NAV_ITEMS.map((item) => {
        const href = item.path(org);
        const active = pathname.startsWith(href);
        if (!item.enabled) {
          return (
            <span
              key={item.id}
              aria-disabled="true"
              title={`Coming in ${item.milestone ?? "a later milestone"}`}
              className="text-muted-foreground/60 flex cursor-not-allowed items-center gap-3 rounded-md px-3 py-2 text-sm"
            >
              <item.icon className="size-4" />
              {item.label}
              <span className="text-muted-foreground/50 ml-auto text-[10px] font-medium tracking-wide uppercase">
                soon
              </span>
            </span>
          );
        }
        return (
          <Link
            key={item.id}
            href={href}
            aria-current={active ? "page" : undefined}
            className={cn(
              "hover:bg-accent hover:text-accent-foreground flex items-center gap-3 rounded-md px-3 py-2 text-sm font-medium transition-colors",
              active ? "bg-accent text-accent-foreground" : "text-muted-foreground",
            )}
          >
            <item.icon className="size-4" />
            {item.label}
          </Link>
        );
      })}
    </nav>
  );
}
