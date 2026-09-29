"use client";

import Link from "next/link";
import { ArrowLeft, CheckCircle2, Loader2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { effectiveRole, useJob, useJobAction, useSession } from "@/lib/query/hooks";
import { StateChip } from "./runs-content";

const ACTIVE = new Set(["queued", "dispatching", "running", "retrying", "canceling"]);
const ROLE_RANK = { viewer: 0, steward: 1, admin: 2 } as const;

/** Job kinds whose success means the corpus re-resolved — duplicates are fresh. */
const RESOLVING_KINDS = new Set(["run_all_full", "run_all_incremental"]);

export function JobDetail({ org, jobId }: { org: string; jobId: string }) {
  const job = useJob(org, jobId);
  const session = useSession();
  const cancel = useJobAction(org, jobId, "cancel");
  const resume = useJobAction(org, jobId, "resume");

  if (job.isPending || !job.data) {
    return (
      <div className="mx-auto flex w-full max-w-3xl min-w-0 flex-col gap-4">
        <Skeleton className="h-8 w-56" />
        <Skeleton className="h-48 w-full" />
      </div>
    );
  }
  const data = job.data;
  const stages = data.progress.stages ?? [];
  const role = effectiveRole(session.data, org);
  const isSteward = role !== null && ROLE_RANK[role] >= ROLE_RANK.steward;

  return (
    <div
      className="mx-auto flex w-full max-w-3xl min-w-0 flex-col gap-4 lg:gap-6"
      data-testid="job-detail"
    >
      <div>
        <Link
          href={`/${org}/runs`}
          className="text-muted-foreground mb-2 inline-flex items-center gap-1 text-sm hover:underline"
        >
          <ArrowLeft className="size-3.5" /> Runs
        </Link>
        <div className="flex flex-wrap items-center gap-3">
          <h1 className="text-xl font-semibold lg:text-2xl">{data.kind}</h1>
          <StateChip state={data.state} />
          <span className="text-muted-foreground font-mono text-xs">{jobId}</span>
          {(data as { created_by?: string | null }).created_by && (
            <span className="text-muted-foreground text-xs" data-testid="job-attribution">
              by{" "}
              {(data as { schedule_id?: string | null }).schedule_id
                ? "schedule"
                : ((data as { created_by?: string }).created_by ?? "")
                    .replace(/^user:/, "")
                    .replace(/ via key:.*$/, "")}
            </span>
          )}
          <span className="flex-1" />
          {isSteward && ACTIVE.has(data.state) && data.state !== "canceling" && (
            <Button
              variant="outline"
              size="sm"
              onClick={() => {
                cancel.mutate();
              }}
              disabled={cancel.isPending}
              data-testid="job-cancel"
            >
              Cancel
            </Button>
          )}
          {isSteward && (data.state === "failed" || data.state === "canceled") && (
            <Button
              variant="outline"
              size="sm"
              onClick={() => {
                resume.mutate();
              }}
              disabled={resume.isPending}
              data-testid="job-resume"
            >
              Resume
            </Button>
          )}
        </div>
      </div>

      <Card data-testid="job-stages">
        <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
          <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
            Stages
          </CardTitle>
        </CardHeader>
        <CardContent className="p-4 pt-2 lg:p-6 lg:pt-2">
          {stages.length === 0 ? (
            <p className="text-muted-foreground text-sm">
              {ACTIVE.has(data.state)
                ? "Waiting for the first stage heartbeat…"
                : "No stage records."}
            </p>
          ) : (
            <ol className="flex flex-col gap-2">
              {stages.map((stage, index) => {
                const isLast = index === stages.length - 1;
                const running = isLast && ACTIVE.has(data.state);
                return (
                  <li
                    key={`${stage.stage}-${String(index)}`}
                    className="flex items-center gap-2 text-sm"
                  >
                    {running ? (
                      <Loader2 className="text-primary size-4 animate-spin" />
                    ) : (
                      <CheckCircle2 className="text-muted-foreground size-4" />
                    )}
                    <span className={running ? "font-medium" : ""}>{stage.stage}</span>
                  </li>
                );
              })}
            </ol>
          )}
        </CardContent>
      </Card>

      {data.state === "succeeded" && RESOLVING_KINDS.has(data.kind) && (
        <Card className="border-emerald-500/40" data-testid="job-success">
          <CardContent className="flex flex-wrap items-center gap-3 p-4 text-sm lg:p-6">
            <CheckCircle2 className="size-5 text-emerald-500" />
            <span className="font-medium">Run complete — see how your data clusters.</span>
            <span className="flex-1" />
            <Button asChild size="sm" data-testid="job-success-cta">
              <Link href={`/${org}/duplicates`}>View duplicate groups</Link>
            </Button>
          </CardContent>
        </Card>
      )}

      {data.error_class && (
        <Card data-testid="job-error">
          <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
            <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
              Failure
            </CardTitle>
          </CardHeader>
          <CardContent className="p-4 pt-2 text-sm lg:p-6 lg:pt-2">
            <p className="font-medium">{data.error_class}</p>
            <p className="text-muted-foreground mt-1 text-xs whitespace-pre-wrap">
              {(data as { error_detail?: string | null }).error_detail ?? ""}
            </p>
            <p className="text-muted-foreground mt-2 text-xs">
              Attempt {data.attempt} of {(data as { max_attempts?: number }).max_attempts ?? "?"}
              {data.error_class === "transient_io" && " — transient failures retry automatically."}
            </p>
          </CardContent>
        </Card>
      )}
    </div>
  );
}
