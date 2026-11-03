"use client";

import Link from "next/link";
import { useRouter } from "next/navigation";
import { useTheme } from "next-themes";
import {
  Building2,
  ChevronsUpDown,
  Layers,
  LogOut,
  Moon,
  Plus,
  Search,
  Settings2,
  ShieldCheck,
  Sun,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { bffFetch } from "@/lib/api/client";
import type { Membership, SessionUser } from "@/lib/auth/types";
import { useDatasets } from "@/lib/query/datasets";
import { useDatasetSlug } from "@/lib/use-dataset-slug";

interface TopBarProps {
  org: string;
  user: SessionUser;
  memberships: Membership[];
  impersonating: boolean;
  onOpenPalette: () => void;
}

export function TopBar({ org, user, memberships, impersonating, onOpenPalette }: TopBarProps) {
  const router = useRouter();
  const { resolvedTheme, setTheme } = useTheme();
  const ds = useDatasetSlug();
  const datasets = useDatasets(org);

  async function signOut() {
    await bffFetch("/api/auth/logout", { method: "POST" });
    router.push("/login");
    router.refresh();
  }

  return (
    <header className="bg-background/95 sticky top-0 z-40 flex h-14 items-center gap-2 border-b px-4 backdrop-blur">
      {/* Org switcher (or a plain label when the user belongs to just this org). */}
      {memberships.length > 1 ? (
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              variant="ghost"
              size="sm"
              className="gap-2 font-semibold"
              data-testid="org-switcher"
            >
              <Building2 className="size-4" />
              {org}
              <ChevronsUpDown className="text-muted-foreground size-3" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start">
            <DropdownMenuLabel>Switch workspace</DropdownMenuLabel>
            {memberships.map((membership) => (
              <DropdownMenuItem
                key={membership.org}
                onSelect={() => {
                  router.push(`/${membership.org}`);
                }}
              >
                <Building2 />
                {membership.org}
                <span className="text-muted-foreground ml-auto text-xs">{membership.role}</span>
              </DropdownMenuItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
      ) : (
        <span
          className="flex items-center gap-2 px-2 text-sm font-semibold"
          data-testid="org-label"
        >
          <Building2 className="size-4" />
          {org}
        </span>
      )}

      {/* Dataset switcher (or a plain label while the org has just one). */}
      {/* Always a menu, even at one dataset: its footer is the only route to
          the manage page and the create flow from inside a dataset, and an
          org with a single dataset is exactly the one that needs to find
          them. The org switcher stays conditional — you cannot create an org
          from here. */}
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            size="sm"
            className="gap-2 font-medium"
            data-testid="dataset-switcher"
          >
            <Layers className="size-4" />
            {ds}
            <ChevronsUpDown className="text-muted-foreground size-3" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start">
          <DropdownMenuLabel>Switch dataset</DropdownMenuLabel>
          {datasets.data?.map((dataset) => (
            <DropdownMenuItem
              key={dataset.name}
              onSelect={() => {
                router.push(`/${org}/${dataset.name}/dashboard`);
              }}
            >
              <Layers />
              {dataset.display_name ?? dataset.name}
              <span className="text-muted-foreground ml-auto text-[10px] tracking-wide uppercase">
                {dataset.entity_type}
              </span>
            </DropdownMenuItem>
          ))}
          <DropdownMenuSeparator />
          <DropdownMenuItem asChild data-testid="manage-datasets">
            <Link href={`/${org}/datasets`}>
              <Settings2 />
              Manage datasets
            </Link>
          </DropdownMenuItem>
          <DropdownMenuItem asChild data-testid="new-dataset-menu">
            <Link href={`/${org}/datasets/new`}>
              <Plus />
              New dataset
            </Link>
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <div className="flex-1" />

      <Button
        variant="outline"
        size="sm"
        className="text-muted-foreground gap-2 lg:w-56 lg:justify-start"
        onClick={onOpenPalette}
        data-testid="open-palette"
        aria-label="Search and commands"
      >
        <Search className="size-4" />
        <span className="hidden lg:inline">Search or jump to…</span>
        <kbd className="bg-muted pointer-events-none ml-auto hidden rounded px-1.5 font-mono text-[10px] lg:inline">
          ⌘K
        </kbd>
      </Button>

      <Button
        variant="ghost"
        size="icon"
        aria-label="Toggle theme"
        onClick={() => {
          setTheme(resolvedTheme === "dark" ? "light" : "dark");
        }}
      >
        <Sun className="size-4 dark:hidden" />
        <Moon className="hidden size-4 dark:block" />
      </Button>

      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="ghost" size="icon" aria-label="Account" data-testid="user-menu">
            <span className="bg-primary text-primary-foreground flex size-7 items-center justify-center rounded-full text-xs font-semibold">
              {user.displayName.slice(0, 1).toUpperCase()}
            </span>
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <DropdownMenuLabel>
            <div className="font-medium">{user.displayName}</div>
            <div className="text-muted-foreground text-xs font-normal">{user.email}</div>
          </DropdownMenuLabel>
          <DropdownMenuSeparator />
          {user.isSuperAdmin && !impersonating && (
            <DropdownMenuItem asChild data-testid="open-console">
              <Link href="/admin/tenants">
                <ShieldCheck />
                Operator console
              </Link>
            </DropdownMenuItem>
          )}
          <DropdownMenuItem onSelect={() => void signOut()} data-testid="sign-out">
            <LogOut />
            Sign out
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </header>
  );
}
