"use client";

import Link from "next/link";
import { useInfiniteQuery } from "@tanstack/react-query";
import { GitMerge } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { bffFetch } from "@/lib/api/client";
import { queryKeys } from "@/lib/query/keys";

interface DuplicateGroup {
  entity_id: string;
  member_count: number;
  members: string[];
  given_name: string | null;
  family_name: string | null;
  email: string | null;
}

interface DuplicatesPage {
  items: DuplicateGroup[];
  snapshot: number;
  next_cursor: string | null;
}

function useDuplicates(org: string) {
  return useInfiniteQuery({
    queryKey: [...queryKeys.org(org), "duplicates"],
    queryFn: ({ pageParam }) => {
      const params = new URLSearchParams({ limit: "50" });
      if (pageParam) params.set("cursor", pageParam);
      return bffFetch<DuplicatesPage>(`/api/orgs/${org}/duplicates?${params.toString()}`);
    },
    initialPageParam: "",
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  });
}

/** What the engine merged on its own (design §5.3 second tab): entities with ≥2 members. */
export function DuplicatesContent({ org }: { org: string }) {
  const query = useDuplicates(org);
  const groups = (query.data?.pages ?? []).flatMap((page) => page.items);
  const snapshot = query.data?.pages[0]?.snapshot;

  return (
    <div className="mx-auto flex max-w-5xl flex-col gap-4 lg:gap-6" data-testid="duplicates-page">
      <div className="flex items-baseline justify-between">
        <h1 className="text-xl font-semibold lg:text-2xl">Duplicates</h1>
        {snapshot !== undefined && (
          <span className="text-muted-foreground text-xs">data as of snapshot {snapshot}</span>
        )}
      </div>
      <p className="text-muted-foreground -mt-2 text-sm">
        Groups the engine merged on its own. Open one to see why — and undo it if it&apos;s wrong.
      </p>

      {query.isPending ? (
        <Skeleton className="h-48 w-full" />
      ) : groups.length === 0 ? (
        <Card>
          <CardContent className="text-muted-foreground p-8 text-center text-sm">
            No duplicate groups — every entity has exactly one source record.
          </CardContent>
        </Card>
      ) : (
        <div className="grid gap-3 lg:grid-cols-2" data-testid="duplicate-groups">
          {groups.map((group) => (
            <Link key={group.entity_id} href={`/${org}/records/${group.entity_id}`}>
              <Card className="hover:border-primary/40 h-full transition-colors">
                <CardContent className="flex flex-col gap-2 p-4">
                  <div className="flex items-center gap-2">
                    <GitMerge className="text-muted-foreground size-4" />
                    <span className="font-medium">
                      {[group.given_name, group.family_name].filter(Boolean).join(" ") ||
                        group.entity_id}
                    </span>
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
      )}

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
