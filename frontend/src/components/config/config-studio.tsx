"use client";

import { useEffect, useState } from "react";
import Link from "next/link";
import { History, Lock, Rocket } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { BffRequestError } from "@/lib/api/client";
import { applyEdits, diffLines, parseConfig, type Thresholds } from "@/lib/domain/config";
import {
  useActiveConfig,
  useConfigVersion,
  useConfigVersions,
  useCreateDraft,
  usePublish,
  useScoreSample,
} from "@/lib/query/config";
import { effectiveRole, useSession } from "@/lib/query/hooks";
import { cn } from "@/lib/utils";
import { SurvivorshipEditor } from "./survivorship-editor";
import { ThresholdEditor } from "./threshold-editor";

function SectionTitle({ children }: { children: React.ReactNode }) {
  return (
    <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
      {children}
    </CardTitle>
  );
}

function VersionHistory({ org }: { org: string }) {
  const versions = useConfigVersions(org);
  const [compare, setCompare] = useState<[number, number] | null>(null);
  const before = useConfigVersion(org, compare ? compare[0] : null);
  const after = useConfigVersion(org, compare ? compare[1] : null);

  return (
    <Card data-testid="version-history">
      <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
        <SectionTitle>Version history</SectionTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-2 p-4 pt-2 lg:p-6 lg:pt-2">
        {(versions.data ?? []).map((row, index, all) => (
          <div key={row.version} className="flex flex-wrap items-center gap-2 text-sm">
            <History className="text-muted-foreground size-4" />
            <span className="font-medium">v{row.version}</span>
            <span
              className={cn(
                "rounded-full px-2 py-0.5 text-xs",
                row.state === "published"
                  ? "bg-emerald-500/15 text-emerald-700"
                  : "bg-muted text-muted-foreground",
              )}
            >
              {row.state}
              {row.tier ? ` · tier ${row.tier}` : ""}
            </span>
            <span className="text-muted-foreground max-w-56 truncate text-xs">
              {row.created_by}
            </span>
            <span className="text-muted-foreground text-xs">{row.created_at}</span>
            {index < all.length - 1 && (
              <button
                type="button"
                className="text-muted-foreground text-xs underline-offset-2 hover:underline"
                data-testid={`diff-${String(row.version)}`}
                onClick={() => {
                  const previous = all[index + 1];
                  if (previous) setCompare([previous.version, row.version]);
                }}
              >
                diff vs v{all[index + 1]?.version}
              </button>
            )}
          </div>
        ))}

        {compare && before.data && after.data && (
          <div className="mt-2 rounded-md border" data-testid="version-diff">
            <div className="text-muted-foreground flex items-center justify-between border-b px-3 py-1.5 text-xs">
              <span>
                v{compare[0]} → v{compare[1]}
              </span>
              <button
                type="button"
                className="underline-offset-2 hover:underline"
                onClick={() => {
                  setCompare(null);
                }}
              >
                close
              </button>
            </div>
            <pre className="max-h-72 overflow-auto p-3 font-mono text-xs">
              {diffLines(before.data.yaml, after.data.yaml)
                .filter((line, _, all) => all.some((l) => l.kind !== "same"))
                .map((line, index) => (
                  <div
                    key={index}
                    className={cn(
                      line.kind === "added" &&
                        "bg-emerald-500/15 text-emerald-700 dark:text-emerald-400",
                      line.kind === "removed" && "bg-red-500/15 text-red-700 dark:text-red-400",
                    )}
                  >
                    {line.kind === "added" ? "+ " : line.kind === "removed" ? "- " : "  "}
                    {line.text}
                  </div>
                ))}
            </pre>
          </div>
        )}
      </CardContent>
    </Card>
  );
}

