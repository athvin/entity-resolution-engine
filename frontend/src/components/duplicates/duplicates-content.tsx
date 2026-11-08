"use client";

import { useMemo, useState } from "react";
import Link from "next/link";
import { GitMerge } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { displayName } from "@/lib/domain/attributes";
import { confidenceLabel } from "@/components/review/decision-panel";
import { useOrgEntityType } from "@/lib/query/config";
import { effectiveRole, useSession } from "@/lib/query/hooks";
import { useDuplicates, type DuplicateGroup } from "@/lib/query/duplicates";
import { useTags } from "@/lib/query/tags";
import {
  EMPTY_GROUP_FILTERS,
  groupFiltersActive,
  resetGroupFilters,
  type GroupFilters,
} from "@/lib/domain/group-filters";
import { cn } from "@/lib/utils";
import { MatchPreviewCard } from "./match-preview-card";
import { PreMergeReport } from "./pre-merge-report";
import { ScoreBandBrowser } from "./score-band-browser";
import { useDatasetSlug } from "@/lib/use-dataset-slug";

const ROLE_RANK = { viewer: 0, steward: 1, admin: 2 } as const;

const TABS = [
  { id: "groups", label: "Groups" },
  { id: "scores", label: "By score" },
] as const;

const SORTS = [
  { id: "members", label: "most records first" },
  { id: "confidence", label: "highest confidence first" },
  { id: "name", label: "by name" },
] as const;

function sourceOf(memberKey: string): string {
  return memberKey.split(":")[0] ?? memberKey;
}

