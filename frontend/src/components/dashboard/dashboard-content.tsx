"use client";

import Link from "next/link";
import { AlertTriangle, CheckCircle2, Circle, Inbox, Loader2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { BffRequestError } from "@/lib/api/client";
import { useJobs, useMetrics, useRuns, type JobRow, type RunRow } from "@/lib/query/hooks";
import { useSubmitJob } from "@/lib/query/steward";

const ACTIVE_STATES = new Set(["queued", "dispatching", "running", "retrying", "canceling"]);

/** The fresh-tenant path (design §5.5's precondition, made a checklist instead
 * of a surprise): deliver files → train → resolve. Renders only while the
 * corpus is empty, and reads progress from real job history. */
function OnboardingChecklist({ org, jobs }: { org: string; jobs: JobRow[] }) {
  const submit = useSubmitJob(org);
  const delivered = jobs.some((job) => job.kind === "run_all_incremental");
  const trained = jobs.some((job) => job.kind === "train" && job.state === "succeeded");
  const preconditionFailures = jobs.filter((job) => job.error_class === "precondition").length;

  const steps = [
    {
      done: delivered,
      label: "Deliver your first files",
      body: (
        <>
          Drop a CSV on each source in{" "}
          <Link href={`/${org}/sources`} className="underline underline-offset-2">
            Sources
          </Link>
          . First deliveries land, then matching waits for a model — that&apos;s the plan, not a
          failure.
        </>
      ),
    },
    {
      done: trained,
      label: "Train the matching model",
      body: (
        <span className="inline-flex flex-wrap items-center gap-2">
          One-time, a couple of minutes on a fresh corpus.
          <Button
            size="sm"
            variant="outline"
            disabled={!delivered || trained || submit.isPending}
            data-testid="onboarding-train"
            onClick={() => {
              submit.mutate({ kind: "train" });
            }}
          >
            Train now
          </Button>
        </span>
      ),
    },
    {
      done: false,
      label: "Run the first resolution",
      body: (
        <span className="inline-flex flex-wrap items-center gap-2">
          Builds your golden records end to end.
          <Button
            size="sm"
            variant="outline"
            disabled={!trained || submit.isPending}
            data-testid="onboarding-run"
            onClick={() => {
              submit.mutate({ kind: "run_all_full" });
            }}
          >
            Run full resolution
          </Button>
        </span>
      ),
    },
  ];

  return (
    <Card className="border-primary/40" data-testid="onboarding-checklist">
      <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
        <CardTitle className="text-base">Three steps to your first golden records</CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-3 p-4 pt-2 text-sm lg:p-6 lg:pt-2">
        {steps.map((step) => (
          <div key={step.label} className="flex items-start gap-2">
            {step.done ? (
              <CheckCircle2 className="mt-0.5 size-4 shrink-0 text-emerald-600" />
            ) : (
              <Circle className="text-muted-foreground mt-0.5 size-4 shrink-0" />
            )}
            <div>
              <span className="font-medium">{step.label}</span>
              <p className="text-muted-foreground mt-0.5">{step.body}</p>
            </div>
          </div>
        ))}
        {preconditionFailures > 0 && !trained && (
          <p
            className="text-muted-foreground border-t pt-2 text-xs"
            data-testid="precondition-note"
          >
            {preconditionFailures} import{preconditionFailures === 1 ? "" : "s"} delivered and
            waiting on the model — they were not lost.
          </p>
        )}
      </CardContent>
    </Card>
  );
}

function Tile({
  label,
  value,
  testId,
}: {
  label: string;
  value: number | undefined;
  testId: string;
}) {
  return (
    <Card data-testid={testId}>
      <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
        <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
          {label}
        </CardTitle>
      </CardHeader>
      <CardContent className="p-4 pt-0 lg:p-6 lg:pt-0">
        {value === undefined ? (
          <Skeleton className="h-8 w-20" />
        ) : (
          <div className="text-2xl font-semibold tabular-nums lg:text-3xl">
            {value.toLocaleString()}
          </div>
        )}
      </CardContent>
    </Card>
  );
}

function describeRun(run: RunRow): string {
  const mode = typeof run.mode === "string" ? run.mode : "run";
  const status = typeof run.status === "string" ? run.status : "unknown";
  const started = typeof run.started_at === "string" ? run.started_at : undefined;
  const finished = typeof run.ended_at === "string" ? run.ended_at : undefined;
  let duration = "";
  if (started && finished) {
    const seconds = Math.round((Date.parse(finished) - Date.parse(started)) / 1000);
    if (Number.isFinite(seconds) && seconds >= 0) duration = ` in ${String(seconds)}s`;
  }
  return `${mode} — ${status}${duration}`;
}

function nudges(reviewCount: number | undefined, jobs: JobRow[] | undefined): string[] {
  const items: string[] = [];
  if (reviewCount !== undefined && reviewCount > 0) {
    items.push(
      reviewCount >= 500
        ? "500+ reviews waiting"
        : `${String(reviewCount)} review${reviewCount === 1 ? "" : "s"} waiting`,
    );
  }
  const lastFinished = (jobs ?? []).find((job) =>
    ["failed", "succeeded", "canceled"].includes(job.state),
  );
  if (lastFinished?.state === "failed") {
    items.push(`last job failed (${lastFinished.error_class ?? "unknown"}) — see Runs`);
  }
  return items;
}

export function DashboardContent({ org }: { org: string }) {
  const metrics = useMetrics(org);
  const runs = useRuns(org);
  const jobs = useJobs(org);
  const reviewCount = metrics.data?.open_reviews;

  if (metrics.error instanceof BffRequestError && metrics.error.error.code === "lake_unavailable") {
    return (
      <div className="flex min-h-[50dvh] flex-col items-center justify-center gap-3 text-center">
        <AlertTriangle className="text-muted-foreground size-8" />
        <p className="font-medium">This workspace&apos;s data lake is unreachable right now.</p>
        <p className="text-muted-foreground text-sm">
          The control plane is up, but reads against the lake failed. Retrying automatically.
        </p>
      </div>
    );
  }

  const activeJob = (jobs.data ?? []).find((job) => ACTIVE_STATES.has(job.state));
  const lastRun = runs.data?.[0];
  const attention = nudges(reviewCount, jobs.data);

  return (
    <div
      className="mx-auto flex w-full max-w-6xl min-w-0 flex-col gap-4 lg:gap-6"
      data-testid="dashboard"
    >
      <div className="flex items-baseline justify-between">
        <h1 className="text-xl font-semibold lg:text-2xl">Dashboard</h1>
        {metrics.data?.snapshot !== undefined && (
          <span className="text-muted-foreground text-xs" data-testid="snapshot-indicator">
            snapshot {metrics.data.snapshot}
          </span>
        )}
      </div>

      <div className="grid grid-cols-2 gap-3 lg:grid-cols-5 lg:gap-4">
        <Tile label="Records" value={metrics.data?.records} testId="tile-records" />
        <Tile label="Entities" value={metrics.data?.entities} testId="tile-entities" />
        <Tile
          label="Duplicate groups"
          value={metrics.data?.duplicate_groups}
          testId="tile-duplicate-groups"
        />
        <Tile
          label="Records in groups"
          value={metrics.data?.records_in_duplicate_groups}
          testId="tile-records-in-groups"
        />
        <Tile label="Open reviews" value={reviewCount} testId="tile-open-reviews" />
      </div>

      {metrics.data?.records === 0 && <OnboardingChecklist org={org} jobs={jobs.data ?? []} />}

      {activeJob && (
        <Card data-testid="active-job-strip">
          <CardContent className="flex items-center gap-3 p-4 text-sm">
            <Loader2 className="size-4 animate-spin" />
            <span className="font-medium">{activeJob.kind}</span>
            <span className="text-muted-foreground">
              {activeJob.state}
              {activeJob.progress.stages?.length
                ? ` — ${activeJob.progress.stages[activeJob.progress.stages.length - 1]?.stage ?? ""}`
                : ""}
            </span>
          </CardContent>
        </Card>
      )}

      <Card data-testid="last-run-strip">
        <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
          <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
            Last run
          </CardTitle>
        </CardHeader>
        <CardContent className="p-4 pt-0 text-sm lg:p-6 lg:pt-0">
          {runs.isPending ? (
            <Skeleton className="h-5 w-64" />
          ) : lastRun ? (
            <span>{describeRun(lastRun)}</span>
          ) : (
            <span className="text-muted-foreground">
              No runs yet — import data and train a model to get started.
            </span>
          )}
        </CardContent>
      </Card>

      {attention.length > 0 && (
        <Card data-testid="attention-nudges">
          <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
            <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
              Needs attention
            </CardTitle>
          </CardHeader>
          <CardContent className="flex flex-col gap-2 p-4 pt-0 text-sm lg:p-6 lg:pt-0">
            {attention.map((item) => (
              <div key={item} className="flex items-center gap-2">
                <Inbox className="text-muted-foreground size-4" />
                {item}
              </div>
            ))}
          </CardContent>
        </Card>
      )}
    </div>
  );
}
