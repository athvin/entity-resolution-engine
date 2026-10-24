"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";

import { cn } from "@/lib/utils";

export function SettingsNav({ org }: { org: string }) {
  const pathname = usePathname();
  const tabs = [
    { href: `/${org}/settings/config`, label: "Config studio" },
    { href: `/${org}/settings/models`, label: "Models" },
    { href: `/${org}/settings/webhooks`, label: "Webhooks" },
    { href: `/${org}/settings/ai`, label: "AI" },
    { href: `/${org}/settings/members`, label: "Members" },
    { href: `/${org}/settings/profile`, label: "Profile" },
    { href: `/${org}/settings/audit`, label: "Audit" },
  ];
  return (
    // `flex-wrap`, not a non-wrapping row: these tabs have not fitted a 360px
    // viewport since there were six of them, and the overflow pushed the config
    // studio's Save button under the lexicon list so the click landed on a
    // nickname instead. A nav that runs off the side of the screen is also
    // simply unreachable there.
    //
    // Wrapping has a cost worth knowing before adding an eighth: a tab added
    // here REARRANGES the rows on a narrow viewport rather than just appending.
    // The AI tab pushed "Members" onto the second row on Pixel 7 — same two
    // rows, same page height, but enough relayout to exceed
    // `maxDiffPixelRatio` and fail `config-studio-mobile-chrome-linux.png`.
    // It slid UNDER that tolerance on desktop and mobile-safari, which is the
    // silent staleness `update-baselines.sh` runs `--update-snapshots=all` to
    // prevent: all three baselines were rewritten, not the one CI named.
    <nav className="flex flex-wrap gap-1" aria-label="Settings">
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
