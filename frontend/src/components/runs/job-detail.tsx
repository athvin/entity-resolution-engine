"use client";

import Link from "next/link";
import { ArrowLeft, CheckCircle2, Circle } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { Working } from "@/components/ui/working";
import { effectiveRole, useJob, useJobAction, useSession } from "@/lib/query/hooks";
import { cn } from "@/lib/utils";
import { StateChip } from "./runs-content";

const ACTIVE = new Set(["queued", "dispatching", "running", "retrying", "canceling"]);
const STOPPED = new Set(["failed", "canceled"]);
const ROLE_RANK = { viewer: 0, steward: 1, admin: 2 } as const;

/**
 * What a single stage row is saying.
 *
 * Only the last reported stage is ever anything but `done`: er-server appends
 * a stage as it starts it, so everything behind the tail necessarily finished.
 *
 * `stopped` is the case the first version of this list got wrong. It rendered
 * a completed check against the tail stage of a *failed* job, which claimed
 * the stage where the run died had succeeded. There is no per-stage outcome in
 * the payload to consult, but the job's own terminal state is enough to know
 * the tail did not finish.
 */
type StageState = "done" | "running" | "stopped";

function stageStateAt(index: number, count: number, jobState: string): StageState {
  if (index !== count - 1) return "done";
  if (ACTIVE.has(jobState)) return "running";
  if (STOPPED.has(jobState)) return "stopped";
  return "done";
}

const STAGE_STATUS: Record<StageState, string> = {
  done: "Done",
  running: "Running",
  stopped: "Stopped",
};

/**
 * The stage list, as an execution trace.
 *
 * Three things carry state here, none of them hue: the marker (filled check /
 * dots / empty circle), the weight of the stage name, and a status word. The
 * rail joining the markers is what makes it read as one pipeline rather than
 * as a list of unrelated lines.
 *
 * Deliberately absent: per-stage durations and a "pending" tail. Both are
 * standard in this kind of trace and both would be invented — `progress`
 * carries nothing but a stage name, and the set of stages a job kind will run
 * is not in the payload either. A trace that pads itself with plausible rows
 * is worse than a short one.
 */
function StageTrace({ stages, jobState }: { stages: { stage: string }[]; jobState: string }) {
  return (
    <ol className="flex flex-col">
      {stages.map((stage, index) => {
        const state = stageStateAt(index, stages.length, jobState);
        const isLast = index === stages.length - 1;
        return (
          <li
            key={`${stage.stage}-${String(index)}`}
            // `rise-in`, not a fade, and not staggered: the row carries a
            // `text-muted-foreground` status word, which sits exactly on the
            // 4.5:1 contrast floor and drops under it at any opacity below 1.
            // Moving it is free; fading it is not.
            //
            // Nothing is lost by dropping the stagger either. Stages arrive
            // one at a time from polling, so each new row is already the only
            // thing mounting — the entrance marks the arrival on its own.
            className="animate-rise-in relative flex items-start gap-3 pb-3 text-sm last:pb-0"
          >
            {!isLast && (
              <span aria-hidden="true" className="bg-border absolute top-5 bottom-0 left-2 w-px" />
            )}
            {/* The marker says *where on the rail* this stage is; the right
             * column says what it is doing. Splitting them keeps the rail a
             * fixed 16px wide, which is what lets the connector line up. */}
            <span className="bg-card relative mt-0.5 flex size-4 shrink-0 items-center justify-center">
              {state === "running" ? (
                <span className="bg-foreground size-2.5 rounded-full" />
              ) : state === "done" ? (
                <CheckCircle2 className="text-foreground animate-mark-in size-4" />
              ) : (
                <Circle className="text-muted-foreground size-4" />
              )}
            </span>
            <span className={cn("min-w-0 break-words", state === "running" && "font-medium")}>
              {stage.stage}
            </span>
            <span className="flex-1" />
            <span className="text-muted-foreground shrink-0 text-xs">
              {state === "running" ? (
                <>
                  {/* The dots are the visual statement; the word is the one
                   * assistive tech gets, so neither audience has to infer. */}
                  <span className="sr-only">{STAGE_STATUS[state]}</span>
                  <Working className="text-foreground align-middle" />
                </>
              ) : (
                STAGE_STATUS[state]
              )}
            </span>
          </li>
        );
      })}
    </ol>
  );
}

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
  const stages = data.progress?.stages ?? [];
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
          <CardTitle className="text-muted-foreground flex items-baseline gap-2 text-xs font-medium tracking-wide uppercase">
            Stages
            {stages.length > 0 && (
              <span className="normal-case tabular-nums">
                {stages.length} reported
                {ACTIVE.has(data.state) && ", still running"}
              </span>
            )}
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
            <StageTrace stages={stages} jobState={data.state} />
          )}
        </CardContent>
      </Card>

      {data.state === "succeeded" && RESOLVING_KINDS.has(data.kind) && (
        /* The one flourish on this surface, and it is affordable because the
         * event is rare: a run finishing is the thing the user has been
         * waiting on, often for minutes. */
        <Card className="animate-scale-in border-foreground/40" data-testid="job-success">
          <CardContent className="flex flex-wrap items-center gap-3 p-4 text-sm lg:p-6">
            <CheckCircle2 className="text-foreground size-5" />
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
