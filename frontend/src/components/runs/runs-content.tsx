"use client";

import Link from "next/link";
import { Loader2 } from "lucide-react";

import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { useJobs, useRuns, type JobRow, type RunRow } from "@/lib/query/hooks";
import { cn } from "@/lib/utils";

const ACTIVE = new Set(["queued", "dispatching", "running", "retrying", "canceling"]);

export function StateChip({ state }: { state: string }) {
  return (
    <span
      className={cn(
        "inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-xs font-medium",
        state === "succeeded" && "bg-secondary text-secondary-foreground",
        state === "failed" && "bg-destructive/10 text-destructive",
        ACTIVE.has(state) && "bg-primary/10 text-primary",
        state === "canceled" && "bg-muted text-muted-foreground",
      )}
    >
      {ACTIVE.has(state) && <Loader2 className="size-3 animate-spin" />}
      {state}
    </span>
  );
}

function duration(run: RunRow): string {
  if (typeof run.started_at !== "string" || typeof run.ended_at !== "string") return "—";
  const seconds = Math.round((Date.parse(run.ended_at) - Date.parse(run.started_at)) / 1000);
  return Number.isFinite(seconds) && seconds >= 0 ? `${String(seconds)}s` : "—";
}

function lastStage(job: JobRow): string {
  const stages = job.progress.stages;
  return stages?.length ? (stages[stages.length - 1]?.stage ?? "") : "";
}

/** Runs & jobs monitor (design §5.4), read-only in M2 plus cancel/resume on detail. */
export function RunsContent({ org }: { org: string }) {
  const jobs = useJobs(org);
  const runs = useRuns(org);
  const activeJob = (jobs.data ?? []).find((job) => ACTIVE.has(job.state));

  return (
    <div className="mx-auto flex max-w-6xl flex-col gap-4 lg:gap-6" data-testid="runs-page">
      <h1 className="text-xl font-semibold lg:text-2xl">Runs</h1>

      {activeJob && (
        <Link href={`/${org}/runs/${activeJob.job_id}`} data-testid="active-job-link">
          <Card className="border-primary/40">
            <CardContent className="flex items-center gap-3 p-4 text-sm">
              <Loader2 className="size-4 animate-spin" />
              <span className="font-medium">{activeJob.kind}</span>
              <StateChip state={activeJob.state} />
              <span className="text-muted-foreground">{lastStage(activeJob)}</span>
              <span className="text-muted-foreground ml-auto text-xs">watch →</span>
            </CardContent>
          </Card>
        </Link>
      )}

      <Card data-testid="jobs-table">
        <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
          <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
            Jobs
          </CardTitle>
        </CardHeader>
        <CardContent className="overflow-x-auto p-4 pt-2 lg:p-6 lg:pt-2">
          {jobs.isPending ? (
            <Skeleton className="h-32 w-full" />
          ) : (jobs.data ?? []).length === 0 ? (
            <p className="text-muted-foreground text-sm">No jobs yet.</p>
          ) : (
            <table className="w-full min-w-[36rem] text-sm">
              <thead>
                <tr className="text-muted-foreground border-b text-left text-xs uppercase">
                  <th className="py-1.5 pr-4 font-medium">Kind</th>
                  <th className="py-1.5 pr-4 font-medium">State</th>
                  <th className="py-1.5 pr-4 font-medium">Attempt</th>
                  <th className="py-1.5 pr-4 font-medium">Failure</th>
                  <th className="py-1.5 font-medium">Job</th>
                </tr>
              </thead>
              <tbody>
                {(jobs.data ?? []).map((job) => (
                  <tr key={job.job_id} className="border-b last:border-0">
                    <td className="py-2 pr-4 font-medium">{job.kind}</td>
                    <td className="py-2 pr-4">
                      <StateChip state={job.state} />
                    </td>
                    <td className="text-muted-foreground py-2 pr-4">
                      {job.attempt}/{(job as { max_attempts?: number }).max_attempts ?? "—"}
                    </td>
                    <td className="text-muted-foreground py-2 pr-4 text-xs">
                      {job.error_class ?? "—"}
                    </td>
                    <td className="py-2">
                      <Link
                        href={`/${org}/runs/${job.job_id}`}
                        className="font-mono text-xs underline-offset-2 hover:underline"
                      >
                        {job.job_id.slice(-8)}
                      </Link>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </CardContent>
      </Card>

      <Card data-testid="runs-table">
        <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
          <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
            Run history (lake)
          </CardTitle>
        </CardHeader>
        <CardContent className="overflow-x-auto p-4 pt-2 lg:p-6 lg:pt-2">
          {runs.isPending ? (
            <Skeleton className="h-24 w-full" />
          ) : (runs.data ?? []).length === 0 ? (
            <p className="text-muted-foreground text-sm">
              No runs yet — import data and train a model to get started.
            </p>
          ) : (
            <table className="w-full min-w-[36rem] text-sm">
              <thead>
                <tr className="text-muted-foreground border-b text-left text-xs uppercase">
                  <th className="py-1.5 pr-4 font-medium">Mode</th>
                  <th className="py-1.5 pr-4 font-medium">Status</th>
                  <th className="py-1.5 pr-4 font-medium">Duration</th>
                  <th className="py-1.5 pr-4 font-medium">Model</th>
                  <th className="py-1.5 font-medium">Reason</th>
                </tr>
              </thead>
              <tbody>
                {(runs.data ?? []).map((run) => (
                  <tr key={run.run_id} className="border-b last:border-0">
                    <td className="py-2 pr-4 font-medium">{run.mode ?? "—"}</td>
                    <td className="py-2 pr-4">
                      <StateChip state={run.status ?? "unknown"} />
                    </td>
                    <td className="text-muted-foreground py-2 pr-4">{duration(run)}</td>
                    <td className="text-muted-foreground py-2 pr-4 font-mono text-xs">
                      {run.model_version ?? "—"}
                    </td>
                    <td className="text-muted-foreground py-2 text-xs">
                      {run.rebuild_reason ?? "—"}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
