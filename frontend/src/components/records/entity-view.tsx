"use client";

import { useState } from "react";
import Link from "next/link";
import { ArrowLeft, GitMerge, Split } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { BffRequestError } from "@/lib/api/client";
import {
  effectiveRole,
  useEntityDetail,
  useSession,
  type EntityEvent,
  type GoldenRecord,
  type LineageRow,
} from "@/lib/query/hooks";
import { useUnmerge } from "@/lib/query/reviews";

const ROLE_RANK = { viewer: 0, steward: 1, admin: 2 } as const;

const ATTRIBUTES: { key: keyof GoldenRecord; label: string }[] = [
  { key: "given_name", label: "Given name" },
  { key: "family_name", label: "Family name" },
  { key: "email", label: "Email" },
  { key: "phone_e164", label: "Phone" },
  { key: "addr_number", label: "Street no." },
  { key: "addr_street", label: "Street" },
  { key: "addr_unit", label: "Unit" },
  { key: "addr_city", label: "City" },
  { key: "addr_region", label: "Region" },
  { key: "addr_postal", label: "Postal" },
  { key: "birth_date", label: "Birth date" },
];

function describeEvent(event: EntityEvent): string {
  const type = event.event_type.replaceAll("_", " ");
  return `${type} — run ${event.run_id}`;
}

/**
 * Entity detail (design §5.2): the golden record, its members side by side,
 * per-field provenance from golden_lineage, and the entity_events timeline.
 */
