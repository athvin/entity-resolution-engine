"use client";

import { useEffect } from "react";
import { useRouter } from "next/navigation";
import { useTheme } from "next-themes";
import { Command } from "cmdk";
import { Building2, LogOut, Moon, Sun } from "lucide-react";

import { bffFetch } from "@/lib/api/client";
import type { Membership } from "@/lib/auth/types";
import { NAV_ITEMS } from "@/lib/nav";

interface CommandPaletteProps {
  org: string;
  memberships: Membership[];
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/**
 * The ⌘K palette — the spine of the app (design doc §3.2). M1 resolves pages,
 * workspaces, theme and session; records/runs/tenants/actions join as their
 * screens land.
 */
export function CommandPalette({ org, memberships, open, onOpenChange }: CommandPaletteProps) {
  const router = useRouter();
  const { setTheme } = useTheme();

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "k" && (event.metaKey || event.ctrlKey)) {
        event.preventDefault();
        onOpenChange(!open);
      }
    }
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open, onOpenChange]);

  function run(action: () => void) {
    onOpenChange(false);
    action();
  }

  return (
    <Command.Dialog
      open={open}
      onOpenChange={onOpenChange}
      label="Search and commands"
      className="bg-popover text-popover-foreground fixed top-0 left-1/2 z-50 h-dvh w-full -translate-x-1/2 overflow-hidden border shadow-lg lg:top-[20%] lg:h-auto lg:max-h-[60dvh] lg:w-[36rem] lg:rounded-xl"
      overlayClassName="fixed inset-0 z-50 bg-black/50"
    >
      <Command.Input
        placeholder="Type a page, workspace or command…"
        className="placeholder:text-muted-foreground h-14 w-full border-b bg-transparent px-4 text-base outline-none lg:h-12 lg:text-sm"
      />
      <Command.List className="max-h-[calc(100dvh-3.5rem)] overflow-y-auto p-2 lg:max-h-96">
        <Command.Empty className="text-muted-foreground px-4 py-8 text-center text-sm">
          Nothing matches.
        </Command.Empty>

        <Command.Group
          heading="Pages"
          className="[&_[cmdk-group-heading]]:text-muted-foreground [&_[cmdk-group-heading]]:px-2 [&_[cmdk-group-heading]]:py-1.5 [&_[cmdk-group-heading]]:text-xs [&_[cmdk-group-heading]]:font-medium"
        >
          {NAV_ITEMS.map((item) => (
            <Command.Item
              key={item.id}
              disabled={!item.enabled}
              onSelect={() => {
                run(() => {
                  router.push(item.path(org));
                });
              }}
              className="data-[selected=true]:bg-accent data-[selected=true]:text-accent-foreground flex min-h-11 cursor-default items-center gap-3 rounded-md px-3 text-sm data-[disabled=true]:opacity-50 lg:min-h-9"
            >
              <item.icon className="size-4" />
              {item.label}
              {!item.enabled && (
                <span className="text-muted-foreground ml-auto text-[10px] uppercase">
                  {item.milestone}
                </span>
              )}
            </Command.Item>
          ))}
        </Command.Group>

        {memberships.length > 1 && (
          <Command.Group
            heading="Workspaces"
            className="[&_[cmdk-group-heading]]:text-muted-foreground [&_[cmdk-group-heading]]:px-2 [&_[cmdk-group-heading]]:py-1.5 [&_[cmdk-group-heading]]:text-xs [&_[cmdk-group-heading]]:font-medium"
          >
            {memberships.map((membership) => (
              <Command.Item
                key={membership.org}
                onSelect={() => {
                  run(() => {
                    router.push(`/${membership.org}/dashboard`);
                  });
                }}
                className="data-[selected=true]:bg-accent data-[selected=true]:text-accent-foreground flex min-h-11 cursor-default items-center gap-3 rounded-md px-3 text-sm lg:min-h-9"
              >
                <Building2 className="size-4" />
                Open {membership.org}
              </Command.Item>
            ))}
          </Command.Group>
        )}

        <Command.Group
          heading="Preferences"
          className="[&_[cmdk-group-heading]]:text-muted-foreground [&_[cmdk-group-heading]]:px-2 [&_[cmdk-group-heading]]:py-1.5 [&_[cmdk-group-heading]]:text-xs [&_[cmdk-group-heading]]:font-medium"
        >
          <Command.Item
            onSelect={() => {
              run(() => {
                setTheme("light");
              });
            }}
            className="data-[selected=true]:bg-accent data-[selected=true]:text-accent-foreground flex min-h-11 cursor-default items-center gap-3 rounded-md px-3 text-sm lg:min-h-9"
          >
            <Sun className="size-4" />
            Light theme
          </Command.Item>
          <Command.Item
            onSelect={() => {
              run(() => {
                setTheme("dark");
              });
            }}
            className="data-[selected=true]:bg-accent data-[selected=true]:text-accent-foreground flex min-h-11 cursor-default items-center gap-3 rounded-md px-3 text-sm lg:min-h-9"
          >
            <Moon className="size-4" />
            Dark theme
          </Command.Item>
          <Command.Item
            onSelect={() => {
              run(() => {
                void bffFetch("/api/auth/logout", { method: "POST" }).then(() => {
                  router.push("/login");
                  router.refresh();
                });
              });
            }}
            className="data-[selected=true]:bg-accent data-[selected=true]:text-accent-foreground flex min-h-11 cursor-default items-center gap-3 rounded-md px-3 text-sm lg:min-h-9"
          >
            <LogOut className="size-4" />
            Sign out
          </Command.Item>
        </Command.Group>
      </Command.List>
    </Command.Dialog>
  );
}