function GroupsTab({ org }: { org: string }) {
  const ds = useDatasetSlug();
  const [filters, setFilters] = useState<GroupFilters>({ ...EMPTY_GROUP_FILTERS });
  const [memberDraft, setMemberDraft] = useState("");
  const query = useDuplicates(org, ds, filters);
  const tags = useTags(org);
  const entityType = useOrgEntityType(org, ds);
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

  // The model/field option lists come from the loaded groups: the vocabulary
  // is whatever the evidence carries, and a select that offered values no
  // group has would only produce empty lists.
  const modelOptions = useMemo(
    () => [...new Set(groups.flatMap((g) => g.model_versions ?? []))].sort(),
    [groups],
  );
  const fieldOptions = useMemo(
    () => [...new Set(groups.flatMap((g) => g.matched_fields ?? []))].sort(),
    [groups],
  );

  const visible = useMemo(() => {
    const filtered = sourceFilter
      ? groups.filter((group) => group.members.some((m) => sourceOf(m) === sourceFilter))
      : groups;
    const sorted = [...filtered];
    if (sort === "members") {
      sorted.sort((a, b) => b.member_count - a.member_count);
    } else if (sort === "confidence") {
      // Null confidence (no evidence) sorts last, not as zero.
      sorted.sort((a, b) => (b.group_confidence ?? -1) - (a.group_confidence ?? -1));
    } else {
      sorted.sort((a, b) =>
        (displayName(a, entityType) || a.entity_id).localeCompare(
          displayName(b, entityType) || b.entity_id,
        ),
      );
    }
    return sorted;
  }, [groups, sort, sourceFilter]);

  const patch = (part: Partial<GroupFilters>) => {
    setFilters((current) => ({ ...current, ...part }));
  };

  if (query.isPending) return <Skeleton className="h-48 w-full" />;

  if (groups.length === 0 && !groupFiltersActive(filters)) {
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
      <form
        className="flex flex-wrap items-center gap-2"
        onSubmit={(event) => {
          event.preventDefault();
          patch({ member: memberDraft.trim() });
        }}
      >
        <input
          type="search"
          value={memberDraft}
          onChange={(event) => {
            setMemberDraft(event.target.value);
            if (event.target.value === "") patch({ member: "" });
          }}
          placeholder="Find the group containing a record ID…"
          aria-label="Find group by member record ID"
          className="border-input bg-background h-9 min-w-64 rounded-md border px-2 text-sm"
          data-testid="group-member-search"
        />
        <Button type="submit" variant="outline" size="sm" data-testid="group-member-search-go">
          Find group
        </Button>
      </form>

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
        <select
          value={filters.size ?? ""}
          onChange={(event) => {
            const value = event.target.value;
            patch({ size: value === "" ? null : (value as GroupFilters["size"]) });
          }}
          className="border-input bg-background h-9 rounded-md border px-2 text-sm"
          aria-label="Filter by group size"
          data-testid="filter-size"
        >
          <option value="">any size</option>
          <option value="pairs">pairs (2)</option>
          <option value="small">3–10 records</option>
          <option value="large">11+ records</option>
        </select>
        <span className="flex items-center gap-1 text-xs">
          <label htmlFor="filter-min-confidence">confidence</label>
          <input
            id="filter-min-confidence"
            type="number"
            min={0}
            max={100}
            value={filters.minConfidence ?? ""}
            onChange={(event) => {
              patch({
                minConfidence: event.target.value === "" ? null : Number(event.target.value),
              });
            }}
            placeholder="min"
            className="border-input bg-background h-9 w-16 rounded-md border px-2 text-sm"
            data-testid="filter-min-confidence"
          />
          <span aria-hidden="true">–</span>
          <input
            type="number"
            min={0}
            max={100}
            value={filters.maxConfidence ?? ""}
            onChange={(event) => {
              patch({
                maxConfidence: event.target.value === "" ? null : Number(event.target.value),
              });
            }}
            placeholder="max"
            aria-label="Maximum confidence"
            className="border-input bg-background h-9 w-16 rounded-md border px-2 text-sm"
            data-testid="filter-max-confidence"
          />
        </span>
        {modelOptions.length > 0 && (
          <select
            value={filters.matchedBy}
            onChange={(event) => {
              patch({ matchedBy: event.target.value });
            }}
            className="border-input bg-background h-9 rounded-md border px-2 text-sm"
            aria-label="Filter by matching model"
            data-testid="filter-model"
          >
            <option value="">any model</option>
            {modelOptions.map((model) => (
              <option key={model} value={model}>
                {model}
              </option>
            ))}
          </select>
        )}
        {fieldOptions.length > 0 && (
          <select
            value={filters.matchedOn}
            onChange={(event) => {
              patch({ matchedOn: event.target.value });
            }}
            className="border-input bg-background h-9 rounded-md border px-2 text-sm"
            aria-label="Filter by matched-on field"
            data-testid="filter-field"
          >
            <option value="">any matched-on field</option>
            {fieldOptions.map((field) => (
              <option key={field} value={field}>
                {field}
              </option>
            ))}
          </select>
        )}
        <select
          value={filters.verified === null ? "" : String(filters.verified)}
          onChange={(event) => {
            const value = event.target.value;
            patch({ verified: value === "" ? null : value === "true" });
          }}
          className="border-input bg-background h-9 rounded-md border px-2 text-sm"
          aria-label="Filter by verification"
          data-testid="filter-verified"
        >
          <option value="">any verification</option>
          <option value="true">verified</option>
          <option value="false">not verified</option>
        </select>
        {(tags.data ?? []).length > 0 && (
          <>
            <select
              value={filters.tag}
              onChange={(event) => {
                patch({ tag: event.target.value });
              }}
              className="border-input bg-background h-9 rounded-md border px-2 text-sm"
              aria-label="Filter to a tag"
              data-testid="filter-tag"
            >
              <option value="">tagged…</option>
              {(tags.data ?? []).map((tag) => (
                <option key={tag.id} value={tag.name}>
                  {tag.name}
                </option>
              ))}
            </select>
            <select
              value={filters.excludeTag}
              onChange={(event) => {
                patch({ excludeTag: event.target.value });
              }}
              className="border-input bg-background h-9 rounded-md border px-2 text-sm"
              aria-label="Exclude a tag"
              data-testid="filter-exclude-tag"
            >
              <option value="">not tagged…</option>
              {(tags.data ?? []).map((tag) => (
                <option key={tag.id} value={tag.name}>
                  {tag.name}
                </option>
              ))}
            </select>
          </>
        )}
        {groupFiltersActive(filters) && (
          <button
            type="button"
            onClick={() => {
              setMemberDraft("");
              setFilters(resetGroupFilters());
            }}
            className="text-muted-foreground hover:text-foreground text-xs underline"
            data-testid="filters-clear"
          >
            clear filters
          </button>
        )}
        <span className="flex-1" />
        {snapshot !== undefined && (
          <span className="text-muted-foreground text-xs">data as of snapshot {snapshot}</span>
        )}
      </div>

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

      {visible.length === 0 ? (
        <Card>
          <CardContent
            className="text-muted-foreground p-8 text-center text-sm"
            data-testid="groups-empty-filtered"
          >
            No groups match these filters.
          </CardContent>
        </Card>
      ) : (
        <div className="grid gap-3 lg:grid-cols-2" data-testid="duplicate-groups">
          {visible.map((group: DuplicateGroup) => (
            <Link key={group.entity_id} href={`/${org}/${ds}/records/${group.entity_id}`}>
              <Card className="hover:border-primary/40 h-full transition-colors">
                <CardContent className="flex flex-col gap-2 p-4">
                  <div className="flex items-center gap-2">
                    <GitMerge className="text-muted-foreground size-4" />
                    <span className="font-medium">
                      {displayName(group, entityType) || group.entity_id}
                    </span>
                    {group.verified && (
                      <span
                        className="text-foreground text-xs font-semibold"
                        data-testid="group-verified"
                      >
                        verified
                      </span>
                    )}
                    <span className="bg-secondary text-secondary-foreground ml-auto rounded-full px-2 py-0.5 text-xs">
                      {group.member_count} records
                    </span>
                    {typeof group.group_confidence === "number" && (
                      <span
                        className="text-muted-foreground text-xs tabular-nums"
                        data-testid="group-confidence"
                      >
                        {confidenceLabel(group.group_confidence)}
                      </span>
                    )}
                  </div>
                  <div className="text-muted-foreground truncate text-sm">
                    {entityType === "account" ? (group.domain ?? "") : (group.email ?? "")}
                  </div>
                  <div className="text-muted-foreground flex flex-wrap gap-1.5 text-[11px]">
                    {group.members.slice(0, 4).map((member) => (
                      <span key={member} className="bg-muted rounded px-1.5 py-0.5 font-mono">
                        {member}
                        {typeof group.member_confidences?.[member] === "number" && (
                          <span className="text-muted-foreground ml-1 tabular-nums">
                            {group.member_confidences[member]}
                          </span>
                        )}
                      </span>
                    ))}
                    {group.members.length > 4 && <span>+{group.members.length - 4} more</span>}
                  </div>
                  {(group.matched_fields ?? []).length > 0 && (
                    <div
                      className="text-muted-foreground flex flex-wrap items-center gap-1.5 text-[11px]"
                      data-testid="matched-fields"
                    >
                      <span>matched on</span>
                      {(group.matched_fields ?? []).map((field) => (
                        <span key={field} className="rounded border px-1.5 py-0.5 font-mono">
                          {field}
                        </span>
                      ))}
                    </div>
                  )}
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
          {isSteward && <MatchPreviewCard org={org} />}
          <PreMergeReport org={org} />
          <GroupsTab org={org} />
        </div>
      ) : (
        <ScoreBandBrowser org={org} isSteward={isSteward} />
      )}
    </div>
  );
}