export function EntityView({ org, entityId }: { org: string; entityId: string }) {
  const query = useEntityDetail(org, entityId);
  const session = useSession();
  const unmerge = useUnmerge(org, entityId);
  const [marked, setMarked] = useState<ReadonlySet<string>>(new Set());
  const [unmergeMessage, setUnmergeMessage] = useState<string | null>(null);
  const role = effectiveRole(session.data, org);
  const isSteward = role !== null && ROLE_RANK[role] >= ROLE_RANK.steward;

  if (query.error instanceof BffRequestError && query.error.status === 404) {
    return (
      <div className="mx-auto max-w-4xl py-16 text-center">
        <p className="font-medium">No entity {entityId} in this workspace.</p>
        <Link href={`/${org}/records`} className="text-sm underline">
          Back to records
        </Link>
      </div>
    );
  }
  if (query.isPending || !query.data) {
    return (
      <div className="mx-auto flex max-w-5xl flex-col gap-4">
        <Skeleton className="h-8 w-64" />
        <Skeleton className="h-40 w-full" />
        <Skeleton className="h-64 w-full" />
      </div>
    );
  }

  const { golden, members, lineage, events } = query.data;
  const lineageByAttribute = new Map<string, LineageRow>(
    lineage.map((row) => [row.attribute, row]),
  );
  const name = [golden.given_name, golden.family_name].filter(Boolean).join(" ") || entityId;

  return (
    <div className="mx-auto flex max-w-5xl flex-col gap-4 lg:gap-6" data-testid="entity-view">
      <div>
        <Link
          href={`/${org}/records`}
          className="text-muted-foreground mb-2 inline-flex items-center gap-1 text-sm hover:underline"
        >
          <ArrowLeft className="size-3.5" /> Records
        </Link>
        <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
          <h1 className="text-xl font-semibold lg:text-2xl" data-testid="entity-name">
            {name}
          </h1>
          <span className="text-muted-foreground font-mono text-xs">{golden.entity_id}</span>
          {members.length > 1 && (
            <span className="bg-secondary text-secondary-foreground inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-xs">
              <GitMerge className="size-3" />
              {members.length} source records
            </span>
          )}
        </div>
      </div>

      <Card data-testid="entity-golden">
        <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
          <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
            Golden record — every value explains where it came from
          </CardTitle>
        </CardHeader>
        <CardContent className="p-4 pt-2 lg:p-6 lg:pt-2">
          <div className="grid gap-x-6 gap-y-3 sm:grid-cols-2 lg:grid-cols-3">
            {ATTRIBUTES.map(({ key, label }) => {
              const value = golden[key];
              if (value === null || value === undefined || value === "") return null;
              const provenance = lineageByAttribute.get(key);
              return (
                <div key={key}>
                  <div className="text-muted-foreground text-xs">{label}</div>
                  <div className="text-sm font-medium">{String(value)}</div>
                  {provenance && (
                    <div
                      className="text-muted-foreground mt-0.5 text-[11px]"
                      data-testid={`lineage-${key}`}
                    >
                      from {provenance.source_system}:{provenance.source_record_id} via{" "}
                      <span className="font-mono">{provenance.rule}</span>
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        </CardContent>
      </Card>

      <Card data-testid="entity-members">
        <CardHeader className="flex-row items-center justify-between space-y-0 p-4 pb-1 lg:p-6 lg:pb-2">
          <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
            Members
          </CardTitle>
          {isSteward && members.length > 1 && (
            <Button
              size="sm"
              variant="outline"
              disabled={marked.size === 0 || marked.size >= members.length || unmerge.isPending}
              data-testid="unmerge-button"
              onClick={() => {
                setUnmergeMessage(null);
                unmerge.mutate([...marked], {
                  onSuccess: (result) => {
                    setMarked(new Set());
                    setUnmergeMessage(
                      result.status === "applied"
                        ? `unmerge applied — ${String(result.pairs)} never-rule${result.pairs === 1 ? "" : "s"} written; the split lands with the reconcile job`
                        : "unmerge staged — it applies when the current run finishes",
                    );
                  },
                  onError: (error) => {
                    setUnmergeMessage(
                      error instanceof BffRequestError ? error.message : "unmerge failed",
                    );
                  },
                });
              }}
            >
              <Split />
              Unmerge {marked.size > 0 ? `${String(marked.size)} ` : ""}selected
            </Button>
          )}
        </CardHeader>
        <CardContent className="overflow-x-auto p-4 pt-2 lg:p-6 lg:pt-2">
          {unmergeMessage && (
            <p className="text-muted-foreground mb-2 text-xs" data-testid="unmerge-status">
              {unmergeMessage}
            </p>
          )}
          <table className="w-full min-w-[28rem] text-sm">
            <thead>
              <tr className="text-muted-foreground border-b text-left text-xs uppercase">
                {isSteward && members.length > 1 && <th className="w-8 py-1.5" />}
                <th className="py-1.5 pr-4 font-medium">Source</th>
                <th className="py-1.5 pr-4 font-medium">Record id</th>
                <th className="py-1.5 pr-4 font-medium">Assigned</th>
                <th className="py-1.5 font-medium">Run</th>
              </tr>
            </thead>
            <tbody>
              {members.map((member) => (
                <tr key={member.record_key} className="border-b last:border-0">
                  {isSteward && members.length > 1 && (
                    <td className="py-2 pr-2">
                      <input
                        type="checkbox"
                        checked={marked.has(member.record_key)}
                        aria-label={`Split out ${member.record_key}`}
                        data-testid={`mark-${member.record_key}`}
                        onChange={() => {
                          setMarked((current) => {
                            const next = new Set(current);
                            if (next.has(member.record_key)) next.delete(member.record_key);
                            else next.add(member.record_key);
                            return next;
                          });
                        }}
                        className="size-4"
                      />
                    </td>
                  )}
                  <td className="py-2 pr-4 font-medium">{member.source_system}</td>
                  <td className="py-2 pr-4 font-mono text-xs">{member.source_record_id}</td>
                  <td className="text-muted-foreground py-2 pr-4 text-xs">
                    {member.assigned_at ?? "—"}
                  </td>
                  <td className="text-muted-foreground py-2 font-mono text-xs">
                    {member.run_id ?? "—"}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </CardContent>
      </Card>

      <Card data-testid="entity-timeline">
        <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
          <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
            History
          </CardTitle>
        </CardHeader>
        <CardContent className="p-4 pt-2 lg:p-6 lg:pt-2">
          {events.length === 0 ? (
            <p className="text-muted-foreground text-sm">No recorded events.</p>
          ) : (
            <ol className="relative flex flex-col gap-3 border-l pl-4">
              {events.map((event) => (
                <li key={event.event_id} className="text-sm">
                  <span className="bg-border absolute -left-[3px] mt-1.5 size-1.5 rounded-full" />
                  <span className="font-medium">{describeEvent(event)}</span>
                  <span className="text-muted-foreground ml-2 text-xs">{event.occurred_at}</span>
                </li>
              ))}
            </ol>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
