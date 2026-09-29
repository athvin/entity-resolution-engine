"use client";

import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Download, Megaphone, Trash2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Skeleton } from "@/components/ui/skeleton";
import { bffFetch, BffRequestError } from "@/lib/api/client";
import { effectiveRole, useSession, type GoldenPage } from "@/lib/query/hooks";
import { queryKeys } from "@/lib/query/keys";

const ROLE_RANK = { viewer: 0, steward: 1, admin: 2 } as const;

interface SegmentRow {
  id: string;
  name: string;
  definition: { q: string; limit: number };
  createdBy: string;
}

function useSegments(org: string) {
  return useQuery({
    queryKey: [...queryKeys.org(org), "segments"],
    queryFn: () => bffFetch<SegmentRow[]>(`/api/orgs/${org}/segments`),
  });
}

function usePreview(org: string, q: string) {
  return useQuery({
    queryKey: [...queryKeys.org(org), "segment-preview", q],
    queryFn: () =>
      bffFetch<GoldenPage>(
        `/api/orgs/${org}/golden-records?limit=5${q ? `&q=${encodeURIComponent(q)}` : ""}`,
      ),
    staleTime: 15_000,
  });
}

async function downloadCsv(org: string, definition: { q: string; limit: number }, name: string) {
  const response = await fetch(`/api/orgs/${org}/segments/export`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(definition),
  });
  if (!response.ok) throw new Error("export failed");
  const blob = await response.blob();
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = `${name || "segment"}.csv`;
  anchor.click();
  URL.revokeObjectURL(url);
}

/** Campaigns v1 (design §5.7): saved segments + snapshot-consistent CSV export.
 * Scheduled deliveries ride the parameterized export job (§7.19) when it lands. */
