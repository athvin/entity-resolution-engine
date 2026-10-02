"use client";

import { useEffect, useState } from "react";
import { useRouter } from "next/navigation";
import { useTheme } from "next-themes";
import { useQuery } from "@tanstack/react-query";
import { Command } from "cmdk";
import { Building2, Eye, LogOut, Moon, Play, ShieldCheck, Sun, User } from "lucide-react";

import { bffFetch } from "@/lib/api/client";
import type { Membership, SessionUser } from "@/lib/auth/types";
import { useToast } from "@/components/ui/toaster";
import { RUN_KINDS, type RunKind } from "@/lib/domain/run-kinds";
import { NAV_ITEMS } from "@/lib/nav";
import { useImpersonation, type AdminOrgRow } from "@/lib/query/admin";
import { effectiveRole, useSession, type GoldenPage } from "@/lib/query/hooks";
import { useSubmitJob } from "@/lib/query/steward";

const ROLE_RANK = { viewer: 0, steward: 1, admin: 2 } as const;

interface CommandPaletteProps {
  org: string;
  user: SessionUser;
  memberships: Membership[];
  impersonating: boolean;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

const GROUP_CLASS =
  "[&_[cmdk-group-heading]]:text-muted-foreground [&_[cmdk-group-heading]]:px-2 [&_[cmdk-group-heading]]:py-1.5 [&_[cmdk-group-heading]]:text-xs [&_[cmdk-group-heading]]:font-medium";
const ITEM_CLASS =
  "data-[selected=true]:bg-accent data-[selected=true]:text-accent-foreground flex min-h-11 cursor-default items-center gap-3 rounded-md px-3 text-sm data-[disabled=true]:opacity-50 lg:min-h-9";

function useDebounced(value: string, ms: number): string {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const timer = setTimeout(() => {
      setDebounced(value);
    }, ms);
    return () => {
      clearTimeout(timer);
    };
  }, [value, ms]);
  return debounced;
}

/**
 * The ⌘K palette — the spine of the app (design doc §3.2). Resolves pages,
 * golden records (live search), workspaces, operator commands, theme, session.
 */