export function ConfigStudio({ org }: { org: string }) {
  const active = useActiveConfig(org);
  const scores = useScoreSample(org);
  const session = useSession();
  const createDraft = useCreateDraft(org);
  const publish = usePublish(org);
  const isAdmin = effectiveRole(session.data, org) === "admin";

  const [thresholds, setThresholds] = useState<Thresholds | null>(null);
  const [survivorship, setSurvivorship] = useState<Record<string, string[]> | null>(null);
  const [publishError, setPublishError] = useState<string | null>(null);
  const [published, setPublished] = useState<{
    version: number;
    tier: string | null;
    jobs: string[];
  } | null>(null);

  // Seed the edit state from the active config exactly once per load.
  useEffect(() => {
    if (active.data && survivorship === null) {
      const view = parseConfig(active.data.yaml);
      if (view.thresholds) setThresholds(view.thresholds);
      setSurvivorship(view.survivorship);
    }
  }, [active.data, survivorship]);

  if (active.error instanceof BffRequestError && active.error.status === 404) {
    return (
      <p className="text-muted-foreground py-16 text-center text-sm">
        No published config yet — provisioning seeds v1.
      </p>
    );
  }
  if (active.isPending || !active.data || survivorship === null) {
    return <Skeleton className="h-96 w-full" />;
  }

  const view = parseConfig(active.data.yaml);
  const dirty =
    JSON.stringify({ t: view.thresholds, s: view.survivorship }) !==
    JSON.stringify({ t: thresholds, s: survivorship });

  async function publishEdits() {
    setPublishError(null);
    setPublished(null);
    if (!active.data || !survivorship) return;
    const yaml = applyEdits(active.data.yaml, {
      ...(thresholds ? { thresholds } : {}),
      survivorship,
    });
    try {
      const draft = await createDraft.mutateAsync(yaml);
      const result = await publish.mutateAsync(draft.version);
      setPublished({ version: result.version, tier: result.tier, jobs: result.jobs_enqueued });
    } catch (error) {
      setPublishError(
        error instanceof BffRequestError
          ? `${error.message}${error.error.pointer ? ` (at ${error.error.pointer})` : ""}`
          : "publish failed",
      );
    }
  }

  return (
    <div
      className="mx-auto flex w-full max-w-4xl min-w-0 flex-col gap-4 lg:gap-6"
      data-testid="config-studio"
    >
      <div className="flex flex-wrap items-center gap-3">
        <h1 className="text-xl font-semibold lg:text-2xl">Config studio</h1>
        <span className="text-muted-foreground text-xs">
          active v{active.data.version} · tenant {view.tenant}
        </span>
        <span className="flex-1" />
        {isAdmin && (
          <Button
            disabled={!dirty || createDraft.isPending || publish.isPending}
            onClick={() => void publishEdits()}
            className="h-auto min-h-11 w-full whitespace-normal sm:w-auto lg:min-h-9"
            data-testid="publish-button"
          >
            <Rocket />
            {createDraft.isPending || publish.isPending
              ? "Publishing…"
              : "Publish changes · tier A (re-band + re-assemble)"}
          </Button>
        )}
      </div>

      {publishError && (
        <p className="text-destructive text-sm" role="alert" data-testid="publish-error">
          {publishError}
        </p>
      )}
      {published && (
        <Card className="border-emerald-500/40" data-testid="publish-result">
          <CardContent className="flex flex-wrap items-center gap-2 p-3 text-sm">
            <Rocket className="size-4 text-emerald-600" />v{published.version} published (tier{" "}
            {published.tier ?? "—"}) — {published.jobs.length} job
            {published.jobs.length === 1 ? "" : "s"} enqueued.
            <Link href={`/${org}/runs`} className="underline underline-offset-2">
              watch in Runs
            </Link>
          </CardContent>
        </Card>
      )}

      {thresholds && (
        <Card data-testid="thresholds-card">
          <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
            <SectionTitle>Thresholds — the gray band is your review queue</SectionTitle>
          </CardHeader>
          <CardContent className="p-4 pt-2 lg:p-6 lg:pt-2">
            <ThresholdEditor
              probabilities={scores.data ?? []}
              value={thresholds}
              onChange={setThresholds}
              disabled={!isAdmin}
            />
          </CardContent>
        </Card>
      )}

      <Card data-testid="survivorship-card">
        <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
          <SectionTitle>Survivorship — which value wins each golden field</SectionTitle>
        </CardHeader>
        <CardContent className="p-4 pt-2 lg:p-6 lg:pt-2">
          <SurvivorshipEditor
            chains={survivorship}
            onChange={(attribute, chain) => {
              setSurvivorship((current) => ({ ...(current ?? {}), [attribute]: chain }));
            }}
            disabled={!isAdmin}
          />
        </CardContent>
      </Card>

      <div className="grid gap-4 lg:grid-cols-2">
        <Card data-testid="blocking-card">
          <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
            <SectionTitle>
              Blocking · tier B <Lock className="ml-1 inline size-3" />
            </SectionTitle>
          </CardHeader>
          <CardContent className="flex flex-col gap-1.5 p-4 pt-2 text-sm lg:p-6 lg:pt-2">
            {view.blocking.map((rule) => (
              <div key={rule.key_type} className="flex items-baseline gap-2">
                <span className="font-medium">{rule.key_type}</span>
                <code className="text-muted-foreground truncate text-xs">{rule.expr}</code>
              </div>
            ))}
            <p className="text-muted-foreground mt-1 text-xs">
              Changes matching — costs a match rebuild. Structured editing arrives with the M5
              hardening pass; read-only here so nothing is a dead end.
            </p>
          </CardContent>
        </Card>

        <Card data-testid="comparisons-card">
          <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
            <SectionTitle>
              Comparisons · tier C <Lock className="ml-1 inline size-3" />
            </SectionTitle>
          </CardHeader>
          <CardContent className="flex flex-col gap-1.5 p-4 pt-2 text-sm lg:p-6 lg:pt-2">
            {Object.entries(view.comparisons).map(([column, spec]) => (
              <div key={column} className="flex flex-wrap items-baseline gap-2">
                <span className="font-medium">{column}</span>
                <span className="text-muted-foreground text-xs">
                  {spec.levels.join(" → ")}
                  {spec.tf && " · tf"}
                </span>
              </div>
            ))}
            <p className="text-muted-foreground mt-1 text-xs">
              Changes scoring — costs a retrain and full rebuild.
            </p>
          </CardContent>
        </Card>
      </div>

      <VersionHistory org={org} />
    </div>
  );
}