export function CampaignsContent({ org }: { org: string }) {
  const session = useSession();
  const segments = useSegments(org);
  const queryClient = useQueryClient();
  const isSteward = (() => {
    const role = effectiveRole(session.data, org);
    return role !== null && ROLE_RANK[role] >= ROLE_RANK.steward;
  })();

  const [name, setName] = useState("");
  const [q, setQ] = useState("");
  const [error, setError] = useState<string | null>(null);
  const preview = usePreview(org, q);

  const create = useMutation({
    mutationFn: () =>
      bffFetch<SegmentRow>(`/api/orgs/${org}/segments`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ name, definition: { q, limit: 1000 } }),
      }),
    onSuccess: () => {
      setName("");
      setError(null);
      void queryClient.invalidateQueries({ queryKey: [...queryKeys.org(org), "segments"] });
    },
    onError: (cause) => {
      setError(cause instanceof BffRequestError ? cause.message : "creation failed");
    },
  });

  const remove = useMutation({
    mutationFn: (segmentId: string) =>
      bffFetch<unknown>(`/api/orgs/${org}/segments/${segmentId}`, { method: "DELETE" }),
    onSettled: () =>
      void queryClient.invalidateQueries({ queryKey: [...queryKeys.org(org), "segments"] }),
  });

  return (
    <div
      className="mx-auto flex w-full max-w-4xl min-w-0 flex-col gap-4 lg:gap-6"
      data-testid="campaigns-page"
    >
      <div>
        <h1 className="flex items-center gap-2 text-xl font-semibold lg:text-2xl">
          <Megaphone className="size-5" />
          Campaigns
        </h1>
        <p className="text-muted-foreground mt-1 text-sm">
          A campaign is a saved segment of your golden records plus what you do with it. Exports are
          snapshot-consistent — a run committing mid-download cannot shear the file.
        </p>
      </div>

      {isSteward && (
        <Card data-testid="segment-builder">
          <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
            <CardTitle className="text-base">New segment</CardTitle>
            <CardDescription>
              The saved artifact is always this inspectable filter — never a prompt.
            </CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-3 p-4 pt-2 lg:p-6 lg:pt-2">
            <div className="flex flex-wrap items-end gap-3">
              <div className="grid w-full gap-1.5 sm:w-auto">
                <Label htmlFor="segment-name">Name</Label>
                <Input
                  id="segment-name"
                  value={name}
                  onChange={(event) => {
                    setName(event.target.value);
                  }}
                  placeholder="Newsletter — bay area"
                  className="w-full sm:w-56"
                  data-testid="segment-name"
                />
              </div>
              <div className="grid w-full gap-1.5 sm:w-auto">
                <Label htmlFor="segment-q">Search filter (name / email)</Label>
                <Input
                  id="segment-q"
                  value={q}
                  onChange={(event) => {
                    setQ(event.target.value);
                  }}
                  placeholder="empty = every golden record"
                  className="w-full sm:w-64"
                  data-testid="segment-q"
                />
              </div>
              <Button
                disabled={!name.trim() || create.isPending}
                onClick={() => {
                  create.mutate();
                }}
                data-testid="segment-create"
              >
                Save segment
              </Button>
            </div>
            {error && (
              <p className="text-destructive text-sm" role="alert">
                {error}
              </p>
            )}
            <div className="text-muted-foreground text-xs" data-testid="segment-preview">
              {preview.isPending ? (
                "previewing…"
              ) : (
                <>
                  Preview (snapshot {preview.data?.snapshot}):{" "}
                  {preview.data?.items
                    .map((item) => [item.given_name, item.family_name].filter(Boolean).join(" "))
                    .join(" · ") || "no matches"}
                  {preview.data?.next_cursor ? " · …more" : ""}
                </>
              )}
            </div>
          </CardContent>
        </Card>
      )}

      {segments.isPending ? (
        <Skeleton className="h-32 w-full" />
      ) : (segments.data ?? []).length === 0 ? (
        <Card>
          <CardContent className="text-muted-foreground p-8 text-center text-sm">
            No segments yet{isSteward ? " — save the first one above." : "."}
          </CardContent>
        </Card>
      ) : (
        <Card data-testid="segments-table">
          <CardContent className="overflow-x-auto p-0">
            <table className="w-full min-w-[32rem] text-sm">
              <thead>
                <tr className="text-muted-foreground border-b text-left text-xs uppercase">
                  <th className="px-4 py-2.5 font-medium">Segment</th>
                  <th className="px-4 py-2.5 font-medium">Filter</th>
                  <th className="px-4 py-2.5 font-medium">By</th>
                  <th className="px-4 py-2.5 text-right font-medium">Actions</th>
                </tr>
              </thead>
              <tbody>
                {(segments.data ?? []).map((segment) => (
                  <tr
                    key={segment.id}
                    className="border-b last:border-0"
                    data-testid={`segment-${segment.name}`}
                  >
                    <td className="px-4 py-2.5 font-medium">{segment.name}</td>
                    <td className="text-muted-foreground px-4 py-2.5 font-mono text-xs">
                      {segment.definition.q || "(all records)"}
                    </td>
                    <td className="text-muted-foreground max-w-40 truncate px-4 py-2.5 text-xs">
                      {segment.createdBy}
                    </td>
                    <td className="px-4 py-2.5 text-right">
                      <span className="inline-flex gap-1">
                        <Button
                          variant="outline"
                          size="sm"
                          data-testid={`export-${segment.name}`}
                          onClick={() => void downloadCsv(org, segment.definition, segment.name)}
                        >
                          <Download /> CSV
                        </Button>
                        <Button
                          variant="outline"
                          size="sm"
                          disabled
                          title="scheduled delivery needs the parameterized export job — roadmap"
                        >
                          Schedule
                        </Button>
                        {isSteward && (
                          <Button
                            variant="ghost"
                            size="icon"
                            aria-label={`Delete ${segment.name}`}
                            onClick={() => {
                              remove.mutate(segment.id);
                            }}
                          >
                            <Trash2 className="size-4" />
                          </Button>
                        )}
                      </span>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </CardContent>
        </Card>
      )}
    </div>
  );
}
