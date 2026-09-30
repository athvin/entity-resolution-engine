"use client";

import { useMemo, useState } from "react";
import Link from "next/link";
import { GitMerge } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { displayName } from "@/lib/domain/attributes";
import { effectiveRole, useSession } from "@/lib/query/hooks";
import { useDuplicates, type DuplicateGroup } from "@/lib/query/duplicates";
import { cn } from "@/lib/utils";
import { PreMergeReport } from "./pre-merge-report";
import { ScoreBandBrowser } from "./score-band-browser";

const ROLE_RANK = { viewer: 0, steward: 1, admin: 2 } as const;

const TABS = [
  { id: "groups", label: "Groups" },
  { id: "scores", label: "By score" },
] as const;

const SORTS = [
  { id: "members", label: "most records first" },
  { id: "name", label: "by name" },
] as const;

function sourceOf(memberKey: string): string {
  return memberKey.split(":")[0] ?? memberKey;
}

function GroupsTab({ org }: { org: string }) {
  const query = useDuplicates(org);
  const [sort, setSort] = useState<(typeof SORTS)[number]["id"]>("members");
  const [sourceFilter, setSourceFilter] = useState<string | null>(null);

  const groups = useMemo(
    () => (query.data?.pages ?? []).flatMap((page) => page.items),
    [query.data],
  );
  const snapshot = query.data?.pages[0]?.snapshot;

  const sources = useMemo(() => {
    const seen = new Set<string>();
    for (const group of groups) for (const member of group.members) seen.add(sourceOf(member));
    return [...seen].sort();
  }, [groups]);

  const visible = useMemo(() => {
    const filtered = sourceFilter
      ? groups.filter((group) => group.members.some((m) => sourceOf(m) === sourceFilter))
      : groups;
    const sorted = [...filtered];
    if (sort === "members") {
      sorted.sort((a, b) => b.member_count - a.member_count);
    } else {
      sorted.sort((a, b) =>
        (displayName(a) || a.entity_id).localeCompare(displayName(b) || b.entity_id),
      );
    }
    return sorted;
  }, [groups, sort, sourceFilter]);

  if (query.isPending) return <Skeleton className="h-48 w-full" />;

  if (groups.length === 0) {
    return (
      <Card>
        <CardContent className="text-muted-foreground p-8 text-center text-sm">
          No duplicate groups — every entity has exactly one source record.
        </CardContent>
      </Card>
    );
  }

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2 text-sm">
        <select
          value={sort}
          onChange={(event) => {
            setSort(event.target.value as (typeof SORTS)[number]["id"]);
          }}
          className="border-input bg-background h-9 rounded-md border px-2 text-sm"
          aria-label="Sort groups"
          data-testid="groups-sort"
        >
          {SORTS.map((entry) => (
            <option key={entry.id} value={entry.id}>
              {entry.label}
            </option>
          ))}
        </select>
        {sources.length > 1 && (
          <div className="flex flex-wrap gap-1" role="group" aria-label="Filter by source">
            {sources.map((source) => (
              <button
                key={source}
                onClick={() => {
                  setSourceFilter((current) => (current === source ? null : source));
                }}
                aria-pressed={sourceFilter === source}
                className={cn(
                  "rounded-full border px-2.5 py-1 text-xs",
                  sourceFilter === source
                    ? "border-primary bg-primary/10 text-primary"
                    : "text-muted-foreground hover:bg-accent",
                )}
                data-testid={`source-chip-${source}`}
              >
                {source}
              </button>
            ))}
          </div>
        )}
        <span className="flex-1" />
        {snapshot !== undefined && (
          <span className="text-muted-foreground text-xs">data as of snapshot {snapshot}</span>
        )}
      </div>

      <div className="grid gap-3 lg:grid-cols-2" data-testid="duplicate-groups">
        {visible.map((group: DuplicateGroup) => (
          <Link key={group.entity_id} href={`/${org}/records/${group.entity_id}`}>
            <Card className="hover:border-primary/40 h-full transition-colors">
              <CardContent className="flex flex-col gap-2 p-4">
                <div className="flex items-center gap-2">
                  <GitMerge className="text-muted-foreground size-4" />
                  <span className="font-medium">{displayName(group) || group.entity_id}</span>
                  <span className="bg-secondary text-secondary-foreground ml-auto rounded-full px-2 py-0.5 text-xs">
                    {group.member_count} records
                  </span>
                </div>
                <div className="text-muted-foreground truncate text-sm">{group.email ?? ""}</div>
                <div className="text-muted-foreground flex flex-wrap gap-1.5 text-[11px]">
                  {group.members.slice(0, 4).map((member) => (
                    <span key={member} className="bg-muted rounded px-1.5 py-0.5 font-mono">
                      {member}
                    </span>
                  ))}
                  {group.members.length > 4 && <span>+{group.members.length - 4} more</span>}
                </div>
              </CardContent>
            </Card>
          </Link>
        ))}
      </div>

      {query.hasNextPage && (
        <Button
          variant="outline"
          onClick={() => void query.fetchNextPage()}
          disabled={query.isFetchingNextPage}
          data-testid="duplicates-more"
        >
          {query.isFetchingNextPage ? "Loading…" : "Load more"}
        </Button>
      )}
    </div>
  );
}

/** What the engine merged on its own (design §5.3): the groups, and — under
 * "By score" — the per-pair evidence those merges rest on. */
export function DuplicatesContent({ org }: { org: string }) {
  const [tab, setTab] = useState<(typeof TABS)[number]["id"]>("groups");
  const session = useSession();
  const role = effectiveRole(session.data, org);
  const isSteward = role !== null && ROLE_RANK[role] >= ROLE_RANK.steward;

  return (
    <div
      className="mx-auto flex w-full max-w-5xl min-w-0 flex-col gap-4 lg:gap-6"
      data-testid="duplicates-page"
    >
      <div className="flex items-center gap-4">
        <h1 className="text-xl font-semibold lg:text-2xl">Duplicates</h1>
        <div className="flex gap-1" role="tablist" aria-label="Duplicates tabs">
          {TABS.map((entry) => (
            <button
              key={entry.id}
              role="tab"
              aria-selected={tab === entry.id}
              onClick={() => {
                setTab(entry.id);
              }}
              className={cn(
                "rounded-md px-3 py-1.5 text-sm font-medium",
                tab === entry.id
                  ? "bg-accent text-accent-foreground"
                  : "text-muted-foreground hover:bg-accent/50",
              )}
              data-testid={`tab-${entry.id}`}
            >
              {entry.label}
            </button>
          ))}
        </div>
      </div>
      <p className="text-muted-foreground -mt-2 text-sm">
        {tab === "groups"
          ? "Groups the engine merged on its own. Open one to see why — and undo it if it's wrong."
          : "Every scored pair in a probability band, with the evidence behind it."}
      </p>

      {tab === "groups" ? (
        <div className="flex flex-col gap-4">
          <PreMergeReport org={org} />
          <GroupsTab org={org} />
        </div>
      ) : (
        <ScoreBandBrowser org={org} isSteward={isSteward} />
      )}
    </div>
  );
}
