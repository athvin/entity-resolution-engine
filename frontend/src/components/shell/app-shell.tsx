"use client";

import { useState } from "react";

import { CommandPalette } from "@/components/palette/command-palette";
import type { Membership, SessionUser } from "@/lib/auth/types";
import { BottomTabs } from "./bottom-tabs";
import { NavRail } from "./nav-rail";
import { TopBar } from "./top-bar";

export interface ShellProps {
  org: string;
  user: SessionUser;
  memberships: Membership[];
  children: React.ReactNode;
}

export function AppShell({ org, user, memberships, children }: ShellProps) {
  const [paletteOpen, setPaletteOpen] = useState(false);
  return (
    <div className="flex min-h-dvh flex-col">
      <TopBar
        org={org}
        user={user}
        memberships={memberships}
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
        memberships={memberships}
        open={paletteOpen}
        onOpenChange={setPaletteOpen}
      />
    </div>
  );
}
