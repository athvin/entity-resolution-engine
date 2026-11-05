"use client";

import { useEffect, useState } from "react";

import { CommandPalette } from "@/components/palette/command-palette";
import type { Membership, SessionUser } from "@/lib/auth/types";
import { installInteractionListener } from "@/lib/telemetry/interaction";
import { BottomTabs } from "./bottom-tabs";
import { ImpersonationBanner, type ImpersonationState } from "./impersonation-banner";
import { NavRail } from "./nav-rail";
import { TopBar } from "./top-bar";

export interface ShellProps {
  org: string;
  user: SessionUser;
  memberships: Membership[];
  impersonation: ImpersonationState | null;
  children: React.ReactNode;
}

export function AppShell({ org, user, memberships, impersonation, children }: ShellProps) {
  const [paletteOpen, setPaletteOpen] = useState(false);
  // One delegated click listener for the whole signed-in surface. Installed
  // here rather than per-component so nothing has to opt in, and returned as a
  // teardown so React strict mode's double-invoke does not leave two.
  useEffect(() => {
    const uninstall = installInteractionListener();
    return uninstall;
  }, []);
  return (
    // `data-app-shell` is the hook `globals.css` keys the fixed-bar scroll
    // padding off — those bars exist only inside this shell.
    <div className="flex min-h-dvh flex-col" data-app-shell>
      {impersonation && <ImpersonationBanner impersonation={impersonation} />}
      <TopBar
        org={org}
        user={user}
        memberships={memberships}
        impersonating={impersonation !== null}
        onOpenPalette={() => {
          setPaletteOpen(true);
        }}
      />
      <div className="flex flex-1">
        <NavRail org={org} />
        <main className="min-w-0 flex-1 px-4 pt-4 pb-24 lg:px-8 lg:pb-8">{children}</main>
      </div>
      <BottomTabs org={org} />
      <CommandPalette
        org={org}
        user={user}
        memberships={memberships}
        impersonating={impersonation !== null}
        open={paletteOpen}
        onOpenChange={setPaletteOpen}
      />
    </div>
  );
}
