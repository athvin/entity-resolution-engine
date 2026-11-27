"use client";

import { useMemo, useState } from "react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { effectiveRole, useSession } from "@/lib/query/hooks";
import {
  useApplyApproved,
  useApplyUpserts,
  useAutoApproval,
  useDecideBatch,
  useDecideUpserts,
  useGeneratePreviews,
  useMergePreview,
  useMergePreviews,
  useRequestRestore,
  useRestore,
  useSetAutoApproval,
  useUpsertPreviews,
  type BatchResult,
  type MergePreviewSummary,
} from "@/lib/query/writeback";
import { useDatasetSlug } from "@/lib/use-dataset-slug";
import { cn } from "@/lib/utils";

const ROLE_RANK = { viewer: 0, steward: 1, admin: 2 } as const;
const STATUSES = [
  { id: "planned", label: "Planned" },
  { id: "approved", label: "Approved" },
  { id: "rejected", label: "Rejected" },
  { id: "superseded", label: "Superseded" },
  { id: "", label: "All" },
] as const;

/** A score on the 0–1 scale, as a percentage with one decimal; "no evidence"
 * when absent — the case an auto-approval rule never approves on. */
export function scoreLabel(score: number | null): string {
  return score === null ? "no evidence" : `${(score * 100).toFixed(1)}%`;
}

/** Who decided: a principal verbatim, or the rule an auto-approval names. */
export function deciderLabel(decidedBy: string | null): string {
  if (decidedBy === null) return "—";
  return decidedBy.startsWith("policy:") ? "auto-approval rule" : decidedBy;
}

function show(value: unknown): string {
  if (value === null || value === undefined) return "—";
  return typeof value === "string" ? value : JSON.stringify(value);
}