export function CommandPalette({
  org,
  user,
  memberships,
  impersonating,
  open,
  onOpenChange,
}: CommandPaletteProps) {
  const router = useRouter();
  const { setTheme } = useTheme();
  const { enter, exit } = useImpersonation();
  const [input, setInput] = useState("");
  const q = useDebounced(input.trim(), 200);
  const submit = useSubmitJob(org);
  const toast = useToast();
  // Through `effectiveRole` and the session, not the `memberships` prop: that
  // prop carries real memberships, and under impersonation the role that
  // decides what may be started is the impersonated one. Same call the Runs
  // page makes, so the two surfaces cannot disagree about who may run.
  const session = useSession();
  const role = effectiveRole(session.data, org);
  const isSteward = role !== null && ROLE_RANK[role] >= ROLE_RANK.steward;

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

  const records = useQuery({
    queryKey: ["org", org, "palette-records", q],
    queryFn: () =>
      bffFetch<GoldenPage>(`/api/orgs/${org}/golden-records?q=${encodeURIComponent(q)}&limit=6`),
    enabled: open && q.length >= 2,
    staleTime: 10_000,
  });

  const adminOrgs = useQuery({
    queryKey: ["admin", "palette-orgs"],
    queryFn: () => bffFetch<AdminOrgRow[]>("/api/admin/orgs"),
    enabled: open && user.isSuperAdmin && !impersonating,
    staleTime: 30_000,
  });

  function run(action: () => void) {
    onOpenChange(false);
    setInput("");
    action();
  }

  /** Start a run from the palette, mirroring the Runs page's split button.
   *
   * The palette closes on select, so the outcome is reported by a toast rather
   * than the inline error the Runs page can show — including the one an
   * operator most needs to see, that the org already has a run in flight. The
   * success path lands on the new job's page, which is where the Runs button
   * goes too: starting a run and not being shown it is a dead end. */
  function startRun(entry: RunKind) {
    submit.mutate(
      { kind: entry.kind, params: entry.params },
      {
        onSuccess: (result) => {
          router.push(`/${org}/runs/${result.job_id}`);
        },
        onError: () => {
          toast({
            title: `Could not start the ${entry.label.toLowerCase()}`,
            description: "the org may already have a run in flight",
            tone: "destructive",
          });
        },
      },
    );
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
        value={input}
        onValueChange={setInput}
        placeholder="Search records, pages, commands…"
        className="placeholder:text-muted-foreground h-14 w-full border-b bg-transparent px-4 text-base outline-none lg:h-12 lg:text-sm"
      />
      <Command.List className="max-h-[calc(100dvh-3.5rem)] overflow-y-auto p-2 lg:max-h-96">
        <Command.Empty className="text-muted-foreground px-4 py-8 text-center text-sm">
          Nothing matches.
        </Command.Empty>

        {(records.data?.items.length ?? 0) > 0 && (
          <Command.Group heading="Records" className={GROUP_CLASS}>
            {records.data?.items.map((record) => {
              const name =
                [record.given_name, record.family_name].filter(Boolean).join(" ") ||
                record.entity_id;
              return (
                <Command.Item
                  key={record.entity_id}
                  value={`record ${name} ${record.email ?? ""} ${record.entity_id}`}
                  onSelect={() => {
                    run(() => {
                      router.push(`/${org}/records/${record.entity_id}`);
                    });
                  }}
                  className={ITEM_CLASS}
                >
                  <User className="size-4" />
                  <span className="min-w-0 truncate">{name}</span>
                  <span className="text-muted-foreground ml-auto truncate text-xs">
                    {record.email ?? ""}
                  </span>
                </Command.Item>
              );
            })}
          </Command.Group>
        )}

        {isSteward && (
          <Command.Group heading="Run" className={GROUP_CLASS}>
            {RUN_KINDS.map((entry) => (
              <Command.Item
                key={entry.kind}
                // Searchable by what a steward types: "run now" finds all three.
                value={`run now ${entry.label}`}
                disabled={submit.isPending}
                onSelect={() => {
                  run(() => {
                    startRun(entry);
                  });
                }}
                className={ITEM_CLASS}
                data-testid={`palette-run-${entry.kind}`}
              >
                <Play className="size-4" />
                {entry.label}
              </Command.Item>
            ))}
          </Command.Group>
        )}

        <Command.Group heading="Pages" className={GROUP_CLASS}>
          {NAV_ITEMS.map((item) => (
            <Command.Item
              key={item.id}
              disabled={!item.enabled}
              onSelect={() => {
                run(() => {
                  router.push(item.path(org));
                });
              }}
              className={ITEM_CLASS}
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
          <Command.Group heading="Workspaces" className={GROUP_CLASS}>
            {memberships.map((membership) => (
              <Command.Item
                key={membership.org}
                onSelect={() => {
                  run(() => {
                    router.push(`/${membership.org}/dashboard`);
                  });
                }}
                className={ITEM_CLASS}
              >
                <Building2 className="size-4" />
                Open {membership.org}
              </Command.Item>
            ))}
          </Command.Group>
        )}

        {user.isSuperAdmin && !impersonating && (
          <Command.Group heading="Operator" className={GROUP_CLASS}>
            <Command.Item
              value="operator console tenants"
              onSelect={() => {
                run(() => {
                  router.push("/admin/tenants");
                });
              }}
              className={ITEM_CLASS}
            >
              <ShieldCheck className="size-4" />
              Operator console
            </Command.Item>
            {(adminOrgs.data ?? []).slice(0, 8).map((row) => (
              <Command.Item
                key={row.name}
                value={`view as ${row.name} ${row.display_name}`}
                disabled={row.state !== "active" || !row.has_credentials}
                onSelect={() => {
                  run(() => {
                    enter.mutate({ org: row.name, role: "admin" });
                  });
                }}
                className={ITEM_CLASS}
              >
                <Eye className="size-4" />
                View {row.name} as admin
              </Command.Item>
            ))}
          </Command.Group>
        )}

        <Command.Group heading="Preferences" className={GROUP_CLASS}>
          {impersonating && (
            <Command.Item
              value="exit impersonation stop viewing"
              onSelect={() => {
                run(() => {
                  exit.mutate();
                });
              }}
              className={ITEM_CLASS}
            >
              <Eye className="size-4" />
              Exit impersonation
            </Command.Item>
          )}
          <Command.Item
            onSelect={() => {
              run(() => {
                setTheme("light");
              });
            }}
            className={ITEM_CLASS}
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
            className={ITEM_CLASS}
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
            className={ITEM_CLASS}
          >
            <LogOut className="size-4" />
            Sign out
          </Command.Item>
        </Command.Group>
      </Command.List>
    </Command.Dialog>
  );
}