export function WritebackContent({ org }: { org: string }) {
  const ds = useDatasetSlug();
  const session = useSession();
  const role = effectiveRole(session.data, org);
  const rank = role === null ? -1 : ROLE_RANK[role];
  const isSteward = rank >= ROLE_RANK.steward;
  const isAdmin = rank >= ROLE_RANK.admin;

  const [status, setStatus] = useState<string>("planned");
  const [selected, setSelected] = useState<string | null>(null);
  const [checked, setChecked] = useState<ReadonlySet<string>>(new Set());
  const [batch, setBatch] = useState<BatchResult | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const previews = useMergePreviews(org, ds, status);
  const items = useMemo(() => previews.data?.items ?? [], [previews.data]);
  const decide = useDecideBatch(org, ds);
  const generate = useGeneratePreviews(org, ds);
  const apply = useApplyApproved(org, ds);

  const decidable = items.filter((p) => p.status === "planned");
  const chosen = decidable.filter((p) => checked.has(p.id));

  function toggle(id: string) {
    setChecked((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }

  function runBatch(decision: "approve" | "reject") {
    decide.mutate(
      { decision, items: chosen.map((p) => ({ id: p.id, fingerprint: p.fingerprint })) },
      {
        onSuccess: (result) => {
          setBatch(result);
          setChecked(new Set());
        },
      },
    );
  }

  return (
    <div
      className="mx-auto flex h-full w-full max-w-6xl min-w-0 flex-col gap-4"
      data-testid="writeback-page"
    >
      <div className="flex flex-wrap items-center gap-3">
        <h1 className="text-xl font-semibold lg:text-2xl">Writeback</h1>
        <div className="flex flex-wrap gap-1" role="tablist" aria-label="Preview status">
          {STATUSES.map((entry) => (
            <button
              key={entry.id || "all"}
              role="tab"
              aria-selected={status === entry.id}
              onClick={() => {
                setStatus(entry.id);
                setChecked(new Set());
              }}
              className={cn(
                "min-h-11 rounded-md px-3 py-1.5 text-sm font-medium",
                status === entry.id
                  ? "bg-accent text-accent-foreground"
                  : "text-muted-foreground hover:bg-accent/50",
              )}
              data-testid={`writeback-tab-${entry.id || "all"}`}
            >
              {entry.label}
            </button>
          ))}
        </div>
        <div className="ml-auto flex gap-2">
          {isSteward ? (
            <Button
              variant="outline"
              disabled={generate.isPending}
              onClick={() => {
                generate.mutate(undefined, {
                  onSuccess: () => {
                    setNotice("Generating previews from the latest results.");
                  },
                });
              }}
            >
              Generate previews
            </Button>
          ) : null}
          {isAdmin ? (
            <Button
              disabled={apply.isPending}
              onClick={() => {
                apply.mutate(undefined, {
                  onSuccess: () => {
                    setNotice("Applying approved previews to Salesforce.");
                  },
                  onError: (error) => {
                    setNotice(error.message);
                  },
                });
              }}
              data-testid="writeback-apply"
            >
              Apply approved
            </Button>
          ) : null}
        </div>
      </div>
      <p className="text-muted-foreground text-sm">
        A preview is a proposed Salesforce merge: one master, its victims, and the master fields the
        golden record would change. Nothing is written until a preview is approved and applied.
      </p>
      {notice ? (
        <p role="status" className="text-sm font-medium" data-testid="writeback-notice">
          {notice}
        </p>
      ) : null}

      {isSteward && chosen.length > 0 ? (
        <div className="flex items-center gap-2" data-testid="writeback-batch-bar">
          <span className="text-sm font-medium tabular-nums">{chosen.length} selected</span>
          <Button
            size="sm"
            disabled={decide.isPending}
            onClick={() => {
              runBatch("approve");
            }}
          >
            Approve {chosen.length}
          </Button>
          <Button
            size="sm"
            variant="outline"
            disabled={decide.isPending}
            onClick={() => {
              runBatch("reject");
            }}
          >
            Reject {chosen.length}
          </Button>
        </div>
      ) : null}
      {batch ? <BatchOutcome result={batch} /> : null}

      <div className="grid min-w-0 gap-4 lg:grid-cols-[minmax(0,3fr)_minmax(0,2fr)]">
        <PreviewTable
          items={items}
          loading={previews.isPending}
          error={previews.error?.message ?? null}
          selected={selected}
          onSelect={setSelected}
          checked={checked}
          onToggle={isSteward ? toggle : null}
        />
        {selected ? (
          <PreviewDetail org={org} ds={ds} id={selected} isAdmin={isAdmin} />
        ) : (
          <Card>
            <CardContent className="text-muted-foreground py-6 text-sm">
              Select a preview to see its master, victims and field changes.
            </CardContent>
          </Card>
        )}
      </div>

      <UpsertPanel org={org} ds={ds} isSteward={isSteward} isAdmin={isAdmin} />
      <AutoApprovalPanel org={org} ds={ds} isAdmin={isAdmin} previews={items} />
    </div>
  );
}

/** Proposed upserts (#998): golden records that came in by upload and are
 * not yet in the Salesforce object, keyed by an External ID that carries the
 * entity id, so applying one twice can never create two records. */
function UpsertPanel({
  org,
  ds,
  isSteward,
  isAdmin,
}: {
  org: string;
  ds: string;
  isSteward: boolean;
  isAdmin: boolean;
}) {
  const upserts = useUpsertPreviews(org, ds);
  const decide = useDecideUpserts(org, ds);
  const apply = useApplyUpserts(org, ds);
  const [checked, setChecked] = useState<ReadonlySet<string>>(new Set());
  const [outcome, setOutcome] = useState<BatchResult | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const items = upserts.data?.items ?? [];
  const chosen = items.filter((u) => u.status === "planned" && checked.has(u.id));

  function run(decision: "approve" | "reject") {
    decide.mutate(
      { decision, items: chosen.map((u) => ({ id: u.id, fingerprint: u.fingerprint })) },
      {
        onSuccess: (result) => {
          setOutcome(result);
          setChecked(new Set());
        },
      },
    );
  }

  return (
    <Card data-testid="writeback-upserts">
      <CardHeader>
        <CardTitle className="text-base">Upserts</CardTitle>
        <p className="text-muted-foreground text-sm">
          Records that arrived by upload and are not yet in Salesforce. Each is keyed by an External
          ID carrying the entity id, so a retry updates the same record and never creates a second.
          A record changed in Salesforce after its proposal is skipped, not overwritten.
        </p>
      </CardHeader>
      <CardContent className="flex flex-col gap-3 text-sm">
        {items.length === 0 ? (
          <p className="text-muted-foreground" data-testid="writeback-upserts-empty">
            No upserts proposed. Set an upsert policy, then generate previews.
          </p>
        ) : (
          <div className="overflow-x-auto rounded-md border">
            <table className="w-full" data-testid="writeback-upserts-table">
              <caption className="sr-only">Proposed upserts</caption>
              <thead className="bg-muted/50 text-left">
                <tr>
                  {isSteward ? (
                    <th scope="col" className="w-10 px-2 py-2">
                      <span className="sr-only">Select</span>
                    </th>
                  ) : null}
                  <th scope="col" className="px-2 py-2 font-medium">
                    Entity
                  </th>
                  <th scope="col" className="px-2 py-2 font-medium">
                    Fields
                  </th>
                  <th scope="col" className="px-2 py-2 font-medium">
                    State
                  </th>
                </tr>
              </thead>
              <tbody>
                {items.map((u) => (
                  <tr key={u.id} className="border-t">
                    {isSteward ? (
                      <td className="px-2 py-1">
                        {u.status === "planned" ? (
                          <input
                            type="checkbox"
                            className="size-5"
                            checked={checked.has(u.id)}
                            onChange={() => {
                              setChecked((prev) => {
                                const next = new Set(prev);
                                if (next.has(u.id)) next.delete(u.id);
                                else next.add(u.id);
                                return next;
                              });
                            }}
                            aria-label={`Select upsert for ${u.entity_id}`}
                          />
                        ) : null}
                      </td>
                    ) : null}
                    <td className="px-2 py-1 font-mono">{u.entity_id}</td>
                    <td className="px-2 py-1">
                      {Object.entries(u.fields)
                        .map(([field, value]) => `${field}: ${show(value)}`)
                        .join(" · ")}
                    </td>
                    <td className="px-2 py-1">
                      {u.apply_status ?? u.status}
                      {u.sf_record_id ? (
                        <span className="text-muted-foreground font-mono"> {u.sf_record_id}</span>
                      ) : null}
                      {u.apply_detail &&
                      u.apply_status !== "created" &&
                      u.apply_status !== "updated" ? (
                        <span className="text-muted-foreground"> — {u.apply_detail}</span>
                      ) : null}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
        <div className="flex flex-wrap items-center gap-2">
          {isSteward && chosen.length > 0 ? (
            <>
              <Button
                size="sm"
                disabled={decide.isPending}
                onClick={() => {
                  run("approve");
                }}
              >
                Approve {chosen.length}
              </Button>
              <Button
                size="sm"
                variant="outline"
                disabled={decide.isPending}
                onClick={() => {
                  run("reject");
                }}
              >
                Reject {chosen.length}
              </Button>
            </>
          ) : null}
          {isAdmin ? (
            <Button
              size="sm"
              variant="outline"
              className="ml-auto"
              disabled={apply.isPending}
              onClick={() => {
                apply.mutate(undefined, {
                  onSuccess: () => {
                    setMessage("Applying approved upserts to Salesforce.");
                  },
                  onError: (error) => {
                    setMessage(error.message);
                  },
                });
              }}
              data-testid="writeback-upserts-apply"
            >
              Apply approved upserts
            </Button>
          ) : null}
        </div>
        {outcome ? <BatchOutcome result={outcome} /> : null}
        {message ? (
          <p role="status" className="font-medium">
            {message}
          </p>
        ) : null}
      </CardContent>
    </Card>
  );
}

function BatchOutcome({ result }: { result: BatchResult }) {
  const ok = result.results.filter((r) => r.status === 200).length;
  const refused = result.results.filter((r) => r.status !== 200);
  return (
    <div role="status" className="text-sm" data-testid="writeback-batch-outcome">
      <p className="font-medium tabular-nums">
        {ok} decided, {refused.length} refused
      </p>
      {refused.length > 0 ? (
        <ul className="text-muted-foreground list-disc pl-5">
          {refused.map((r) => (
            <li key={r.id}>
              <span className="font-mono">{r.id}</span>: {r.detail ?? `HTTP ${String(r.status)}`}
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}

function PreviewTable({
  items,
  loading,
  error,
  selected,
  onSelect,
  checked,
  onToggle,
}: {
  items: MergePreviewSummary[];
  loading: boolean;
  error: string | null;
  selected: string | null;
  onSelect: (id: string) => void;
  checked: ReadonlySet<string>;
  onToggle: ((id: string) => void) | null;
}) {
  if (loading) return <p className="text-muted-foreground text-sm">Loading previews…</p>;
  if (error) return <p className="text-sm font-medium">Previews could not be loaded: {error}</p>;
  if (items.length === 0) {
    return (
      <p className="text-muted-foreground text-sm" data-testid="writeback-empty">
        No previews in this state. Generate previews after a resolution run.
      </p>
    );
  }
  return (
    <div className="overflow-x-auto rounded-md border">
      <table className="w-full text-sm" data-testid="writeback-table">
        <caption className="sr-only">Merge previews</caption>
        <thead className="bg-muted/50 text-left">
          <tr>
            {onToggle ? (
              <th scope="col" className="w-10 px-2 py-2">
                <span className="sr-only">Select</span>
              </th>
            ) : null}
            <th scope="col" className="px-2 py-2 font-medium">
              Master
            </th>
            <th scope="col" className="px-2 py-2 font-medium">
              Object
            </th>
            <th scope="col" className="px-2 py-2 text-right font-medium">
              Members
            </th>
            <th scope="col" className="px-2 py-2 text-right font-medium">
              Score
            </th>
            <th scope="col" className="px-2 py-2 font-medium">
              State
            </th>
          </tr>
        </thead>
        <tbody>
          {items.map((p) => (
            <tr
              key={p.id}
              className={cn("border-t", selected === p.id && "bg-accent")}
              data-testid="writeback-row"
            >
              {onToggle ? (
                <td className="px-2 py-1">
                  {p.status === "planned" ? (
                    <input
                      type="checkbox"
                      className="size-5"
                      checked={checked.has(p.id)}
                      onChange={() => {
                        onToggle(p.id);
                      }}
                      aria-label={`Select preview for ${p.master_record_key}`}
                    />
                  ) : null}
                </td>
              ) : null}
              <td className="px-2 py-1">
                <button
                  className="min-h-11 text-left font-mono underline-offset-2 hover:underline"
                  onClick={() => {
                    onSelect(p.id);
                  }}
                >
                  {p.master_record_key}
                </button>
              </td>
              <td className="px-2 py-1">{p.object}</td>
              <td className="px-2 py-1 text-right tabular-nums">{p.member_count}</td>
              <td
                className={cn(
                  "px-2 py-1 text-right tabular-nums",
                  p.score === null && "text-muted-foreground",
                )}
              >
                {scoreLabel(p.score)}
              </td>
              <td className="px-2 py-1">
                {p.status}
                {p.decided_by?.startsWith("policy:") ? (
                  <span className="text-muted-foreground"> · auto</span>
                ) : null}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function PreviewDetail({
  org,
  ds,
  id,
  isAdmin,
}: {
  org: string;
  ds: string;
  id: string;
  isAdmin: boolean;
}) {
  const detail = useMergePreview(org, ds, id);
  const applied = detail.data?.apply ?? null;
  const restorable =
    applied !== null && (applied.status === "applied" || applied.status === "failed");
  const restore = useRestore(org, ds, id, applied !== null);
  const requestRestore = useRequestRestore(org, ds);

  if (detail.isPending) return <p className="text-muted-foreground text-sm">Loading preview…</p>;
  if (detail.error) return <p className="text-sm font-medium">{detail.error.message}</p>;
  const d = detail.data;
  const members = d.proposal.members ?? [];
  const master = d.proposal.master_record_key ?? d.summary.master_record_key;
  const changes = Object.entries(d.field_changes);

  return (
    <Card data-testid="writeback-detail">
      <CardHeader>
        <CardTitle className="text-base">
          {d.summary.object} merge into <span className="font-mono">{master}</span>
        </CardTitle>
        <p className="text-muted-foreground text-sm">
          Score {scoreLabel(d.summary.score)} · {d.summary.member_count} members · decided by{" "}
          {deciderLabel(d.decided_by)}
          {d.decision_reason ? ` (${d.decision_reason})` : ""}
        </p>
      </CardHeader>
      <CardContent className="flex flex-col gap-4 text-sm">
        <section aria-label="Field changes">
          <h2 className="mb-1 font-medium">Master field changes</h2>
          {changes.length === 0 ? (
            <p className="text-muted-foreground">No field changes; the merge only.</p>
          ) : (
            <table className="w-full">
              <thead className="text-muted-foreground text-left">
                <tr>
                  <th scope="col" className="py-1 font-medium">
                    Field
                  </th>
                  <th scope="col" className="py-1 font-medium">
                    Now
                  </th>
                  <th scope="col" className="py-1 font-medium">
                    After
                  </th>
                </tr>
              </thead>
              <tbody>
                {changes.map(([field, change]) => (
                  <tr key={field} className="border-t">
                    <td className="py-1 font-mono">{field}</td>
                    <td className="text-muted-foreground py-1">{show(change.before)}</td>
                    <td className="py-1 font-semibold">{show(change.after)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </section>
        <section aria-label="Members">
          <h2 className="mb-1 font-medium">Members</h2>
          <ul className="flex flex-col gap-1">
            {members.map((m) => (
              <li key={m.record_key} className="flex flex-wrap gap-x-2">
                <span className={cn("font-mono", m.record_key === master && "font-semibold")}>
                  {m.record_key}
                </span>
                <span className="text-muted-foreground font-mono">{m.external_id}</span>
                <span className="text-muted-foreground">
                  {m.record_key === master ? "master" : "merged into the master"}
                </span>
              </li>
            ))}
          </ul>
          <p className="text-muted-foreground mt-1">
            {d.related_counts_state === "known"
              ? "Related-record counts are known."
              : "Related-record counts are unavailable; Salesforce reparents them to the master."}
          </p>
        </section>
        {applied ? (
          <section aria-label="Apply" data-testid="writeback-apply-state">
            <h2 className="mb-1 font-medium">Apply</h2>
            <p>
              <span className="font-semibold">{applied.status.replace("_", " ")}</span>
              {applied.detail ? ` — ${applied.detail}` : ""}
            </p>
          </section>
        ) : null}
        {restore.data ? (
          <section aria-label="Undo" data-testid="writeback-restore">
            <h2 className="mb-1 font-medium">Undo</h2>
            <p>
              <span className="font-semibold">{restore.data.status}</span>
              {restore.data.detail ? ` — ${restore.data.detail}` : ""}
            </p>
            {restore.data.conflicts.length > 0 ? (
              <ul className="list-disc pl-5">
                {restore.data.conflicts.map((c, index) => (
                  <li key={`${c.record ?? ""}-${c.field ?? ""}-${String(index)}`}>
                    {c.field ? <span className="font-mono">{c.field}</span> : null} {c.reason}
                  </li>
                ))}
              </ul>
            ) : null}
            <p className="text-muted-foreground">
              Records the merge reparented to the master are not moved back.
            </p>
          </section>
        ) : null}
        {isAdmin && restorable && !restore.data ? (
          <Button
            variant="outline"
            disabled={requestRestore.isPending}
            onClick={() => {
              requestRestore.mutate(id);
            }}
            data-testid="writeback-undo"
          >
            Undo this merge
          </Button>
        ) : null}
        <section aria-label="History">
          <h2 className="mb-1 font-medium">History</h2>
          <ol className="flex flex-col gap-0.5">
            {d.history.map((h) => (
              <li key={`${h.at}-${h.to_status}`} className="flex flex-wrap gap-x-2">
                <span className="font-medium">{h.to_status}</span>
                <span className="text-muted-foreground">{deciderLabel(h.actor)}</span>
                <time className="text-muted-foreground tabular-nums" dateTime={h.at}>
                  {new Date(h.at).toLocaleString()}
                </time>
              </li>
            ))}
          </ol>
        </section>
      </CardContent>
    </Card>
  );
}

function AutoApprovalPanel({
  org,
  ds,
  isAdmin,
  previews,
}: {
  org: string;
  ds: string;
  isAdmin: boolean;
  previews: MergePreviewSummary[];
}) {
  const rules = useAutoApproval(org, ds);
  const save = useSetAutoApproval(org, ds);
  const connections = useMemo(
    () =>
      [
        ...new Set([
          ...previews.map((p) => p.connection_id),
          ...(rules.data?.items ?? []).map((r) => r.connection_id),
        ]),
      ].sort(),
    [previews, rules.data],
  );
  const [connection, setConnection] = useState("");
  const [object, setObject] = useState("Contact");
  const [minScore, setMinScore] = useState("0.95");
  const [maxMembers, setMaxMembers] = useState("2");
  const [enabled, setEnabled] = useState(true);
  const [message, setMessage] = useState<string | null>(null);

  return (
    <Card data-testid="writeback-auto-approval">
      <CardHeader>
        <CardTitle className="text-base">Auto-approval</CardTitle>
        <p className="text-muted-foreground text-sm">
          A rule approves a preview as it is generated when its score is at least the minimum and
          its group is no larger than the maximum. Changing a rule never changes a preview already
          decided, and a preview with no score evidence is never auto-approved.
        </p>
      </CardHeader>
      <CardContent className="flex flex-col gap-3 text-sm">
        {(rules.data?.items ?? []).length === 0 ? (
          <p className="text-muted-foreground">No rules: every preview waits for a reviewer.</p>
        ) : (
          <ul className="flex flex-col gap-1">
            {(rules.data?.items ?? []).map((r) => (
              <li key={r.id}>
                <span className="font-mono">{r.connection_id}</span> · {r.object} ·{" "}
                {r.enabled ? (
                  <span className="font-semibold">
                    score ≥ {scoreLabel(r.min_score)}, ≤ {r.max_members} members
                  </span>
                ) : (
                  <span className="text-muted-foreground">off</span>
                )}{" "}
                <span className="text-muted-foreground">by {r.created_by}</span>
              </li>
            ))}
          </ul>
        )}
        {isAdmin ? (
          <form
            className="grid gap-3 sm:grid-cols-5 sm:items-end"
            onSubmit={(event) => {
              event.preventDefault();
              save.mutate(
                {
                  connection_id: connection || connections[0] || "",
                  object,
                  enabled,
                  min_score: Number(minScore),
                  max_members: Number(maxMembers),
                },
                {
                  onSuccess: () => {
                    setMessage("Rule saved.");
                  },
                  onError: (error) => {
                    setMessage(error.message);
                  },
                },
              );
            }}
          >
            <div className="flex flex-col gap-1">
              <Label htmlFor="aa-connection">Connection</Label>
              <Input
                id="aa-connection"
                list="aa-connections"
                value={connection || connections[0] || ""}
                onChange={(event) => {
                  setConnection(event.target.value);
                }}
              />
              <datalist id="aa-connections">
                {connections.map((c) => (
                  <option key={c} value={c} />
                ))}
              </datalist>
            </div>
            <div className="flex flex-col gap-1">
              <Label htmlFor="aa-object">Object</Label>
              <select
                id="aa-object"
                className="border-input bg-background min-h-11 rounded-md border px-2"
                value={object}
                onChange={(event) => {
                  setObject(event.target.value);
                }}
              >
                <option>Lead</option>
                <option>Contact</option>
                <option>Account</option>
              </select>
            </div>
            <div className="flex flex-col gap-1">
              <Label htmlFor="aa-min">Minimum score (0–1)</Label>
              <Input
                id="aa-min"
                inputMode="decimal"
                value={minScore}
                onChange={(event) => {
                  setMinScore(event.target.value);
                }}
              />
            </div>
            <div className="flex flex-col gap-1">
              <Label htmlFor="aa-max">Maximum members</Label>
              <Input
                id="aa-max"
                inputMode="numeric"
                value={maxMembers}
                onChange={(event) => {
                  setMaxMembers(event.target.value);
                }}
              />
            </div>
            <div className="flex items-center gap-2">
              <input
                id="aa-enabled"
                type="checkbox"
                className="size-5"
                checked={enabled}
                onChange={(event) => {
                  setEnabled(event.target.checked);
                }}
              />
              <Label htmlFor="aa-enabled">On</Label>
              <Button type="submit" size="sm" disabled={save.isPending} className="ml-auto">
                Save rule
              </Button>
            </div>
          </form>
        ) : null}
        {message ? (
          <p role="status" className="font-medium">
            {message}
          </p>
        ) : null}
      </CardContent>
    </Card>
  );
}
